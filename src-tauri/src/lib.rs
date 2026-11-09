use orbit_core::{
    AuditLog, CommandRequest, CommandResponse, CommandStatus, ScreenshotRequest, command_catalog,
};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
use sysinfo::{Components, System};
use tauri::Manager;
use tauri::{
    AppHandle, Emitter, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt;
use xcap::Monitor;

mod clipboard;
mod input_recorder;
mod screenshot;

const FLOATING_SIZE: (f64, f64) = (104.0, 104.0);
const SIDE_SIZE: (f64, f64) = (54.0, 132.0);
const NOTCH_SIZE: (f64, f64) = (320.0, 62.0);
/// 展开圆盘窗口：紧贴最大外圈（半径 132）并留出顶部提示、底部按钮的位置
const EXPANDED_SIZE: (f64, f64) = (340.0, 340.0);
/// 贴近屏幕边缘多少像素内才吸附，避免大老远就触发停靠
const EDGE_THRESHOLD: i32 = 24;

/// 子进程一律不创建控制台窗口：OCR / 温度 / 录屏 / 转换调用外部程序时不再闪黑框。
trait NoWindow {
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for std::process::Command {
    #[cfg(target_os = "windows")]
    fn no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        self.creation_flags(CREATE_NO_WINDOW)
    }

    #[cfg(not(target_os = "windows"))]
    fn no_window(&mut self) -> &mut Self {
        self
    }
}

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
    if enabled {
        // 一次 UAC 拉起常驻 helper；已在运行则不重复弹。
        start_elevated_watch()?;
    } else {
        stop_elevated_watch();
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
        (DockPosition::Floating, true) => EXPANDED_SIZE,
        (DockPosition::Left | DockPosition::Right, false) => SIDE_SIZE,
        (DockPosition::Top, false) => NOTCH_SIZE,
        (DockPosition::Left | DockPosition::Right, true) => EXPANDED_SIZE,
        (DockPosition::Top, true) => EXPANDED_SIZE,
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

    #[test]
    fn window_forty_pixels_from_edge_stays_floating() {
        assert_eq!(
            determine_dock_position(40, 500, 104, 0, 0, 1920),
            DockPosition::Floating
        );
        assert_eq!(
            determine_dock_position(1740, 500, 104, 0, 0, 1920),
            DockPosition::Floating
        );
    }

    #[test]
    fn dash_state_samples_plausible_live_values() {
        let dash = super::DashState::new();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let (cpu, total_mb, avail_mb) = dash.sample();
        assert!(
            cpu.is_finite() && (0.0..=100.0).contains(&cpu),
            "cpu={cpu}"
        );
        assert!(total_mb > 0, "total_mb={total_mb}");
        assert!(avail_mb <= total_mb, "avail={avail_mb} total={total_mb}");
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
fn quit_app(app: AppHandle) {
    app.exit(0);
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

/// CPU 使用率是两次采样之间的差值，必须复用同一个 System；
/// 每次调用都新建实例只能拿到首次采样的垃圾值（常见 0% 或 100%）。
struct DashState(Mutex<System>);

impl DashState {
    fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        Self(Mutex::new(system))
    }

    /// 返回 (CPU 占用百分比, 总内存 MB, 可用内存 MB)。
    fn sample(&self) -> (f32, u64, u64) {
        let mut sys = self.0.lock().unwrap();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        (
            sys.global_cpu_usage(),
            sys.total_memory() / 1024,
            sys.available_memory() / 1024,
        )
    }
}

// 管理员增强采用「一次 UAC、常驻读取」：helper --watch 每 5 秒把温度写入
// 共享 JSON，主进程只读文件，避免每次仪表盘刷新都弹 UAC。
fn helper_flag_path() -> std::path::PathBuf {
    std::env::temp_dir().join("orbit_helper_run.flag")
}

fn helper_live_path() -> std::path::PathBuf {
    std::env::temp_dir().join("orbit_helper_live.json")
}

fn elevated_watch_running() -> bool {
    if !helper_flag_path().exists() {
        return false;
    }
    std::fs::metadata(helper_live_path())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|age| age < std::time::Duration::from_secs(30))
        .unwrap_or(false)
}

fn start_elevated_watch() -> Result<(), String> {
    if elevated_watch_running() {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| format!("exe 路径获取失败：{e}"))?;
    let helper_path = exe
        .parent()
        .ok_or_else(|| "找不到 exe 目录".to_string())?
        .join("orbit-elevated-helper.exe");
    if !helper_path.exists() {
        return Err(format!("helper 不存在：{}", helper_path.display()));
    }

    let flag = helper_flag_path();
    let out = helper_live_path();
    std::fs::write(&flag, std::process::id().to_string()).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&out);

    // CreateProcess 不能 runas，必须走 ShellExecute（PowerShell -Verb RunAs）。
    // 不带 -Wait：helper 常驻，UAC 只在本次启动时弹一次。
    let ps = format!(
        "Start-Process -FilePath '{}' -ArgumentList '--watch \"{}\" \"{}\" {}' -Verb RunAs -WindowStyle Hidden",
        helper_path.to_string_lossy().replace("'", "''"),
        flag.to_string_lossy().replace("'", "''"),
        out.to_string_lossy().replace("'", "''"),
        std::process::id(),
    );
    std::process::Command::new("powershell.exe")
        .no_window()
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("无法拉起管理员 helper：{e}"))?;
    Ok(())
}

