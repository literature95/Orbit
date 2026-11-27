//! 格式转换：命令总线门面。
//!
//! 框架在 `conversion/`：引擎只声明直接边，`RoutePlanner` 用 Dijkstra 推导路由，
//! `JobAllocator` 负责输出预占与回滚，`ConvertJob` 承载异步状态与取消令牌。
//! 本文件只负责命令编排：formats / plan / format / start / cancel / status。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use orbit_core::{ConvertFormatOption, ConvertJobState, ConvertJobStatus, ConvertPlan, ConvertPlanStep};
use tauri::{AppHandle, Manager};

use crate::conversion::{
    ConversionStep, ConvertJob, ConverterRegistry, JobAllocator, JobRegistry, RoutePlanner, formats,
};
use crate::recording::resolve_ffmpeg;

/// 命令 `convert.format`：同步执行，返回最终输出路径。
pub(crate) async fn convert_format_impl(
    app: &AppHandle,
    input_path: String,
    output_path: String,
    format: String,
) -> Result<String, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        execute_conversion(&app, &input_path, &output_path, &format, None)
    })
    .await
    .map_err(|error| format!("转换工作线程异常退出：{error}"))?
}

/// 命令 `convert.formats`：由引擎声明的边推导出的目标格式集合。
pub(crate) fn convert_formats_impl(
    source_path: Option<String>,
) -> Result<Vec<ConvertFormatOption>, String> {
    let registry = ConverterRegistry::global();
    let planner = RoutePlanner::new(registry.all());
    // 有源文件时，只返回从该格式确实能规划出路由的目标。
    let from = source_path.as_deref().and_then(|path| source_format(path).ok());
    let reachable: std::collections::HashSet<String> = planner.targets().into_iter().collect();
    // 圆盘只展示常用且确实可到达的目标；tiff/bmp/tga 等仍可由 convert.plan 规划。
    Ok(formats::common_targets()
        .into_iter()
        .filter(|to| {
            reachable.contains(to)
                && from.as_deref().map_or(true, |source| planner.plan(source, to).is_some())
        })
        .map(|id| ConvertFormatOption {
            label: formats::label(&id).unwrap_or(id.as_str()).to_string(),
            id,
        })
        .collect())
}

/// 命令 `convert.plan`：只读预览路由，不创建或修改任何文件。
pub(crate) fn convert_plan_impl(input_path: String, format: String) -> Result<ConvertPlan, String> {
    let from = source_format(&input_path)?;
    let registry = ConverterRegistry::global();
    let planner = RoutePlanner::new(registry.all());
    let route = planner
        .plan(&from, &format)
        .ok_or_else(|| unsupported(&from, &format, &planner))?;
    Ok(ConvertPlan {
        from,
        to: format.to_ascii_lowercase(),
        steps: route
            .steps
            .into_iter()
            .map(|step| ConvertPlanStep {
                converter_id: step.converter_id.to_string(),
                from: step.from,
                to: step.to,
            })
            .collect(),
        cost: route.cost,
    })
}

/// 命令 `convert.start`：先同步校验参数与路由，再后台执行，立即返回任务快照。
pub(crate) fn convert_start_impl(
    app: &AppHandle,
    input_path: String,
    output_path: String,
    format: String,
) -> Result<ConvertJobStatus, String> {
    let from = source_format(&input_path)?;
    let registry = ConverterRegistry::global();
    let planner = RoutePlanner::new(registry.all());
    if planner.plan(&from, &format).is_none() {
        return Err(unsupported(&from, &format, &planner));
    }

    let job = Arc::new(ConvertJob::new(generate_job_id(), input_path.clone(), format.clone()));
    let status = job.snapshot();
    JobRegistry::global().add(job.clone());

    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = execute_conversion(&app, &input_path, &output_path, &format, Some(&job));
    });
    Ok(status)
}

/// 命令 `convert.cancel`：置取消令牌，后台任务在下一处检查点停下并回滚。
pub(crate) fn convert_cancel_impl(job_id: String) -> Result<(), String> {
    let job = JobRegistry::global()
        .get(&job_id)
        .ok_or_else(|| format!("转换任务不存在：{job_id}"))?;
    job.cancel.cancel();
    Ok(())
}

/// 命令 `convert.status`：查询任务当前状态。
pub(crate) fn convert_status_impl(job_id: String) -> Result<ConvertJobStatus, String> {
    JobRegistry::global()
        .status(&job_id)
        .ok_or_else(|| format!("转换任务不存在：{job_id}"))
}

fn unsupported(from: &str, to: &str, planner: &RoutePlanner<'_>) -> String {
    format!(
        "不支持的转换：{from} → {}（支持：{}）",
        to.to_ascii_lowercase(),
        planner.targets().join(" / ")
    )
}

/// 任何阶段的失败都要把任务推进到终态，避免前端一直等到 Queued/Running。
fn finish_failure(app: &AppHandle, job: Option<&ConvertJob>, error: &str) {
    if let Some(job) = job {
        let cancelled = job.cancel.is_cancelled();
        job.update(|status| {
            status.state = if cancelled {
                ConvertJobState::Cancelled
            } else {
                ConvertJobState::Failed
            };
            status.error = Some(error.to_string());
            status.progress = 1.0;
        });
        job.notify(app);
    }
}

fn generate_job_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    )
}

