use orbit_core::{
    AuditLog, CommandRequest, CommandResponse, CommandStatus, DockPosition, PermissionState,
    RegionFinishParams, RegionSelectionStart, SetupResult, SystemMetrics, WindowLayout, WindowSize,
};
use std::sync::Mutex;
use std::time::Instant;
use sysinfo::{Components, System};
use tauri::Manager;
use tauri::{
    AppHandle, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt;
use xcap::Monitor;

mod bus;
mod clipboard;
mod elevated;
mod input_recorder;
mod screenshot;

/// tauri.conf.json 只描述窗口初始尺寸；停靠/展开态尺寸集中在此，
/// 由 `overlay.layout` 发布给前端，避免 Rust 与 CSS 各写一份数字。
const SIDE_SIZE: WindowSize = WindowSize { width: 54.0, height: 132.0 };
const NOTCH_SIZE: WindowSize = WindowSize { width: 320.0, height: 62.0 };
/// 展开圆盘窗口：紧贴最大外圈（半径 132）并留出顶部提示、底部按钮的位置
const EXPANDED_SIZE: WindowSize = WindowSize { width: 340.0, height: 340.0 };
/// 弹窗相对窗口的内边距：0 表示严格按配置尺寸铺满。
const PANEL_INSET: f64 = 0.0;
/// 贴近屏幕边缘多少像素内才吸附，避免大老远就触发停靠
const EDGE_THRESHOLD: i32 = 24;

/// 窗口尺寸的唯一发布点：初始尺寸读取 tauri.conf.json，状态尺寸取上面的常量。
/// 找不到配置窗口时直接报错，不在代码里再抄一份兜底数字。
pub(crate) fn layout_config(
    manager: &impl tauri::Manager<tauri::Wry>,
) -> Result<WindowLayout, String> {
    let configured = |label: &str| -> Result<WindowSize, String> {
        manager
            .config()
            .app
            .windows
            .iter()
            .find(|window| window.label == label)
            .map(|window| WindowSize {
                width: window.width,
                height: window.height,
            })
            .ok_or_else(|| format!("tauri.conf.json 缺少 {label} 窗口配置"))
    };
    Ok(WindowLayout {
        floating: configured("main")?,
        side: SIDE_SIZE,
        notch: NOTCH_SIZE,
        expanded: EXPANDED_SIZE,
        clipboard: configured("clipboard")?,
        selection: configured("selection")?,
        panel_inset: PANEL_INSET,
    })
}

/// 覆盖层当前状态使用的逻辑尺寸；前端由同一份 layout 注入 CSS 变量。
fn overlay_size(layout: &WindowLayout, dock: DockPosition, expanded: bool) -> WindowSize {
    match (dock, expanded) {
        (DockPosition::Floating, false) => layout.floating,
        (DockPosition::Left | DockPosition::Right, false) => layout.side,
        (DockPosition::Top, false) => layout.notch,
        (_, true) => layout.expanded,
    }
}

/// 子进程一律不创建控制台窗口：OCR / 温度 / 录屏 / 转换调用外部程序时不再闪黑框。
trait NoWindow {
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for std::process::Command {
    #[cfg(target_os = "windows")]
    fn no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        self.creation_flags(CREATE_NO_WINDOW)
    }

    #[cfg(not(target_os = "windows"))]
    fn no_window(&mut self) -> &mut Self {
        self
    }
}

#[derive(Clone, Copy)]
struct OverlayLayout {
    dock: DockPosition,
    expanded: bool,
}

#[derive(Default)]
struct OverlayState(Mutex<OverlayLayout>);
pub struct ElevatedToggle(pub Mutex<bool>); // 用户启用后才会拉 helper

pub(crate) fn set_elevated_enabled_impl(
    app: &AppHandle,
    enabled: bool,
) -> Result<bool, String> {
    if enabled {
        // 一次 UAC 拉起常驻 helper；已在运行则不重复弹。
        elevated::start_watch().map_err(|error| error.message())?;
    } else {
        elevated::stop_watch();
    }
    *app.state::<ElevatedToggle>().0.lock().unwrap() = enabled;
    Ok(enabled)
}

pub(crate) fn get_elevated_status_impl(app: &AppHandle) -> bool {
    *app.state::<ElevatedToggle>().0.lock().unwrap() || elevated::is_watch_running()
}

impl Default for OverlayLayout {
    fn default() -> Self {
        Self {
            dock: DockPosition::Floating,
            expanded: false,
        }
    }
}

/// 登录启动的真实开关逻辑：总线处理器与单元测试共用，避免两套行为漂移。
pub(crate) fn apply_autostart(app: &AppHandle, enabled: bool) -> Result<SetupResult, String> {
    let autostart = app.autolaunch();
    if enabled {
        autostart.enable().map_err(|error| error.to_string())?;
    } else {
        autostart.disable().map_err(|error| error.to_string())?;
    }

    let result = setup_result(app)?;
    if result.autostart_configured != enabled {
        return Err(if enabled {
            "登录启动项注册后仍处于关闭状态。".to_string()
        } else {
            "登录启动项已关闭，但状态检查仍显示启用。".to_string()
        });
    }
    Ok(result)
}

pub(crate) fn setup_result(app: &AppHandle) -> Result<SetupResult, String> {
    let permissions = [
        ("screen-capture", PermissionState::Unknown),
        ("accessibility-input", PermissionState::Unknown),
    ]
    .into_iter()
    .map(|(id, state)| (id.to_string(), state))
    .collect();
    let autostart_configured = app
        .autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())?;

    Ok(SetupResult {
        permissions,
        autostart_configured,
    })
}

