use orbit_core::{
    AuditLog, CommandRequest, CommandResponse, CommandStatus, DockPosition, InputMouseParams,
    KbReplayParams, KbReplayResult, KbRecordStateEvent, RecordingItem, RegionFinishParams,
    RegionSelectionStart, SetupResult, WindowLayout, WindowSize,
};
use std::sync::Mutex;
use tauri::Manager;
use tauri::{AppHandle, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use xcap::Monitor;

mod bindings;
mod bus;
mod clipboard;
mod convert;
mod elevated;
mod input_recorder;
mod mcp_bridge;
mod monitor;
mod ocr;
mod ocr_backend;
mod recording;
mod remote;
#[cfg(target_os = "windows")]
mod wgc;
mod screenshot;
mod windows;

// 保持 crate 根符号不变：bus.rs 等调用点通过 `crate::...` 引用这些名字，
// 拆分后经再导出继续生效，调用点零改动。
pub(crate) use convert::convert_format_impl;
pub(crate) use monitor::DashState;
pub(crate) use ocr_backend::ocr_current_screen_impl;
pub(crate) use recording::{
    RecordState, list_recovery_impl, record_pause_impl, record_resume_impl, record_start_impl,
    record_stop_impl,
};

/// tauri.conf.json 只描述窗口初始尺寸；停靠/展开态尺寸集中在此，
/// 由 `overlay.layout` 发布给前端，避免 Rust 与 CSS 各写一份数字。
const SIDE_SIZE: WindowSize = WindowSize { width: 54.0, height: 132.0 };
const NOTCH_SIZE: WindowSize = WindowSize { width: 320.0, height: 62.0 };
/// 展开圆盘窗口：紧贴最大外圈（半径 132）并留出顶部提示、底部按钮的位置
const EXPANDED_SIZE: WindowSize = WindowSize { width: 340.0, height: 340.0 };
/// 弹窗相对窗口的内边距：0 表示严格按配置尺寸铺满。
const PANEL_INSET: f64 = 0.0;
/// 贴近屏幕边缘多少像素内才吸附，避免大老远就触发停靠
const EDGE_THRESHOLD: i32 = 24;

/// 日志后端在 debug / release 都必须存在：release 只保留 Warn 以上，
/// 避免录屏状态、快捷键注册等失败在正式包里被静默吞掉。
fn log_level(debug_build: bool) -> log::LevelFilter {
    if debug_build {
        log::LevelFilter::Info
    } else {
        log::LevelFilter::Warn
    }
}

/// 窗口尺寸的唯一发布点：初始尺寸读取 tauri.conf.json，状态尺寸取上面的常量。
/// 找不到配置窗口时直接报错，不在代码里再抄一份兜底数字。
pub(crate) fn layout_config(
    manager: &impl tauri::Manager<tauri::Wry>,
) -> Result<WindowLayout, String> {
    let configured = |label: &str| -> Result<WindowSize, String> {
        manager
            .config()
            .app
            .windows
            .iter()
            .find(|window| window.label == label)
            .map(|window| WindowSize {
                width: window.width,
                height: window.height,
            })
            .ok_or_else(|| format!("tauri.conf.json 缺少 {label} 窗口配置"))
    };
    Ok(WindowLayout {
        floating: configured("main")?,
        side: SIDE_SIZE,
        notch: NOTCH_SIZE,
        expanded: EXPANDED_SIZE,
        clipboard: configured("clipboard")?,
        selection: configured("selection")?,
        panel_inset: PANEL_INSET,
    })
}

/// 覆盖层当前状态使用的逻辑尺寸；前端由同一份 layout 注入 CSS 变量。
/// ball_radius 仅在 floating 折叠态生效：窗口边长 = 球直径（不小于默认 82），
/// 保证折叠态悬浮球始终完整可见、不被裁切。
fn overlay_size(
    layout: &WindowLayout,
    dock: DockPosition,
    expanded: bool,
    ball_radius: f64,
) -> WindowSize {
    match (dock, expanded) {
        (DockPosition::Floating, false) => {
            let edge = (ball_radius * 2.0).max(FLOATING_MIN_SIZE).min(layout.expanded.width);
            WindowSize { width: edge, height: edge }
        }
        (DockPosition::Left | DockPosition::Right, false) => layout.side,
        (DockPosition::Top, false) => layout.notch,
        (_, true) => layout.expanded,
    }
}

/// 子进程一律不创建控制台窗口：OCR / 温度 / 录屏 / 转换调用外部程序时不再闪黑框。
pub(crate) trait NoWindow {
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

#[derive(Clone, Copy)]
pub(crate) struct OverlayLayout {
    pub(crate) dock: DockPosition,
    pub(crate) expanded: bool,
    /// 中心球半径（px），由前端布局编辑保存后同步；折叠态窗口边长 = 半径 × 2。
    pub(crate) ball_radius: f64,
}

/// 折叠态悬浮球最小窗口边长（沿用 tauri.conf 原有 104×104）。
const FLOATING_MIN_SIZE: f64 = 104.0;
/// 默认中心球半径（与前端 DEFAULT_BALL_RADIUS 一致）。
const DEFAULT_BALL_RADIUS: u32 = 41;

#[derive(Default)]
pub(crate) struct OverlayState(Mutex<OverlayLayout>);
pub struct ElevatedToggle(pub Mutex<bool>); // 用户启用后才会拉 helper

pub(crate) fn set_elevated_enabled_impl(
    app: &AppHandle,
    enabled: bool,
) -> Result<bool, String> {
    if enabled {
        // 一次 UAC 拉起常驻 helper；已在运行则不重复弹。
        elevated::start_watch().map_err(|error| error.message())?;
    } else {
        elevated::stop_watch();
    }
    *app.state::<ElevatedToggle>().0.lock().unwrap() = enabled;
    Ok(enabled)
}

pub(crate) fn get_elevated_status_impl(app: &AppHandle) -> bool {
    *app.state::<ElevatedToggle>().0.lock().unwrap() || elevated::is_watch_running()
}

impl Default for OverlayLayout {
    fn default() -> Self {
        Self {
            dock: DockPosition::Floating,
            expanded: false,
            ball_radius: DEFAULT_BALL_RADIUS as f64,
        }
    }
}

/// 登录启动的真实开关逻辑：总线处理器与单元测试共用，避免两套行为漂移。
pub(crate) fn apply_autostart(app: &AppHandle, enabled: bool) -> Result<SetupResult, String> {
    let autostart = app.autolaunch();
    if enabled {
        autostart.enable().map_err(|error| error.to_string())?;
    } else {
        autostart.disable().map_err(|error| error.to_string())?;
    }

    let result = setup_result(app)?;
    if result.autostart_configured != enabled {
        return Err(if enabled {
            "登录启动项注册后仍处于关闭状态。".to_string()
        } else {
            "登录启动项已关闭，但状态检查仍显示启用。".to_string()
        });
    }
    Ok(result)
}

pub(crate) fn setup_result(app: &AppHandle) -> Result<SetupResult, String> {
    let permissions = [
        ("screen-capture", orbit_core::PermissionState::Unknown),
        ("accessibility-input", orbit_core::PermissionState::Unknown),
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
    // 目录直接来自总线注册表：契约与实际可路由命令永远一致。
    let mut specs: Vec<orbit_core::CommandSpec> = bus::registry()
        .values()
        .map(|entry| entry.spec.clone())
        .collect();
    specs.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(specs)
}

#[tauri::command]
async fn execute_command(
    app: AppHandle,
    audit: tauri::State<'_, AuditLog>,
    request: CommandRequest,
) -> Result<CommandResponse, String> {
    Ok(bus::execute(app, audit.inner().clone(), request, "orb").await)
}

pub(crate) fn start_region_selection_impl(
    app: &AppHandle,
    layout: OverlayLayout,
) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 Orbit 主窗口。".to_string())?;
    let selection = windows::ensure(app, windows::SELECTION_LABEL)?;
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
    bus::emit(
        &app,
        "region-selection-start",
        &RegionSelectionStart { monitor_id },
    )
    .map_err(|error| error.to_string())?;

    main.hide().map_err(|error| error.to_string())?;
    if let Err(error) = selection.show() {
        let _ = main.show();
        return Err(error.to_string());
    }
    if let Err(error) = selection.set_focus() {
        let _ = selection.hide();
        return match restore_main_overlay(app, layout) {
            Ok(()) => Err(error.to_string()),
            Err(restore_error) => Err(format!(
                "无法聚焦区域截图窗口：{error}；恢复 Orbit 主窗口失败：{restore_error}"
            )),
        };
    }
    Ok(())
}

fn restore_main_overlay(app: &AppHandle, layout: OverlayLayout) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 Orbit 主窗口。".to_string())?;
    set_overlay_layout(&main, layout.dock, layout.expanded, layout.ball_radius)?;
    main.show().map_err(|error| error.to_string())
}

pub(crate) async fn finish_region_selection_impl(
    app: &AppHandle,
    layout: OverlayLayout,
    params: RegionFinishParams,
) -> Result<CommandResponse, String> {
    let RegionFinishParams {
        monitor_id,
        x,
        y,
        width,
        height,
    } = params;
    let selection = windows::ensure(app, windows::SELECTION_LABEL)?;
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
    let audit = app.state::<AuditLog>().inner().clone();
    let response = bus::execute(app.clone(), audit, request, "orb").await;
    let restore = restore_main_overlay(app, layout);
    let capture_error = || {
        response
            .error
            .clone()
            .unwrap_or_else(|| "截图命令执行失败".to_string())
    };
    match (response.status, restore) {
        (CommandStatus::Ok, Ok(())) => Ok(response),
        (CommandStatus::Ok, Err(error)) => {
            Err(format!("截图已处理，但无法恢复 Orbit 主窗口：{error}"))
        }
        (CommandStatus::Error, Ok(())) => Err(capture_error()),
        (CommandStatus::Error, Err(restore_error)) => Err(format!(
            "{}；恢复 Orbit 主窗口失败：{restore_error}",
            capture_error()
        )),
    }
}

pub(crate) fn cancel_region_selection_impl(
    app: &AppHandle,
    layout: OverlayLayout,
) -> Result<(), String> {
    // 取消是幂等操作：选区窗口可能尚未创建（用户没开始取景），此时只恢复主窗口。
    if let Some(selection) = app.get_webview_window(windows::SELECTION_LABEL) {
        selection.hide().map_err(|error| error.to_string())?;
    }
    restore_main_overlay(app, layout)
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

/// 设置中心球半径：更新覆盖层状态并立即重设窗口尺寸。
/// 折叠态窗口边长 = 球直径；展开态窗口不变。半径做物理范围钳制。
pub(crate) fn set_ball_radius_impl(app: &AppHandle, radius: f64) -> Result<(), String> {
    if !radius.is_finite() {
        return Err("球半径必须是有限数值。".to_string());
    }
    let radius = radius.round().clamp(24.0, 136.0);
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 Orbit 主窗口。".to_string())?;
    let state = app.state::<OverlayState>();
    let mut layout = state.0.lock().map_err(|error| error.to_string())?;
    layout.ball_radius = radius;
    set_overlay_layout(&main, layout.dock, layout.expanded, radius)?;
    Ok(())
}

pub(crate) fn dock_position(window: &WebviewWindow) -> Result<DockPosition, String> {
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

pub(crate) fn set_overlay_layout(
    window: &WebviewWindow,
    dock: DockPosition,
    expanded: bool,
    ball_radius: f64,
) -> Result<(), String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let current_size = window.outer_size().map_err(|error| error.to_string())?;
    let scale_factor = window.scale_factor().map_err(|error| error.to_string())?;
    let center_x = position.x + current_size.width as i32 / 2;
    let center_y = position.y + current_size.height as i32 / 2;
    let (monitor_left, monitor_top, monitor_right, monitor_bottom) = monitor_bounds(window)?;
    let layout = layout_config(window)?;
    let logical_size = overlay_size(&layout, dock, expanded, ball_radius);
    let target_width = (logical_size.width * scale_factor).round() as u32;
    let target_height = (logical_size.height * scale_factor).round() as u32;
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
        .set_size(tauri::LogicalSize::new(
            logical_size.width,
            logical_size.height,
        ))
        .map_err(|error| error.to_string())?;
    window
        .set_position(PhysicalPosition::new(target_x, target_y))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{DockPosition, determine_dock_position};

    #[test]
    fn release_build_keeps_a_log_backend() {
        assert_eq!(super::log_level(true), log::LevelFilter::Info);
        assert_eq!(super::log_level(false), log::LevelFilter::Warn);
        assert_ne!(super::log_level(false), log::LevelFilter::Off);
    }

    #[test]
    fn overlay_size_follows_layout_states() {
        use super::{WindowLayout, WindowSize, overlay_size};
        const fn size(width: f64, height: f64) -> WindowSize {
            WindowSize { width, height }
        }
        let layout = WindowLayout {
            floating: size(104.0, 104.0),
            side: size(54.0, 132.0),
            notch: size(320.0, 62.0),
            expanded: size(340.0, 340.0),
            clipboard: size(420.0, 560.0),
            selection: size(1.0, 1.0),
            panel_inset: 0.0,
        };
        let default_ball = 41.0;
        assert_eq!(
            overlay_size(&layout, DockPosition::Floating, false, default_ball),
            size(104.0, 104.0)
        );
        assert_eq!(
            overlay_size(&layout, DockPosition::Left, false, default_ball),
            size(54.0, 132.0)
        );
        assert_eq!(
            overlay_size(&layout, DockPosition::Right, false, default_ball),
            size(54.0, 132.0)
        );
        assert_eq!(
            overlay_size(&layout, DockPosition::Top, false, default_ball),
            size(320.0, 62.0)
        );
        assert_eq!(
            overlay_size(&layout, DockPosition::Floating, true, default_ball),
            size(340.0, 340.0)
        );
        assert_eq!(
            overlay_size(&layout, DockPosition::Top, true, default_ball),
            size(340.0, 340.0)
        );
    }

    #[test]
    fn floating_window_edge_follows_ball_diameter() {
        use super::{WindowLayout, WindowSize, overlay_size};
        const fn size(width: f64, height: f64) -> WindowSize {
            WindowSize { width, height }
        }
        let layout = WindowLayout {
            floating: size(104.0, 104.0),
            side: size(54.0, 132.0),
            notch: size(320.0, 62.0),
            expanded: size(340.0, 340.0),
            clipboard: size(420.0, 560.0),
            selection: size(1.0, 1.0),
            panel_inset: 0.0,
        };
        // 大球：折叠态窗口边长 = 球直径。
        assert_eq!(
            overlay_size(&layout, DockPosition::Floating, false, 60.0),
            size(120.0, 120.0)
        );
        // 小球：不低于最小窗口 104。
        assert_eq!(
            overlay_size(&layout, DockPosition::Floating, false, 30.0),
            size(104.0, 104.0)
        );
        // 展开态窗口不受球半径影响。
        assert_eq!(
            overlay_size(&layout, DockPosition::Floating, true, 60.0),
            size(340.0, 340.0)
        );
    }

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
}

// 管理员 helper 的文件握手与错误映射见 elevated.rs。

// ============ 键鼠录制 ============

struct KbRecordState(Mutex<Option<input_recorder::RecordingSession>>);

fn emit_kb_record_state(app: &AppHandle, active: bool, redacted_count: u32) {
    let event = KbRecordStateEvent { active, redacted_count };
    if let Err(error) = bus::emit(app, "kb-record-state", &event) {
        log::warn!("键鼠录制状态事件发送失败：{error}");
    }
}

pub(crate) fn kb_record_start_impl(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<KbRecordState>();
    let mut guard = state.0.lock().unwrap();
    if guard.is_some() {
        return Err("已经在录制中".to_string());
    }
    *guard = Some(input_recorder::start()?);
    emit_kb_record_state(app, true, 0);
    Ok(())
}

pub(crate) fn kb_record_stop_impl(app: &AppHandle) -> Result<Vec<input_recorder::KbEvent>, String> {
    let session = app
        .state::<KbRecordState>()
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "没有正在录制的会话".to_string())?;
    let (events, duration_ms, redacted) = input_recorder::stop(session)?;

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("应用数据目录获取失败：{e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    match input_recorder::save_recording(&dir, &events, duration_ms) {
        Ok(id) => log::info!("键鼠录制已保存：{id}（{} 个事件）", events.len()),
        Err(error) => {
            emit_kb_record_state(app, false, redacted);
            return Err(format!("录制已停止，但保存失败：{error}"));
        }
    }
    emit_kb_record_state(app, false, redacted);
    Ok(events)
}

pub(crate) fn kb_record_list_impl(app: &AppHandle) -> Result<Vec<RecordingItem>, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("应用数据目录获取失败：{e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    input_recorder::list_recordings(&dir)
}

/// 回放一条已保存的键鼠录制；耗时与漂移同时返回，供 P-09 测量。
pub(crate) fn kb_replay_impl(
    app: &AppHandle,
    params: KbReplayParams,
) -> Result<KbReplayResult, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("应用数据目录获取失败：{e}"))?;
    let (events, _duration_ms) = input_recorder::load_recording(&dir, &params.recording_id)?;
    let speed = params.speed.unwrap_or(1.0);
    input_recorder::replay(&events, &params.recording_id, speed)
}

