//! Windows.Graphics.Capture（WGC）采集 POC。
//!
//! 目标（借鉴 Imago 的采集栈）：验证「不经过 ffmpeg、不轮询截图、由系统合成器
//! 直接给帧」的路线在本机可用，并拿到可复用的抓帧函数。当前只做：
//!
//! 1. D3D11 设备 → WinRT `IDirect3DDevice`；
//! 2. 以显示器为采集项创建 free-threaded 帧池；
//! 3. 轮询 `TryGetNextFrame`（POC 简化，正式版应改为 `FrameArrived` 事件）；
//! 4. 纹理回读到 staging texture，转 RGBA 落 PNG。
//!
//! 不改变现有录屏行为：`record.*` 仍走 ffmpeg。POC 通过后再替换采集层，
//! 届时可同时解决「自身窗口入画」「依赖用户安装 FFmpeg」「无原生暂停」三个缺陷。

// POC 阶段只有冒烟测试使用这些函数；接入录屏后删除本属性。
#![allow(dead_code)]

use orbit_core::OrbitError;
use std::time::{Duration, Instant};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, POINT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice,
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    HMONITOR, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::RoGetActivationFactory;
use windows::core::{HSTRING, Interface};

fn apartment() -> Result<(), OrbitError> {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;
    if hr.is_ok() || hr.0 == RPC_E_CHANGED_MODE {
        Ok(())
    } else {
        Err(OrbitError::system(format!("COM 初始化失败：{hr:?}")))
    }
}

struct D3D {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
}

fn create_d3d() -> Result<D3D, OrbitError> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|error| OrbitError::system(format!("创建 D3D11 设备失败：{error}")))?;
    }
    Ok(D3D {
        device: device.ok_or_else(|| OrbitError::system("D3D11 设备为空"))?,
        context: context.ok_or_else(|| OrbitError::system("D3D11 上下文为空"))?,
    })
}

fn capture_item_for_monitor(monitor: HMONITOR) -> Result<GraphicsCaptureItem, OrbitError> {
    let interop: IGraphicsCaptureItemInterop = unsafe {
        RoGetActivationFactory(&HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureItem"))
    }
    .map_err(|error| OrbitError::system(format!("获取 WGC 工厂失败：{error}")))?;
    unsafe { interop.CreateForMonitor(monitor) }
        .map_err(|error| OrbitError::system(format!("创建采集项失败：{error}")))
}

/// 抓取主显示器的一帧并保存为 PNG，返回 (width, height)。
///
/// POC 语义：不裁剪、不连续采集；正式接入时把「取帧 + 尺寸」抽成
/// `ScreenSource`，由录屏时间轴驱动。
pub(crate) fn capture_primary_monitor_to_png(path: &str) -> Result<(u32, u32), OrbitError> {
    let (width, height, rgba) = capture_primary_monitor_rgba()?;
    let image = xcap::image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| OrbitError::system("RGBA 缓冲区尺寸不匹配"))?;
    image
        .save(path)
        .map_err(|error| OrbitError::system(format!("保存 PNG 失败：{error}")))?;
    Ok((width, height))
}

/// 只取一帧原始 RGBA（不含 PNG 编码），供耗时分解与后续编码器接入。
pub(crate) fn capture_primary_monitor_rgba() -> Result<(u32, u32, Vec<u8>), OrbitError> {
    apartment()?;
    let d3d = create_d3d()?;

    let dxgi: IDXGIDevice = d3d
        .device
        .cast()
        .map_err(|error| OrbitError::system(format!("获取 DXGI 设备失败：{error}")))?;
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .map_err(|error| OrbitError::system(format!("包装 WinRT 设备失败：{error}")))?;
    let winrt_device: IDirect3DDevice = inspectable
        .cast()
        .map_err(|error| OrbitError::system(format!("转换为 IDirect3DDevice 失败：{error}")))?;

    let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    let item = capture_item_for_monitor(monitor)?;
    let size = item
        .Size()
        .map_err(|error| OrbitError::system(format!("读取采集项尺寸失败：{error}")))?;

    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        size,
    )
    .map_err(|error| OrbitError::system(format!("创建帧池失败：{error}")))?;
    let session: GraphicsCaptureSession = pool
        .CreateCaptureSession(&item)
        .map_err(|error| OrbitError::system(format!("创建采集会话失败：{error}")))?;
    // 光标与黄色采集边框都不进画面；Win10 上边框 API 可能不可用，忽略失败。
    let _ = session.SetIsCursorCaptureEnabled(false);
    let _ = session.SetIsBorderRequired(false);
    session
        .StartCapture()
        .map_err(|error| OrbitError::system(format!("开始采集失败：{error}")))?;

    // POC 简化：轮询取帧（正式版改 FrameArrived 事件 + 阻塞等待）。
    let deadline = Instant::now() + Duration::from_secs(3);
    let frame = loop {
        match pool.TryGetNextFrame() {
            Ok(frame) => break frame,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                let _ = session.Close();
                return Err(OrbitError::system(format!(
                    "3 秒内未收到 WGC 帧：{error}"
                )));
            }
        }
    };

    let result = copy_frame_to_rgba(&d3d, frame);
    let _ = session.Close();
    result
}