#[tauri::command]
fn get_command_catalog() -> Result<Vec<orbit_core::CommandSpec>, String> {
    // 目录直接来自总线注册表：契约与实际可路由命令永远一致。
    let mut specs: Vec<orbit_core::CommandSpec> = bus::registry()
        .values()
        .map(|entry| entry.spec.clone())
        .collect();
    specs.sort_by(|a, b| a.id.cmp(b.id));
    Ok(specs)
}

#[tauri::command]
async fn execute_command(
    app: AppHandle,
    audit: tauri::State<'_, AuditLog>,
    request: CommandRequest,
) -> Result<CommandResponse, String> {
    Ok(bus::execute(app, audit.inner().clone(), request).await)
}

pub(crate) fn start_region_selection_impl(
    app: &AppHandle,
    layout: OverlayLayout,
) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 Orbit 主窗口。".to_string())?;
    let selection = app
        .get_webview_window("selection")
        .ok_or_else(|| "找不到区域截图窗口。".to_string())?;
    let monitor = main
        .current_monitor()
        .map_err(|error| error.to_string())?
        .or(main.primary_monitor().map_err(|error| error.to_string())?)
        .ok_or_else(|| "无法确定区域截图所在的显示器。".to_string())?;
    let position = monitor.position();
    let size = monitor.size();
    let capture_monitor = Monitor::all()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|candidate| {
            candidate.x().ok() == Some(position.x)
                && candidate.y().ok() == Some(position.y)
                && candidate.width().ok() == Some(size.width)
                && candidate.height().ok() == Some(size.height)
        })
        .ok_or_else(|| "无法将窗口显示器映射到截图设备。".to_string())?;
    let monitor_id = capture_monitor.id().map_err(|error| error.to_string())?;

    selection
        .set_size(PhysicalSize::new(size.width, size.height))
        .map_err(|error| error.to_string())?;
    selection
        .set_position(PhysicalPosition::new(position.x, position.y))
        .map_err(|error| error.to_string())?;
    bus::emit(
        &app,
        "region-selection-start",
        &RegionSelectionStart { monitor_id },
    )
    .map_err(|error| error.to_string())?;

    main.hide().map_err(|error| error.to_string())?;
    if let Err(error) = selection.show() {
        let _ = main.show();
        return Err(error.to_string());
    }
    if let Err(error) = selection.set_focus() {
        let _ = selection.hide();
        return match restore_main_overlay(app, layout) {
            Ok(()) => Err(error.to_string()),
            Err(restore_error) => Err(format!(
                "无法聚焦区域截图窗口：{error}；恢复 Orbit 主窗口失败：{restore_error}"
            )),
        };
    }
    Ok(())
}

fn restore_main_overlay(app: &AppHandle, layout: OverlayLayout) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 Orbit 主窗口。".to_string())?;
    set_overlay_layout(&main, layout.dock, layout.expanded)?;
    main.show().map_err(|error| error.to_string())
}

pub(crate) async fn finish_region_selection_impl(
    app: &AppHandle,
    layout: OverlayLayout,
    params: RegionFinishParams,
) -> Result<CommandResponse, String> {
    let RegionFinishParams {
        monitor_id,
        x,
        y,
        width,
        height,
    } = params;
    let selection = app
        .get_webview_window("selection")
        .ok_or_else(|| "找不到区域截图窗口。".to_string())?;
    selection.hide().map_err(|error| error.to_string())?;
    let request = CommandRequest {
        command_id: "screenshot.capture".to_string(),
        params: serde_json::json!({
            "mode": "region",
            "monitorId": monitor_id,
            "x": x,
            "y": y,
            "width": width,
            "height": height
        }),
    };
    let audit = app.state::<AuditLog>().inner().clone();
    let response = bus::execute(app.clone(), audit, request).await;
    let restore = restore_main_overlay(app, layout);
    let capture_error = || {
        response
            .error
            .clone()
            .unwrap_or_else(|| "截图命令执行失败".to_string())
    };
    match (response.status, restore) {
        (CommandStatus::Ok, Ok(())) => Ok(response),
        (CommandStatus::Ok, Err(error)) => {
            Err(format!("截图已处理，但无法恢复 Orbit 主窗口：{error}"))
        }
        (CommandStatus::Error, Ok(())) => Err(capture_error()),
        (CommandStatus::Error, Err(restore_error)) => Err(format!(
            "{}；恢复 Orbit 主窗口失败：{restore_error}",
            capture_error()
        )),
    }
}

