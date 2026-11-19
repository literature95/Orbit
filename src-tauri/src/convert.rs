//! 格式转换：FFmpeg 转码命令封装。
//!
//! 从 lib.rs 按主题拆出（纯移动，不改行为）。

use tauri::AppHandle;

use crate::NoWindow;
use crate::recording::resolve_ffmpeg;

const CONVERT_FORMATS: &[&str] = &["mp3", "wav", "mp4", "gif", "webp"];

pub(crate) async fn convert_format_impl(
    app: &AppHandle,
    input_path: String,
    output_path: String,
    format: String,
) -> Result<String, String> {
    if !CONVERT_FORMATS.contains(&format.as_str()) {
        return Err(format!(
            "不支持的目标格式：{format}（支持：{}）",
            CONVERT_FORMATS.join(" / ")
        ));
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ffmpeg = resolve_ffmpeg(&app);
        convert_with_ffmpeg(&ffmpeg, &input_path, &output_path, &format)
    })
    .await
    .map_err(|error| format!("转换工作线程异常退出：{error}"))?
}

/// 实际执行转换；与测试共用，便于单独验证 ffmpeg 参数与输出路径。
fn convert_with_ffmpeg(
    ffmpeg: &std::path::Path,
    input_path: &str,
    output_path: &str,
    format: &str,
) -> Result<String, String> {
    let mut output = output_path.to_string();
    if !output.to_lowercase().ends_with(&format!(".{format}")) {
        output = format!("{}.{}", output.trim_end_matches('.'), format);
    }
    let out_path = std::path::Path::new(&output);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let args: Vec<&str> = match format {
        "mp3" => vec!["-y", "-i", input_path, "-vn", "-acodec", "libmp3lame", "-q:a", "2", &output],
        "wav" => vec!["-y", "-i", input_path, "-vn", "-acodec", "pcm_s16le", &output],
        "mp4" => vec!["-y", "-i", input_path, "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-c:a", "aac", &output],
        "gif" => vec!["-y", "-i", input_path, "-vf", "fps=12,scale=480:-1", &output],
        "webp" => vec!["-y", "-i", input_path, "-vf", "fps=15", "-loop", "0", &output],
        _ => vec!["-y", "-i", input_path, &output],
    };

    let converted = std::process::Command::new(ffmpeg)
        .no_window()
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "找不到 ffmpeg。请安装 ffmpeg 并加入 PATH，或设置 FFMPEG_PATH 环境变量。".to_string()
            } else {
                format!("FFmpeg 启动失败：{}", e)
            }
        })?;

    if converted.status.success() {
        Ok(output)
    } else {
        let stderr = String::from_utf8_lossy(&converted.stderr);
        let chars: Vec<char> = stderr.chars().collect();
        let tail: String = chars[chars.len().saturating_sub(400)..].iter().collect();
        Err(format!("转换失败（退出码：{:?}）：{}", converted.status.code(), tail.trim()))
    }
}

/// 转换相关的最小实机冒烟测试（原 lib.rs smoke 模块中转换部分，纯移动）。
#[cfg(all(test, feature = "smoke"))]
mod smoke {
    use super::*;

    fn ffmpeg_available() -> bool {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("orbit-smoke");
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir.join(name)
    }

    #[test]
    fn convert_tone_to_mp3() {
        if !ffmpeg_available() {
            eprintln!("跳过：未找到 ffmpeg");
            return;
        }
        let input = temp_path("convert-smoke.wav");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.3",
                &input.to_string_lossy(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("生成测试音频失败");
        assert!(status.success(), "生成测试音频失败");

        let output = convert_with_ffmpeg(
            std::path::Path::new("ffmpeg"),
            &input.to_string_lossy(),
            &temp_path("convert-smoke").to_string_lossy(),
            "mp3",
        )
        .expect("转换失败");
        assert!(output.ends_with(".mp3"), "输出扩展名不正确：{output}");
        let size = std::fs::metadata(&output).expect("转换文件不存在").len();
        assert!(size > 0, "转换文件为空");
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }
}
