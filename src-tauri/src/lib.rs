use orbit_core::{
    AuditLog, CommandRequest, CommandResponse, CommandStatus, ScreenshotRequest, command_catalog,
};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use sysinfo::{Components, System};
use tauri::Manager;
use tauri::{
    AppHandle, Emitter, PhysicalPosition, PhysicalSize, WebviewWindow, Window,
    menu::{ContextMenu, Menu, MenuItem},
};
use tauri_plugin_autostart::ManagerExt;
use xcap::Monitor;

mod screenshot;

const FLOATING_SIZE: (f64, f64) = (104.0, 104.0);
const SIDE_SIZE: (f64, f64) = (54.0, 132.0);
const NOTCH_SIZE: (f64, f64) = (292.0, 62.0);
const ISLAND_SIZE: (f64, f64) = (420.0, 260.0);
const EDGE_THRESHOLD: i32 = 72;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum DockPosition {
    #[default]
    Floating,
    Left,
    Right,
    Top,
}

#[derive(Clone, Copy)]
struct OverlayLayout {
    dock: DockPosition,
    expanded: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlaySnapshot {
    dock: DockPosition,
    expanded: bool,
}

#[derive(Default)]
struct OverlayState(Mutex<OverlayLayout>);
struct ElevatedToggle(pub Mutex<bool>); // 用户启用后才会拉 helper

#[tauri::command]
fn set_elevated_enabled(app: AppHandle, enabled: bool) -> Result<bool, String> {
    // 首次启用弹 UAC（拉一次 helper 验证权限）
    if enabled {
        let _ = spawn_elevated_helper("temperature"); // 忽略结果，主要是触发 UAC
    }
    *app.state::<ElevatedToggle>().0.lock().unwrap() = enabled;
    Ok(enabled)
}

#[tauri::command]
fn get_elevated_status(app: AppHandle) -> bool {
    *app.state::<ElevatedToggle>().0.lock().unwrap()
}

impl Default for OverlayLayout {
    fn default() -> Self {
        Self {
            dock: DockPosition::Floating,
            expanded: false,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemMetrics {
    cpu_usage: f32,
    cpu_temperature: Option<f32>,
    game_fps: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupResult {
    permissions: HashMap<String, PermissionState>,
    autostart_configured: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum PermissionState {
    Unknown,
}

#[tauri::command]
fn complete_first_run_setup(app: AppHandle) -> Result<SetupResult, String> {
    set_autostart_enabled(app, true)
}

#[tauri::command]
fn get_setup_status(app: AppHandle) -> Result<SetupResult, String> {
    setup_result(&app)
}

#[tauri::command]
fn set_autostart_enabled(app: AppHandle, enabled: bool) -> Result<SetupResult, String> {
    let autostart = app.autolaunch();
    if enabled {
        autostart.enable().map_err(|error| error.to_string())?;
    } else {
        autostart.disable().map_err(|error| error.to_string())?;
    }

    let result = setup_result(&app)?;
    if result.autostart_configured != enabled {
        return Err(if enabled {
            "登录启动项注册后仍处于关闭状态。".to_string()
        } else {
            "登录启动项已关闭，但状态检查仍显示启用。".to_string()
        });
    }
    Ok(result)
}

fn setup_result(app: &AppHandle) -> Result<SetupResult, String> {
    let permissions = [
        ("screen-capture", PermissionState::Unknown),
        ("accessibility-input", PermissionState::Unknown),
    ]
    .into_iter()
    .map(|(id, state)| (id.to_string(), state))
    .collect();
    let autostart_configured = app
        .autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())?;

    Ok(SetupResult {
        permissions,
        autostart_configured,
    })
}

#[tauri::command]
fn get_command_catalog() -> Result<Vec<orbit_core::CommandSpec>, String> {
    command_catalog().map_err(|error| error.to_string())
}

#[tauri::command]
async fn execute_command(
    audit: tauri::State<'_, AuditLog>,
    request: CommandRequest,
) -> Result<CommandResponse, String> {
    execute_request(audit.inner().clone(), request).await
}

async fn execute_request(
    audit: AuditLog,
    request: CommandRequest,
) -> Result<CommandResponse, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let command_id = request.command_id.clone();
        let result = if command_id == "screenshot.capture" {
            serde_json::from_value::<ScreenshotRequest>(request.params.clone())
                .map_err(|error| error.to_string())
                .and_then(screenshot::capture_to_clipboard)
        } else {
            Err(format!("未知命令：{command_id}"))
        };
        let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let status = if result.is_ok() {
            CommandStatus::Ok
        } else {
            CommandStatus::Error
        };
        audit
            .record(&command_id, &request.params, status, duration_ms)
            .map_err(|error| format!("命令执行结果无法写入审计日志：{error}"))?;

        let (data, error) = match result {
            Ok(data) => (Some(data), None),
            Err(error) => (None, Some(error)),
        };
        Ok(CommandResponse {
            command_id,
            status,
            data,
            error,
            duration_ms,
        })
    })
    .await
    .map_err(|error| format!("命令工作线程异常退出：{error}"))?
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegionSelectionStart {
    monitor_id: u32,
}

#[tauri::command]
fn start_region_selection(
    app: AppHandle,
    state: tauri::State<'_, OverlayState>,
) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 Orbit 主窗口。".to_string())?;
    let selection = app
        .get_webview_window("selection")
        .ok_or_else(|| "找不到区域截图窗口。".to_string())?;
    let monitor = main
        .current_monitor()
        .map_err(|error| error.to_string())?
        .or(main.primary_monitor().map_err(|error| error.to_string())?)
        .ok_or_else(|| "无法确定区域截图所在的显示器。".to_string())?;
    let position = monitor.position();
    let size = monitor.size();
    let capture_monitor = Monitor::all()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|candidate| {
            candidate.x().ok() == Some(position.x)
                && candidate.y().ok() == Some(position.y)
                && candidate.width().ok() == Some(size.width)
                && candidate.height().ok() == Some(size.height)
        })
        .ok_or_else(|| "无法将窗口显示器映射到截图设备。".to_string())?;
    let monitor_id = capture_monitor.id().map_err(|error| error.to_string())?;

