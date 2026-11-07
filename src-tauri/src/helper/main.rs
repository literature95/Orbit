// orbit-elevated-helper
// 主进程普通权限跑，需要管理员功能时（温度/网络）调用这个 helper。
// 本文件编译成独立二进制，manifest 要求 requireAdministrator。
// 
// 用法：orbit-elevated-helper --query temperature   → stdout 输出 JSON
//       orbit-elevated-helper --query all           → 全部

#![windows_subsystem = "windows"]

use serde::Serialize;
use std::env;
use std::process::ExitCode;

#[derive(Serialize, Default)]
struct HelperResult {
    success: bool,
    temperature_c: Option<f32>,
    disks: Vec<DiskInfo>,
    error: Option<String>,
}

#[derive(Serialize)]
struct DiskInfo {
    name: String,
    total_gb: f64,
    used_gb: f64,
    used_percent: f32,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let query = args.iter().position(|a| a == "--query")
        .and_then(|i| args.get(i + 1).cloned())
        .unwrap_or_else(|| "all".into());

    let mut result = HelperResult::default();

    // 温度：通过 PowerShell 调 WMI
    if query == "temperature" || query == "all" {
        match read_temperature() {
            Ok(c) => { result.temperature_c = Some(c); result.success = true; }
            Err(e) => { result.error = Some(e); }
        }
    }

    // 磁盘：sysinfo 读（磁盘普通权限就能读，但一起做了方便）
    if query == "disks" || query == "all" {
        result.disks = read_disks();
    }

    let json = serde_json::to_string(&result).unwrap_or_else(|_| r#"{"success":false,"error":"json serialize failed"}"#.into());
    println!("{}", json);
    ExitCode::SUCCESS
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
    let output = std::process::Command::new("powershell.exe")
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

fn read_disks() -> Vec<DiskInfo> {
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();
    disks.iter().map(|d| {
        let total = d.total_space() as f64 / (1024.0 * 1024.0 * 1024.0);
        let avail = d.available_space() as f64 / (1024.0 * 1024.0 * 1024.0);
        let used = total - avail;
        let pct: f32 = if total > 0.0 { (used / total * 100.0) as f32 } else { 0.0 };
        DiskInfo {
            name: d.name().to_string_lossy().to_string(),
            total_gb: (total * 10.0).round() / 10.0,
            used_gb: (used * 10.0).round() / 10.0,
            used_percent: pct.round(),
        }
    }).collect()
}