pub(crate) fn cancel_region_selection_impl(
    app: &AppHandle,
    layout: OverlayLayout,
) -> Result<(), String> {
    let selection = app
        .get_webview_window("selection")
        .ok_or_else(|| "找不到区域截图窗口。".to_string())?;
    selection.hide().map_err(|error| error.to_string())?;
    restore_main_overlay(app, layout)
}

fn monitor_bounds(window: &WebviewWindow) -> Result<(i32, i32, i32, i32), String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let current_size = window.outer_size().map_err(|error| error.to_string())?;
    let center_x = position.x + current_size.width as i32 / 2;
    let center_y = position.y + current_size.height as i32 / 2;
    let monitor = window
        .monitor_from_point(center_x as f64, center_y as f64)
        .map_err(|error| error.to_string())?
        .or(window
            .primary_monitor()
            .map_err(|error| error.to_string())?);
    monitor
        .map(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            (
                position.x,
                position.y,
                position.x + size.width as i32,
                position.y + size.height as i32,
            )
        })
        .ok_or_else(|| "无法确定悬浮球所在的显示器。".to_string())
}

fn dock_position(window: &WebviewWindow) -> Result<DockPosition, String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let size = window.outer_size().map_err(|error| error.to_string())?;
    let (left, top, right, _) = monitor_bounds(window)?;
    Ok(determine_dock_position(
        position.x,
        position.y,
        size.width as i32,
        left,
        top,
        right,
    ))
}

fn determine_dock_position(
    x: i32,
    y: i32,
    width: i32,
    left: i32,
    top: i32,
    right: i32,
) -> DockPosition {
    if y <= top + EDGE_THRESHOLD {
        DockPosition::Top
    } else if x <= left + EDGE_THRESHOLD {
        DockPosition::Left
    } else if x + width >= right - EDGE_THRESHOLD {
        DockPosition::Right
    } else {
        DockPosition::Floating
    }
}

