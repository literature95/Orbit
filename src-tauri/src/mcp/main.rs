//! Orbit MCP sidecar：stdio JSON-RPC（MCP 2024-11-05）→ 本地桥 → 命令总线。
//!
//! 独立性：本二进制不链接 Tauri，只依赖 `orbit-core`。它从
//! `app_data_dir()/mcp-bridge.json` 读取回环端口与一次性 token，把只读工具调用
//! 转发给正在运行的 Orbit 主进程。stdout 只输出 MCP 消息，日志一律走 stderr。
//!
//! 暴露范围：只有 `orbit_core::read_only_tools()` 里的工具；sidecar 与主进程
//! 各自做一次只读校验（双重保险）。

use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use orbit_core::{BridgeInfo, is_exposed, normalize_arguments, read_only_tools};
use serde_json::{Value, json};

const PROTOCOL_VERSION: &str = "2024-11-05";
const BRIDGE_FILE: &str = "mcp-bridge.json";
/// 应用标识与 `tauri.conf.json` 的 identifier 保持一致。
const APP_IDENTIFIER: &str = "com.orbit.desktop";

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("orbit-mcp: 读取 stdin 失败：{error}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_message(&line) {
            if writeln!(out, "{response}").is_err() || out.flush().is_err() {
                break;
            }
        }
    }
}

/// 处理一条 JSON-RPC 消息；通知（无 id）返回 None，不输出响应。
fn handle_message(line: &str) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return Some(error_response(
                Value::Null,
                -32700,
                format!("JSON 解析失败：{error}"),
            ));
        }
    };
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    // 通知没有 id：initialize 之后的 initialized 是 MCP 客户端必发通知。
    let Some(id) = id else {
        return None;
    };

    match method {
        "initialize" => Some(success(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "orbit-mcp", "version": env!("CARGO_PKG_VERSION") }
            }),
        )),
        "ping" => Some(success(id, json!({}))),
        "tools/list" => match read_only_tools() {
            Ok(tools) => Some(success(id, json!({ "tools": tools }))),
            Err(error) => Some(error_response(
                id,
                -32603,
                format!("读取工具目录失败：{error}"),
            )),
        },
        "tools/call" => Some(handle_tools_call(id, &params)),
        other => Some(error_response(
            id,
            -32601,
            format!("不支持的方法：{other}"),
        )),
    }
}

fn handle_tools_call(id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if name.is_empty() {
        return error_response(id, -32602, "tools/call 缺少 name".to_string());
    }
    if !is_exposed(name) {
        return success(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": format!("命令 {name} 不在 MCP 只读白名单内，已拒绝执行。")
                }],
                "isError": true
            }),
        );
    }
    let arguments = normalize_arguments(params.get("arguments").cloned());
    match call_bridge(name, arguments) {
        Ok(response) => {
            let failed = response.get("status").and_then(Value::as_str) != Some("ok");
            let text = if failed {
                response
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("命令执行失败")
                    .to_string()
            } else {
                serde_json::to_string_pretty(response.get("data").unwrap_or(&Value::Null))
                    .unwrap_or_else(|_| "null".to_string())
            };
            success(
                id,
                json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": failed
                }),
            )
        }
        Err(error) => success(
            id,
            json!({
                "content": [{ "type": "text", "text": error }],
                "isError": true
            }),
        ),
    }
}

/// 连接本地桥并执行一次命令；返回主进程的 `CommandResponse` JSON。
fn call_bridge(command_id: &str, params: Value) -> Result<Value, String> {
    let path = bridge_path();
    let raw = std::fs::read_to_string(&path).map_err(|error| {
        format!(
            "找不到 MCP 桥信息（{}）：{error}。请确认 Orbit 正在运行。",
            path.display()
        )
    })?;
    let info: BridgeInfo =
        serde_json::from_str(&raw).map_err(|error| format!("MCP 桥信息解析失败：{error}"))?;

    let stream = TcpStream::connect(("127.0.0.1", info.port))
        .map_err(|error| format!("连接 Orbit MCP 桥失败：{error}。请确认 Orbit 正在运行。"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;
    let mut writer = stream.try_clone().map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream);

    writeln!(writer, "{}", json!({ "token": info.token }))
        .map_err(|error| format!("提交桥 token 失败：{error}"))?;
    writer.flush().map_err(|error| error.to_string())?;
    let mut ack = String::new();
    reader
        .read_line(&mut ack)
        .map_err(|error| format!("读取桥鉴权结果失败：{error}"))?;
    let ack: Value = serde_json::from_str(&ack).map_err(|error| format!("桥鉴权响应非法：{error}"))?;
    if ack.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err("Orbit MCP 桥拒绝了本次连接（token 不匹配，可能来自旧实例）。".to_string());
    }

    let request = json!({ "commandId": command_id, "params": params });
    writeln!(writer, "{request}").map_err(|error| format!("发送桥请求失败：{error}"))?;
    writer.flush().map_err(|error| error.to_string())?;
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .map_err(|error| format!("读取桥响应失败：{error}"))?;
    serde_json::from_str(&response).map_err(|error| format!("桥响应不是合法 JSON：{error}"))
}

fn bridge_path() -> PathBuf {
    if let Ok(path) = std::env::var("ORBIT_MCP_BRIDGE") {
        return PathBuf::from(path);
    }
    let base = std::env::var("APPDATA").unwrap_or_default();
    PathBuf::from(base).join(APP_IDENTIFIER).join(BRIDGE_FILE)
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
