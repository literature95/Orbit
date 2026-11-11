//! 命令总线：注册表化路由 + 统一审计。
//!
//! 所有能力命令在 `registry()` 中登记：契约（CommandSpec）来自
//! orbit-core::command_catalog 单一事实源，处理器持有强类型反序列化与执行逻辑。
//! `execute()` 是唯一入口：查表路由 → 执行 → 异步写审计 → 组装 CommandResponse。
//!
//! `get_command_catalog` 直接读取本注册表，保证对外目录与可路由命令一致。

use crate::{
    DashState, ElevatedToggle, OverlayState, apply_autostart, clipboard, dock_position,
    elevated, layout_config, screenshot, set_overlay_layout, setup_result,
};
use orbit_core::{
    AuditLog, CommandRequest, CommandResponse, CommandSpec, CommandStatus, DashInfo, EmptyParams,
    ClipFlagParams, ClipIdParams, ClipListParams, ClipMonitorParams, ClipWriteParams,
    ConvertFormatParams, MonitorSnapshot, OcrImageParams, OverlaySnapshot, RegionFinishParams,
    ScreenshotRequest, SetAutostartParams, SetElevatedParams, SetOverlayOpenParams,
    SystemInfo, command_catalog, event_catalog,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};

/// 处理器上下文：处理器通过 `ctx.app` 访问窗口与托管状态，
/// 避免把 Tauri State 类型耦合进处理器签名。
pub struct CommandCtx {
    pub app: AppHandle,
}

pub type BoxFut = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;

pub struct BusCommand {
    pub spec: CommandSpec,
    pub run: Arc<dyn Fn(CommandCtx, Value) -> BoxFut + Send + Sync>,
}

/// 同步处理器包装：放入阻塞线程池执行，避免长任务卡住异步执行器。
fn sync<F, T>(f: F) -> Arc<dyn Fn(CommandCtx, Value) -> BoxFut + Send + Sync>
where
    F: Fn(CommandCtx, Value) -> Result<T, String> + Send + Sync + 'static,
    T: Serialize + Send + 'static,
{
    let f = Arc::new(f);
    Arc::new(move |ctx, params| {
        let f = Arc::clone(&f);
        Box::pin(async move {
            tauri::async_runtime::spawn_blocking(move || f(ctx, params))
                .await
                .map_err(|error| format!("命令工作线程异常退出：{error}"))?
                .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()))
        })
    })
}

fn catalog_spec(catalog: &[CommandSpec], id: &str) -> CommandSpec {
    catalog
        .iter()
        .find(|spec| spec.id == id)
        .cloned()
        .unwrap_or_else(|| panic!("命令 {id} 缺少 CommandSpec，请先在 orbit-core::command_catalog 登记"))
}