fn stop_elevated_watch() {
    // 删标志文件后 helper 会在下一个循环（≤5 秒）自行退出并清理输出。
    let _ = std::fs::remove_file(helper_flag_path());
}

fn read_live_temperature() -> Option<f32> {
    let meta = std::fs::metadata(helper_live_path()).ok()?;
    let age = meta.modified().ok()?.elapsed().ok()?;
    if age > std::time::Duration::from_secs(30) {
        return None;
    }
    let raw = std::fs::read_to_string(helper_live_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("temperature_c")?
        .as_f64()
        .map(|t| t as f32)
}

#[tauri::command]
fn get_dash_info(app: AppHandle, dash: tauri::State<'_, DashState>) -> Result<DashInfo, String> {
    let (cpu_percent, total_mem, avail_mem) = dash.sample();

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

    // 只读常驻 helper 写的共享文件，绝不在此触发 UAC。
    let enabled = *app.state::<ElevatedToggle>().0.lock().unwrap();
    let temperature_c = if enabled { read_live_temperature() } else { None };

    Ok(DashInfo {
        cpu_percent,
        memory_used_percent: mem_percent,
        memory_used_mb: used_mb,
        memory_total_mb: total_mem,
        uptime_secs,
        time_now,
        clipboard_count: app
            .try_state::<clipboard::ClipboardState>()
            .map(|state| state.count())
            .unwrap_or(0),
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
fn clip_write(state: tauri::State<'_, clipboard::ClipboardState>, text: String) -> Result<(), String> {
    state.suppress_text_write(&text);
    let mut board = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    board.set_text(text).map_err(|e| e.to_string())?;
    Ok(())
}

// ============ 录屏 ============

/// FFmpeg 查找顺序：FFMPEG_PATH 环境变量 → sidecar 资源目录 → 系统 PATH。
/// 录屏与格式转换共用同一规则。
fn resolve_ffmpeg(app: &AppHandle) -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("FFMPEG_PATH") {
        return std::path::PathBuf::from(p);
    }
    if let Ok(sidecar) = app
        .path()
        .resolve("ffmpeg", tauri::path::BaseDirectory::Resource)
    {
        if sidecar.exists() {
            return sidecar;
        }
    }
    std::path::PathBuf::from("ffmpeg")
}

struct RecordState(Mutex<Option<RecordHandle>>);
struct RecordHandle {
    path: String,
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
}

#[tauri::command]
fn record_start(app: AppHandle) -> Result<String, String> {
    if app.state::<RecordState>().0.lock().unwrap().is_some() {
        return Err("已经在录制中".to_string());
    }
    // 找 FFmpeg（sidecar 或 PATH）
    let ffmpeg = resolve_ffmpeg(&app);

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

    let mut child = std::process::Command::new(&ffmpeg)
        .no_window()
        .args(&args)
        .stdin(std::process::Stdio::piped())
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
    let stdin = child.stdin.take();

    let state = app.state::<RecordState>();
    *state.0.lock().unwrap() = Some(RecordHandle { path: path_str.clone(), child, stdin });
    Ok(path_str)
}

#[tauri::command]
async fn record_stop(app: AppHandle) -> Result<Option<String>, String> {
    let handle = app
        .state::<RecordState>()
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "没有正在录制的会话".to_string())?;

    tauri::async_runtime::spawn_blocking(move || {
        use std::io::Write;
        let mut handle = handle;
        // 向 FFmpeg stdin 写 q：flush 缓冲并正常写完 mp4 尾部后再退出。
        if let Some(mut stdin) = handle.stdin.take() {
            let _ = stdin.write_all(b"q");
            let _ = stdin.flush();
        }
        let deadline = Instant::now() + std::time::Duration::from_secs(8);
        loop {
            match handle.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        // 兜底：FFmpeg 无响应时才强杀（此时尾部可能不完整）。
                        let _ = handle.child.kill();
                        let _ = handle.child.wait();
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(_) => break,
            }
        }
        Ok(Some(handle.path))
    })
    .await
    .map_err(|error| format!("录屏停止线程异常退出：{error}"))?
}

// ============ OCR ============

/// 将窗口所在显示器映射到 xcap 捕获设备（与区域选区使用同一映射规则）。
fn current_capture_monitor(window: &WebviewWindow) -> Result<Monitor, String> {
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or(window.primary_monitor().map_err(|e| e.to_string())?)
        .ok_or_else(|| "无法确定 OCR 所在的显示器。".to_string())?;
    let position = monitor.position();
    let size = monitor.size();
    Monitor::all()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|candidate| {
            candidate.x().ok() == Some(position.x)
                && candidate.y().ok() == Some(position.y)
                && candidate.width().ok() == Some(size.width)
                && candidate.height().ok() == Some(size.height)
        })
        .ok_or_else(|| "无法将窗口显示器映射到截图设备。".to_string())
}

