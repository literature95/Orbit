//! Orbit 远程 OCR：服务端 + 客户端 + 截屏上传。
//!
//! 数据流（与 `docs/REMOTE_OCR.md` 一致）：
//!
//! ```text
//! 客户端机器                         服务端机器
//! capture_primary_screen_png()  →  POST /v1/ocr (PNG bytes)
//!                                  →  保存 PNG 到 data_dir/images/
//!                                  →  orbit_ocr::recognize(path)
//!     打印 / 返回 OcrResult      ←  {"ok":true,"image":...,"result":{...}}
//! ```
//!
//! 安全边界：Bearer token 鉴权、只接受 `POST /v1/ocr` 与 `GET /health`、
//! 无执行/文件浏览接口；TLS 由反向代理终止（见文档）。客户端是显式命令行
//! 工具，不常驻、不隐藏。

pub mod http;

use orbit_core::OcrResult;
use std::{
    io::Write,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const DEFAULT_BIND: &str = "127.0.0.1:8787";
pub const DEFAULT_MAX_BODY: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub bind: String,
    pub token: String,
    pub data_dir: PathBuf,
    pub max_body: usize,
}

impl ServerConfig {
    pub fn new(token: impl Into<String>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            bind: DEFAULT_BIND.to_string(),
            token: token.into(),
            data_dir: data_dir.into(),
            max_body: DEFAULT_MAX_BODY,
        }
    }
}

/// 生成 32 字节随机 token（64 hex）。
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("系统随机数不可用");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn bearer_token(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("authorization"))
        .and_then(|(_, value)| value.strip_prefix("Bearer ").or_else(|| value.strip_prefix("bearer ")))
        .map(str::trim)
}

/// 启动服务端并阻塞。打印 `listening on <addr>` 后进入 accept 循环。
pub fn run(config: ServerConfig) -> std::io::Result<()> {
    std::fs::create_dir_all(config.data_dir.join("images"))?;
    let listener = TcpListener::bind(&config.bind)?;
    let address = listener.local_addr()?;
    println!("orbit-ocr-server listening on {address}");
    println!("data dir: {}", config.data_dir.display());
    use std::io::Write as _;
    std::io::stdout().flush().ok();

    let config = Arc::new(config);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let config = Arc::clone(&config);
                std::thread::spawn(move || {
                    if let Err(error) = handle_connection(stream, &config) {
                        eprintln!("请求处理失败：{error}");
                    }
                });
            }
            Err(error) => eprintln!("接受连接失败：{error}"),
        }
    }
    Ok(())
}

fn handle_connection(mut stream: TcpStream, config: &ServerConfig) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let request = match http::read_request(&mut stream, config.max_body) {
        Ok(request) => request,
        Err(error) => {
            let status = if matches!(error, http::HttpError::TooLarge) { 413 } else { 400 };
            return respond_json(&mut stream, status, &error_body(&error.to_string()));
        }
    };

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => respond_json(
            &mut stream,
            200,
            &serde_json::json!({
                "ok": true,
                "service": "orbit-ocr",
                "version": env!("CARGO_PKG_VERSION"),
            })
            .to_string(),
        ),
        ("POST", "/v1/ocr") => {
            let authorized = bearer_token(&request.headers)
                .map(|token| constant_time_eq(token, &config.token))
                .unwrap_or(false);
            if !authorized {
                return respond_json(
                    &mut stream,
                    401,
                    &serde_json::json!({"ok": false, "error": "未授权：缺少或错误的 Bearer token"}).to_string(),
                );
            }
            if request.body.is_empty() {
                return respond_json(
                    &mut stream,
                    400,
                    &serde_json::json!({"ok": false, "error": "请求体为空，应为 PNG 字节"}).to_string(),
                );
            }
            let image_path = save_image(&config.data_dir, &request.body)?;
            match orbit_ocr::recognize(&image_path.to_string_lossy()) {
                Ok(result) => {
                    let body = serde_json::json!({
                        "ok": true,
                        "image": image_path.to_string_lossy(),
                        "result": result,
                    });
                    respond_json(&mut stream, 200, &body.to_string())
                }
                Err(error) => respond_json(
                    &mut stream,
                    500,
                    &serde_json::json!({"ok": false, "error": error}).to_string(),
                ),
            }
        }
        _ => respond_json(
            &mut stream,
            404,
            &serde_json::json!({"ok": false, "error": "未知路由"}).to_string(),
        ),
    }
}

