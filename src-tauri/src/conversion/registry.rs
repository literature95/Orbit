//! 引擎注册表：能力发现的唯一入口。
//!
//! P0 只注册 FFmpeg；P2 会在此加入状态缓存、后台探测与引擎优先级。

use std::sync::{Arc, OnceLock};

use super::engine::ffmpeg::FfmpegConverter;
use super::model::Converter;

pub(crate) struct ConverterRegistry {
    converters: Vec<Arc<dyn Converter>>,
}

impl ConverterRegistry {
    pub(crate) fn global() -> &'static Self {
        static REGISTRY: OnceLock<ConverterRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| Self::new(vec![Arc::new(FfmpegConverter::new())]))
    }

    fn new(converters: Vec<Arc<dyn Converter>>) -> Self {
        Self { converters }
    }

    pub(crate) fn all(&self) -> &[Arc<dyn Converter>] {
        &self.converters
    }

    pub(crate) fn by_id(&self, id: &str) -> Option<Arc<dyn Converter>> {
        self.converters.iter().find(|c| c.id() == id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_exposes_ffmpeg_with_stable_id() {
        let registry = ConverterRegistry::global();
        assert_eq!(registry.all().len(), 1);
        assert!(registry.by_id("ffmpeg").is_some());
        assert!(registry.by_id("missing").is_none());
    }
}
