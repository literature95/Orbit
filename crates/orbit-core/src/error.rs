//! 统一错误类型：命令总线的错误分类契约。
//!
//! 处理器可以返回 `Result<_, OrbitError>`（显式分类）或继续返回
//! `Result<_, String>`（经 `From<String>` 归类为 `System`），两条路径
//! 最终都由 `bus::execute` 组装成带 `error_kind` 的 `CommandResponse`，
//! 前端据此区分「参数错误 / 资源缺失 / 权限不足 / 超时 / 系统故障」。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 错误类别：跨 Rust / TS / MCP schema 的稳定枚举。
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorKind {
    /// 参数不合法或缺失（调用方问题，不应重试原参数）。
    Param,
    /// 目标资源不存在（窗口、文件、命令 id 等）。
    NotFound,
    /// 权限不足（UAC、管理员 helper、受保护资源）。
    Permission,
    /// 操作超时或等待方未响应。
    Timeout,
    /// 系统级故障（OS API、子进程、IO 等）。
    System,
    /// 当前平台或构建不支持该能力。
    Unsupported,
}

/// 统一错误：类别 + 面向用户的中文消息。
#[derive(Clone, Debug, PartialEq)]
pub struct OrbitError {
    pub kind: ErrorKind,
    pub message: String,
}

impl OrbitError {
    pub fn param(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Param, message: message.into() }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::NotFound, message: message.into() }
    }

    pub fn permission(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Permission, message: message.into() }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Timeout, message: message.into() }
    }

    pub fn system(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::System, message: message.into() }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Unsupported, message: message.into() }
    }
}

impl std::fmt::Display for OrbitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OrbitError {}

/// 旧式字符串错误默认归类为 System，保证存量 handler 无需改动。
impl From<String> for OrbitError {
    fn from(message: String) -> Self {
        Self::system(message)
    }
}

impl From<&str> for OrbitError {
    fn from(message: &str) -> Self {
        Self::system(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_errors_default_to_system_kind() {
        let error: OrbitError = "磁盘满了".to_string().into();
        assert_eq!(error.kind, ErrorKind::System);
        assert_eq!(error.to_string(), "磁盘满了");
    }

    #[test]
    fn constructors_carry_kind_and_message() {
        let error = OrbitError::param("参数不合法：missing field `id`");
        assert_eq!(error.kind, ErrorKind::Param);
        assert!(error.message.contains("missing field"));
    }

    #[test]
    fn kind_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(ErrorKind::NotFound).unwrap(),
            serde_json::json!("not_found")
        );
    }
}
