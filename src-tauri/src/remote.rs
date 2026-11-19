//! 远程只读服务：配对、截屏、OCR，**只绑定回环地址**。
//!
//! 这是远程协助的最小安全子集，不是 Meterpreter 式后渗透：
//! - 没有 stager / stage、进程注入、反射加载；
//! - 没有注册表持久化、开机自启、隐藏驻留；
//! - 没有提权、键鼠注入、shell、文件传输、摄像头/麦克风；
//! - 每次启动生成一次性令牌，只存在内存里，停止或退出即失效；
//! - 所有请求写审计，来源固定为 `remote`；
//! - 端点只接受 127.0.0.1，跨机访问必须由公司 VPN 或 SSH 隧道转发。
//!
//! 协议：4 字节大端长度前缀 + JSON。
//! 请求：`{ "token": "...", "kind": "screenshot" | "ocr", "monitorId": null }`
//! 响应：`{ "ok": true, "data": ... }` 或 `{ "ok": false, "error": "..." }`。

use base64::Engine as _;
use orbit_core::{
    AuditLog, CommandStatus, OcrResult, RemoteEndpoint, RemoteRequestParams,
    RemoteScreenshotResult, RemoteStartParams, RemoteStatus, ScreenshotMode, ScreenshotRequest,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{Cursor, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

const LOOPBACK_HOST: &str = "127.0.0.1";
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// 远程只读服务的进程内状态。服务默认关闭，只有显式命令才能启动。
pub struct RemoteState(Mutex<Option<ServerHandle>>);

impl RemoteState {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }
}

struct ServerHandle {
    addr: SocketAddr,
    token: String,
    running: Arc<AtomicBool>,
    active_sessions: Arc<AtomicU32>,
    join: Option<std::thread::JoinHandle<()>>,
}

/// 启动服务：绑定 127.0.0.1，返回端点和一次性令牌。
pub fn start_server(app: &AppHandle, params: RemoteStartParams) -> Result<RemoteEndpoint, String> {
    let state = app.state::<RemoteState>();
    let mut guard = state.0.lock().map_err(|error| error.to_string())?;
    if guard.is_some() {
        return Err("远程只读服务已在运行。".to_string());
    }

    let listener = TcpListener::bind((LOOPBACK_HOST, params.port.unwrap_or(0)))
        .map_err(|error| format!("无法绑定回环端口：{error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("无法设置服务为非阻塞：{error}"))?;
    let addr = listener
        .local_addr()
        .map_err(|error| format!("无法读取服务端口：{error}"))?;
    let token = generate_token()?;

    let running = Arc::new(AtomicBool::new(true));
    let active_sessions = Arc::new(AtomicU32::new(0));
    let thread_running = Arc::clone(&running);
    let thread_active = Arc::clone(&active_sessions);
    let thread_token = token.clone();
    let thread_app = app.clone();

    let join = std::thread::spawn(move || {
        while thread_running.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _peer)) => {
                    let app = thread_app.clone();
                    let token = thread_token.clone();
                    let active = Arc::clone(&thread_active);
                    active.fetch_add(1, Ordering::SeqCst);
                    std::thread::spawn(move || {
                        let result = handle_connection(app, stream, &token);
                        if let Err(error) = result {
                            log::warn!("远程只读请求失败：{error}");
                        }
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => {
                    log::warn!("远程只读服务 accept 失败：{error}");
                    break;
                }
            }
        }
    });

    *guard = Some(ServerHandle {
        addr,
        token: token.clone(),
        running,
        active_sessions,
        join: Some(join),
    });
    drop(guard);

    emit_state(app)?;
    Ok(RemoteEndpoint {
        host: LOOPBACK_HOST.to_string(),
        port: addr.port(),
        token,
    })
}

/// 停止服务并清空内存中的令牌；重复停止是幂等的。
pub fn stop_server(app: &AppHandle) -> Result<bool, String> {
    let state = app.state::<RemoteState>();
    let mut guard = state.0.lock().map_err(|error| error.to_string())?;
    let Some(mut handle) = guard.take() else {
        return Ok(false);
    };
    handle.running.store(false, Ordering::SeqCst);
    if let Some(join) = handle.join.take() {
        let _ = join.join();
    }
    drop(guard);
    emit_state(app)?;
    Ok(true)
}

pub fn status(app: &AppHandle) -> Result<RemoteStatus, String> {
    let state = app.state::<RemoteState>();
    let guard = state.0.lock().map_err(|error| error.to_string())?;
    Ok(match guard.as_ref() {
        Some(handle) => RemoteStatus {
            running: true,
            endpoint: Some(handle.addr.to_string()),
            active_sessions: handle.active_sessions.load(Ordering::SeqCst),
        },
        None => RemoteStatus {
            running: false,
            endpoint: None,
            active_sessions: 0,
        },
    })
}

/// 控制器：请求远端抓一帧并写入本机剪贴板。
pub fn fetch_screenshot(params: RemoteRequestParams) -> Result<RemoteScreenshotResult, String> {
    let data = request(&params, "screenshot")?;
    let encoded = data
        .get("pngBase64")
        .and_then(Value::as_str)
        .ok_or_else(|| "远程响应缺少截图数据。".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("远程截图解码失败：{error}"))?;
    let image = xcap::image::load_from_memory(&bytes)
        .map_err(|error| format!("远程截图解析失败：{error}"))?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| format!("打开本机剪贴板失败：{error}"))?;
    clipboard
        .set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: std::borrow::Cow::Owned(rgba.into_raw()),
        })
        .map_err(|error| format!("写入本机剪贴板失败：{error}"))?;
    Ok(RemoteScreenshotResult { width, height })
}

