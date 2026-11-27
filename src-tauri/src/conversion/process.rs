//! 外部进程运行器：超时、停滞检测、取消与进程树清理。
//!
//! 借鉴 Filee 的 ProcessRunner / StallWatch：无 shell、无窗口、
//! 排空 stderr 防管道写满，卡住时不拖垮调用方。

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::NoWindow;
use crate::conversion::CancelToken;

pub(crate) struct RunOutcome {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stderr: String,
}

/// 运行外部程序。stall 为 0 时禁用停滞检测。
pub(crate) fn run(
    executable: &Path,
    args: &[&str],
    timeout: Duration,
    stall: Duration,
    cancel: &CancelToken,
) -> Result<RunOutcome, String> {
    let mut child = Command::new(executable)
        .no_window()
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                format!("找不到外部程序：{}", executable.display())
            } else {
                format!("启动外部程序失败：{error}")
            }
        })?;

    let last_output = Arc::new(Mutex::new(Instant::now()));
    let stderr_thread = if let Some(stderr) = child.stderr.take() {
        let last = Arc::clone(&last_output);
        Some(std::thread::spawn(move || {
            let mut reader = stderr;
            let mut buffer = String::new();
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => {
                        buffer.push_str(&String::from_utf8_lossy(&chunk[..count]));
                        if let Ok(mut stamp) = last.lock() {
                            *stamp = Instant::now();
                        }
                    }
                    Err(_) => break,
                }
            }
            buffer
        }))
    } else {
        None
    };

    let started = Instant::now();
    let status = loop {
        if cancel.is_cancelled() {
            kill_tree(&mut child);
            return Err("转换已取消".to_string());
        }
        if started.elapsed() > timeout {
            kill_tree(&mut child);
            return Err(format!("转换超时（超过 {} 秒）", timeout.as_secs()));
        }
        if !stall.is_zero() {
            let quiet = last_output.lock().map(|stamp| stamp.elapsed()).unwrap_or_default();
            if quiet > stall {
                kill_tree(&mut child);
                return Err(format!("转换停滞（{} 秒无输出）", stall.as_secs()));
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                kill_tree(&mut child);
                return Err(format!("等待外部程序失败：{error}"));
            }
        }
    };
    let stderr = stderr_thread.and_then(|handle| handle.join().ok()).unwrap_or_default();
    Ok(RunOutcome { status, stderr })
}

fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        // 先让 Windows 结束整棵进程树，再兜底 kill 直接子进程。
        let pid = child.id().to_string();
        let _ = Command::new("taskkill")
            .no_window()
            .args(["/T", "/F", "/PID", &pid])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