    selection
        .set_size(PhysicalSize::new(size.width, size.height))
        .map_err(|error| error.to_string())?;
    selection
        .set_position(PhysicalPosition::new(position.x, position.y))
        .map_err(|error| error.to_string())?;
    selection
        .emit(
            "region-selection-start",
            RegionSelectionStart { monitor_id },
        )
        .map_err(|error| error.to_string())?;

    main.hide().map_err(|error| error.to_string())?;
    if let Err(error) = selection.show() {
        let _ = main.show();
        return Err(error.to_string());
    }
    if let Err(error) = selection.set_focus() {
        let _ = selection.hide();
        return match restore_main_overlay(&app, state.inner()) {
            Ok(()) => Err(error.to_string()),
            Err(restore_error) => Err(format!(
                "无法聚焦区域截图窗口：{error}；恢复 Orbit 主窗口失败：{restore_error}"
            )),
        };
    }
    Ok(())
}

fn restore_main_overlay(app: &AppHandle, state: &OverlayState) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 Orbit 主窗口。".to_string())?;
    let layout = *state.0.lock().map_err(|error| error.to_string())?;
    set_overlay_layout(&main, layout.dock, layout.expanded)?;
    main.show().map_err(|error| error.to_string())
}

#[tauri::command]
async fn finish_region_selection(
    app: AppHandle,
    audit: tauri::State<'_, AuditLog>,
    state: tauri::State<'_, OverlayState>,
    monitor_id: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<CommandResponse, String> {
    let selection = app
        .get_webview_window("selection")
        .ok_or_else(|| "找不到区域截图窗口。".to_string())?;
    selection.hide().map_err(|error| error.to_string())?;
    let request = CommandRequest {
        command_id: "screenshot.capture".to_string(),
        params: serde_json::json!({
            "mode": "region",
            "monitorId": monitor_id,
            "x": x,
            "y": y,
            "width": width,
            "height": height
        }),
    };
    let result = execute_request(audit.inner().clone(), request).await;
    let restore = restore_main_overlay(&app, state.inner());
    match (result, restore) {
        (Ok(response), Ok(())) => Ok(response),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(format!("截图已处理，但无法恢复 Orbit 主窗口：{error}")),
        (Err(capture_error), Err(restore_error)) => Err(format!(
            "{capture_error}；恢复 Orbit 主窗口失败：{restore_error}"
        )),
    }
}

#[tauri::command]
fn cancel_region_selection(
    app: AppHandle,
    state: tauri::State<'_, OverlayState>,
) -> Result<(), String> {
    let selection = app
        .get_webview_window("selection")
        .ok_or_else(|| "找不到区域截图窗口。".to_string())?;
    selection.hide().map_err(|error| error.to_string())?;
    restore_main_overlay(&app, state.inner())
}

fn monitor_bounds(window: &WebviewWindow) -> Result<(i32, i32, i32, i32), String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let current_size = window.outer_size().map_err(|error| error.to_string())?;
    let center_x = position.x + current_size.width as i32 / 2;
    let center_y = position.y + current_size.height as i32 / 2;
    let monitor = window
        .monitor_from_point(center_x as f64, center_y as f64)
        .map_err(|error| error.to_string())?
        .or(window
            .primary_monitor()
            .map_err(|error| error.to_string())?);
    monitor
        .map(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            (
                position.x,
                position.y,
                position.x + size.width as i32,
                position.y + size.height as i32,
            )
        })
        .ok_or_else(|| "无法确定悬浮球所在的显示器。".to_string())
}