/// 控制器：请求远端执行本地 OCR，只回传结构化文本。
pub fn fetch_ocr(params: RemoteRequestParams) -> Result<OcrResult, String> {
    let data = request(&params, "ocr")?;
    serde_json::from_value(data).map_err(|error| format!("远程 OCR 响应解析失败：{error}"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteEnvelope {
    token: String,
    kind: String,
    monitor_id: Option<u32>,
}

fn handle_connection(
    app: AppHandle,
    mut stream: TcpStream,
    expected_token: &str,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|error| error.to_string())?;

    let request_bytes = read_frame(&mut stream, MAX_REQUEST_BYTES)?;
    let envelope: RemoteEnvelope = serde_json::from_slice(&request_bytes)
        .map_err(|error| format!("远程请求格式不合法：{error}"))?;

    if !token_matches(expected_token, &envelope.token) {
        write_response(&mut stream, &json!({ "ok": false, "error": "配对令牌无效。" }))?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    ensure_com();

    let started = Instant::now();
    let kind = envelope.kind;
    let result = match kind.as_str() {
        "screenshot" => remote_screenshot(envelope.monitor_id),
        "ocr" => remote_ocr(envelope.monitor_id),
        other => Err(format!("不支持的远程能力：{other}")),
    };
    let status = if result.is_ok() {
        CommandStatus::Ok
    } else {
        CommandStatus::Error
    };

    if let Some(audit) = app.try_state::<AuditLog>() {
        let command_id = match kind.as_str() {
            "screenshot" => "remote.screenshot",
            "ocr" => "remote.ocr",
            _ => "remote.request",
        };
        let params = json!({ "kind": kind, "monitorId": envelope.monitor_id });
        let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        if let Err(error) = audit.record(command_id, &params, status, duration_ms, "remote") {
            log::error!("远程审计写入失败（{command_id}）：{error}");
        }
    }

    match result {
        Ok(data) => write_response(&mut stream, &json!({ "ok": true, "data": data })),
        Err(error) => write_response(&mut stream, &json!({ "ok": false, "error": error })),
    }
}

fn remote_screenshot(monitor_id: Option<u32>) -> Result<Value, String> {
    let (png, width, height) = capture_png(monitor_id)?;
    Ok(json!({
        "width": width,
        "height": height,
        "pngBase64": base64::engine::general_purpose::STANDARD.encode(png),
    }))
}

fn remote_ocr(monitor_id: Option<u32>) -> Result<Value, String> {
    let (png, _, _) = capture_png(monitor_id)?;
    let path = std::env::temp_dir().join(format!(
        "orbit-remote-{}-{}.png",
        std::process::id(),
        now_nanos()
    ));
    std::fs::write(&path, png).map_err(|error| format!("写入临时截图失败：{error}"))?;
    let path_string = path.to_string_lossy().to_string();
    let result = crate::ocr::default_engine().recognize(&path_string);
    let _ = std::fs::remove_file(&path);
    match result {
        Ok(ocr) => serde_json::to_value(ocr).map_err(|error| error.to_string()),
        Err(error) => Err(error),
    }
}

fn capture_png(monitor_id: Option<u32>) -> Result<(Vec<u8>, u32, u32), String> {
    let (image, _) = crate::screenshot::capture_image(ScreenshotRequest {
        mode: ScreenshotMode::Fullscreen,
        monitor_id,
        x: None,
        y: None,
        width: None,
        height: None,
    })?;
    let width = image.width();
    let height = image.height();
    let mut cursor = Cursor::new(Vec::new());
    xcap::image::DynamicImage::ImageRgba8(image)
        .write_to(&mut cursor, xcap::image::ImageFormat::Png)
        .map_err(|error| format!("远程截图编码失败：{error}"))?;
    Ok((cursor.into_inner(), width, height))
}

fn request(params: &RemoteRequestParams, kind: &str) -> Result<Value, String> {
    let endpoint = parse_endpoint(&params.endpoint)?;
    let mut stream = TcpStream::connect_timeout(&endpoint, IO_TIMEOUT)
        .map_err(|error| format!("连接远程端点失败：{error}"))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|error| error.to_string())?;

    let body = serde_json::to_vec(&json!({
        "token": params.token,
        "kind": kind,
        "monitorId": params.monitor_id,
    }))
    .map_err(|error| error.to_string())?;
    write_frame(&mut stream, &body)?;

    let response_bytes = read_frame(&mut stream, MAX_RESPONSE_BYTES)?;
    let response: Value = serde_json::from_slice(&response_bytes)
        .map_err(|error| format!("远程响应格式不合法：{error}"))?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("远程请求失败。")
            .to_string());
    }
    response
        .get("data")
        .cloned()
        .ok_or_else(|| "远程响应缺少 data。".to_string())
}

