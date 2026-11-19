//! 事件绑定层（L2）：`bindings.json` 持久化、快捷键注册与动作分发。
//!
//! - 磁盘位置：`app_data_dir()/bindings.json`，原子写入（`.tmp` + rename）。
//! - 文件缺失/损坏/校验失败时回落到出厂配置并重写，不阻断启动。
//! - 全局快捷键由 Tauri 插件注册，配置变更后立即重新注册（P-07：无需重启）。
//! - 事件由前端通过 `binding.dispatch` 转发，动作统一走这里解析执行。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use orbit_core::{
    BindingsConfig, CommandRequest, CommandStatus, ErrorKind, EventBindingKeyParams, OrbitError,
    RemoveShortcutParams, UpsertEventBindingParams, UpsertShortcutParams,
};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// 托管状态：配置文件路径 + 内存中的当前配置。
pub struct BindingsState {
    path: PathBuf,
    config: Mutex<BindingsConfig>,
}

/// 启动期装配：加载或重建 `bindings.json` 并返回托管状态。
pub(crate) fn init_state(app: &AppHandle) -> Result<BindingsState, Box<dyn std::error::Error>> {
    let path = app.path().app_data_dir()?.join("bindings.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let (config, needs_rewrite) = load_or_default(&path);
    if needs_rewrite {
        if let Err(error) = save(&path, &config) {
            log::warn!("写入默认 bindings.json 失败：{error}");
        }
    }
    Ok(BindingsState { path, config: Mutex::new(config) })
}

/// 读取配置；缺失或非法时回落到出厂配置并标记需要重写。
fn load_or_default(path: &Path) -> (BindingsConfig, bool) {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!("读取 bindings.json 失败（回落到出厂配置）：{error}");
            }
            return (BindingsConfig::default_config(), true);
        }
    };
    match serde_json::from_str::<BindingsConfig>(&raw) {
        Ok(config) => match config.validate() {
            Ok(()) => (config, false),
            Err(error) => {
                log::warn!("bindings.json 校验失败（回落到出厂配置）：{error}");
                (BindingsConfig::default_config(), true)
            }
        },
        Err(error) => {
            log::warn!("bindings.json 解析失败（回落到出厂配置）：{error}");
            (BindingsConfig::default_config(), true)
        }
    }
}

/// 原子写入：先写 `.tmp` 再 rename，避免半截文件。
fn save(path: &Path, config: &BindingsConfig) -> Result<(), String> {
    let body = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|error| format!("写入 {:?} 失败：{error}", tmp))?;
    std::fs::rename(&tmp, path).map_err(|error| format!("替换 {:?} 失败：{error}", path))?;
    Ok(())
}

/// 取当前配置快照（克隆，避免跨 await 持有锁）。
fn snapshot(app: &AppHandle) -> Result<BindingsConfig, OrbitError> {
    let state = app
        .try_state::<BindingsState>()
        .ok_or_else(|| OrbitError::system("事件绑定状态未初始化。"))?;
    state
        .config
        .lock()
        .map(|guard| guard.clone())
        .map_err(|error| OrbitError::system(format!("事件绑定锁已毒化：{error}")))
}

/// 修改并持久化配置：校验 → 落盘 → 更新内存 → 广播变化。
fn mutate(
    app: &AppHandle,
    edit: impl FnOnce(&mut BindingsConfig) -> Result<(), String>,
) -> Result<BindingsConfig, OrbitError> {
    let state = app
        .try_state::<BindingsState>()
        .ok_or_else(|| OrbitError::system("事件绑定状态未初始化。"))?;
    let mut guard = state
        .config
        .lock()
        .map_err(|error| OrbitError::system(format!("事件绑定锁已毒化：{error}")))?;
    let mut next = guard.clone();
    edit(&mut next).map_err(OrbitError::param)?;
    next.validate().map_err(OrbitError::param)?;
    save(&state.path, &next).map_err(OrbitError::system)?;
    *guard = next.clone();
    drop(guard);
    if let Err(error) = crate::bus::emit(app, "binding://changed", &next) {
        log::warn!("广播 binding://changed 失败：{error}");
    }
    Ok(next)
}