fn copy_frame_to_rgba(
    d3d: &D3D,
    frame: windows::Graphics::Capture::Direct3D11CaptureFrame,
) -> Result<(u32, u32, Vec<u8>), OrbitError> {
    let surface = frame
        .Surface()
        .map_err(|error| OrbitError::system(format!("读取帧表面失败：{error}")))?;
    let access: IDirect3DDxgiInterfaceAccess = surface
        .cast()
        .map_err(|error| OrbitError::system(format!("获取 DXGI 接口失败：{error}")))?;
    let texture: ID3D11Texture2D = unsafe { access.GetInterface() }
        .map_err(|error| OrbitError::system(format!("获取 D3D 纹理失败：{error}")))?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };
    let (width, height) = (desc.Width, desc.Height);
    if width == 0 || height == 0 {
        return Err(OrbitError::system("WGC 帧尺寸为 0"));
    }

    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: desc.Format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging: Option<ID3D11Texture2D> = None;
    unsafe {
        d3d.device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .map_err(|error| OrbitError::system(format!("创建回读纹理失败：{error}")))?;
    }
    let staging = staging.ok_or_else(|| OrbitError::system("回读纹理为空"))?;
    unsafe { d3d.context.CopyResource(&staging, &texture) };

    let mut mapped = windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        d3d.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|error| OrbitError::system(format!("映射回读纹理失败：{error}")))?;
    }

    let width_usize = width as usize;
    let height_usize = height as usize;
    let mut rgba = vec![0u8; width_usize * height_usize * 4];
    let row_pitch = mapped.RowPitch as usize;
    unsafe {
        let base = mapped.pData as *const u8;
        for y in 0..height_usize {
            let row = std::slice::from_raw_parts(base.add(y * row_pitch), width_usize * 4);
            // BGRA → RGBA
            for x in 0..width_usize {
                let src = x * 4;
                let dst = (y * width_usize + x) * 4;
                rgba[dst] = row[src + 2];
                rgba[dst + 1] = row[src + 1];
                rgba[dst + 2] = row[src];
                rgba[dst + 3] = row[src + 3];
            }
        }
        d3d.context.Unmap(&staging, 0);
    }

    Ok((width, height, rgba))
}

#[cfg(all(test, target_os = "windows", feature = "smoke"))]
mod tests {
    use super::*;

    /// WGC POC 实机冒烟：抓主显示器一帧、落 PNG，并与 xcap 枚举到的显示器尺寸对照。
    #[test]
    fn wgc_captures_primary_monitor_frame() {
        let dir = std::env::temp_dir().join("orbit-smoke");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wgc-poc.png");
        let (width, height) = capture_primary_monitor_to_png(&path.to_string_lossy())
            .expect("WGC 抓帧失败");

        let image = xcap::image::open(&path).expect("读取 WGC 产物 PNG 失败");
        assert_eq!(image.width(), width);
        assert_eq!(image.height(), height);
        assert!(width >= 640 && height >= 480, "抓帧尺寸异常：{width}x{height}");

        let monitor = xcap::Monitor::all().expect("枚举显示器失败").remove(0);
        assert_eq!(
            (width, height),
            (monitor.width().unwrap(), monitor.height().unwrap()),
            "WGC 帧尺寸应与主显示器尺寸一致"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// 单帧耗时（D3D 初始化 → 首帧 → 回读 RGBA），为替换 gdigrab 提供成本证据。
    /// 注意：PNG 编码另计，真实录屏走 Media Foundation，不落 PNG。
    #[test]
    fn wgc_single_frame_latency_is_reasonable() {
        let mut capture_samples = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            let (width, height, rgba) =
                capture_primary_monitor_rgba().expect("WGC 抓帧失败");
            capture_samples.push(started.elapsed().as_millis());
            assert_eq!(rgba.len(), (width * height * 4) as usize);
        }
        eprintln!("WGC 单帧耗时（设备/会话/回读，不含 PNG）：{capture_samples:?} ms");

        // PNG 编码单独计时，避免把编码成本误算到采集上。
        let dir = std::env::temp_dir().join("orbit-smoke");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wgc-png.png");
        let (width, height, rgba) = capture_primary_monitor_rgba().expect("WGC 抓帧失败");
        let image = xcap::image::RgbaImage::from_raw(width, height, rgba).unwrap();
        let png_started = Instant::now();
        image.save(&path).expect("保存 PNG 失败");
        eprintln!("WGC 产物 PNG 编码耗时：{} ms", png_started.elapsed().as_millis());
        let _ = std::fs::remove_file(&path);

        assert!(
            capture_samples.iter().all(|ms| *ms < 1500),
            "WGC 单帧耗时异常：{capture_samples:?} ms"
        );
    }

    /// 自身窗口排除：录制期间给悬浮球加 WDA_EXCLUDEFROMCAPTURE，画面里就不再出现。
    /// 这里用一个临时 Win32 窗口验证 API 可用且可读写，避免依赖悬浮球实例。
    #[test]
    fn window_can_be_excluded_from_capture() {
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetWindowDisplayAffinity,
            SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WS_POPUP,
            WINDOW_EX_STYLE,
        };
        use windows::core::w;

        apartment().expect("COM 初始化失败");
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("orbit-wgc-poc"),
                WS_POPUP,
                0,
                0,
                8,
                8,
                None,
                None,
                None,
                None,
            )
        }
        .expect("创建测试窗口失败");

        unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }
            .expect("设置 WDA_EXCLUDEFROMCAPTURE 失败（需要 Windows 10 2004+）");
        let mut affinity: u32 = 0;
        unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) }.expect("读取窗口捕获亲和性失败");
        assert_eq!(affinity, WDA_EXCLUDEFROMCAPTURE.0);

        unsafe { SetWindowDisplayAffinity(hwnd, WDA_NONE) }.expect("恢复 WDA_NONE 失败");
        unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) }.expect("读取窗口捕获亲和性失败");
        assert_eq!(affinity, WDA_NONE.0);

        unsafe { DestroyWindow(hwnd) }.expect("销毁测试窗口失败");
    }
}