fn dock_position(window: &WebviewWindow) -> Result<DockPosition, String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let size = window.outer_size().map_err(|error| error.to_string())?;
    let (left, top, right, _) = monitor_bounds(window)?;
    Ok(determine_dock_position(
        position.x,
        position.y,
        size.width as i32,
        left,
        top,
        right,
    ))
}

fn determine_dock_position(
    x: i32,
    y: i32,
    width: i32,
    left: i32,
    top: i32,
    right: i32,
) -> DockPosition {
    if y <= top + EDGE_THRESHOLD {
        DockPosition::Top
    } else if x <= left + EDGE_THRESHOLD {
        DockPosition::Left
    } else if x + width >= right - EDGE_THRESHOLD {
        DockPosition::Right
    } else {
        DockPosition::Floating
    }
}

fn set_overlay_layout(
    window: &WebviewWindow,
    dock: DockPosition,
    expanded: bool,
) -> Result<(), String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let current_size = window.outer_size().map_err(|error| error.to_string())?;
    let scale_factor = window.scale_factor().map_err(|error| error.to_string())?;
    let center_x = position.x + current_size.width as i32 / 2;
    let center_y = position.y + current_size.height as i32 / 2;
    let (monitor_left, monitor_top, monitor_right, monitor_bottom) = monitor_bounds(window)?;
    let logical_size = match (dock, expanded) {
        (DockPosition::Floating, false) => FLOATING_SIZE,
        (DockPosition::Floating, true) => (400.0, 400.0),
        (DockPosition::Left | DockPosition::Right, false) => SIDE_SIZE,
        (DockPosition::Top, false) => NOTCH_SIZE,
        (DockPosition::Left | DockPosition::Right, true) => (400.0, 400.0),
        (DockPosition::Top, true) => (400.0, 400.0),
    };
    let target_width = (logical_size.0 * scale_factor).round() as u32;
    let target_height = (logical_size.1 * scale_factor).round() as u32;
    let max_x = monitor_right - target_width as i32;
    let max_y = monitor_bottom - target_height as i32;
    let target_x = match dock {
        DockPosition::Floating => center_x - target_width as i32 / 2,
        DockPosition::Left => monitor_left,
        DockPosition::Right => max_x,
        DockPosition::Top => {
            (monitor_left + (monitor_right - monitor_left) / 2) - target_width as i32 / 2
        }
    }
    .clamp(monitor_left, max_x.max(monitor_left));
    let target_y = match dock {
        DockPosition::Top => monitor_top,
        DockPosition::Left | DockPosition::Right | DockPosition::Floating => {
            center_y - target_height as i32 / 2
        }
    }
    .clamp(monitor_top, max_y.max(monitor_top));

    window
        .set_size(tauri::LogicalSize::new(logical_size.0, logical_size.1))
        .map_err(|error| error.to_string())?;
    window
        .set_position(PhysicalPosition::new(target_x, target_y))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn dock_overlay(
    window: WebviewWindow,
    state: tauri::State<'_, OverlayState>,
) -> Result<OverlaySnapshot, String> {
    let dock = dock_position(&window)?;
    let mut layout = state.0.lock().map_err(|error| error.to_string())?;
    if layout.dock != dock {
        layout.dock = dock;
        layout.expanded = false;
    }
    set_overlay_layout(&window, layout.dock, layout.expanded)?;
    Ok(OverlaySnapshot {
        dock: layout.dock,
        expanded: layout.expanded,
    })
}