#[tauri::command]
async fn ocr_image(image_path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || ocr_image_blocking(&image_path))
        .await
        .map_err(|error| format!("OCR 工作线程异常退出：{error}"))?
}

fn ocr_image_blocking(image_path: &str) -> Result<String, String> {
    // Windows.Media.Ocr 通过 PowerShell 调用（Windows 10+ 内置）。
    // 路径经环境变量传入，不做字符串内插，避免引号破坏脚本或注入。
    // Await 用反射调用泛型 AsTask：PowerShell 直接调用时无法对 __ComObject
    // 解析泛型重载（MethodCountCouldNotFindBest），必须显式指定结果类型。
    let ps_script = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Storage.StorageFile, Windows.Storage, ContentType=WindowsRuntime]
$null = [Windows.Storage.Streams.IRandomAccessStream, Windows.Storage.Streams, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.SoftwareBitmap, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Media.Ocr.OcrEngine, Windows.Media.Ocr, ContentType=WindowsRuntime]
$null = [Windows.Media.Ocr.OcrResult, Windows.Media.Ocr, ContentType=WindowsRuntime]

$asTaskOpen = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
  $_.Name -eq 'AsTask' -and $_.IsGenericMethod -and $_.GetGenericArguments().Count -eq 1 -and
  $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -like 'IAsyncOperation*'
}
function Await($op, $resultType) {
  $task = $asTaskOpen.MakeGenericMethod($resultType).Invoke($null, @($op))
  $task.Wait() | Out-Null
  return $task.Result
}

$path = $env:ORBIT_OCR_PATH
$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($path)) ([Windows.Storage.StorageFile])
$ras = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($ras)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
$result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])
$result.Text
"#;

    let output = std::process::Command::new("powershell.exe")
        .no_window()
        .env("ORBIT_OCR_PATH", image_path)
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_script])
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
async fn ocr_current_screen(window: WebviewWindow) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // 截取悬浮球当前所在显示器（多显示器环境下不再固定截第一块）。
        let monitor = current_capture_monitor(&window)?;
        let image = monitor.capture_image().map_err(|e| e.to_string())?;

        let tmp = std::env::temp_dir().join(format!(
            "orbit_ocr_{}_{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        image
            .save(&tmp)
            .map_err(|e| format!("临时截图保存失败：{}", e))?;

        // RAII guard: 函数返回时无论如何都会清理
        let _cleanup = TempFileGuard(tmp.clone());

        let path = tmp.to_string_lossy().to_string();
        ocr_image_blocking(&path)
    })
    .await
    .map_err(|error| format!("OCR 工作线程异常退出：{error}"))?
}