fn set_overlay_layout(
    window: &WebviewWindow,
    dock: DockPosition,
    expanded: bool,
) -> Result<(), String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let current_size = window.outer_size().map_err(|error| error.to_string())?;
    let scale_factor = window.scale_factor().map_err(|error| error.to_string())?;
    let center_x = position.x + current_size.width as i32 / 2;
    let center_y = position.y + current_size.height as i32 / 2;
    let (monitor_left, monitor_top, monitor_right, monitor_bottom) = monitor_bounds(window)?;
    let layout = layout_config(window)?;
    let logical_size = overlay_size(&layout, dock, expanded);
    let target_width = (logical_size.width * scale_factor).round() as u32;
    let target_height = (logical_size.height * scale_factor).round() as u32;
    let max_x = monitor_right - target_width as i32;
    let max_y = monitor_bottom - target_height as i32;
    let target_x = match dock {
        DockPosition::Floating => center_x - target_width as i32 / 2,
        DockPosition::Left => monitor_left,
        DockPosition::Right => max_x,
        DockPosition::Top => {
            (monitor_left + (monitor_right - monitor_left) / 2) - target_width as i32 / 2
        }
    }
    .clamp(monitor_left, max_x.max(monitor_left));
    let target_y = match dock {
        DockPosition::Top => monitor_top,
        DockPosition::Left | DockPosition::Right | DockPosition::Floating => {
            center_y - target_height as i32 / 2
        }
    }
    .clamp(monitor_top, max_y.max(monitor_top));

    window
        .set_size(tauri::LogicalSize::new(
            logical_size.width,
            logical_size.height,
        ))
        .map_err(|error| error.to_string())?;
    window
        .set_position(PhysicalPosition::new(target_x, target_y))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{DockPosition, determine_dock_position};

    #[test]
    fn overlay_size_follows_layout_states() {
        use super::{overlay_size, WindowLayout, WindowSize};
        const fn size(width: f64, height: f64) -> WindowSize {
            WindowSize { width, height }
        }
        let layout = WindowLayout {
            floating: size(104.0, 104.0),
            side: size(54.0, 132.0),
            notch: size(320.0, 62.0),
            expanded: size(340.0, 340.0),
            clipboard: size(420.0, 560.0),
            selection: size(1.0, 1.0),
            panel_inset: 0.0,
        };
        assert_eq!(
            overlay_size(&layout, DockPosition::Floating, false),
            size(104.0, 104.0)
        );
        assert_eq!(
            overlay_size(&layout, DockPosition::Left, false),
            size(54.0, 132.0)
        );
        assert_eq!(
            overlay_size(&layout, DockPosition::Right, false),
            size(54.0, 132.0)
        );
        assert_eq!(
            overlay_size(&layout, DockPosition::Top, false),
            size(320.0, 62.0)
        );
        assert_eq!(
            overlay_size(&layout, DockPosition::Floating, true),
            size(340.0, 340.0)
        );
        assert_eq!(
            overlay_size(&layout, DockPosition::Top, true),
            size(340.0, 340.0)
        );
    }

    #[test]
    fn top_edge_takes_priority_at_screen_corners() {
        assert_eq!(
            determine_dock_position(0, 0, 104, 0, 0, 1920),
            DockPosition::Top
        );
    }

    #[test]
    fn side_edges_dock_to_the_corresponding_sidebar() {
        assert_eq!(
            determine_dock_position(20, 500, 104, 0, 0, 1920),
            DockPosition::Left
        );
        assert_eq!(
            determine_dock_position(1810, 500, 104, 0, 0, 1920),
            DockPosition::Right
        );
    }

    #[test]
    fn window_near_no_edge_remains_floating() {
        assert_eq!(
            determine_dock_position(500, 300, 104, 0, 0, 1920),
            DockPosition::Floating
        );
    }

    #[test]
    fn window_forty_pixels_from_edge_stays_floating() {
        assert_eq!(
            determine_dock_position(40, 500, 104, 0, 0, 1920),
            DockPosition::Floating
        );
        assert_eq!(
            determine_dock_position(1740, 500, 104, 0, 0, 1920),
            DockPosition::Floating
        );
    }

    #[test]
    fn dash_state_samples_plausible_live_values() {
        let dash = super::DashState::new();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let (cpu, total_mb, avail_mb) = dash.sample();
        assert!(
            cpu.is_finite() && (0.0..=100.0).contains(&cpu),
            "cpu={cpu}"
        );
        assert!(total_mb > 0, "total_mb={total_mb}");
        assert!(avail_mb <= total_mb, "avail={avail_mb} total={total_mb}");
    }

    #[test]
    fn monitor_snapshot_exposes_per_core_usage() {
        let dash = super::DashState::new();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let (cpu, cores, total_mb, _) = dash.snapshot();
        assert!(
            cpu.is_finite() && (0.0..=100.0).contains(&cpu),
            "cpu={cpu}"
        );
        assert!(!cores.is_empty(), "没有读到 CPU 核心");
        assert!(
            cores.iter().all(|usage| usage.is_finite() && (0.0..=100.0).contains(usage)),
            "核心占用越界：{cores:?}"
        );
        assert!(total_mb > 0, "total_mb={total_mb}");
    }
}

// ============ 灵动岛仪表盘 ============

/// CPU 使用率是两次采样之间的差值，必须复用同一个 System；
/// 每次调用都新建实例只能拿到首次采样的垃圾值（常见 0% 或 100%）。
pub struct DashState(Mutex<System>);

impl DashState {
    fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        Self(Mutex::new(system))
    }

    /// 返回 (CPU 占用百分比, 总内存 MB, 可用内存 MB)。
    pub fn sample(&self) -> (f32, u64, u64) {
        let (cpu, _, total_mb, avail_mb) = self.snapshot();
        (cpu, total_mb, avail_mb)
    }

    /// 返回 (CPU 总占用, 每核占用, 总内存 MB, 可用内存 MB)。
    pub fn snapshot(&self) -> (f32, Vec<f32>, u64, u64) {
        let mut sys = self.0.lock().unwrap();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let per_core = sys.cpus().iter().map(|cpu| cpu.cpu_usage()).collect();
        (
            sys.global_cpu_usage(),
            per_core,
            sys.total_memory() / 1024,
            sys.available_memory() / 1024,
        )
    }
}

// 管理员 helper 的文件握手与错误映射见 elevated.rs。

// ============ 录屏 ============

/// FFmpeg 查找顺序：FFMPEG_PATH 环境变量 → sidecar 资源目录 → 系统 PATH。
/// 录屏与格式转换共用同一规则。
fn resolve_ffmpeg(app: &AppHandle) -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("FFMPEG_PATH") {
        return std::path::PathBuf::from(p);
    }
    if let Ok(sidecar) = app
        .path()
        .resolve("ffmpeg", tauri::path::BaseDirectory::Resource)
    {
        if sidecar.exists() {
            return sidecar;
        }
    }
    std::path::PathBuf::from("ffmpeg")
}

struct RecordState(Mutex<Option<RecordHandle>>);
struct RecordHandle {
    path: String,
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
}