/// 命令注册表：id → 契约 + 处理器。契约统一从 command_catalog 读取，
/// 处理器只负责「反序列化参数 → 执行 → 返回可序列化结果」。
pub fn registry() -> &'static HashMap<&'static str, BusCommand> {
    static REGISTRY: OnceLock<HashMap<&'static str, BusCommand>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let catalog = command_catalog().expect("命令目录生成失败");
        let mut map: HashMap<&'static str, BusCommand> = HashMap::new();

        map.insert(
            "screenshot.capture",
            BusCommand {
                spec: catalog_spec(&catalog, "screenshot.capture"),
                run: sync(|_ctx, params| {
                    let request: ScreenshotRequest = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    screenshot::capture_to_clipboard(request)
                }),
            },
        );

        map.insert(
            "system.info",
            BusCommand {
                spec: catalog_spec(&catalog, "system.info"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    let window = ctx
                        .app
                        .get_webview_window("main")
                        .ok_or_else(|| "找不到 Orbit 主窗口。".to_string())?;
                    let monitor = window
                        .current_monitor()
                        .map_err(|e| e.to_string())?
                        .or(window.primary_monitor().map_err(|e| e.to_string())?)
                        .ok_or_else(|| "无法获取屏幕信息".to_string())?;
                    let size = monitor.size();
                    let scale = window.scale_factor().map_err(|e| e.to_string())?;
                    let mut sys = sysinfo::System::new();
                    sys.refresh_memory();
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
                }),
            },
        );

        map.insert(
            "system.dash",
            BusCommand {
                spec: catalog_spec(&catalog, "system.dash"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    let app = &ctx.app;
                    let (cpu_percent, total_mem, avail_mem) =
                        app.state::<DashState>().sample();
                    let used_mb = total_mem.saturating_sub(avail_mem);
                    let mem_percent = if total_mem > 0 {
                        (used_mb as f32 / total_mem as f32) * 100.0
                    } else {
                        0.0
                    };
                    // 只读常驻 helper 写的共享文件，绝不在此触发 UAC。
                    let enabled = *app.state::<ElevatedToggle>().0.lock().unwrap()
                        || elevated::is_watch_running();
                    let temperature_c = if enabled {
                        elevated::read_temperature()
                    } else {
                        None
                    };
                    Ok(DashInfo {
                        cpu_percent,
                        memory_used_percent: mem_percent,
                        memory_used_mb: used_mb,
                        memory_total_mb: total_mem,
                        uptime_secs: sysinfo::System::uptime(),
                        time_now: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs().to_string())
                            .unwrap_or_else(|_| "0".to_string()),
                        clipboard_count: app
                            .try_state::<clipboard::ClipboardState>()
                            .map(|state| state.count())
                            .unwrap_or(0),
                        temperature_c,
                    })
                }),
            },
        );

        map.insert(
            "system.monitor",
            BusCommand {
                spec: catalog_spec(&catalog, "system.monitor"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    let app = &ctx.app;
                    let (cpu_percent, per_core, total_mem, avail_mem) =
                        app.state::<DashState>().snapshot();
                    let used_mb = total_mem.saturating_sub(avail_mem);
                    let memory_used_percent = if total_mem > 0 {
                        (used_mb as f32 / total_mem as f32) * 100.0
                    } else {
                        0.0
                    };
                    let temperature_enabled = *app.state::<ElevatedToggle>().0.lock().unwrap()
                        || elevated::is_watch_running();
                    let temperature_c = if temperature_enabled {
                        elevated::read_temperature()
                    } else {
                        None
                    };
                    Ok(MonitorSnapshot {
                        cpu_percent,
                        per_core,
                        memory_used_percent,
                        memory_used_mb: used_mb,
                        memory_total_mb: total_mem,
                        uptime_secs: sysinfo::System::uptime(),
                        temperature_c,
                        temperature_enabled,
                    })
                }),
            },
        );

        map.insert(
            "overlay.dock",
            BusCommand {
                spec: catalog_spec(&catalog, "overlay.dock"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    let window = ctx
                        .app
                        .get_webview_window("main")
                        .ok_or_else(|| "找不到 Orbit 主窗口。".to_string())?;
                    let dock = dock_position(&window)?;
                    let state = ctx.app.state::<OverlayState>();
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
                }),
            },
        );

        map.insert(
            "overlay.set_open",
            BusCommand {
                spec: catalog_spec(&catalog, "overlay.set_open"),
                run: sync(|ctx, params| {
                    let SetOverlayOpenParams { open } = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    let window = ctx
                        .app
                        .get_webview_window("main")
                        .ok_or_else(|| "找不到 Orbit 主窗口。".to_string())?;
                    let state = ctx.app.state::<OverlayState>();
                    let mut layout = state.0.lock().map_err(|error| error.to_string())?;
                    set_overlay_layout(&window, layout.dock, open)?;
                    layout.expanded = open;
                    Ok(Value::Null)
                }),
            },
        );

        map.insert(
            "overlay.layout",
            BusCommand {
                spec: catalog_spec(&catalog, "overlay.layout"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    layout_config(&ctx.app)
                }),
            },
        );

        map.insert(
            "app.setup.status",
            BusCommand {
                spec: catalog_spec(&catalog, "app.setup.status"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    setup_result(&ctx.app)
                }),
            },
        );

        map.insert(
            "app.setup.complete",
            BusCommand {
                spec: catalog_spec(&catalog, "app.setup.complete"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    apply_autostart(&ctx.app, true)
                }),
            },
        );

        map.insert(
            "app.autostart.set",
            BusCommand {
                spec: catalog_spec(&catalog, "app.autostart.set"),
                run: sync(|ctx, params| {
                    let SetAutostartParams { enabled } = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    apply_autostart(&ctx.app, enabled)
                }),
            },
        );

        map.insert(
            "app.quit",
            BusCommand {
                spec: catalog_spec(&catalog, "app.quit"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    ctx.app.exit(0);
                    Ok(Value::Null)
                }),
            },
        );

        map.insert(
            "clipboard.read",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.read"),
                run: sync(|_ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    clipboard::read_text()
                }),
            },
        );

        map.insert(
            "clipboard.list",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.list"),
                run: sync(|ctx, params| {
                    let ClipListParams {
                        query,
                        filter,
                        limit,
                    } = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    let state = clip_state(&ctx.app)?;
                    clipboard::list(&state, query, filter, limit)
                }),
            },
        );

        map.insert(
            "clipboard.count",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.count"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    let state = clip_state(&ctx.app)?;
                    Ok(clipboard::count(&state))
                }),
            },
        );

        map.insert(
            "clipboard.image_preview",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.image_preview"),
                run: sync(|ctx, params| {
                    let ClipIdParams { id } = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    let state = clip_state(&ctx.app)?;
                    clipboard::image_preview(&state, id)
                }),
            },
        );

        map.insert(
            "clipboard.get_monitor",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.get_monitor"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    let state = clip_state(&ctx.app)?;
                    Ok(clipboard::get_monitor(&state))
                }),
            },
        );

        map.insert(
            "clipboard.shortcut",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.shortcut"),
                run: sync(|_ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    Ok(clipboard::shortcut())
                }),
            },
        );

        map.insert(
            "clipboard.write",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.write"),
                run: sync(|ctx, params| {
                    let ClipWriteParams { text } = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    let state = clip_state(&ctx.app)?;
                    clipboard::write_text(&state, &text)
                }),
            },
        );

        map.insert(
            "clipboard.write_back",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.write_back"),
                run: sync(|ctx, params| {
                    let ClipIdParams { id } = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    let state = clip_state(&ctx.app)?;
                    clipboard::write_back(&state, id)
                }),
            },
        );

        map.insert(
            "clipboard.set_pin",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.set_pin"),
                run: sync(|ctx, params| {
                    let ClipFlagParams { id, value } = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    let state = clip_state(&ctx.app)?;
                    clipboard::set_pin(&state, id, value)
                }),
            },
        );

        map.insert(
            "clipboard.set_favorite",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.set_favorite"),
                run: sync(|ctx, params| {
                    let ClipFlagParams { id, value } = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    let state = clip_state(&ctx.app)?;
                    clipboard::set_favorite(&state, id, value)
                }),
            },
        );

        map.insert(
            "clipboard.set_monitor",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.set_monitor"),
                run: sync(|ctx, params| {
                    let ClipMonitorParams { enabled } = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    let state = clip_state(&ctx.app)?;
                    Ok(clipboard::set_monitor(&state, enabled))
                }),
            },
        );

        map.insert(
            "clipboard.delete",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.delete"),
                run: sync(|ctx, params| {
                    let ClipIdParams { id } = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    let state = clip_state(&ctx.app)?;
                    clipboard::delete(&state, id)
                }),
            },
        );

        map.insert(
            "clipboard.clear",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.clear"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    let state = clip_state(&ctx.app)?;
                    clipboard::clear(&state)
                }),
            },
        );

        map.insert(
            "clipboard.open_window",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.open_window"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    clipboard::open_window(&ctx.app)
                }),
            },
        );

        map.insert(
            "clipboard.close_window",
            BusCommand {
                spec: catalog_spec(&catalog, "clipboard.close_window"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    clipboard::close_window(&ctx.app)
                }),
            },
        );

        map.insert(
            "record.start",
            BusCommand {
                spec: catalog_spec(&catalog, "record.start"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    crate::record_start_impl(&ctx.app)
                }),
            },
        );

        map.insert(
            "record.stop",
            BusCommand {
                spec: catalog_spec(&catalog, "record.stop"),
                run: Arc::new(|ctx, params| {
                    Box::pin(async move {
                        let _: EmptyParams = serde_json::from_value(params)
                            .map_err(|error| format!("参数不合法：{error}"))?;
                        let path = crate::record_stop_impl(&ctx.app).await?;
                        serde_json::to_value(path).map_err(|error| error.to_string())
                    })
                }),
            },
        );

        map.insert(
            "ocr.image",
            BusCommand {
                spec: catalog_spec(&catalog, "ocr.image"),
                run: sync(|_ctx, params| {
                    let OcrImageParams { image_path } = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    crate::ocr_image_blocking(&image_path)
                }),
            },
        );

        map.insert(
            "ocr.current_screen",
            BusCommand {
                spec: catalog_spec(&catalog, "ocr.current_screen"),
                run: Arc::new(|ctx, params| {
                    Box::pin(async move {
                        let _: EmptyParams = serde_json::from_value(params)
                            .map_err(|error| format!("参数不合法：{error}"))?;
                        let window = ctx
                            .app
                            .get_webview_window("main")
                            .ok_or_else(|| "找不到 Orbit 主窗口。".to_string())?;
                        let text = crate::ocr_current_screen_impl(&window).await?;
                        serde_json::to_value(text).map_err(|error| error.to_string())
                    })
                }),
            },
        );

        map.insert(
            "kb_record.start",
            BusCommand {
                spec: catalog_spec(&catalog, "kb_record.start"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    crate::kb_record_start_impl(&ctx.app)
                }),
            },
        );

        map.insert(
            "kb_record.stop",
            BusCommand {
                spec: catalog_spec(&catalog, "kb_record.stop"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    crate::kb_record_stop_impl(&ctx.app)
                }),
            },
        );

        map.insert(
            "convert.format",
            BusCommand {
                spec: catalog_spec(&catalog, "convert.format"),
                run: Arc::new(|ctx, params| {
                    Box::pin(async move {
                        let ConvertFormatParams {
                            input_path,
                            output_path,
                            format,
                        } = serde_json::from_value(params)
                            .map_err(|error| format!("参数不合法：{error}"))?;
                        let output =
                            crate::convert_format_impl(&ctx.app, input_path, output_path, format)
                                .await?;
                        serde_json::to_value(output).map_err(|error| error.to_string())
                    })
                }),
            },
        );

        map.insert(
            "region.start",
            BusCommand {
                spec: catalog_spec(&catalog, "region.start"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    let layout = overlay_layout(&ctx.app)?;
                    crate::start_region_selection_impl(&ctx.app, layout)
                }),
            },
        );

        map.insert(
            "region.finish",
            BusCommand {
                spec: catalog_spec(&catalog, "region.finish"),
                run: Arc::new(|ctx, params| {
                    Box::pin(async move {
                        let params: RegionFinishParams = serde_json::from_value(params)
                            .map_err(|error| format!("参数不合法：{error}"))?;
                        let layout = overlay_layout(&ctx.app)?;
                        let response =
                            crate::finish_region_selection_impl(&ctx.app, layout, params).await?;
                        serde_json::to_value(response).map_err(|error| error.to_string())
                    })
                }),
            },
        );

        map.insert(
            "region.cancel",
            BusCommand {
                spec: catalog_spec(&catalog, "region.cancel"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    let layout = overlay_layout(&ctx.app)?;
                    crate::cancel_region_selection_impl(&ctx.app, layout)
                }),
            },
        );

        map.insert(
            "elevated.set",
            BusCommand {
                spec: catalog_spec(&catalog, "elevated.set"),
                run: sync(|ctx, params| {
                    let SetElevatedParams { enabled } = serde_json::from_value(params)
                        .map_err(|error| format!("参数不合法：{error}"))?;
                    crate::set_elevated_enabled_impl(&ctx.app, enabled)
                }),
            },
        );

        map.insert(
            "elevated.status",
            BusCommand {
                spec: catalog_spec(&catalog, "elevated.status"),
                run: sync(|ctx, params| {
                    let _: EmptyParams =
                        serde_json::from_value(params).map_err(|error| format!("参数不合法：{error}"))?;
                    Ok(crate::get_elevated_status_impl(&ctx.app))
                }),
            },
        );

        map
    })
}