// RAII 临时文件清理 guard
struct TempFileGuard(std::path::PathBuf);
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ============ 键鼠录制 ============

struct KbRecordState(Mutex<Option<input_recorder::RecordingSession>>);

#[tauri::command]
fn kb_record_start(app: AppHandle) -> Result<(), String> {
    let state = app.state::<KbRecordState>();
    let mut guard = state.0.lock().unwrap();
    if guard.is_some() {
        return Err("已经在录制中".to_string());
    }
    *guard = Some(input_recorder::start()?);
    Ok(())
}

#[tauri::command]
fn kb_record_stop(app: AppHandle) -> Result<Vec<input_recorder::KbEvent>, String> {
    let session = app
        .state::<KbRecordState>()
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "没有正在录制的会话".to_string())?;
    let (events, duration_ms) = input_recorder::stop(session)?;

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("应用数据目录获取失败：{e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    match input_recorder::save_recording(&dir, &events, duration_ms) {
        Ok(id) => log::info!("键鼠录制已保存：{id}（{} 个事件）", events.len()),
        Err(error) => return Err(format!("录制已停止，但保存失败：{error}")),
    }
    Ok(events)
}

// ============ 格式转换 ============

const CONVERT_FORMATS: &[&str] = &["mp3", "wav", "mp4", "gif", "webp"];

#[tauri::command]
async fn convert_format(
    app: AppHandle,
    input_path: String,
    output_path: String,
    format: String,
) -> Result<String, String> {
    if !CONVERT_FORMATS.contains(&format.as_str()) {
        return Err(format!(
            "不支持的目标格式：{format}（支持：{}）",
            CONVERT_FORMATS.join(" / ")
        ));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let ffmpeg = resolve_ffmpeg(&app);
        let mut output = output_path.clone();
        if !output.to_lowercase().ends_with(&format!(".{format}")) {
            output = format!("{}.{}", output.trim_end_matches('.'), format);
        }
        let out_path = std::path::Path::new(&output);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let args: Vec<&str> = match format.as_str() {
            "mp3" => vec!["-y", "-i", &input_path, "-vn", "-acodec", "libmp3lame", "-q:a", "2", &output],
            "wav" => vec!["-y", "-i", &input_path, "-vn", "-acodec", "pcm_s16le", &output],
            "mp4" => vec!["-y", "-i", &input_path, "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-c:a", "aac", &output],
            "gif" => vec!["-y", "-i", &input_path, "-vf", "fps=12,scale=480:-1", &output],
            "webp" => vec!["-y", "-i", &input_path, "-vf", "fps=15", "-loop", "0", &output],
            _ => vec!["-y", "-i", &input_path, &output],
        };

        let converted = std::process::Command::new(&ffmpeg)
            .no_window()
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "找不到 ffmpeg。请安装 ffmpeg 并加入 PATH，或设置 FFMPEG_PATH 环境变量。".to_string()
                } else {
                    format!("FFmpeg 启动失败：{}", e)
                }
            })?;

        if converted.status.success() {
            Ok(output)
        } else {
            let stderr = String::from_utf8_lossy(&converted.stderr);
            let chars: Vec<char> = stderr.chars().collect();
            let tail: String = chars[chars.len().saturating_sub(400)..].iter().collect();
            Err(format!("转换失败（退出码：{:?}）：{}", converted.status.code(), tail.trim()))
        }
    })
    .await
    .map_err(|error| format!("转换工作线程异常退出：{error}"))?
}