pub(crate) fn record_start_impl(app: &AppHandle) -> Result<String, String> {
    if app.state::<RecordState>().0.lock().unwrap().is_some() {
        return Err("已经在录制中".to_string());
    }
    // 找 FFmpeg（sidecar 或 PATH）
    let ffmpeg = resolve_ffmpeg(app);

    // 生成输出路径：桌面/Orbit/orbit_rec_*.mp4
    let home = std::env::var("USERPROFILE")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(std::path::PathBuf::from))
        .map_err(|_| "找不到用户目录".to_string())?;
    let dir = home.join("Desktop").join("Orbit");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let filename = format!(
        "orbit_rec_{}.mp4",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    );
    let path = dir.join(&filename);
    let path_str = path.to_string_lossy().to_string();
    let handle = spawn_ffmpeg_recording(&ffmpeg, &path_str)?;
    *app.state::<RecordState>().0.lock().unwrap() = Some(handle);
    Ok(path_str)
}

/// 启动 FFmpeg gdigrab 全屏录制，返回句柄；与测试共用。
fn spawn_ffmpeg_recording(
    ffmpeg: &std::path::Path,
    path_str: &str,
) -> Result<RecordHandle, String> {
    // FFmpeg gdigrab 全屏录制，H264 编码，4Mbps
    let args = [
        "-y",
        "-f", "gdigrab",
        "-framerate", "30",
        "-draw_mouse", "1",
        "-i", "desktop",
        "-c:v", "libx264",
        "-preset", "veryfast",
        "-b:v", "4M",
        "-pix_fmt", "yuv420p",
        path_str,
    ];

    let mut child = std::process::Command::new(ffmpeg)
        .no_window()
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "找不到 ffmpeg。请安装 ffmpeg 并加入 PATH，或设置 FFMPEG_PATH 环境变量。".to_string()
            } else {
                format!("FFmpeg 启动失败：{}", e)
            }
        })?;
    let stdin = child.stdin.take();
    Ok(RecordHandle {
        path: path_str.to_string(),
        child,
        stdin,
    })
}

/// 正常收尾：向 stdin 写 q 让 FFmpeg 写完 mp4 尾部，超时才强杀。
fn finish_recording(handle: RecordHandle) -> Result<Option<String>, String> {
    use std::io::Write;
    let mut handle = handle;
    if let Some(mut stdin) = handle.stdin.take() {
        let _ = stdin.write_all(b"q");
        let _ = stdin.flush();
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(8);
    loop {
        match handle.child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    // 兜底：FFmpeg 无响应时才强杀（此时尾部可能不完整）。
                    let _ = handle.child.kill();
                    let _ = handle.child.wait();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => break,
        }
    }
    Ok(Some(handle.path))
}

pub(crate) async fn record_stop_impl(app: &AppHandle) -> Result<Option<String>, String> {
    let handle = app
        .state::<RecordState>()
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "没有正在录制的会话".to_string())?;

    tauri::async_runtime::spawn_blocking(move || finish_recording(handle))
        .await
        .map_err(|error| format!("录屏停止线程异常退出：{error}"))?
}

// ============ OCR ============

/// 将窗口所在显示器映射到 xcap 捕获设备（与区域选区使用同一映射规则）。
fn current_capture_monitor(window: &WebviewWindow) -> Result<Monitor, String> {
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or(window.primary_monitor().map_err(|e| e.to_string())?)
        .ok_or_else(|| "无法确定 OCR 所在的显示器。".to_string())?;
    let position = monitor.position();
    let size = monitor.size();
    Monitor::all()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|candidate| {
            candidate.x().ok() == Some(position.x)
                && candidate.y().ok() == Some(position.y)
                && candidate.width().ok() == Some(size.width)
                && candidate.height().ok() == Some(size.height)
        })
        .ok_or_else(|| "无法将窗口显示器映射到截图设备。".to_string())
}

pub(crate) fn ocr_image_blocking(image_path: &str) -> Result<String, String> {
    // Windows.Media.Ocr 通过 PowerShell 调用（Windows 10+ 内置）。
    // 路径经环境变量传入，不做字符串内插，避免引号破坏脚本或注入。
    // Await 用反射调用泛型 AsTask：PowerShell 直接调用时无法对 __ComObject
    // 解析泛型重载（MethodCountCouldNotFindBest），必须显式指定结果类型。
    let ps_script = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Storage.StorageFile, Windows.Storage, ContentType=WindowsRuntime]
$null = [Windows.Storage.Streams.IRandomAccessStream, Windows.Storage.Streams, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.SoftwareBitmap, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Media.Ocr.OcrEngine, Windows.Media.Ocr, ContentType=WindowsRuntime]
$null = [Windows.Media.Ocr.OcrResult, Windows.Media.Ocr, ContentType=WindowsRuntime]

