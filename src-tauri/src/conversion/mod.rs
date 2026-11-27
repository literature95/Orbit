//! 转换框架（P0）：引擎只声明「直接转换」，多跳路由由 Dijkstra 自动推导。
//!
//! 分层对应 Filee 的设计：
//! - `model`      — 边 / 引擎契约 / 执行步骤
//! - `engine`     — 具体引擎（当前 FFmpeg）
//! - `registry`   — 引擎注册表与能力发现
//! - `planner`    — 最短路寻路
//!
//! 命令总线门面在 `convert.rs`，本模块不依赖 Tauri，便于纯单测。

pub(crate) mod engine;
pub(crate) mod formats;
mod job;
mod model;
mod output;
mod process;
mod planner;
mod registry;

pub(crate) use job::{ConvertJob, JobRegistry};
pub(crate) use model::{CancelToken, ConversionStep, Converter, ConverterStatus};
pub(crate) use output::JobAllocator;
pub(crate) use planner::{Route, RouteStep, RoutePlanner};
pub(crate) use registry::ConverterRegistry;