/// 在**窗口创建之前**注册所有后端状态。
///
/// Tauri 2 的 `setup()` 顺序是「先按 config 创建窗口（此时前端 onMount 的 IPC 已
/// 送达 WebView2 回调链）→ 再执行用户 setup 回调」。在 setup 里才 `app.manage()`
/// 会让启动期命令触发 `state() called before manage()`，panic 穿越 FFI 边界后
/// 表现为 `__fastfail` / 0xc0000409，进程启动即崩溃。plugin.initialize 由
/// `AppBuilder::build` 在窗口创建前调用，是唯一可靠的注册时机。
fn setup_early_state(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&dir)?;
    app.manage(AuditLog::open(dir.join("orbit.db"))?);
    clipboard::init_state(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    unsafe {
        // Set this before Tauri initializes WebKitGTK.
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    tauri::Builder::default()
        // 状态注册必须早于窗口创建，见 setup_early_state 注释。
        .plugin(
            tauri::plugin::Builder::<_, ()>::new("orbit-early-state")
                .setup(|app, _api| setup_early_state(app))
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(OverlayState::default())
        .manage(RecordState(Mutex::new(None)))
        .manage(KbRecordState(Mutex::new(None)))
        .manage(ElevatedToggle(Mutex::new(false)))
        .manage(DashState::new())
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
            quit_app,
            get_system_info,
            get_dash_info,
            set_elevated_enabled,
            get_elevated_status,
            clip_read,
            clip_write,
            clipboard::clip_list,
            clipboard::clip_count,
            clipboard::clip_write_back,
            clipboard::clip_set_pin,
            clipboard::clip_set_favorite,
            clipboard::clip_delete,
            clipboard::clip_clear,
            clipboard::clip_set_monitor,
            clipboard::clip_get_monitor,
            clipboard::clip_open_window,
            clipboard::clip_close_window,
            clipboard::clip_image_preview,
            clipboard::clip_shortcut,
            record_start,
            record_stop,
            ocr_image,
            ocr_current_screen,
            kb_record_start,
            kb_record_stop,
            convert_format
        ])
        // 剪贴板窗口失焦自动隐藏：延迟 140ms 复查，规避 Windows 焦点切换抖动。
        .on_window_event(|window, event| {
            if window.label() != "clipboard" {
                return;
            }
            if !matches!(event, WindowEvent::Focused(false)) {
                return;
            }
            let app = window.app_handle().clone();
            let Some(state) = app.try_state::<clipboard::ClipboardState>() else {
                return;
            };
            if !state.can_auto_hide() {
                return;
            }
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(140));
                if let Some(clip_window) = app.get_webview_window("clipboard") {
                    if clip_window.is_focused().unwrap_or(false) {
                        return;
                    }
                    let _ = clip_window.hide();
                }
            });
        })
        .setup(|app| {
            // AuditLog / ClipboardState 已在 early_state_plugin 中注册，
            // 这里只启动依赖它们的后台线程。
            clipboard::start_watcher(app.handle())?;

            // CPU 使用率是两次采样的差值，首次读数至少要间隔
            // MINIMUM_CPU_UPDATE_INTERVAL；启动后后台补采一次，
            // 避免刘海第一帧拿到无效的 0%。
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
                    let _ = handle.state::<DashState>().sample();
                });
            }

            // 全局快捷键唤起剪贴板。
            // 注意：`on_shortcut` 内部通过 run_on_main_thread + 同步 recv 完成注册，
            // 而 setup 运行在事件循环启动前的主线程，此时调用会死锁；因此延后到事件循环
            // 就绪后由后台线程注册，注册失败（被其他程序占用）只告警，不阻断启动。
            {
                use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
                let handle = app.handle().clone();
                std::thread::Builder::new()
                    .name("clipboard-shortcut".to_string())
                    .spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(600));
                        if let Err(error) = handle.global_shortcut().on_shortcut(
                            clipboard::CLIP_SHORTCUT,
                            |app, _shortcut, event| {
                                if event.state == ShortcutState::Pressed {
                                    clipboard::toggle_clipboard_window(app);
                                }
                            },
                        ) {
                            log::warn!(
                                "全局快捷键 {} 注册失败（可能已被占用）：{error}",
                                clipboard::CLIP_SHORTCUT
                            );
                        }
                    })
                    .map_err(|error| format!("快捷键注册线程启动失败：{error}"))?;
            }

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
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                // 通知常驻 helper 退出；崩溃场景由 helper 的父进程存活检查兜底。
                stop_elevated_watch();
            }
        });
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