$asTaskOpen = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
  $_.Name -eq 'AsTask' -and $_.IsGenericMethod -and $_.GetGenericArguments().Count -eq 1 -and
  $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -like 'IAsyncOperation*'
}
function Await($op, $resultType) {
  $task = $asTaskOpen.MakeGenericMethod($resultType).Invoke($null, @($op))
  $task.Wait() | Out-Null
  return $task.Result
}

$path = $env:ORBIT_OCR_PATH
$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($path)) ([Windows.Storage.StorageFile])
$ras = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($ras)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
$result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])
$result.Text
"#;

    let output = std::process::Command::new("powershell.exe")
        .no_window()
        .env("ORBIT_OCR_PATH", image_path)
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_script])
        .output()
        .map_err(|e| format!("PowerShell 启动失败：{}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&output.stderr).to_string();
        Err(format!("OCR 失败：{}", err))
    }
}

pub(crate) async fn ocr_current_screen_impl(window: &WebviewWindow) -> Result<String, String> {
    let window = window.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // 截取悬浮球当前所在显示器（多显示器环境下不再固定截第一块）。
        let monitor = current_capture_monitor(&window)?;
        let image = monitor.capture_image().map_err(|e| e.to_string())?;

        let tmp = std::env::temp_dir().join(format!(
            "orbit_ocr_{}_{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        image
            .save(&tmp)
            .map_err(|e| format!("临时截图保存失败：{}", e))?;

        // RAII guard: 函数返回时无论如何都会清理
        let _cleanup = TempFileGuard(tmp.clone());

        let path = tmp.to_string_lossy().to_string();
        ocr_image_blocking(&path)
    })
    .await
    .map_err(|error| format!("OCR 工作线程异常退出：{error}"))?
}

// RAII 临时文件清理 guard
struct TempFileGuard(std::path::PathBuf);
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ============ 键鼠录制 ============

struct KbRecordState(Mutex<Option<input_recorder::RecordingSession>>);

pub(crate) fn kb_record_start_impl(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<KbRecordState>();
    let mut guard = state.0.lock().unwrap();
    if guard.is_some() {
        return Err("已经在录制中".to_string());
    }
    *guard = Some(input_recorder::start()?);
    Ok(())
}

pub(crate) fn kb_record_stop_impl(app: &AppHandle) -> Result<Vec<input_recorder::KbEvent>, String> {
    let session = app
        .state::<KbRecordState>()
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "没有正在录制的会话".to_string())?;
    let (events, duration_ms) = input_recorder::stop(session)?;

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("应用数据目录获取失败：{e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    match input_recorder::save_recording(&dir, &events, duration_ms) {
        Ok(id) => log::info!("键鼠录制已保存：{id}（{} 个事件）", events.len()),
        Err(error) => return Err(format!("录制已停止，但保存失败：{error}")),
    }
    Ok(events)
}

// ============ 格式转换 ============

const CONVERT_FORMATS: &[&str] = &["mp3", "wav", "mp4", "gif", "webp"];

pub(crate) async fn convert_format_impl(
    app: &AppHandle,
    input_path: String,
    output_path: String,
    format: String,
) -> Result<String, String> {
    if !CONVERT_FORMATS.contains(&format.as_str()) {
        return Err(format!(
            "不支持的目标格式：{format}（支持：{}）",
            CONVERT_FORMATS.join(" / ")
        ));
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ffmpeg = resolve_ffmpeg(&app);
        convert_with_ffmpeg(&ffmpeg, &input_path, &output_path, &format)
    })
    .await
    .map_err(|error| format!("转换工作线程异常退出：{error}"))?
}

/// 实际执行转换；与测试共用，便于单独验证 ffmpeg 参数与输出路径。
fn convert_with_ffmpeg(
    ffmpeg: &std::path::Path,
    input_path: &str,
    output_path: &str,
    format: &str,
) -> Result<String, String> {
    let mut output = output_path.to_string();
    if !output.to_lowercase().ends_with(&format!(".{format}")) {
        output = format!("{}.{}", output.trim_end_matches('.'), format);
    }
    let out_path = std::path::Path::new(&output);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let args: Vec<&str> = match format {
        "mp3" => vec!["-y", "-i", input_path, "-vn", "-acodec", "libmp3lame", "-q:a", "2", &output],
        "wav" => vec!["-y", "-i", input_path, "-vn", "-acodec", "pcm_s16le", &output],
        "mp4" => vec!["-y", "-i", input_path, "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-c:a", "aac", &output],
        "gif" => vec!["-y", "-i", input_path, "-vf", "fps=12,scale=480:-1", &output],
        "webp" => vec!["-y", "-i", input_path, "-vf", "fps=15", "-loop", "0", &output],
        _ => vec!["-y", "-i", input_path, &output],
    };

    let converted = std::process::Command::new(ffmpeg)
        .no_window()
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "找不到 ffmpeg。请安装 ffmpeg 并加入 PATH，或设置 FFMPEG_PATH 环境变量。".to_string()
            } else {
                format!("FFmpeg 启动失败：{}", e)
            }
        })?;

    if converted.status.success() {
        Ok(output)
    } else {
        let stderr = String::from_utf8_lossy(&converted.stderr);
        let chars: Vec<char> = stderr.chars().collect();
        let tail: String = chars[chars.len().saturating_sub(400)..].iter().collect();
        Err(format!("转换失败（退出码：{:?}）：{}", converted.status.code(), tail.trim()))
    }
}

