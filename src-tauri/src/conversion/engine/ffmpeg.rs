//! FFmpeg 引擎：音视频转码、视频抽帧、图片转动图/视频。
//!
//! 边由 LazyLock 生成，覆盖常见音视频源；与 image 引擎组合形成多跳路由，
//! 例如 `mp4 → png`（本引擎抽帧）→ `tiff`（image 引擎）。

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use crate::conversion::CancelToken;
use crate::conversion::model::{
    ConversionEdge, ConversionStep, Converter, ConverterStatus,
};
use crate::conversion::process;

const VIDEO_SOURCES: &[&str] = &["mp4", "mov", "mkv", "avi", "webm", "m4v"];
const AUDIO_SOURCES: &[&str] = &["mp3", "wav", "flac", "aac", "ogg", "m4a"];
const IMAGE_SOURCES: &[&str] = &["png", "jpg", "webp", "gif", "bmp"];
const VIDEO_TARGETS: &[&str] = &["mp4", "gif", "webp"];
const IMAGE_TARGETS: &[&str] = &["png", "jpg", "webp", "gif"];
const AUDIO_TARGETS: &[&str] = &["mp3", "wav", "flac"];

static EDGES: LazyLock<Vec<ConversionEdge>> = LazyLock::new(|| {
    let mut edges = Vec::new();
    // 视频 → 图片（抽帧）/ 视频 / 音频
    for from in VIDEO_SOURCES {
        for to in IMAGE_TARGETS.iter().chain(VIDEO_TARGETS).chain(AUDIO_TARGETS) {
            edges.push(ConversionEdge::new(from, to));
        }
    }
    // 音频 → 音频
    for from in AUDIO_SOURCES {
        for to in AUDIO_TARGETS {
            if from != to {
                edges.push(ConversionEdge::new(from, to));
            }
        }
    }
    // 图片 → 动图 / 视频
    for from in IMAGE_SOURCES {
        for to in ["mp4", "gif", "webp"] {
            edges.push(ConversionEdge::new(from, to));
        }
    }
    edges
});

pub(crate) struct FfmpegConverter;

impl FfmpegConverter {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Converter for FfmpegConverter {
    fn id(&self) -> &'static str {
        "ffmpeg"
    }

    fn display_name(&self) -> &'static str {
        "FFmpeg"
    }

    fn edges(&self) -> &'static [ConversionEdge] {
        &EDGES
    }

    fn status(&self) -> ConverterStatus {
        let exe = std::env::var_os("FFMPEG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("ffmpeg"));
        match std::process::Command::new(exe).arg("-version").output() {
            Ok(output) if output.status.success() => ConverterStatus::Available { version: None },
            _ => ConverterStatus::Unavailable {
                reason: "找不到 ffmpeg（请安装并加入 PATH，或设置 FFMPEG_PATH）".to_string(),
            },
        }
    }

    fn convert(
        &self,
        step: &ConversionStep,
        _on_progress: &dyn Fn(f32),
    ) -> Result<Vec<PathBuf>, String> {
        let output = convert_with_ffmpeg(
            &step.tool_path,
            &step.input_path.to_string_lossy(),
            &step.output_path.to_string_lossy(),
            &step.to,
            &step.cancel,
        )?;
        Ok(vec![PathBuf::from(output)])
    }
}

/// 实际执行转换；与冒烟测试共用，便于单独验证 ffmpeg 参数与输出路径。
pub(crate) fn convert_with_ffmpeg(
    ffmpeg: &Path,
    input_path: &str,
    output_path: &str,
    format: &str,
    cancel: &CancelToken,
) -> Result<String, String> {
    let mut output = output_path.to_string();
    if !output.to_lowercase().ends_with(&format!(".{format}")) {
        output = format!("{}.{}", output.trim_end_matches('.'), format);
    }
    if let Some(parent) = Path::new(&output).parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let args: Vec<&str> = match format {
        "mp3" => vec!["-y", "-i", input_path, "-vn", "-acodec", "libmp3lame", "-q:a", "2", &output],
        "wav" => vec!["-y", "-i", input_path, "-vn", "-acodec", "pcm_s16le", &output],
        "flac" => vec!["-y", "-i", input_path, "-vn", "-acodec", "flac", &output],
        "mp4" => vec!["-y", "-i", input_path, "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-c:a", "aac", &output],
        "gif" => vec!["-y", "-i", input_path, "-vf", "fps=12,scale=480:-1", &output],
        "webp" => vec!["-y", "-i", input_path, "-vf", "fps=15", "-loop", "0", &output],
        "png" => vec!["-y", "-i", input_path, "-frames:v", "1", &output],
        "jpg" => vec!["-y", "-i", input_path, "-frames:v", "1", "-q:v", "2", &output],
        other => return Err(format!("不支持的目标格式：{other}")),
    };

    // 10 分钟超时、60 秒无输出视为停滞；两者都支持中途取消。
    let outcome = process::run(
        ffmpeg,
        &args,
        Duration::from_secs(600),
        Duration::from_secs(60),
        cancel,
    )?;
    if outcome.status.success() {
        Ok(output)
    } else {
        let chars: Vec<char> = outcome.stderr.chars().collect();
        let tail: String = chars[chars.len().saturating_sub(400)..].iter().collect();
        Err(format!("转换失败（退出码：{:?}）：{}", outcome.status.code(), tail.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_cover_video_audio_and_extraction() {
        let has = |from: &str, to: &str| EDGES.iter().any(|e| e.from == from && e.to == to);
        assert!(has("mp4", "png"), "视频抽帧是多跳的第一跳");
        assert!(has("mov", "wav"), "视频转音频");
        assert!(has("flac", "mp3"), "音频互转");
        assert!(has("png", "gif"), "图片转动图");
        assert!(EDGES.len() > 30);
    }
}