/// 剪贴板状态由 lib.rs 启动时托管；缺失说明初始化失败，直接报错而不是静默降级。
fn clip_state(app: &AppHandle) -> Result<tauri::State<'_, clipboard::ClipboardState>, String> {
    app.try_state::<clipboard::ClipboardState>()
        .ok_or_else(|| "剪贴板状态未初始化。".to_string())
}

/// 覆盖层布局按值取出，避免在异步处理器里跨 await 持有 Mutex 守卫。
fn overlay_layout(app: &AppHandle) -> Result<crate::OverlayLayout, String> {
    let state = app.state::<OverlayState>();
    let layout = *state.0.lock().map_err(|error| error.to_string())?;
    Ok(layout)
}

/// 事件通道唯一发送口：先校验 id 已在 `orbit-core::event_catalog` 登记，
/// 再交给 Tauri。事件是高频通知而非命令，不写审计。
pub fn emit<T: Serialize + Clone>(app: &AppHandle, event_id: &str, payload: &T) -> Result<(), String> {
    let catalog = event_catalog().map_err(|error| error.to_string())?;
    if !catalog.iter().any(|spec| spec.id == event_id) {
        return Err(format!("事件 {event_id} 未在 event_catalog 登记"));
    }
    app.emit(event_id, payload.clone())
        .map_err(|error| error.to_string())
}

/// 命令总线唯一入口：路由 → 执行 → 异步审计 → 组装响应。
///
/// 审计写入放入后台任务，不阻塞响应路径；写入失败只记录日志，
/// 不再把审计失败上升为命令失败（避免可执行的命令被日志系统拖垮）。
pub async fn execute(app: AppHandle, audit: AuditLog, request: CommandRequest) -> CommandResponse {
    let started = Instant::now();
    let command_id = request.command_id.clone();

    let result = match registry().get(command_id.as_str()) {
        Some(entry) => (entry.run)(CommandCtx { app: app.clone() }, request.params.clone()).await,
        None => Err(format!("未知命令：{command_id}")),
    };
    let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let status = if result.is_ok() {
        CommandStatus::Ok
    } else {
        CommandStatus::Error
    };

    {
        let audit = audit;
        let command_id = command_id.clone();
        let params = request.params.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = audit.record(&command_id, &params, status, duration_ms) {
                log::error!("审计写入失败（{command_id}）：{error}");
            }
        });
    }

    let (data, error) = match result {
        Ok(data) => (Some(data), None),
        Err(error) => (None, Some(error)),
    };
    CommandResponse {
        command_id,
        status,
        data,
        error,
        duration_ms,
    }
}
