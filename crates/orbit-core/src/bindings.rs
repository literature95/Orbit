//! 事件绑定（L2）契约：事件槽位 + 修饰键 + 动作 + 持久化文档结构。
//!
//! `bindings.json` 的单一事实源。Rust 侧负责校验、持久化与快捷键注册，
//! 前端负责把悬浮球上的点击/滚轮等事件转发给 `binding.dispatch`。
//! 文件缺失或损坏时回落到 `BindingsConfig::default_config()`，不阻断启动。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

/// 事件槽位：悬浮球/覆盖层可被绑定的事件。
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum BindingEvent {
    LeftClick,
    LeftDoubleClick,
    MiddleClick,
    RightClick,
    WheelUp,
    WheelDown,
    Drop,
    HoverEnter,
    HoverLeave,
}

/// 修饰键组合：与事件一起构成绑定键。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Modifier {
    #[default]
    None,
    Ctrl,
    Alt,
    Shift,
    #[serde(rename = "ctrl+alt")]
    CtrlAlt,
    #[serde(rename = "ctrl+shift")]
    CtrlShift,
    #[serde(rename = "alt+shift")]
    AltShift,
    #[serde(rename = "ctrl+alt+shift")]
    CtrlAltShift,
}

impl Modifier {
    /// 所有合法取值，供前端下拉与后端校验共用。
    pub fn all() -> &'static [Modifier] {
        &[
            Modifier::None,
            Modifier::Ctrl,
            Modifier::Alt,
            Modifier::Shift,
            Modifier::CtrlAlt,
            Modifier::CtrlShift,
            Modifier::AltShift,
            Modifier::CtrlAltShift,
        ]
    }
}

/// 绑定动作：执行命令、触发宏，或打开某个菜单。
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum BindingAction {
    /// 走命令总线执行 `id`，可选固定参数。
    Command {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<Value>,
    },
    /// 触发已保存的宏（回放尚未实现，先占位并拒绝执行）。
    Macro { id: String },
    /// 打开菜单：`clipboard` 为剪贴板面板，`radial-main` / `radial-cursor` 为轮盘（预留）。
    Menu { menu: String },
}

/// 事件绑定：事件 + 修饰键 → 动作。
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub event: BindingEvent,
    pub modifier: Modifier,
    pub action: BindingAction,
}

/// 全局快捷键绑定：加速键字符串 → 动作。
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShortcutBinding {
    /// 形如 `Alt+V`、`CmdOrCtrl+Shift+Space`，由 Tauri 解析。
    pub accelerator: String,
    pub action: BindingAction,
}

/// `bindings.json` 顶层结构。
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingsConfig {
    pub version: u32,
    pub bindings: Vec<Binding>,
    pub shortcuts: Vec<ShortcutBinding>,
}

/// 当前配置版本；结构变更时递增并写迁移。
pub const BINDINGS_VERSION: u32 = 1;
/// 事件绑定条数上限，防止异常文件无限增长。
pub const MAX_BINDINGS: usize = 64;
/// 全局快捷键条数上限。
pub const MAX_SHORTCUTS: usize = 32;
/// 允许的菜单名白名单。
pub const MENUS: &[&str] = &["clipboard", "radial-main", "radial-cursor"];

impl BindingsConfig {
    /// 出厂配置：剪贴板面板仍挂在 `Alt+V` 上（与迁移前行为一致）。
    pub fn default_config() -> Self {
        Self {
            version: BINDINGS_VERSION,
            bindings: Vec::new(),
            shortcuts: vec![ShortcutBinding {
                accelerator: "Alt+V".to_string(),
                action: BindingAction::Menu { menu: "clipboard".to_string() },
            }],
        }
    }

    /// 结构校验：版本、数量上限、菜单白名单、加速键形态、重复键。
    pub fn validate(&self) -> Result<(), String> {
        if self.version != BINDINGS_VERSION {
            return Err(format!(
                "bindings.json 版本 {} 不受支持（当前 {BINDINGS_VERSION}）",
                self.version
            ));
        }
        if self.bindings.len() > MAX_BINDINGS {
            return Err(format!("事件绑定超过上限 {MAX_BINDINGS} 条"));
        }
        if self.shortcuts.len() > MAX_SHORTCUTS {
            return Err(format!("全局快捷键超过上限 {MAX_SHORTCUTS} 条"));
        }
        let mut seen_events = std::collections::HashSet::new();
        for binding in &self.bindings {
            validate_action(&binding.action)?;
            if !seen_events.insert((binding.event, binding.modifier)) {
                return Err(format!(
                    "事件 {:?} + 修饰键 {:?} 存在重复绑定",
                    binding.event, binding.modifier
                ));
            }
        }
        let mut seen_accelerators = std::collections::HashSet::new();
        for shortcut in &self.shortcuts {
            validate_action(&shortcut.action)?;
            if !is_plausible_accelerator(&shortcut.accelerator) {
                return Err(format!("快捷键 `{}` 形态不合法", shortcut.accelerator));
            }
            if !seen_accelerators.insert(shortcut.accelerator.to_lowercase()) {
                return Err(format!("快捷键 `{}` 重复", shortcut.accelerator));
            }
        }
        Ok(())
    }