#[cfg(test)]
mod tests {
    use super::{DockPosition, determine_dock_position};

    #[test]
    fn top_edge_takes_priority_at_screen_corners() {
        assert_eq!(
            determine_dock_position(0, 0, 104, 0, 0, 1920),
            DockPosition::Top
        );
    }

    #[test]
    fn side_edges_dock_to_the_corresponding_sidebar() {
        assert_eq!(
            determine_dock_position(20, 500, 104, 0, 0, 1920),
            DockPosition::Left
        );
        assert_eq!(
            determine_dock_position(1810, 500, 104, 0, 0, 1920),
            DockPosition::Right
        );
    }

    #[test]
    fn window_near_no_edge_remains_floating() {
        assert_eq!(
            determine_dock_position(500, 300, 104, 0, 0, 1920),
            DockPosition::Floating
        );
    }
}

#[tauri::command]
fn set_overlay_open(
    window: WebviewWindow,
    state: tauri::State<'_, OverlayState>,
    open: bool,
) -> Result<(), String> {
    let mut layout = state.0.lock().map_err(|error| error.to_string())?;
    set_overlay_layout(&window, layout.dock, open)?;
    layout.expanded = open;
    Ok(())
}

#[tauri::command]
fn show_orb_context_menu(app: AppHandle, window: Window) -> Result<(), String> {
    let configure = MenuItem::with_id(&app, "orb--configure", "一键设置", true, None::<&str>)
        .map_err(|error| error.to_string())?;
    let quit = MenuItem::with_id(&app, "orb--quit", "退出 Orbit", true, None::<&str>)
        .map_err(|error| error.to_string())?;
    let menu = Menu::with_items(&app, &[&configure, &quit]).map_err(|error| error.to_string())?;
    menu.popup(window).map_err(|error| error.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemInfo {
    os_name: String,
    os_version: String,
    screen_width: u32,
    screen_height: u32,
    scale_factor: f64,
    total_memory_mb: u64,
    available_memory_mb: u64,
}

#[tauri::command]
fn get_system_info(window: WebviewWindow) -> Result<SystemInfo, String> {
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or(window.primary_monitor().map_err(|e| e.to_string())?)
        .ok_or_else(|| "无法获取屏幕信息".to_string())?;
    let size = monitor.size();
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let mut sys = System::new();
    sys.refresh_memory();
    // 获取 OS 版本（Tauri 2 内置不可用，用 sysinfo 版本或硬编码）
    let os_version = sysinfo::System::kernel_version()
        .unwrap_or_else(|| "unknown".to_string());
    Ok(SystemInfo {
        os_name: std::env::consts::OS.to_string(),
        os_version,
        screen_width: size.width,
        screen_height: size.height,
        scale_factor: scale,
        total_memory_mb: sys.total_memory() / 1024,
        available_memory_mb: sys.available_memory() / 1024,
    })
}

// ============ 灵动岛仪表盘 ============

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DashInfo {
    cpu_percent: f32,
    memory_used_percent: f32,
    memory_used_mb: u64,
    memory_total_mb: u64,
    uptime_secs: u64,
    time_now: String,
    clipboard_count: usize,
    temperature_c: Option<f32>,
}

// 用 ShellExecuteW runas 拉起 helper.exe（会弹 UAC），读 stdout
fn spawn_elevated_helper(query: &str) -> Result<serde_json::Value, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("exe 路径获取失败：{}", e))?;
    let helper_path = exe
        .parent()
        .ok_or_else(|| "找不到 exe 目录".to_string())?
        .join(if cfg!(debug_assertions) {
            "orbit-elevated-helper.exe"
        } else {
            "orbit-elevated-helper.exe"
        });

    if !helper_path.exists() {
        return Err(format!("helper 不存在：{}", helper_path.display()));
    }

    // 用 cmd /c shell:runas 拉起 + 重定向 stdout 到临时文件
    let tmp_out = std::env::temp_dir().join("orbit_helper_out.json");
    let tmp_err = std::env::temp_dir().join("orbit_helper_err.txt");

    let helper_str = helper_path.to_string_lossy().to_string();
    let out_str = tmp_out.to_string_lossy().to_string();
    let err_str = tmp_err.to_string_lossy().to_string();
    let args = format!(
        "/c start /wait /b \"\" runas /user:%USERNAME% \"{helper_str}\" --query {query} > \"{out_str}\" 2> \"{err_str}\""
    );

    // 简化：直接用 CreateProcess 让 helper 自己以 runas 启动
    // 但 CreateProcess 不能 runas，必须 ShellExecute
    // 所以用 PowerShell Start-Process -Verb RunAs -Wait -RedirectStandardOutput
    let ps = format!(
        "$p = Start-Process -FilePath '{}' -ArgumentList '--query {}' -Verb RunAs -Wait -PassThru -WindowStyle Hidden -RedirectStandardOutput '{}' -RedirectStandardError '{}'; exit $p.ExitCode",
        helper_str.replace("'", "''"),
        query,
        out_str.replace("'", "''"),
        err_str.replace("'", "''"),
    );

    let _ = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .status();

    // 读输出
    let raw = std::fs::read_to_string(&tmp_out).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp_out);
    let _ = std::fs::remove_file(&tmp_err);

    // 解析 JSON
    serde_json::from_str::<serde_json::Value>(raw.trim())
        .map_err(|e| format!("helper 输出解析失败：{} (raw='{}')", e, raw))
}