fn parse_endpoint(endpoint: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = endpoint
        .parse()
        .map_err(|_| "端点格式应为 127.0.0.1:端口。".to_string())?;
    if !addr.ip().is_loopback() {
        return Err("为安全起见，远程端点只允许回环地址；请使用公司 VPN 或 SSH 隧道。".to_string());
    }
    Ok(addr)
}

fn read_frame(stream: &mut TcpStream, max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut length = [0u8; 4];
    stream
        .read_exact(&mut length)
        .map_err(|error| format!("读取远程帧长度失败：{error}"))?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > max_bytes {
        return Err(format!("远程帧长度超出限制：{length}"));
    }
    let mut payload = vec![0u8; length];
    stream
        .read_exact(&mut payload)
        .map_err(|error| format!("读取远程帧内容失败：{error}"))?;
    Ok(payload)
}

fn write_frame(stream: &mut TcpStream, payload: &[u8]) -> Result<(), String> {
    let length = u32::try_from(payload.len())
        .map_err(|_| "远程帧过大，无法编码长度。".to_string())?;
    stream
        .write_all(&length.to_be_bytes())
        .and_then(|_| stream.write_all(payload))
        .and_then(|_| stream.flush())
        .map_err(|error| format!("写入远程帧失败：{error}"))
}

fn write_response(stream: &mut TcpStream, response: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(response).map_err(|error| error.to_string())?;
    write_frame(stream, &body)
}

fn token_matches(expected: &str, provided: &str) -> bool {
    let expected = expected.as_bytes();
    let provided = provided.as_bytes();
    if expected.len() != provided.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.iter().zip(provided) {
        diff |= a ^ b;
    }
    diff == 0
}

fn generate_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| format!("生成配对令牌失败：{error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn emit_state(app: &AppHandle) -> Result<(), String> {
    let status = status(app)?;
    crate::bus::emit(app, "remote://state", &status)
}

#[cfg(target_os = "windows")]
fn ensure_com() {
    use ::windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    // 已有套间（RPC_E_CHANGED_MODE）也可继续调用 WinRT/xcap，忽略返回值。
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison_is_exact_and_constant_length() {
        assert!(token_matches("abc123", "abc123"));
        assert!(!token_matches("abc123", "abc124"));
        assert!(!token_matches("abc123", "abc12"));
        assert!(!token_matches("abc123", "abc1234"));
    }

    #[test]
    fn endpoint_only_accepts_loopback() {
        assert!(parse_endpoint("127.0.0.1:47100").is_ok());
        assert!(parse_endpoint("192.168.1.20:47100").is_err());
        assert!(parse_endpoint("example.com:47100").is_err());
    }

    #[test]
    fn generated_token_is_256_bit_hex() {
        let token = generate_token().expect("令牌生成失败");
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(token, generate_token().expect("令牌生成失败"));
    }

    #[test]
    fn frame_roundtrip_preserves_payload() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let payload = read_frame(&mut stream, 1024).unwrap();
            write_frame(&mut stream, &payload).unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        write_frame(&mut stream, b"orbit-remote").unwrap();
        let echoed = read_frame(&mut stream, 1024).unwrap();
        assert_eq!(echoed, b"orbit-remote");
        handle.join().unwrap();
    }
}
