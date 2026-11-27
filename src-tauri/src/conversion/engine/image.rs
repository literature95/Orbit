//! Rust 原生图片引擎：纯内存解码/编码，无需外部程序。
//!
//! 与 FFmpeg 配合形成真正的多跳路由：
//! `mp4 → png`（ffmpeg 抽帧）→ `tiff / bmp / tga`（本引擎）。

use std::path::PathBuf;
use std::sync::LazyLock;

use crate::conversion::model::{ConversionEdge, ConversionStep, Converter, ConverterStatus};

const IMAGE_FORMATS: &[&str] = &["png", "jpg", "webp", "bmp", "tiff", "tga", "gif"];

static EDGES: LazyLock<Vec<ConversionEdge>> = LazyLock::new(|| {
    let mut edges = Vec::new();
    for from in IMAGE_FORMATS {
        for to in IMAGE_FORMATS {
            if from != to {
                edges.push(ConversionEdge::new(from, to));
            }
        }
    }
    edges
});

pub(crate) struct ImageConverter;

impl Converter for ImageConverter {
    fn id(&self) -> &'static str {
        "image"
    }

    fn display_name(&self) -> &'static str {
        "Rust image"
    }

    fn edges(&self) -> &'static [ConversionEdge] {
        &EDGES
    }

    fn status(&self) -> ConverterStatus {
        ConverterStatus::Available { version: None }
    }

    fn convert(
        &self,
        step: &ConversionStep,
        _on_progress: &dyn Fn(f32),
    ) -> Result<Vec<PathBuf>, String> {
        if step.cancel.is_cancelled() {
            return Err("转换已取消".to_string());
        }
        let image = image::open(&step.input_path)
            .map_err(|error| format!("读取图片失败：{error}"))?;
        let format = image::ImageFormat::from_extension(&step.to)
            .ok_or_else(|| format!("不支持的图片格式：{}", step.to))?;
        image
            .save_with_format(&step.output_path, format)
            .map_err(|error| format!("写入图片失败：{error}"))?;
        Ok(vec![step.output_path.clone()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_engine_covers_all_pairs() {
        assert_eq!(EDGES.len(), IMAGE_FORMATS.len() * (IMAGE_FORMATS.len() - 1));
    }
}