fn execute_conversion(
    app: &AppHandle,
    input_path: &str,
    output_path: &str,
    format: &str,
    job: Option<&ConvertJob>,
) -> Result<String, String> {
    if let Some(job) = job {
        job.update(|status| {
            status.state = ConvertJobState::Running;
            status.progress = 0.02;
        });
        job.notify(app);
    }

    let from = match source_format(input_path) {
        Ok(from) => from,
        Err(error) => {
            finish_failure(app, job, &error);
            return Err(error);
        }
    };
    let registry = ConverterRegistry::global();
    let planner = RoutePlanner::new(registry.all());
    let route = match planner.plan(&from, format) {
        Some(route) if !route.steps.is_empty() => route,
        _ => {
            let error = unsupported(&from, format, &planner);
            finish_failure(app, job, &error);
            return Err(error);
        }
    };
    if let Some(job) = job {
        job.update(|status| {
            status.from = from.clone();
            status.to = format.to_ascii_lowercase();
            status.progress = 0.1;
        });
        job.notify(app);
    }

    // 任务私有工作目录：中间文件只落在这里，结束（成功或失败）都会清理。
    let work_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法定位应用数据目录：{error}"))?
        .join("convert")
        .join(generate_job_id());
    let mut allocator = JobAllocator::new(work_dir);

    let ffmpeg = resolve_ffmpeg(app);
    let cancel = job.map(|job| job.cancel.clone()).unwrap_or_default();
    let mut current = PathBuf::from(input_path);

    let result = (|| -> Result<PathBuf, String> {
        let last = route.steps.len() - 1;
        for (index, step) in route.steps.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err("转换已取消".to_string());
            }
            if let Some(job) = job {
                let progress = 0.1 + 0.8 * (index as f32 / route.steps.len() as f32);
                job.update(|status| status.progress = progress);
                job.notify(app);
            }
            let output = if index == last {
                allocator.final_output(Path::new(output_path), &step.to)?
            } else {
                allocator.temp_output(&step.to)?
            };
            let converter = registry
                .by_id(step.converter_id)
                .ok_or_else(|| format!("转换引擎未注册：{}", step.converter_id))?;
            converter.convert(
                &ConversionStep {
                    input_path: current.clone(),
                    output_path: output.clone(),
                    from: step.from.clone(),
                    to: step.to.clone(),
                    tool_path: ffmpeg.clone(),
                    cancel: cancel.clone(),
                },
                &|_| {},
            )?;
            current = output;
        }
        Ok(current)
    })();

    // 成功保留输出；失败/取消只回滚本次独占的路径，不碰用户已有文件。
    match result {
        Ok(path) => {
            let output = path.to_string_lossy().into_owned();
            allocator.finish();
            if let Some(job) = job {
                let cancelled = job.cancel.is_cancelled();
                job.update(|status| {
                    status.state = if cancelled {
                        ConvertJobState::Cancelled
                    } else {
                        ConvertJobState::Completed
                    };
                    status.progress = 1.0;
                    if !cancelled {
                        status.output_path = Some(output.clone());
                    }
                });
                job.notify(app);
            }
            Ok(output)
        }
        Err(error) => {
            allocator.discard();
            finish_failure(app, job, &error);
            Err(error)
        }
    }
}

/// 从输入路径推断源格式；无扩展名时无法规划，显式报错而不是猜。
fn source_format(path: &str) -> Result<String, String> {
    let path_buf = PathBuf::from(path);
    let extension = path_buf
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| "无法从输入路径推断源格式，请保留文件扩展名。".to_string())?;
    formats::normalize(extension)
        .map(|id| id.to_string())
        .ok_or_else(|| format!("不支持的源格式：.{extension}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_formats_are_reachable_and_bounded() {
        let options = convert_formats_impl(None).unwrap();
        let ids: Vec<&str> = options.iter().map(|option| option.id.as_str()).collect();
        assert!(ids.contains(&"png"));
        assert!(ids.contains(&"mp3"));
        assert!(ids.len() <= 12, "圆盘扇区数量必须可控");
        assert!(options.iter().all(|option| !option.label.is_empty()), "扇区需要友好标签");
    }

    #[test]
    fn formats_are_filtered_by_source_file() {
        let all = convert_formats_impl(None).unwrap();
        let from_mp3 = convert_formats_impl(Some("C:\\tmp\\a.mp3".into())).unwrap();
        assert!(from_mp3.len() < all.len());
        assert!(from_mp3.iter().any(|option| option.id == "wav"), "音频可转 wav");
        assert!(!from_mp3.iter().any(|option| option.id == "mp4"), "音频不能直接转视频");
    }

    #[test]
    fn source_format_requires_an_extension() {
        assert_eq!(source_format("C:\\tmp\\a.MP4").unwrap(), "mp4");
        assert!(source_format("C:\\tmp\\no_extension").is_err());
    }
}

/// 转换相关的最小实机冒烟测试（原 lib.rs smoke 模块中转换部分，纯移动）。
#[cfg(all(test, feature = "smoke"))]
mod smoke {
    use super::*;
    use crate::conversion::engine::ffmpeg::convert_with_ffmpeg;

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
            &crate::conversion::CancelToken::default(),
        )
        .expect("转换失败");
        assert!(output.ends_with(".mp3"), "输出扩展名不正确：{output}");
        let size = std::fs::metadata(&output).expect("转换文件不存在").len();
        assert!(size > 0, "转换文件为空");
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }
}