fn error_body(message: &str) -> String {
    serde_json::json!({"ok": false, "error": message}).to_string()
}

fn respond_json(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    http::write_response(stream, status, "application/json; charset=utf-8", body.as_bytes())
}

/// 落盘：`<data_dir>/images/<unix_ms>-<counter>.png`，返回绝对/相对路径。
fn save_image(data_dir: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let images = data_dir.join("images");
    std::fs::create_dir_all(&images)?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let mut path = images.join(format!("{millis}.png"));
    let mut counter = 0u32;
    while path.exists() {
        counter += 1;
        path = images.join(format!("{millis}-{counter}.png"));
    }
    let mut file = std::fs::File::create(&path)?;
    file.write_all(bytes)?;
    Ok(path)
}

/// 服务端响应解包。
#[derive(Debug, serde::Deserialize)]
pub struct UploadResponse {
    pub ok: bool,
    #[serde(default)]
    pub image: Option<PathBuf>,
    #[serde(default)]
    pub result: Option<OcrResult>,
    #[serde(default)]
    pub error: Option<String>,
}

/// 客户端：把 PNG 字节 POST 到服务端，返回解析后的结果。
pub fn upload_png(server: &str, token: &str, png: &[u8]) -> Result<UploadResponse, String> {
    let mut stream = TcpStream::connect(server)
        .map_err(|error| format!("连接服务端 {server} 失败：{error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|error| format!("设置读超时失败：{error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(60)))
        .map_err(|error| format!("设置写超时失败：{error}"))?;

    let header = format!(
        "POST /v1/ocr HTTP/1.1\r\nHost: {server}\r\nAuthorization: Bearer {token}\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        png.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(png))
        .and_then(|_| stream.flush())
        .map_err(|error| format!("上传失败：{error}"))?;

    let mut raw = Vec::new();
    use std::io::Read as _;
    stream
        .read_to_end(&mut raw)
        .map_err(|error| format!("读取响应失败：{error}"))?;
    let text = String::from_utf8(raw).map_err(|error| format!("响应不是 UTF-8：{error}"))?;
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| "响应缺少头部/正文分隔".to_string())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| "响应缺少状态码".to_string())?;
    let parsed: UploadResponse = serde_json::from_str(body)
        .map_err(|error| format!("响应 JSON 解析失败（HTTP {status}）：{error}；正文：{body}"))?;
    Ok(parsed)
}

/// 截取主显示器并编码为 PNG 字节（客户端侧，Windows）。
#[cfg(target_os = "windows")]
pub fn capture_primary_screen_png() -> Result<Vec<u8>, String> {
    let monitors = xcap::Monitor::all().map_err(|error| format!("枚举显示器失败：{error}"))?;
    let monitor = monitors
        .into_iter()
        .next()
        .ok_or_else(|| "没有可用显示器".to_string())?;
    let image = monitor
        .capture_image()
        .map_err(|error| format!("截屏失败：{error}"))?;
    let mut buffer = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut buffer, xcap::image::ImageFormat::Png)
        .map_err(|error| format!("PNG 编码失败：{error}"))?;
    Ok(buffer.into_inner())
}

#[cfg(not(target_os = "windows"))]
pub fn capture_primary_screen_png() -> Result<Vec<u8>, String> {
    Err("当前平台暂不支持截屏上传".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_generation_is_64_hex_chars() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn constant_time_compare_matches_only_equal_tokens() {
        assert!(constant_time_eq("abc123", "abc123"));
        assert!(!constant_time_eq("abc123", "abc124"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("", "x"));
    }

    #[test]
    fn bearer_parsing_accepts_both_cases_and_rejects_others() {
        let headers = vec![("Authorization".to_string(), "Bearer T".to_string())];
        assert_eq!(bearer_token(&headers), Some("T"));
        let headers = vec![("authorization".to_string(), "bearer T".to_string())];
        assert_eq!(bearer_token(&headers), Some("T"));
        let headers = vec![("authorization".to_string(), "Basic T".to_string())];
        assert_eq!(bearer_token(&headers), None);
        assert_eq!(bearer_token(&[]), None);
    }

    #[test]
    fn save_image_writes_unique_files() {
        let dir = std::env::temp_dir().join(format!("orbit-ocr-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = save_image(&dir, b"one").unwrap();
        let second = save_image(&dir, b"two").unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
