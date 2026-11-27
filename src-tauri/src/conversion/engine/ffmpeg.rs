//! FFmpeg 引擎：把原 `convert.rs` 的格式白名单与参数表收进 Converter 契约。
//!
//! 每个目标格式是一条从通配源（`*`）出发的边。新增目标格式只需在这里加一条边
//! 和一个参数分支，规划器、`convert.formats`、`convert.plan` 自动随之更新。

use std::path::{Path, PathBuf};

use crate::NoWindow;
use crate::conversion::model::{
    ANY_FORMAT, ConversionEdge, ConversionStep, Converter, ConverterStatus,
};

/// 目标格式 → ffmpeg 参数。与旧实现保持一致，避免行为漂移。
const EDGES: &[ConversionEdge] = &[
    ConversionEdge::new(ANY_FORMAT, "mp3"),
    ConversionEdge::new(ANY_FORMAT, "wav"),
    ConversionEdge::new(ANY_FORMAT, "mp4"),
    ConversionEdge::new(ANY_FORMAT, "gif"),
    ConversionEdge::new(ANY_FORMAT, "webp"),
];

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
        EDGES
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
    cancel: &crate::conversion::CancelToken,
) -> Result<String, String> {
    let mut output = output_path.to_string();
    if !output.to_lowercase().ends_with(&format!(".{format}")) {
        output = format!("{}.{}", output.trim_end_matches('.'), format);
    }
    let out_path = Path::new(&output);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let args: Vec<&str> = match format {
        "mp3" => vec!["-y", "-i", input_path, "-vn", "-acodec", "libmp3lame", "-q:a", "2", &output],
        "wav" => vec!["-y", "-i", input_path, "-vn", "-acodec", "pcm_s16le", &output],
        "mp4" => vec!["-y", "-i", input_path, "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-c:a", "aac", &output],
        "gif" => vec!["-y", "-i", input_path, "-vf", "fps=12,scale=480:-1", &output],
        "webp" => vec!["-y", "-i", input_path, "-vf", "fps=15", "-loop", "0", &output],
        other => return Err(format!("不支持的目标格式：{other}")),
    };

    let mut child = std::process::Command::new(ffmpeg)
        .no_window()
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "找不到 ffmpeg。请安装 ffmpeg 并加入 PATH，或设置 FFMPEG_PATH 环境变量。".to_string()
            } else {
                format!("FFmpeg 启动失败：{error}")
            }
        })?;

    // stderr 必须在独立线程排空，否则 ffmpeg 日志写满管道后会阻塞。
    let stderr_thread = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut buffer = String::new();
            use std::io::Read;
            let _ = stderr.read_to_string(&mut buffer);
            buffer
        })
    });

    // 轮询等待：每 50ms 检查一次取消令牌，支持运行中取消。
    let status = loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("转换已取消".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(error) => return Err(format!("等待 ffmpeg 退出失败：{error}")),
        }
    };
    let stderr = stderr_thread
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();

    if status.success() {
        Ok(output)
    } else {
        let chars: Vec<char> = stderr.chars().collect();
        let tail: String = chars[chars.len().saturating_sub(400)..].iter().collect();
        Err(format!("转换失败（退出码：{:?}）：{}", status.code(), tail.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_edges_cover_the_previous_whitelist() {
        let targets: Vec<&str> = EDGES.iter().map(|edge| edge.to).collect();
        assert_eq!(targets, vec!["mp3", "wav", "mp4", "gif", "webp"]);
        assert!(EDGES.iter().all(|edge| edge.from == ANY_FORMAT));
    }
}