/// 本地鼠标控制：先纯校验归类，再交给平台层合成输入。
/// 不涉及网络与跨进程转发，参数错误以 OrbitError::Param 归类。
pub(crate) fn input_mouse_impl(params: InputMouseParams) -> Result<(), orbit_core::OrbitError> {
    let command = input_recorder::plan_mouse(&params).map_err(orbit_core::OrbitError::param)?;
    input_recorder::apply_mouse(command).map_err(orbit_core::OrbitError::system)
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
    app.manage(bindings::init_state(app)?);
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
        .manage(remote::RemoteState::new())
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
            get_command_catalog,
            execute_command
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

            // 只读 MCP 桥：随机回环端口 + 一次性 token，写 app_data_dir/mcp-bridge.json。
            // 失效只告警，不影响桌面主功能。
            if let Err(error) = mcp_bridge::start(app.handle()) {
                log::warn!("MCP 桥启动失败：{error}");
            }

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

            // 全局快捷键（L2 事件绑定层）：默认 Alt+V 打开剪贴板面板，
            // 录制期间语义切换为「备用停止」，由 bindings::execute_action 实现。
            // 注册延后到事件循环就绪，失败只告警，不阻断启动。
            bindings::start_shortcut_registration(app.handle());

            #[cfg(target_os = "windows")]
            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = disable_window_transitions(&window) {
                    log::warn!("Failed to disable window transitions: {error}");
                }
            }

            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(log_level(cfg!(debug_assertions)))
                    .build(),
            )?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                // 通知常驻 helper 退出；崩溃场景由 helper 的父进程存活检查兜底。
                elevated::stop_watch();
                // 远程只读服务默认关闭；若用户显式开启过，退出时关闭监听并丢弃令牌。
                let _ = remote::stop_server(app);
            }
        });
}

#[cfg(target_os = "windows")]
fn disable_window_transitions(window: &WebviewWindow) -> Result<(), String> {
    use std::mem::size_of;
    // 用绝对路径 ::windows 指向外部 crate，避免与本地 `windows` 模块同名冲突。
    use ::windows::Win32::Graphics::Dwm::{DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute};

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