/// 重新注册全部全局快捷键（先注销再按新配置注册）。
///
/// 注册失败（多被其他程序占用）只告警，不阻断配置保存：配置文件是事实源，
/// 下次启动或再次修改时会重试。
pub(crate) fn apply_shortcuts(app: &AppHandle) -> Result<(), OrbitError> {
    let config = snapshot(app)?;
    let manager = app.global_shortcut();
    if let Err(error) = manager.unregister_all() {
        log::warn!("注销全局快捷键失败：{error}");
    }
    for shortcut in &config.shortcuts {
        let accelerator = shortcut.accelerator.clone();
        let handler_accelerator = accelerator.clone();
        let register = manager.on_shortcut(&*accelerator, move |app, _shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            let action = crate::bindings::resolve_shortcut_now(app, &handler_accelerator);
            match action {
                Some(action) => spawn_action(app.clone(), action),
                None => log::warn!("快捷键 {handler_accelerator} 已无绑定动作"),
            }
        });
        if let Err(error) = register {
            log::warn!("全局快捷键 {accelerator} 注册失败（可能已被占用）：{error}");
        }
    }
    Ok(())
}

fn resolve_shortcut_now(app: &AppHandle, accelerator: &str) -> Option<orbit_core::BindingAction> {
    snapshot(app).ok()?.resolve_shortcut(accelerator)
}

/// 启动期延后注册：`on_shortcut` 内部通过 run_on_main_thread + 同步 recv 完成，
/// 在事件循环启动前的主线程调用会死锁，因此放到后台线程。
pub(crate) fn start_shortcut_registration(app: &AppHandle) {
    let handle = app.clone();
    let spawned = std::thread::Builder::new()
        .name("bindings-shortcut".to_string())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(600));
            if let Err(error) = apply_shortcuts(&handle) {
                log::warn!("事件绑定快捷键注册失败：{error}");
            }
        });
    if let Err(error) = spawned {
        log::warn!("快捷键注册线程启动失败：{error}");
    }
}

/// 异步执行动作：命令走总线，菜单做本地窗口切换，宏暂不支持。
pub(crate) async fn execute_action(
    app: &AppHandle,
    action: orbit_core::BindingAction,
    source: &'static str,
) -> Result<Value, OrbitError> {
    use orbit_core::BindingAction;
    match action {
        BindingAction::Command { id, args } => {
            let audit = app
                .state::<orbit_core::AuditLog>()
                .inner()
                .clone();
            let request = CommandRequest {
                command_id: id,
                params: args.unwrap_or_else(|| json!({})),
            };
            let response = crate::bus::execute(app.clone(), audit, request, source).await;
            match response.status {
                CommandStatus::Ok => Ok(response.data.unwrap_or(Value::Null)),
                CommandStatus::Error => Err(OrbitError {
                    kind: response.error_kind.unwrap_or(ErrorKind::System),
                    message: response
                        .error
                        .unwrap_or_else(|| "绑定命令执行失败。".to_string()),
                }),
            }
        }
        BindingAction::Menu { menu } if menu == "clipboard" => {
            // 录制期间 `Alt+V` 语义为「备用停止」，与迁移前的硬编码行为保持一致。
            let recording = app
                .state::<crate::RecordState>()
                .0
                .lock()
                .map(|guard| guard.is_some())
                .unwrap_or(false);
            if recording {
                crate::record_stop_impl(app).await?;
            } else {
                crate::clipboard::toggle_clipboard_window(app);
            }
            Ok(Value::Null)
        }
        BindingAction::Menu { menu } => Err(OrbitError::unsupported(format!(
            "菜单 `{menu}` 由前端呈现，Rust 侧不执行"
        ))),
        BindingAction::Macro { id } => Err(OrbitError::unsupported(format!(
            "宏 `{id}` 回放尚未实现"
        ))),
    }
}

/// 后台执行动作：快捷键等同步回调里不阻塞事件线程。
fn spawn_action(app: AppHandle, action: orbit_core::BindingAction) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = execute_action(&app, action.clone(), "hotkey").await {
            log::warn!("绑定动作执行失败（{action:?}）：{error}");
        }
    });
}

// ==================== 命令处理器 ====================

