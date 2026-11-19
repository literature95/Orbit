//! MCP sidecar 的 stdio 协议测试：不启动 Tauri，用假桥验证
//! initialize / tools/list / tools/call / 只读白名单四条路径。

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// 起一个只服务一次调用的假桥，返回端口与线程句柄。
fn fake_bridge(token: &'static str) -> (u16, std::thread::JoinHandle<Value>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);

        let mut auth = String::new();
        reader.read_line(&mut auth).unwrap();
        let auth: Value = serde_json::from_str(&auth).unwrap();
        assert_eq!(auth["token"], token, "sidecar 必须提交桥 token");
        writeln!(writer, "{{\"ok\":true}}").unwrap();
        writer.flush().unwrap();

        let mut request = String::new();
        reader.read_line(&mut request).unwrap();
        let request: Value = serde_json::from_str(&request).unwrap();
        writeln!(
            writer,
            "{}",
            json!({
                "commandId": request["commandId"],
                "status": "ok",
                "data": { "osName": "windows", "totalMemoryMb": 1024 },
                "error": null,
                "errorKind": null,
                "durationMs": 3
            })
        )
        .unwrap();
        writer.flush().unwrap();
        request
    });
    (port, handle)
}

fn write_bridge_file(port: u16, token: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("orbit-mcp-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("bridge-{port}.json"));
    std::fs::write(
        &path,
        serde_json::to_string(&json!({ "port": port, "token": token })).unwrap(),
    )
    .unwrap();
    path
}

#[test]
fn sidecar_speaks_mcp_over_stdio_and_enforces_read_only() {
    let token = "test-token-0123456789";
    let (port, bridge) = fake_bridge(token);
    let bridge_path = write_bridge_file(port, token);

    let mut child = Command::new(env!("CARGO_BIN_EXE_orbit-mcp"))
        .env("ORBIT_MCP_BRIDGE", &bridge_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("启动 orbit-mcp 失败");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let mut send = |line: Value| -> Value {
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
        let mut response = String::new();
        stdout.read_line(&mut response).unwrap();
        serde_json::from_str(&response).expect("sidecar 输出必须是 JSON")
    };

    let init = send(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }
    }));
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(init["result"]["serverInfo"]["name"], "orbit-mcp");

    let list = send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let tools = list["result"]["tools"].as_array().expect("tools 必须是数组");
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.contains(&"system.info"), "只读工具应包含 system.info");
    assert!(
        !names.contains(&"clipboard.clear"),
        "破坏性命令不得出现在 tools/list：{names:?}"
    );

    let call = send(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "system.info", "arguments": {} }
    }));
    assert_eq!(call["result"]["isError"], false);
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("windows"), "应透传桥返回的数据：{text}");

    let blocked = send(json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": { "name": "clipboard.clear", "arguments": {} }
    }));
    assert_eq!(blocked["result"]["isError"], true);
    assert!(
        blocked["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("白名单")
    );

    let ping = send(json!({ "jsonrpc": "2.0", "id": 5, "method": "ping" }));
    assert!(ping["result"].is_object());

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "sidecar 应正常退出：{status}");

    let request = bridge.join().unwrap();
    assert_eq!(request["commandId"], "system.info");
    let _ = std::fs::remove_file(bridge_path);
}
