//! 端到端冒烟：真实启动 `orbit-ocr-server`，上传合成文字图，校验落盘与识别结果。
#![cfg(all(feature = "smoke", target_os = "windows"))]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Server {
    child: Child,
    address: String,
    data_dir: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_server(token: &str, data_dir: &PathBuf) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_orbit-ocr-server"))
        .args([
            "--bind",
            "127.0.0.1:0",
            "--token",
            token,
            "--data-dir",
        ])
        .arg(data_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("启动服务端失败");

    let stdout = child.stdout.take().expect("服务端 stdout 不可用");
    let mut reader = BufReader::new(stdout);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut line = String::new();
    let address = loop {
        line.clear();
        if reader.read_line(&mut line).expect("读取服务端输出失败") == 0 {
            panic!("服务端在打印监听地址前退出");
        }
        if let Some(rest) = line.trim().strip_prefix("orbit-ocr-server listening on ") {
            break rest.to_string();
        }
        assert!(Instant::now() < deadline, "等待监听地址超时，最后一行：{line}");
    };
    Server {
        child,
        address,
        data_dir: data_dir.clone(),
    }
}

fn create_text_png(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("orbit-ocr-e2e");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let script = r#"
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap 800,200
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::White)
$font = New-Object System.Drawing.Font('Arial',64,[System.Drawing.FontStyle]::Bold)
$g.DrawString('ORBIT 123', $font, [System.Drawing.Brushes]::Black, 20, 50)
$bmp.Save($env:ORBIT_SMOKE_PNG, [System.Drawing.Imaging.ImageFormat]::Png)
"#;
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("ORBIT_SMOKE_PNG", &path)
        .status()
        .expect("启动 PowerShell 失败");
    assert!(status.success(), "生成测试图片失败");
    path
}

#[test]
fn upload_image_is_saved_recognized_and_returned() {
    let token = orbit_ocr_server::generate_token();
    let data_dir = std::env::temp_dir().join(format!("orbit-ocr-e2e-data-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    let server = start_server(&token, &data_dir);

    let png_path = create_text_png("e2e.png");
    let png = std::fs::read(&png_path).unwrap();
    let response = orbit_ocr_server::upload_png(&server.address, &token, &png)
        .expect("上传应成功");
    assert!(response.ok, "服务端返回失败：{:?}", response.error);
    let result = response.result.expect("缺少 OCR 结果");
    assert!(
        result.text.to_uppercase().contains("ORBIT"),
        "OCR 未识别出测试文字：{:?}",
        result.text
    );
    assert!(!result.lines.is_empty(), "缺少行信息");

    let saved = response.image.expect("缺少落盘路径");
    assert!(saved.starts_with(&server.data_dir), "落盘路径不在数据目录：{saved:?}");
    assert!(saved.exists(), "落盘文件不存在：{saved:?}");
    assert_eq!(std::fs::read(&saved).unwrap(), png, "落盘内容应与上传字节一致");

    let _ = std::fs::remove_file(&png_path);
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[test]
fn wrong_token_is_rejected_with_401() {
    let token = orbit_ocr_server::generate_token();
    let data_dir = std::env::temp_dir().join(format!("orbit-ocr-e2e-auth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    let server = start_server(&token, &data_dir);

    let png = std::fs::read(create_text_png("e2e-auth.png")).unwrap();
    let response = orbit_ocr_server::upload_png(&server.address, "deadbeef", &png)
        .expect("HTTP 层应返回可解析响应");
    assert!(!response.ok, "错误 token 不应成功");
    let error = response.error.unwrap_or_default();
    assert!(error.contains("未授权"), "错误信息应说明未授权：{error}");

    // 拒绝的请求不应落盘。
    let images = server.data_dir.join("images");
    let count = std::fs::read_dir(&images).map(|dir| dir.count()).unwrap_or(0);
    assert_eq!(count, 0, "未授权请求不应保存图片");

    let _ = std::fs::remove_dir_all(&data_dir);
}