pub(crate) fn list_impl(app: &AppHandle) -> Result<BindingsConfig, OrbitError> {
    snapshot(app)
}

pub(crate) fn upsert_event_impl(
    app: &AppHandle,
    params: UpsertEventBindingParams,
) -> Result<BindingsConfig, OrbitError> {
    mutate(app, |config| {
        config.bindings.retain(|binding| {
            !(binding.event == params.event && binding.modifier == params.modifier)
        });
        config.bindings.push(orbit_core::Binding {
            event: params.event,
            modifier: params.modifier,
            action: params.action,
        });
        Ok(())
    })
}

pub(crate) fn remove_event_impl(
    app: &AppHandle,
    params: EventBindingKeyParams,
) -> Result<BindingsConfig, OrbitError> {
    mutate(app, |config| {
        config.bindings.retain(|binding| {
            !(binding.event == params.event && binding.modifier == params.modifier)
        });
        Ok(())
    })
}

pub(crate) fn upsert_shortcut_impl(
    app: &AppHandle,
    params: UpsertShortcutParams,
) -> Result<BindingsConfig, OrbitError> {
    let config = mutate(app, |config| {
        config
            .shortcuts
            .retain(|shortcut| !shortcut.accelerator.eq_ignore_ascii_case(&params.accelerator));
        config.shortcuts.push(orbit_core::ShortcutBinding {
            accelerator: params.accelerator.trim().to_string(),
            action: params.action,
        });
        Ok(())
    })?;
    if let Err(error) = apply_shortcuts(app) {
        log::warn!("快捷键变更后重新注册失败：{error}");
    }
    Ok(config)
}

pub(crate) fn remove_shortcut_impl(
    app: &AppHandle,
    params: RemoveShortcutParams,
) -> Result<BindingsConfig, OrbitError> {
    let config = mutate(app, |config| {
        config
            .shortcuts
            .retain(|shortcut| !shortcut.accelerator.eq_ignore_ascii_case(&params.accelerator));
        Ok(())
    })?;
    if let Err(error) = apply_shortcuts(app) {
        log::warn!("快捷键变更后重新注册失败：{error}");
    }
    Ok(config)
}

/// 前端事件入口：解析绑定并执行动作；无绑定返回 null。
pub(crate) async fn dispatch_impl(
    app: &AppHandle,
    params: EventBindingKeyParams,
) -> Result<Value, OrbitError> {
    let config = snapshot(app)?;
    match config.resolve(params.event, params.modifier) {
        Some(action) => execute_action(app, action, "orb").await,
        None => Ok(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("orbit-bindings-test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn missing_file_falls_back_to_default_and_rewrites() {
        let path = temp_config_path("missing-bindings.json");
        let _ = std::fs::remove_file(&path);
        let (config, rewrite) = load_or_default(&path);
        assert!(rewrite);
        assert_eq!(config, BindingsConfig::default_config());
    }

    #[test]
    fn corrupt_and_invalid_files_fall_back() {
        let path = temp_config_path("corrupt-bindings.json");
        std::fs::write(&path, b"not json").unwrap();
        let (_, rewrite) = load_or_default(&path);
        assert!(rewrite, "损坏文件必须回落到出厂配置并重写");

        std::fs::write(&path, r#"{"version":1,"bindings":[],"shortcuts":[]}"#).unwrap();
        let (_, rewrite) = load_or_default(&path);
        assert!(!rewrite, "合法配置不应触发重写");
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = temp_config_path("roundtrip-bindings.json");
        let mut config = BindingsConfig::default_config();
        config.bindings.push(orbit_core::Binding {
            event: orbit_core::BindingEvent::WheelUp,
            modifier: orbit_core::Modifier::Alt,
            action: orbit_core::BindingAction::Command {
                id: "screenshot.capture".to_string(),
                args: None,
            },
        });
        save(&path, &config).unwrap();
        let (loaded, rewrite) = load_or_default(&path);
        assert!(!rewrite);
        assert_eq!(
            loaded.resolve(orbit_core::BindingEvent::WheelUp, orbit_core::Modifier::Alt),
            Some(orbit_core::BindingAction::Command {
                id: "screenshot.capture".to_string(),
                args: None,
            })
        );
        let _ = std::fs::remove_file(&path);
    }
}