#[tauri::command]
fn get_dash_info(app: AppHandle) -> Result<DashInfo, String> {
    let mut sys = System::new();
    sys.refresh_cpu_all();
    sys.refresh_memory();

    let cpu_percent = sys
        .cpus()
        .iter()
        .map(|c| c.cpu_usage())
        .sum::<f32>()
        / std::cmp::max(sys.cpus().len(), 1) as f32;

    let total_mem = sys.total_memory() / 1024;
    let avail_mem = sys.available_memory() / 1024;
    let used_mb = total_mem.saturating_sub(avail_mem);
    let mem_percent = if total_mem > 0 {
        (used_mb as f32 / total_mem as f32) * 100.0
    } else {
        0.0
    };

    let uptime_secs = System::uptime();
    let time_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string());

    // 只有用户启用了管理员增强才弹 UAC 拉 helper
    let enabled = *app.state::<ElevatedToggle>().0.lock().unwrap();
    let temperature_c = if enabled {
        spawn_elevated_helper("temperature")
            .ok()
            .and_then(|v| v.get("temperature_c").and_then(|t| t.as_f64()).map(|f| f as f32))
    } else {
        None
    };

    Ok(DashInfo {
        cpu_percent,
        memory_used_percent: mem_percent,
        memory_used_mb: used_mb,
        memory_total_mb: total_mem,
        uptime_secs,
        time_now,
        clipboard_count: 0,
        temperature_c,
    })
}

// ============ 剪贴板 ============

#[tauri::command]
fn clip_read() -> Result<String, String> {
    let mut board = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    board.get_text().map_err(|e| e.to_string())
}

#[tauri::command]
fn clip_write(text: String) -> Result<(), String> {
    let mut board = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    board.set_text(text).map_err(|e| e.to_string())?;
    Ok(())
}