/// 在**窗口创建之前**注册所有后端状态。
///
/// Tauri 2 的 `setup()` 顺序是「先按 config 创建窗口（此时前端 onMount 的 IPC 已
/// 送达 WebView2 回调链）→ 再执行用户 setup 回调」。在 setup 里才 `app.manage()`
/// 会让启动期命令触发 `state() called before manage()`，panic 穿越 FFI 边界后
/// 表现为 `__fastfail` / 0xc0000409，进程启动即崩溃。plugin.initialize 由
/// `AppBuilder::build` 在窗口创建前调用，是唯一可靠的注册时机。
fn setup_early_state(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&dir)?;
    app.manage(AuditLog::open(dir.join("orbit.db"))?);
    clipboard::init_state(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    unsafe {
        // Set this before Tauri initializes WebKitGTK.
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    tauri::Builder::default()
        // 状态注册必须早于窗口创建，见 setup_early_state 注释。
        .plugin(
            tauri::plugin::Builder::<_, ()>::new("orbit-early-state")
                .setup(|app, _api| setup_early_state(app))
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(OverlayState::default())
        .manage(RecordState(Mutex::new(None)))
        .manage(KbRecordState(Mutex::new(None)))
        .manage(ElevatedToggle(Mutex::new(false)))
        .manage(DashState::new())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("Orbit")
                .build(),
        )
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(tauri_plugin_window_state::StateFlags::POSITION)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_command_catalog,
            execute_command
        ])
        // 剪贴板窗口失焦自动隐藏：延迟 140ms 复查，规避 Windows 焦点切换抖动。
        .on_window_event(|window, event| {
            if window.label() != "clipboard" {
                return;
            }
            if !matches!(event, WindowEvent::Focused(false)) {
                return;
            }
            let app = window.app_handle().clone();
            let Some(state) = app.try_state::<clipboard::ClipboardState>() else {
                return;
            };
            if !state.can_auto_hide() {
                return;
            }
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(140));
                if let Some(clip_window) = app.get_webview_window("clipboard") {
                    if clip_window.is_focused().unwrap_or(false) {
                        return;
                    }
                    let _ = clip_window.hide();
                }
            });
        })
        .setup(|app| {
            // AuditLog / ClipboardState 已在 early_state_plugin 中注册，
            // 这里只启动依赖它们的后台线程。
            clipboard::start_watcher(app.handle())?;

            // CPU 使用率是两次采样的差值，首次读数至少要间隔
            // MINIMUM_CPU_UPDATE_INTERVAL；启动后后台补采一次，
            // 避免刘海第一帧拿到无效的 0%。
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
                    let _ = handle.state::<DashState>().sample();
                });
            }

            // 全局快捷键唤起剪贴板。
            // 注意：`on_shortcut` 内部通过 run_on_main_thread + 同步 recv 完成注册，
            // 而 setup 运行在事件循环启动前的主线程，此时调用会死锁；因此延后到事件循环
            // 就绪后由后台线程注册，注册失败（被其他程序占用）只告警，不阻断启动。
            {
                use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
                let handle = app.handle().clone();
                std::thread::Builder::new()
                    .name("clipboard-shortcut".to_string())
                    .spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(600));
                        if let Err(error) = handle.global_shortcut().on_shortcut(
                            clipboard::CLIP_SHORTCUT,
                            |app, _shortcut, event| {
                                if event.state == ShortcutState::Pressed {
                                    clipboard::toggle_clipboard_window(app);
                                }
                            },
                        ) {
                            log::warn!(
                                "全局快捷键 {} 注册失败（可能已被占用）：{error}",
                                clipboard::CLIP_SHORTCUT
                            );
                        }
                    })
                    .map_err(|error| format!("快捷键注册线程启动失败：{error}"))?;
            }

            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut system = System::new();
                let mut components = Components::new();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    system.refresh_cpu_usage();
                    components.refresh(false);
                    let cpu_temperature = components
                        .iter()
                        .filter(|component| {
                            let label = component.label().to_ascii_lowercase();
                            label.contains("cpu")
                                || label.contains("package")
                                || label.contains("tctl")
                                || label.contains("tdie")
                        })
                        .filter_map(|component| component.temperature())
                        .max_by(f32::total_cmp);
                    if let Err(error) = bus::emit(
                        &app_handle,
                        "system-metrics",
                        &SystemMetrics {
                            cpu_usage: system.global_cpu_usage(),
                            cpu_temperature,
                            game_fps: None,
                        },
                    ) {
                        log::warn!("Failed to emit system metrics: {error}");
                    }
                }
            });

            #[cfg(target_os = "windows")]
            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = disable_window_transitions(&window) {
                    log::warn!("Failed to disable window transitions: {error}");
                }
            }

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                // 通知常驻 helper 退出；崩溃场景由 helper 的父进程存活检查兜底。
                elevated::stop_watch();
            }
        });
}

