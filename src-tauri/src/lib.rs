use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use sysinfo::{Components, System};
#[cfg(target_os = "windows")]
use tauri::Manager;
use tauri::{
    AppHandle, Emitter, PhysicalPosition, WebviewWindow, Window,
    menu::{ContextMenu, Menu, MenuItem},
};
use tauri_plugin_autostart::ManagerExt;

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
    let autostart = app.autolaunch();
    autostart.enable().map_err(|error| error.to_string())?;
    let autostart_configured = autostart.is_enabled().map_err(|error| error.to_string())?;
    if !autostart_configured {
        return Err("登录启动项注册后仍处于关闭状态。".to_string());
    }

    let permissions = [
        ("screen-capture", PermissionState::Unknown),
        ("accessibility-input", PermissionState::Unknown),
    ]
    .into_iter()
    .map(|(id, state)| (id.to_string(), state))
    .collect();

    Ok(SetupResult {
        permissions,
        autostart_configured,
    })
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
        (DockPosition::Floating, true) => ISLAND_SIZE,
        (DockPosition::Left | DockPosition::Right, false) => SIDE_SIZE,
        (DockPosition::Top, false) => NOTCH_SIZE,
        (DockPosition::Left | DockPosition::Right, true) => (360.0, 300.0),
        (DockPosition::Top, true) => ISLAND_SIZE,
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    unsafe {
        // Set this before Tauri initializes WebKitGTK.
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    tauri::Builder::default()
        .manage(OverlayState::default())
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
            dock_overlay,
            set_overlay_open,
            show_orb_context_menu
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