// ============ 录屏 ============

struct RecordState(Mutex<Option<RecordHandle>>);
struct RecordHandle {
    path: String,
    child: std::process::Child,
}

#[tauri::command]
fn record_start(app: AppHandle) -> Result<String, String> {
    // 找 FFmpeg（sidecar 或 PATH）
    let ffmpeg = match std::env::var_os("FFMPEG_PATH") {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            // 先查 sidecar 目录，再查 PATH
            let sidecar = app.path().resolve("ffmpeg", tauri::path::BaseDirectory::Resource)
                .map_err(|e| format!("sidecar 路径错误：{}", e))?;
            if sidecar.exists() { sidecar }
            else { std::path::PathBuf::from("ffmpeg") }
        }
    };

    // 生成输出路径：桌面/Orbit/orbit_rec_*.mp4
    let home = std::env::var("USERPROFILE")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(std::path::PathBuf::from))
        .map_err(|_| "找不到用户目录".to_string())?;
    let dir = home.join("Desktop").join("Orbit");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let filename = format!(
        "orbit_rec_{}.mp4",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    );
    let path = dir.join(&filename);
    let path_str = path.to_string_lossy().to_string();

    // FFmpeg gdigrab 全屏录制，H264 编码，4Mbps
    let args = [
        "-y",
        "-f", "gdigrab",
        "-framerate", "30",
        "-draw_mouse", "1",
        "-i", "desktop",
        "-c:v", "libx264",
        "-preset", "veryfast",
        "-b:v", "4M",
        "-pix_fmt", "yuv420p",
        path_str.as_str(),
    ];

    let child = std::process::Command::new(&ffmpeg)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "找不到 ffmpeg。请安装 ffmpeg 并加入 PATH，或设置 FFMPEG_PATH 环境变量。".to_string()
            } else {
                format!("FFmpeg 启动失败：{}", e)
            }
        })?;

    let state = app.state::<RecordState>();
    *state.0.lock().unwrap() = Some(RecordHandle { path: path_str.clone(), child });
    Ok(path_str)
}

#[tauri::command]
fn record_stop(app: AppHandle) -> Result<Option<String>, String> {
    let state = app.state::<RecordState>();
    let handle = state.0.lock().unwrap().take().ok_or_else(|| "没有正在录制的会话".to_string())?;
    drop(state); // 释放 Mutex 锁

    let pid = handle.child.id();
    // Windows taskkill 给进程发优雅关闭信号（FFmpeg 收到后会 flush buffer 再退出）
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // 给 FFmpeg 2 秒写文件，然后强制收尾
    let mut child = handle.child;
    std::thread::sleep(std::time::Duration::from_secs(2));
    let _ = child.kill();
    let _ = child.wait();

    Ok(Some(handle.path))
}

// ============ OCR ============

#[tauri::command]
fn ocr_image(image_path: String) -> Result<String, String> {
    // Windows.Media.Ocr 通过 PowerShell 调用（Windows 10+ 内置）
    let ps_script = format!(
        r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Storage.StorageFile, Windows.Storage, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Media.Ocr.OcrEngine, Windows.Media.Ocr, ContentType=WindowsRuntime]

function Await($op) {{
    $task = [System.WindowsRuntimeSystemExtensions]::AsTask($op)
    $task.Wait() | Out-Null
    return $task.Result
}}

$path = '{image_path}'
$file = Await([Windows.Storage.StorageFile]::GetFileFromPathAsync($path))
$ras = Await($file.OpenAsync([Windows.Storage.FileAccessMode]::Read))
$decoder = Await([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($ras))
$bitmap = Await($decoder.GetSoftwareBitmapAsync())
$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
$result = Await($engine.RecognizeAsync($bitmap))
$result.Text
"#
    );

    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_script])
        .output()
        .map_err(|e| format!("PowerShell 启动失败：{}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&output.stderr).to_string();
        Err(format!("OCR 失败：{}", err))
    }
}

