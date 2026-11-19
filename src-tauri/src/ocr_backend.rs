//! 桌面端 OCR 编排：把"窗口所在显示器 → 截图 → 结构化识别"接起来。
//!
//! 图片识别本身在共享 crate `orbit-ocr`（原生 WinRT 优先、PowerShell 回退），
//! 本模块只负责桌面特有的显示器映射与临时文件生命周期。

use orbit_core::OcrResult;
use tauri::WebviewWindow;
use xcap::Monitor;

use crate::ocr;

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

pub(crate) async fn ocr_current_screen_impl(window: &WebviewWindow) -> Result<OcrResult, String> {
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
        ocr::default_engine().recognize(&path)
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

/// OCR 相关的最小实机冒烟测试（PowerShell 回退路径 + 整屏链路）。
#[cfg(all(test, feature = "smoke"))]
mod smoke {
    use xcap::Monitor;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("orbit-smoke");
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir.join(name)
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
    fn powershell_backend_reads_synthetic_text() {
        let path = create_text_png("ocr-smoke.png");
        let result = orbit_ocr::recognize_with_powershell(&path.to_string_lossy())
            .expect("PowerShell OCR 失败");
        let _ = std::fs::remove_file(&path);
        assert!(
            result.text.to_uppercase().contains("ORBIT"),
            "OCR 未识别出测试文字：{:?}",
            result.text
        );
        assert!(!result.lines.is_empty(), "OCR 结果缺少行信息");
        let words: Vec<_> = result
            .lines
            .iter()
            .flat_map(|line| line.words.iter())
            .collect();
        assert!(!words.is_empty(), "OCR 结果缺少词级 bbox");
        assert!(
            words
                .iter()
                .all(|word| word.bbox[2] > word.bbox[0] && word.bbox[3] > word.bbox[1]),
            "词级 bbox 不是有效矩形"
        );
    }

    #[test]
    fn ocr_current_screen_pipeline_captures_primary_screen() {
        let monitor = Monitor::all().expect("枚举显示器失败").remove(0);
        let image = monitor.capture_image().expect("截取屏幕失败");
        let path = temp_path("ocr-screen-smoke.png");
        image.save(&path).expect("保存截图失败");
        let result = orbit_ocr::recognize_with_powershell(&path.to_string_lossy())
            .expect("屏幕 OCR 失败");
        let _ = std::fs::remove_file(&path);
        // 屏幕内容不可控，只要求整条链路可执行并返回结构化结果。
        assert!(result.text.len() < 1_000_000);
    }
}
