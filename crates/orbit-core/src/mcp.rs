//! MCP 工具目录：从命令目录派生的只读工具集。
//!
//! 安全边界：只有 `RiskLevel::ReadOnly` 的命令会成为 MCP 工具，
//! `Mutating` / `Destructive` 一律不暴露（README：mouse_click / keyboard_type /
//! replay_macro 默认不给 MCP）。sidecar 与主进程各自用同一份目录做双重校验。

use crate::contract::{CommandSpec, RiskLevel, command_catalog};
use schemars::JsonSchema;
use serde::Serialize;

/// 主进程写给 sidecar 的桥连接信息（仅回环地址 + 一次性 token）。
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeInfo {
    pub port: u16,
    pub token: String,
}

/// 一个暴露给 MCP 的工具描述。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    /// 工具名 = 命令 id（如 `system.info`），避免两套命名漂移。
    pub name: String,
    pub description: String,
    /// 参数 JSON Schema，直接取命令契约的 `params_schema`。
    pub input_schema: serde_json::Value,
}

/// 只读工具列表（按 id 排序，保证 tools/list 输出稳定）。
pub fn read_only_tools() -> Result<Vec<McpTool>, serde_json::Error> {
    let mut tools: Vec<McpTool> = command_catalog()?
        .into_iter()
        .filter(|spec| spec.risk == RiskLevel::ReadOnly)
        .map(|spec| tool_from_spec(&spec))
        .collect();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tools)
}

/// 某个命令 id 是否允许通过 MCP 调用。
pub fn is_exposed(id: &str) -> bool {
    command_catalog()
        .map(|catalog| {
            catalog
                .iter()
                .any(|spec| spec.id == id && spec.risk == RiskLevel::ReadOnly)
        })
        .unwrap_or(false)
}

fn tool_from_spec(spec: &CommandSpec) -> McpTool {
    let description = if spec.idempotent {
        format!("{}（只读，幂等）", spec.label)
    } else {
        format!("{}（只读）", spec.label)
    };
    McpTool {
        name: spec.id.to_string(),
        description,
        input_schema: spec.params_schema.clone(),
    }
}

/// tools/call 的 `arguments` 必须是对象；缺省按空参数处理。
pub fn normalize_arguments(arguments: Option<serde_json::Value>) -> serde_json::Value {
    match arguments {
        None => serde_json::json!({}),
        Some(value) if value.is_null() => serde_json::json!({}),
        Some(value) => value,
    }
}

#[derive(Clone, Debug, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpCallRequest {
    pub name: String,
    #[serde(default)]
    pub arguments: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_read_only_commands_are_exposed() {
        let tools = read_only_tools().unwrap();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert!(names.contains(&"system.info"));
        assert!(names.contains(&"clipboard.list"));
        assert!(!names.contains(&"clipboard.delete"), "破坏性命令不能暴露给 MCP");
        assert!(!names.contains(&"kb.replay"), "输入合成不能暴露给 MCP");
        assert!(!names.contains(&"screenshot.capture"), "写剪贴板的命令不暴露");
    }

    #[test]
    fn exposure_check_matches_catalog_risk() {
        assert!(is_exposed("system.dash"));
        assert!(!is_exposed("clipboard.clear"));
        assert!(!is_exposed("does.not.exist"));
    }

    #[test]
    fn tools_are_sorted_and_carry_schemas() {
        let tools = read_only_tools().unwrap();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "tools/list 输出应稳定排序");
        assert!(tools.iter().all(|tool| tool.input_schema.is_object()));
    }

    #[test]
    fn arguments_default_to_empty_object() {
        assert_eq!(normalize_arguments(None), serde_json::json!({}));
        assert_eq!(normalize_arguments(Some(serde_json::Value::Null)), serde_json::json!({}));
        assert_eq!(
            normalize_arguments(Some(serde_json::json!({"query": "x"}))),
            serde_json::json!({"query": "x"})
        );
    }
}