#[tauri::command]
fn ocr_current_screen() -> Result<String, String> {
    // 1. 用 xcap 全屏截图存临时 PNG
    let monitors = xcap::Monitor::all().map_err(|e| e.to_string())?;
    let monitor = monitors.first().ok_or_else(|| "找不到显示器".to_string())?;
    let image = monitor.capture_image().map_err(|e| e.to_string())?;

    let tmp = std::env::temp_dir().join("orbit_ocr_temp.png");
    image
        .save(&tmp)
        .map_err(|e| format!("临时截图保存失败：{}", e))?;

    // RAII guard: 函数返回时无论如何都会清理
    let _cleanup = TempFileGuard(tmp.clone());

    let path = tmp.to_string_lossy().to_string();
    ocr_image(path) // 直接返回 OCR 结果，cleanup guard drop 时自动删文件
}

// RAII 临时文件清理 guard
struct TempFileGuard(std::path::PathBuf);
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ============ 键鼠录制 ============

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct KbEvent {
    timestamp: u64,
    kind: String,
    key: Option<String>,
    x: Option<i32>,
    y: Option<i32>,
    button: Option<String>,
    direction: Option<String>,
}

struct KbRecordState(pub Mutex<Option<Arc<Mutex<Vec<KbEvent>>>>>);

#[tauri::command]
fn kb_record_start(app: AppHandle) -> Result<(), String> {
    let state = app.state::<KbRecordState>();
    let mut guard = state.0.lock().unwrap();
    if guard.is_some() {
        return Err("已经在录制中".to_string());
    }

    // 新的事件缓冲区
    let events = Arc::new(Mutex::new(Vec::<KbEvent>::new()));
    let start = std::time::Instant::now();
    let ev_clone = events.clone();
    let _guard = guard; // 保持 Mutex 锁定，防止 stop 在 listen 线程启动前读到 None

    // rdev listen 是阻塞的，必须放到独立 std::thread
    std::thread::Builder::new()
        .name("orbit-kb-listener".into())
        .spawn(move || {
            use rdev::{listen, Event, EventType};
            let _ = listen(move |event: Event| {
                let ts = start.elapsed().as_millis() as u64;
                let kind = match &event.event_type {
                    EventType::KeyPress(_) => "key_press",
                    EventType::KeyRelease(_) => "key_release",
                    EventType::MouseMove { .. } => "mouse_move",
                    EventType::ButtonPress(_) | EventType::ButtonRelease(_) => "mouse_button",
                    EventType::Wheel { .. } => "mouse_wheel",
                };
                let key = match &event.event_type {
                    EventType::KeyPress(k) | EventType::KeyRelease(k) => Some(format!("{:?}", k)),
                    _ => None,
                };
                let (x, y) = match &event.event_type {
                    EventType::MouseMove { x, y } => (Some(*x as i32), Some(*y as i32)),
                    _ => (None, None),
                };
                let button = match &event.event_type {
                    EventType::ButtonPress(b) | EventType::ButtonRelease(b) => Some(format!("{:?}", b)),
                    _ => None,
                };
                let direction = match &event.event_type {
                    EventType::Wheel { delta_y, .. } => {
                        Some(if *delta_y > 0 { "down".to_string() } else { "up".to_string() })
                    }
                    _ => None,
                };
                ev_clone.lock().unwrap().push(KbEvent {
                    timestamp: ts,
                    kind: kind.into(),
                    key, x, y, button, direction,
                });
            });
        })
        .map_err(|e| format!("键鼠监听器启动失败：{}", e))?;

    // 把 Arc 存到 state
    drop(_guard);
    *state.0.lock().unwrap() = Some(events);
    Ok(())
}

#[tauri::command]
fn kb_record_stop(app: AppHandle) -> Result<Vec<KbEvent>, String> {
    let state = app.state::<KbRecordState>();
    let arc_opt = state.0.lock().unwrap().take();
    let arc = arc_opt.ok_or_else(|| "没有正在录制的会话".to_string())?;
    // 读出所有事件（listener 线程仍在后台跑，Arc drop 后 clone 那份还在，但我们不关心）
    let events = arc.lock().unwrap().clone();
    Ok(events)
}

