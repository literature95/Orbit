//! 转换框架的最小契约。
//!
//! 设计要点（借鉴 Filee）：引擎只声明自己能做的直接转换边，
//! 组合能力（多跳）交给 RoutePlanner 推导；新增引擎即自动解锁新路由。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 通配源格式：FFmpeg 可读取任意输入，故其边对所有源格式生效。
pub(crate) const ANY_FORMAT: &str = "*";

/// 一条直接转换边。`cost` 10 为常规路径，越大越不优先。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ConversionEdge {
    pub(crate) from: &'static str,
    pub(crate) to: &'static str,
    pub(crate) cost: u32,
    /// 只能单独成路由，不能作为多跳中的中间跳。
    pub(crate) direct_only: bool,
}

impl ConversionEdge {
    pub(crate) const fn new(from: &'static str, to: &'static str) -> Self {
        Self { from, to, cost: 10, direct_only: false }
    }
}

/// 引擎当前可用性。缺失依赖的引擎不应在 UI 里被当作可转换。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConverterStatus {
    Available { version: Option<String> },
    Unavailable { reason: String },
}

impl ConverterStatus {
    pub(crate) fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

/// 取消令牌：引擎在长循环里检查，总线在任务间检查。
#[derive(Clone, Debug, Default)]
pub(crate) struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 一跳的执行上下文。
///
/// `tool_path` 是该引擎需要的外部程序路径（由总线用 `resolve_ffmpeg` 统一解析），
/// 这样框架本身不依赖 Tauri，也不让每个引擎各自找程序。
#[derive(Clone, Debug)]
pub(crate) struct ConversionStep {
    pub(crate) input_path: PathBuf,
    pub(crate) output_path: PathBuf,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) tool_path: PathBuf,
    pub(crate) cancel: CancelToken,
}

/// 所有转换引擎实现的契约。
pub(crate) trait Converter: Send + Sync + 'static {
    /// 稳定 id，用于注册、路由回溯与设置。
    fn id(&self) -> &'static str;

    /// 人类可读名称（不本地化）。
    fn display_name(&self) -> &'static str;

    /// 只声明直接转换；组合路由由 planner 推导。
    fn edges(&self) -> &'static [ConversionEdge];

    /// 可用性检查，应足够便宜（P2 会加入缓存与后台探测）。
    fn status(&self) -> ConverterStatus;

    /// 执行一跳，返回实际写出的文件。`on_progress` 取值 0..1。
    fn convert(
        &self,
        step: &ConversionStep,
        on_progress: &dyn Fn(f32),
    ) -> Result<Vec<PathBuf>, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_availability() {
        assert!(ConverterStatus::Available { version: None }.is_available());
        assert!(!ConverterStatus::Unavailable { reason: "缺失".into() }.is_available());
    }
}
