use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CommandCategory {
    Capture,
    Clipboard,
    Input,
    Ocr,
    Convert,
    Theme,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RiskLevel {
    ReadOnly,
    Mutating,
    Destructive,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CommandStatus {
    Ok,
    Error,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CommandRequest {
    pub command_id: String,
    #[ts(type = "unknown")]
    pub params: Value,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommandResponse {
    pub command_id: String,
    pub status: CommandStatus,
    #[ts(type = "unknown | null")]
    pub data: Option<Value>,
    pub error: Option<String>,
    #[ts(type = "number")]
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub category: CommandCategory,
    pub risk: RiskLevel,
    #[ts(type = "unknown")]
    pub params_schema: Value,
    #[ts(type = "unknown")]
    pub returns_schema: Value,
    pub idempotent: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ScreenshotMode {
    Fullscreen,
    Region,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ScreenshotRequest {
    pub mode: ScreenshotMode,
    pub monitor_id: Option<u32>,
    pub x: Option<u32>,
    pub y: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScreenshotResponse {
    width: u32,
    height: u32,
    monitor_count: usize,
}

pub fn command_catalog() -> Result<Vec<CommandSpec>, serde_json::Error> {
    let params_schema = serde_json::to_value(schemars::schema_for!(ScreenshotRequest))?;
    let returns_schema = serde_json::to_value(schemars::schema_for!(ScreenshotResponse))?;
    Ok(vec![CommandSpec {
        id: "screenshot.capture",
        label: "截图到剪贴板",
        category: CommandCategory::Capture,
        risk: RiskLevel::Mutating,
        params_schema,
        returns_schema,
        idempotent: false,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_exposes_only_the_registered_screenshot_command() {
        let catalog = command_catalog().unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].id, "screenshot.capture");
        assert_eq!(catalog[0].risk, RiskLevel::Mutating);
    }

    #[test]
    fn screenshot_request_rejects_unknown_fields() {
        let request = serde_json::from_value::<ScreenshotRequest>(serde_json::json!({
            "mode": "fullscreen",
            "outputPath": "unexpected.png"
        }));
        assert!(request.is_err());
    }
}