// ============ 格式转换 ============

#[tauri::command]
fn convert_format(input_path: String, output_path: String, format: String) -> Result<String, String> {
    // 用 FFmpeg CLI，用户 PATH 里要有 ffmpeg
    let mut output = output_path.clone();
    if !output.ends_with(&format) {
        output = format!("{}.{}", output.trim_end_matches('.'), format);
    }
    let out_path = std::path::Path::new(&output);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let args: Vec<&str> = match format.as_str() {
        "mp3" => vec!["-y", "-i", &input_path, "-vn", "-acodec", "libmp3lame", "-q:a", "2", &output],
        "gif" => vec!["-y", "-i", &input_path, "-vf", "fps=12,scale=480:-1", &output],
        "webp" => vec!["-y", "-i", &input_path, "-vf", "fps=15", "-loop", "0", &output],
        _ => vec!["-y", "-i", &input_path, &output],
    };

    let status = std::process::Command::new("ffmpeg")
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "找不到 ffmpeg，请安装并加入 PATH".to_string()
            } else {
                format!("FFmpeg 启动失败：{}", e)
            }
        })?;

    if status.success() { Ok(output) }
    else { Err(format!("转换失败（退出码：{:?}）", status.code())) }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    unsafe {
        // Set this before Tauri initializes WebKitGTK.
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(OverlayState::default())
        .manage(RecordState(Mutex::new(None)))
        .manage(KbRecordState(Mutex::new(None)))
        .manage(ElevatedToggle(Mutex::new(false)))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("Orbit")
                .build(),
        )
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(tauri_plugin_window_state::StateFlags::POSITION)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            complete_first_run_setup,
            get_setup_status,
            set_autostart_enabled,
            get_command_catalog,
            execute_command,
            start_region_selection,
            finish_region_selection,
            cancel_region_selection,
            dock_overlay,
            set_overlay_open,
            show_orb_context_menu,
            get_system_info,
            get_dash_info,
            set_elevated_enabled,
            get_elevated_status,
            clip_read,
            clip_write,
            record_start,
            record_stop,
            ocr_image,
            ocr_current_screen,
            kb_record_start,
            kb_record_stop,
            convert_format
        ])
        .on_menu_event(|app, event| match event.id().as_ref() {
            "orb--configure" => {
                if let Err(error) = app.emit("orb-setup-request", ()) {
                    log::error!("Failed to dispatch setup request: {error}");
                }
            }
            "orb--quit" => app.exit(0),
            _ => {}
        })
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let audit = AuditLog::open(app_data_dir.join("orbit.db"))?;
            app.manage(audit);

            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut system = System::new();
                let mut components = Components::new();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    system.refresh_cpu_usage();
                    components.refresh(false);
                    let cpu_temperature = components
                        .iter()
                        .filter(|component| {
                            let label = component.label().to_ascii_lowercase();
                            label.contains("cpu")
                                || label.contains("package")
                                || label.contains("tctl")
                                || label.contains("tdie")
                        })
                        .filter_map(|component| component.temperature())
                        .max_by(f32::total_cmp);
                    if let Err(error) = app_handle.emit(
                        "system-metrics",
                        SystemMetrics {
                            cpu_usage: system.global_cpu_usage(),
                            cpu_temperature,
                            game_fps: None,
                        },
                    ) {
                        log::warn!("Failed to emit system metrics: {error}");
                    }
                }
            });

            #[cfg(target_os = "windows")]
            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = disable_window_transitions(&window) {
                    log::warn!("Failed to disable window transitions: {error}");
                }
            }

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}

#[cfg(target_os = "windows")]
fn disable_window_transitions(window: &WebviewWindow) -> Result<(), String> {
    use std::mem::size_of;
    use windows::Win32::Graphics::Dwm::{DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute};

    let disabled = 1i32;
    unsafe {
        DwmSetWindowAttribute(
            window.hwnd().map_err(|error| error.to_string())?,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &disabled as *const i32 as *const _,
            size_of::<i32>() as u32,
        )
    }
    .map_err(|error| error.to_string())
}