#[cfg(target_os = "windows")]
fn disable_window_transitions(window: &WebviewWindow) -> Result<(), String> {
    use std::mem::size_of;
    use windows::Win32::Graphics::Dwm::{DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute};

    let disabled = 1i32;
    unsafe {
        DwmSetWindowAttribute(
            window.hwnd().map_err(|error| error.to_string())?,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &disabled as *const i32 as *const _,
            size_of::<i32>() as u32,
        )
    }
    .map_err(|error| error.to_string())
}

/// 重路径命令的最小实机冒烟：只用真实系统 API，不依赖窗口与 IPC。
#[cfg(test)]
mod smoke {
    use super::*;
    use xcap::Monitor;

    fn ffmpeg_available() -> bool {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("orbit-smoke");
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir.join(name)
    }

    #[test]
    fn record_start_stop_produces_video() {
        if !ffmpeg_available() {
            eprintln!("跳过：未找到 ffmpeg");
            return;
        }
        let path = temp_path("record-smoke.mp4");
        let path_str = path.to_string_lossy().to_string();
        let handle = spawn_ffmpeg_recording(std::path::Path::new("ffmpeg"), &path_str)
            .expect("启动录制失败");
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let stopped = finish_recording(handle).expect("停止录制失败");
        assert_eq!(stopped.as_deref(), Some(path_str.as_str()));
        let size = std::fs::metadata(&path).expect("录制文件不存在").len();
        assert!(size > 0, "录制文件为空");
        let _ = std::fs::remove_file(&path);
    }

    fn create_text_png(name: &str) -> std::path::PathBuf {
        let path = temp_path(name);
        let script = r#"
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap 800,200
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::White)
$font = New-Object System.Drawing.Font('Arial',64,[System.Drawing.FontStyle]::Bold)
$g.DrawString('ORBIT 123', $font, [System.Drawing.Brushes]::Black, 20, 50)
$bmp.Save($env:ORBIT_SMOKE_PNG, [System.Drawing.Imaging.ImageFormat]::Png)
"#;
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("ORBIT_SMOKE_PNG", &path)
            .status()
            .expect("启动 PowerShell 失败");
        assert!(status.success(), "生成测试图片失败");
        path
    }

    #[test]
    fn ocr_image_reads_synthetic_text() {
        let path = create_text_png("ocr-smoke.png");
        let text = ocr_image_blocking(&path.to_string_lossy()).expect("OCR 失败");
        let _ = std::fs::remove_file(&path);
        assert!(
            text.to_uppercase().contains("ORBIT"),
            "OCR 未识别出测试文字：{text:?}"
        );
    }

    #[test]
    fn ocr_current_screen_pipeline_captures_primary_screen() {
        let monitor = Monitor::all().expect("枚举显示器失败").remove(0);
        let image = monitor.capture_image().expect("截取屏幕失败");
        let path = temp_path("ocr-screen-smoke.png");
        image.save(&path).expect("保存截图失败");
        let text = ocr_image_blocking(&path.to_string_lossy()).expect("屏幕 OCR 失败");
        let _ = std::fs::remove_file(&path);
        // 屏幕内容不可控，只要求整条链路可执行并返回文本。
        assert!(text.len() < 1_000_000);
    }

    #[test]
    fn convert_tone_to_mp3() {
        if !ffmpeg_available() {
            eprintln!("跳过：未找到 ffmpeg");
            return;
        }
        let input = temp_path("convert-smoke.wav");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.3",
                &input.to_string_lossy(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("生成测试音频失败");
        assert!(status.success(), "生成测试音频失败");

        let output = convert_with_ffmpeg(
            std::path::Path::new("ffmpeg"),
            &input.to_string_lossy(),
            &temp_path("convert-smoke").to_string_lossy(),
            "mp3",
        )
        .expect("转换失败");
        assert!(output.ends_with(".mp3"), "输出扩展名不正确：{output}");
        let size = std::fs::metadata(&output).expect("转换文件不存在").len();
        assert!(size > 0, "转换文件为空");
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }
}
