// orbit-elevated-helper
// 主进程普通权限运行；需要温度时由主进程用 PowerShell `-Verb RunAs` 拉起本 helper。
// helper 自身没有嵌入 requireAdministrator manifest，提权发生在启动它的 ShellExecute 上。
//
// 用法：orbit-elevated-helper --watch <flag> <out> <parent_pid>
//   常驻模式：一次 UAC 授权后循环读温度并原子写入 <out>（JSON），
//   <flag> 被删除或父进程退出时自行结束并清理 <out>。

#![windows_subsystem = "windows"]

use serde::Serialize;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Serialize, Default)]
struct HelperResult {
    success: bool,
    temperature_c: Option<f32>,
    error: Option<String>,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    let Some(pos) = args.iter().position(|a| a == "--watch") else {
        eprintln!("用法：orbit-elevated-helper --watch <flag> <out> <parent_pid>");
        return ExitCode::FAILURE;
    };
    let (Some(flag), Some(out), Some(pid)) = (
        args.get(pos + 1),
        args.get(pos + 2),
        args.get(pos + 3).and_then(|s| s.parse::<u32>().ok()),
    ) else {
        eprintln!("--watch 需要 <flag> <out> <parent_pid> 三个参数");
        return ExitCode::FAILURE;
    };
    watch_loop(PathBuf::from(flag), PathBuf::from(out), pid)
}

fn watch_loop(flag: PathBuf, out: PathBuf, parent_pid: u32) -> ExitCode {
    while flag.exists() && parent_alive(parent_pid) {
        let mut result = HelperResult::default();
        match read_temperature() {
            Ok(c) => {
                result.temperature_c = Some(c);
                result.success = true;
            }
            Err(e) => result.error = Some(e),
        }
        if let Ok(json) = serde_json::to_string(&result) {
            // 原子写：先写临时文件再 rename，主进程不会读到半截 JSON。
            let tmp = out.with_extension("tmp");
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &out);
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(out.with_extension("tmp"));
    ExitCode::SUCCESS
}

#[cfg(target_os = "windows")]
fn parent_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        // 259 = STILL_ACTIVE：进程句柄可开且退出码仍为 ACTIVE 才算活着。
        let alive = GetExitCodeProcess(handle, &mut code).is_ok() && code == 259;
        let _ = CloseHandle(handle);
        alive
    }
}

#[cfg(not(target_os = "windows"))]
fn parent_alive(_pid: u32) -> bool {
    true
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    #[test]
    fn detects_own_process_as_alive() {
        assert!(super::parent_alive(std::process::id()));
    }

    #[test]
    fn watch_writes_output_and_exits_on_flag_removal() {
        let dir = std::env::temp_dir();
        let flag = dir.join("orbit_helper_test.flag");
        let out = dir.join("orbit_helper_test.json");
        std::fs::write(&flag, "test").unwrap();
        let out_clone = out.clone();
        let flag_for_thread = flag.clone();
        let handle = std::thread::spawn(move || {
            super::watch_loop(flag_for_thread, out_clone, std::process::id());
        });
        // 第一轮写入应在 5 秒内出现（温度查询失败也会写带 error 的 JSON）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !out.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert!(out.exists(), "watch 未写出共享文件");
        let raw = std::fs::read_to_string(&out).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(value.get("success").is_some());
        std::fs::remove_file(&flag).unwrap();
        handle.join().unwrap();
        assert!(!out.exists(), "退出后未清理共享文件");
    }
}

fn read_temperature() -> Result<f32, String> {
    // WMI MSAcpi_ThermalZoneTemperature 开尔文 → 摄氏度
    let ps = r#"
try {
    $t = Get-WmiObject -Namespace root/wmi -Class MSAcpi_ThermalZoneTemperature -ErrorAction SilentlyContinue |
        Measure-Object -Property CurrentTemperature -Average
    if ($t -and $t.Average) {
        [math]::Round(($t.Average - 2732) / 10, 1)
    } else {
        Write-Output 'NaN'
    }
} catch {
    Write-Output 'NaN'
}
"#;
    let mut cmd = std::process::Command::new("powershell.exe");
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd
        .args(["-NoProfile", "-NonInteractive", "-Command", ps])
        .output()
        .map_err(|e| format!("PS 启动失败：{}", e))?;

    if !output.status.success() {
        return Err(format!("PS 退出码：{:?}", output.status.code()));
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s == "NaN" {
        return Err("MSAcpi_ThermalZoneTemperature 不可用（可能需要管理员或硬件不支持）".into());
    }
    s.parse::<f32>().map_err(|_| format!("无法解析温度：{}", s))
}


