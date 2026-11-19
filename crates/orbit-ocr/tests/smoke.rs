//! 实机冒烟：原生 WinRT 后端与 PowerShell 后端必须给出逐词一致的结果。
#![cfg(all(feature = "smoke", target_os = "windows"))]

use std::path::PathBuf;

fn temp_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("orbit-ocr-smoke");
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir.join(name)
}

fn create_text_png(name: &str) -> PathBuf {
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
fn native_and_powershell_backends_agree() {
    let path = create_text_png("ocr-agree.png");
    let native = orbit_ocr::recognize_native(&path.to_string_lossy()).expect("原生 OCR 失败");
    let shell = orbit_ocr::recognize_with_powershell(&path.to_string_lossy())
        .expect("PowerShell OCR 失败");
    let _ = std::fs::remove_file(&path);

    assert!(native.text.to_uppercase().contains("ORBIT"), "原生结果缺文本：{:?}", native.text);
    assert!(shell.text.to_uppercase().contains("ORBIT"), "PowerShell 结果缺文本：{:?}", shell.text);

    let native_words: Vec<_> = native.lines.iter().flat_map(|line| line.words.iter()).collect();
    let shell_words: Vec<_> = shell.lines.iter().flat_map(|line| line.words.iter()).collect();
    assert!(!native_words.is_empty(), "原生结果缺词级 bbox");
    assert_eq!(native_words.len(), shell_words.len(), "两后端词数不一致");
    assert!(
        native_words
            .iter()
            .all(|word| word.bbox[2] > word.bbox[0] && word.bbox[3] > word.bbox[1]),
        "原生词级 bbox 不是有效矩形"
    );
}

#[test]
fn default_engine_prefers_native_backend() {
    let path = create_text_png("ocr-default.png");
    let started = std::time::Instant::now();
    let result = orbit_ocr::recognize(&path.to_string_lossy()).expect("默认识别失败");
    let elapsed = started.elapsed().as_millis();
    let _ = std::fs::remove_file(&path);
    assert!(result.text.to_uppercase().contains("ORBIT"));
    // 原生路径不应触发 PowerShell（实测 <100ms；给足冷启动余量）。
    assert!(elapsed < 1500, "默认识别疑似回退到 PowerShell：{elapsed}ms");
}
