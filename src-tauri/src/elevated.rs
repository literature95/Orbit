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

/// helper 输出的有效期；存活判断与温度读取必须共用同一阈值。
const LIVE_STALE: Duration = Duration::from_secs(30);

fn live_is_fresh(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.elapsed().ok())
        .map(|age| age <= LIVE_STALE)
        .unwrap_or(false)
}

pub fn is_watch_running() -> bool {
    if !helper_flag_path().exists() {
        return false;
    }
    live_is_fresh(&helper_live_path())
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

/// 读取最近一次温度；文件缺失或超过 `LIVE_STALE` 视为不可用。
pub fn read_temperature() -> Option<f32> {
    read_temperature_from(&helper_live_path())
}

/// 路径可注入，便于单测在不依赖真实 helper 的前提下验证解析与过期行为。
fn read_temperature_from(path: &std::path::Path) -> Option<f32> {
    if !live_is_fresh(path) {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
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

    fn temp_live_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "orbit-elevated-test-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir.join("live.json")
    }

    #[test]
    fn temperature_is_none_without_live_file() {
        let path = temp_live_path("missing");
        assert_eq!(read_temperature_from(&path), None, "缺少 live 文件应返回 None");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn temperature_reads_fresh_live_file_and_rejects_bad_payload() {
        let path = temp_live_path("fresh");
        std::fs::write(&path, r#"{"temperature_c": 42.5}"#).unwrap();
        assert_eq!(read_temperature_from(&path), Some(42.5));
        std::fs::write(&path, r#"{"temperature_c": "bad"}"#).unwrap();
        assert_eq!(read_temperature_from(&path), None, "非数值温度应返回 None");
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(read_temperature_from(&path), None, "非法 JSON 应返回 None");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn stale_threshold_is_shared_by_watch_and_read() {
        assert_eq!(LIVE_STALE, Duration::from_secs(30));
    }
}
