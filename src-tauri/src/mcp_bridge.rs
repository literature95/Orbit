//! MCP 本地桥：主进程持有一个仅绑定 127.0.0.1 的 TCP 服务，供 `orbit-mcp`
//! sidecar 转发工具调用到命令总线。
//!
//! 安全边界：
//! - 只监听回环地址，端口随机分配；
//! - 连接后第一行必须提交启动时生成的一次性 token；
//! - 只接受 `RiskLevel::ReadOnly` 的命令（`orbit_core::is_exposed`），
//!   写/破坏性命令一律拒绝，即使 sidecar 校验被绕过也拿不到；
//! - 连接信息写入 `app_data_dir()/mcp-bridge.json`，文件内含 token，
//!   不经过任何网络出口。

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::time::Duration;

use orbit_core::{
    BridgeInfo, CommandRequest, CommandResponse, CommandStatus, ErrorKind, is_exposed,
};
use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

#[derive(Deserialize)]
struct AuthLine {
    token: String,
}

#[derive(Deserialize)]
struct CallLine {
    #[serde(rename = "commandId")]
    command_id: String,
    #[serde(default)]
    params: Value,
}

const BRIDGE_FILE: &str = "mcp-bridge.json";

/// 启动桥并写连接文件；失败时返回错误但不阻断应用启动。
pub(crate) fn start(app: &AppHandle) -> Result<BridgeInfo, String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("MCP 桥监听失败：{error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("MCP 桥读取端口失败：{error}"))?
        .port();
    let info = BridgeInfo { port, token: new_token() };

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("应用数据目录获取失败：{error}"))?;
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let path = dir.join(BRIDGE_FILE);
    let body = serde_json::to_string_pretty(&info).map_err(|error| error.to_string())?;
    std::fs::write(&path, body).map_err(|error| format!("写入 MCP 桥信息失败：{error}"))?;

    let handle = app.clone();
    let token = info.token.clone();
    std::thread::Builder::new()
        .name("orbit-mcp-bridge".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let handle = handle.clone();
                        let token = token.clone();
                        std::thread::spawn(move || {
                            if let Err(error) = serve(stream, handle, token) {
                                log::debug!("MCP 桥连接结束：{error}");
                            }
                        });
                    }
                    Err(error) => log::warn!("MCP 桥接受连接失败：{error}"),
                }
            }
        })
        .map_err(|error| format!("MCP 桥线程启动失败：{error}"))?;

    log::info!("MCP 桥已启动：127.0.0.1:{port}");
    Ok(info)
}

fn serve(stream: TcpStream, app: AppHandle, token: String) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|error| error.to_string())?;
    let mut writer = stream.try_clone().map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream);

    let mut line = String::new();
    if reader.read_line(&mut line).map_err(|error| error.to_string())? == 0 {
        return Ok(());
    }
    let auth: AuthLine = serde_json::from_str(&line)
        .map_err(|_| "桥协议错误：第一行必须是 {token} JSON".to_string())?;
    if !token_eq(&auth.token, &token) {
        let _ = writeln!(writer, "{}", serde_json::json!({"ok": false, "error": "unauthorized"}));
        return Err("MCP 桥拒绝未授权连接".to_string());
    }
    writeln!(writer, "{}", serde_json::json!({"ok": true})).map_err(|error| error.to_string())?;

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) => return Err(format!("读取桥请求失败：{error}")),
        }
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_call(&app, &line);
        let encoded = serde_json::to_string(&response).map_err(|error| error.to_string())?;
        writeln!(writer, "{encoded}").map_err(|error| error.to_string())?;
        if writer.flush().is_err() {
            return Ok(());
        }
    }
}

/// 校验并执行一次桥请求；任何失败都包成 `CommandResponse` 返回给 sidecar。
fn handle_call(app: &AppHandle, line: &str) -> CommandResponse {
    let started = std::time::Instant::now();
    let call: CallLine = match serde_json::from_str(line) {
        Ok(call) => call,
        Err(error) => {
            return failure("", ErrorKind::Param, format!("桥请求解析失败：{error}"), started);
        }
    };
    if !is_exposed(&call.command_id) {
        // 被白名单拦截的调用也要留痕（README §9：result = denied, source = mcp）。
        if let Some(state) = app.try_state::<orbit_core::AuditLog>() {
            let _ = state.record_denied(&call.command_id, &call.params, "mcp");
        }
        return failure(
            &call.command_id,
            ErrorKind::Permission,
            format!("命令 {} 未通过 MCP 只读白名单校验。", call.command_id),
            started,
        );
    }

    let audit = app.state::<orbit_core::AuditLog>().inner().clone();
    let request = CommandRequest {
        command_id: call.command_id.clone(),
        params: call.params,
    };
    let response =
        tauri::async_runtime::block_on(crate::bus::execute(app.clone(), audit, request, "mcp"));
    log::info!(
        "MCP 调用 {}：{:?}（{} ms）",
        response.command_id,
        response.status,
        response.duration_ms
    );
    response
}

fn failure(
    command_id: &str,
    kind: ErrorKind,
    message: String,
    started: std::time::Instant,
) -> CommandResponse {
    CommandResponse {
        command_id: command_id.to_string(),
        status: CommandStatus::Error,
        data: None,
        error: Some(message),
        error_kind: Some(kind),
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    }
}

/// 一次性 token：OS 熵（RandomState）+ 时间 + 进程 id 派生，长度固定 32 hex。
fn new_token() -> String {
    use sha2::{Digest, Sha256};
    use std::hash::RandomState;
    let seed = format!(
        "{:?}|{:?}|{}",
        RandomState::new(),
        std::time::SystemTime::now(),
        std::process::id()
    );
    Sha256::digest(seed.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 定长比较，避免按字节提前返回。
fn token_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_hex_and_unique() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn token_comparison_is_strict() {
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "ab"));
        assert!(!token_eq("", "x"));
    }
}