    /// 解析事件绑定：命中则克隆动作。
    pub fn resolve(&self, event: BindingEvent, modifier: Modifier) -> Option<BindingAction> {
        self.bindings
            .iter()
            .find(|binding| binding.event == event && binding.modifier == modifier)
            .map(|binding| binding.action.clone())
    }

    /// 解析加速键（大小写不敏感），返回对应动作。
    pub fn resolve_shortcut(&self, accelerator: &str) -> Option<BindingAction> {
        self.shortcuts
            .iter()
            .find(|shortcut| shortcut.accelerator.eq_ignore_ascii_case(accelerator))
            .map(|shortcut| shortcut.action.clone())
    }
}

fn validate_action(action: &BindingAction) -> Result<(), String> {
    match action {
        BindingAction::Command { id, .. } if id.trim().is_empty() => {
            Err("命令绑定缺少命令 id".to_string())
        }
        BindingAction::Macro { id } if id.trim().is_empty() => Err("宏绑定缺少宏 id".to_string()),
        BindingAction::Menu { menu } if !MENUS.contains(&menu.as_str()) => {
            Err(format!("未知菜单 `{menu}`，可选：{}", MENUS.join(" / ")))
        }
        _ => Ok(()),
    }
}

/// 只做形态校验（`+` 分隔且至少一个主键），真实解析交给 Tauri。
fn is_plausible_accelerator(accelerator: &str) -> bool {
    let trimmed = accelerator.trim();
    if trimmed.is_empty() || trimmed.contains(' ') {
        return false;
    }
    trimmed.split('+').all(|part| !part.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid_and_keeps_alt_v() {
        let config = BindingsConfig::default_config();
        config.validate().unwrap();
        assert_eq!(
            config.resolve_shortcut("alt+v"),
            Some(BindingAction::Menu { menu: "clipboard".to_string() })
        );
    }

    #[test]
    fn wire_format_matches_documented_bindings_json() {
        let raw = r#"{
            "version": 1,
            "bindings": [
                { "event": "left-click", "modifier": "ctrl", "action": { "type": "command", "id": "screenshot.region" } }
            ],
            "shortcuts": [
                { "accelerator": "Alt+V", "action": { "type": "menu", "menu": "clipboard" } }
            ]
        }"#;
        let config: BindingsConfig = serde_json::from_str(raw).unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.resolve(BindingEvent::LeftClick, Modifier::Ctrl),
            Some(BindingAction::Command { id: "screenshot.region".to_string(), args: None })
        );
    }

    #[test]
    fn duplicate_event_binding_is_rejected() {
        let mut config = BindingsConfig::default_config();
        let binding = Binding {
            event: BindingEvent::WheelUp,
            modifier: Modifier::None,
            action: BindingAction::Command { id: "theme.next".to_string(), args: None },
        };
        config.bindings.push(binding.clone());
        config.bindings.push(binding);
        assert!(config.validate().unwrap_err().contains("重复绑定"));
    }

    #[test]
    fn unknown_menu_and_bad_accelerator_are_rejected() {
        let mut config = BindingsConfig::default_config();
        config.shortcuts[0].action = BindingAction::Menu { menu: "nope".to_string() };
        assert!(config.validate().unwrap_err().contains("未知菜单"));

        let mut config = BindingsConfig::default_config();
        config.shortcuts[0].accelerator = "Alt V".to_string();
        assert!(config.validate().unwrap_err().contains("形态不合法"));
    }

    #[test]
    fn extra_fields_and_bad_version_are_rejected() {
        let raw = r#"{"version":1,"bindings":[],"shortcuts":[],"extra":true}"#;
        assert!(serde_json::from_str::<BindingsConfig>(raw).is_err());

        let raw = r#"{"version":9,"bindings":[],"shortcuts":[]}"#;
        let config: BindingsConfig = serde_json::from_str(raw).unwrap();
        assert!(config.validate().unwrap_err().contains("版本 9"));
    }

    #[test]
    fn modifier_wire_format_uses_plus_separated_names() {
        assert_eq!(serde_json::to_value(Modifier::CtrlAlt).unwrap(), serde_json::json!("ctrl+alt"));
        assert_eq!(serde_json::to_value(Modifier::None).unwrap(), serde_json::json!("none"));
    }

    /// 显式导出配置结构：ts-rs 的自动导出只覆盖叶子类型，
    /// `BindingsConfig` 及其嵌套结构需要手动 export_all 才能出现在 bindings/。
    #[test]
    fn exports_bindings_typescript_bindings() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bindings");
        let cfg = ts_rs::Config::new().with_out_dir(&dir);
        BindingsConfig::export_all(&cfg).expect("导出 BindingsConfig 失败");
    }
}
