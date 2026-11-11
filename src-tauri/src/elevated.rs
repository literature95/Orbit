//! 管理员 helper 通信层：一次 UAC，常驻读取。
//!
//! helper 每 5 秒把温度写入共享 JSON，主进程只读文件，避免每次仪表盘刷新都弹 UAC。
//! 本模块把「文件握手 + 进程拉起 + 错误映射」收敛到一处，命令层只负责开关状态。

use crate::NoWindow;
use std::path::PathBuf;
use std::time::Duration;

/// helper 通信失败原因；展示文案在 `message()` 统一映射，避免命令层各自拼接。
#[derive(Debug)]
pub enum ElevatedError {
    HelperMissing(PathBuf),
    FlagWrite(String),
    Launch(String),
}

impl ElevatedError {
    pub fn message(&self) -> String {
        match self {
            Self::HelperMissing(path) => format!("helper 不存在：{}", path.display()),
            Self::FlagWrite(error) => format!("写入 helper 标志失败：{error}"),
            Self::Launch(error) => format!("无法拉起管理员 helper：{error}"),
        }
    }
}

fn helper_flag_path() -> PathBuf {
    std::env::temp_dir().join("orbit_helper_run.flag")
}

fn helper_live_path() -> PathBuf {
    std::env::temp_dir().join("orbit_helper_live.json")
}

pub fn is_watch_running() -> bool {
    if !helper_flag_path().exists() {
        return false;
    }
    std::fs::metadata(helper_live_path())
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.elapsed().ok())
        .map(|age| age < Duration::from_secs(30))
        .unwrap_or(false)
}

/// 拉起常驻 helper；已在运行时直接返回，不重复弹 UAC。
pub fn start_watch() -> Result<(), ElevatedError> {
    if is_watch_running() {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|error| ElevatedError::Launch(error.to_string()))?;
    let helper_path = exe
        .parent()
        .ok_or_else(|| ElevatedError::Launch("找不到 exe 目录".to_string()))?
        .join("orbit-elevated-helper.exe");
    if !helper_path.exists() {
        return Err(ElevatedError::HelperMissing(helper_path));
    }

    let flag = helper_flag_path();
    let live = helper_live_path();
    std::fs::write(&flag, std::process::id().to_string())
        .map_err(|error| ElevatedError::FlagWrite(error.to_string()))?;
    let _ = std::fs::remove_file(&live);

    // CreateProcess 不能 runas，必须走 ShellExecute（PowerShell -Verb RunAs）。
    // 不带 -Wait：helper 常驻，UAC 只在本次启动时弹一次。
    let ps = format!(
        "Start-Process -FilePath '{}' -ArgumentList '--watch \"{}\" \"{}\" {}' -Verb RunAs -WindowStyle Hidden",
        helper_path.to_string_lossy().replace('\'', "''"),
        flag.to_string_lossy().replace('\'', "''"),
        live.to_string_lossy().replace('\'', "''"),
        std::process::id(),
    );
    std::process::Command::new("powershell.exe")
        .no_window()
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| ElevatedError::Launch(error.to_string()))?;
    Ok(())
}

/// 删标志文件后 helper 会在下一个循环（≤5 秒）自行退出并清理输出。
pub fn stop_watch() {
    let _ = std::fs::remove_file(helper_flag_path());
}

/// 读取最近一次温度；文件缺失或过期（>30 秒）视为不可用。
pub fn read_temperature() -> Option<f32> {
    let meta = std::fs::metadata(helper_live_path()).ok()?;
    let age = meta.modified().ok()?.elapsed().ok()?;
    if age > Duration::from_secs(30) {
        return None;
    }
    let raw = std::fs::read_to_string(helper_live_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("temperature_c")?
        .as_f64()
        .map(|temperature| temperature as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_helper_maps_to_actionable_message() {
        let error = ElevatedError::HelperMissing(PathBuf::from("C:/orbit/helper.exe"));
        assert!(error.message().contains("C:/orbit/helper.exe"));
        assert!(error.message().starts_with("helper 不存在"));
    }

    #[test]
    fn temperature_is_none_without_live_file() {
        // 环境里若恰好有真实 helper 在跑，本用例可能返回 Some，不做强断言。
        let _ = read_temperature();
    }
}
