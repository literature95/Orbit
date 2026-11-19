//! 录屏：FFmpeg gdigrab 分段录制、暂停/恢复状态机、恢复区与分段拼接。
//!
//! 从 lib.rs 按主题拆出（纯移动，不改行为）。

use orbit_core::{CaptureRegion, RecordStateEvent, RecoveryItem};
use std::sync::Mutex;
use std::time::Instant;
use tauri::AppHandle;
use tauri::Manager;
use xcap::Monitor;

use crate::bus;
use crate::NoWindow;

/// FFmpeg 查找顺序：FFMPEG_PATH 环境变量 → sidecar 资源目录 → 系统 PATH。
/// 录屏与格式转换共用同一规则。
pub(crate) fn resolve_ffmpeg(app: &AppHandle) -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("FFMPEG_PATH") {
        return std::path::PathBuf::from(p);
    }
    if let Ok(sidecar) = app
        .path()
        .resolve("ffmpeg", tauri::path::BaseDirectory::Resource)
    {
        if sidecar.exists() {
            return sidecar;
        }
    }
    std::path::PathBuf::from("ffmpeg")
}

pub(crate) struct RecordState(pub(crate) Mutex<Option<Recording>>);
struct RecordHandle {
    path: String,
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
}

/// 一次录制会话：由若干段组成，暂停时结束当前段，继续时开启新段，
/// 停止时把所有段拼接成单个 MP4。gdigrab 无原生暂停，分段拼接让
/// 暂停期间不计入最终时间轴（复刻 Kiri 的活跃时钟语义）。
pub(crate) struct Recording {
    ffmpeg: std::path::PathBuf,
    region: Option<ResolvedRegion>,
    output_dir: std::path::PathBuf,
    base_name: String,
    final_path: std::path::PathBuf,
    segments: Vec<std::path::PathBuf>,
    current: Option<RecordHandle>,
    paused: bool,
}

/// 录屏区域经显示器 origin 换算后的 gdigrab 虚拟桌面绝对坐标。
#[derive(Clone, Copy, Debug, PartialEq)]
struct ResolvedRegion {
    abs_x: i32,
    abs_y: i32,
    width: u32,
    height: u32,
}

/// monitorId 来自 xcap（与区域截图同一来源），x/y 为该显示器内的物理像素。
fn resolve_record_region(region: &CaptureRegion) -> Result<ResolvedRegion, String> {
    if region.width == 0 || region.height == 0 {
        return Err("录屏区域的宽高必须大于 0。".to_string());
    }
    let monitors = Monitor::all().map_err(|error| error.to_string())?;
    let mut origin = None;
    for candidate in &monitors {
        if candidate.id().map_err(|error| error.to_string())? == region.monitor_id {
            origin = Some((
                candidate.x().map_err(|error| error.to_string())?,
                candidate.y().map_err(|error| error.to_string())?,
            ));
            break;
        }
    }
    let (origin_x, origin_y) =
        origin.ok_or_else(|| format!("录屏区域引用了不存在的显示器 {}", region.monitor_id))?;
    Ok(ResolvedRegion {
        abs_x: origin_x.saturating_add(i32::try_from(region.x).unwrap_or(i32::MAX)),
        abs_y: origin_y.saturating_add(i32::try_from(region.y).unwrap_or(i32::MAX)),
        width: region.width,
        height: region.height,
    })
}

/// 构造 gdigrab 参数：与显示器枚举解耦，便于单测。
/// 区域录制时在 `-i desktop` 前插入 offset_x / offset_y / video_size。
fn ffmpeg_record_args(path_str: &str, region: Option<&ResolvedRegion>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-f".into(),
        "gdigrab".into(),
        "-framerate".into(),
        "30".into(),
        "-draw_mouse".into(),
        "1".into(),
    ];
    if let Some(region) = region {
        args.extend([
            "-offset_x".into(),
            region.abs_x.to_string(),
            "-offset_y".into(),
            region.abs_y.to_string(),
            "-video_size".into(),
            format!("{}x{}", region.width, region.height),
        ]);
    }
    args.extend([
        "-i".into(),
        "desktop".into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-b:v".into(),
        "4M".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        path_str.into(),
    ]);
    args
}

/// 恢复区：收尾被强杀时保留可能不完整的 MP4，避免素材直接丢失。
fn recovery_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("record-recovery"))
}

fn recovery_manifest(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("recovery.json")
}

fn register_recovery_item(dir: &std::path::Path, item: RecoveryItem) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let manifest = recovery_manifest(dir);
    let mut items: Vec<RecoveryItem> = if manifest.exists() {
        let raw = std::fs::read_to_string(&manifest).map_err(|error| error.to_string())?;
        serde_json::from_str(&raw).unwrap_or_default()
    } else {
        Vec::new()
    };
    items.push(item);
    let staged = dir.join("recovery.json.tmp");
    let payload = serde_json::to_string_pretty(&items).map_err(|error| error.to_string())?;
    std::fs::write(&staged, payload).map_err(|error| error.to_string())?;
    std::fs::rename(&staged, &manifest).map_err(|error| error.to_string())?;
    Ok(())
}

fn list_recovery_items(dir: &std::path::Path) -> Result<Vec<RecoveryItem>, String> {
    let manifest = recovery_manifest(dir);
    if !manifest.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&manifest).map_err(|error| error.to_string())?;
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

pub(crate) fn list_recovery_impl(app: &AppHandle) -> Result<Vec<RecoveryItem>, String> {
    list_recovery_items(&recovery_dir(app)?)
}

/// 切换悬浮球主窗口可见性。录制期间不再隐藏（改为显示计时），
/// gdigrab 无法像 WGC 那样排除自身窗口，接受其进入录制画面。
fn set_main_visible(app: &AppHandle, visible: bool) {
    if let Some(window) = app.get_webview_window("main") {
        let result = if visible { window.show() } else { window.hide() };
        if let Err(error) = result {
            log::warn!("切换悬浮球可见性失败：{error}");
        }
    }
}

fn emit_record_state(
    app: &AppHandle,
    phase: &str,
    path: Option<String>,
    message: Option<String>,
    paused: bool,
) {
    let event = RecordStateEvent {
        phase: phase.to_string(),
        path,
        message,
        paused,
    };
    if let Err(error) = bus::emit(app, "record-state", &event) {
        log::warn!("录屏状态事件发送失败：{error}");
    }
}

pub(crate) fn record_start_impl(
    app: &AppHandle,
    region: Option<CaptureRegion>,
    output_dir: Option<String>,
) -> Result<String, String> {
    let state = app.state::<RecordState>();
    let mut guard = state.0.lock().unwrap();
    if guard.is_some() {
        return Err("已经在录制中".to_string());
    }
    let ffmpeg = resolve_ffmpeg(app);
    let resolved = region.as_ref().map(resolve_record_region).transpose()?;

    // 输出目录：优先使用前端配置的保存位置，缺省回退到桌面/Orbit。
    let dir = match output_dir {
        Some(dir) if !dir.trim().is_empty() => std::path::PathBuf::from(dir),
        _ => {
            let home = std::env::var("USERPROFILE")
                .map(std::path::PathBuf::from)
                .or_else(|_| std::env::var("HOME").map(std::path::PathBuf::from))
                .map_err(|_| "找不到用户目录".to_string())?;
            home.join("Desktop").join("Orbit")
        }
    };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let base_name = format!("orbit_rec_{}", stamp);
    let final_path = dir.join(format!("{}.mp4", base_name));
    let seg0 = dir.join(format!("{}_seg0.mp4", base_name));
    let seg0_str = seg0.to_string_lossy().to_string();
    let handle = spawn_ffmpeg_recording(&ffmpeg, &seg0_str, resolved.as_ref())?;
    let recording = Recording {
        ffmpeg,
        region: resolved,
        output_dir: dir.clone(),
        base_name,
        final_path: final_path.clone(),
        segments: Vec::new(),
        current: Some(handle),
        paused: false,
    };
    *guard = Some(recording);
    drop(guard);
    set_main_visible(app, true);
    emit_record_state(app, "started", None, None, false);
    Ok(final_path.to_string_lossy().to_string())
}

/// 暂停：优雅结束当前段并标记为暂停态；继续时再开启新段。
pub(crate) fn record_pause_impl(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<RecordState>();
    let mut guard = state.0.lock().unwrap();
    let recording = guard
        .as_mut()
        .ok_or_else(|| "没有正在录制的会话".to_string())?;
    if recording.paused {
        return Err("已经在暂停状态".to_string());
    }
    if let Some(handle) = recording.current.take() {
        let dir = recovery_dir(app)
            .unwrap_or_else(|_| std::env::temp_dir().join("orbit-record-recovery"));
        let outcome = finish_recording(handle, &dir)?;
        recording.segments.push(std::path::PathBuf::from(outcome.path));
    }
    recording.paused = true;
    drop(guard);
    emit_record_state(app, "started", None, None, true);
    Ok(())
}

/// 继续：暂停态下开启新段，恢复录制。
pub(crate) fn record_resume_impl(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<RecordState>();
    let mut guard = state.0.lock().unwrap();
    let recording = guard
        .as_mut()
        .ok_or_else(|| "没有正在录制的会话".to_string())?;
    if !recording.paused {
        return Err("当前未处于暂停状态".to_string());
    }
    let seg_index = recording.segments.len();
    let seg_path = recording
        .output_dir
        .join(format!("{}_seg{}.mp4", recording.base_name, seg_index));
    let seg_str = seg_path.to_string_lossy().to_string();
    let handle = spawn_ffmpeg_recording(&recording.ffmpeg, &seg_str, recording.region.as_ref())?;
    recording.current = Some(handle);
    recording.paused = false;
    drop(guard);
    emit_record_state(app, "started", None, None, false);
    Ok(())
}

/// 启动 FFmpeg gdigrab 录制，返回句柄；与测试共用。
fn spawn_ffmpeg_recording(
    ffmpeg: &std::path::Path,
    path_str: &str,
    region: Option<&ResolvedRegion>,
) -> Result<RecordHandle, String> {
    let args = ffmpeg_record_args(path_str, region);
    let mut child = std::process::Command::new(ffmpeg)
        .no_window()
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "找不到 ffmpeg。请安装 ffmpeg 并加入 PATH，或设置 FFMPEG_PATH 环境变量。".to_string()
            } else {
                format!("FFmpeg 启动失败：{}", e)
            }
        })?;
    let stdin = child.stdin.take();
    Ok(RecordHandle {
        path: path_str.to_string(),
        child,
        stdin,
    })
}

struct RecordOutcome {
    path: String,
    recovered: bool,
}

/// 正常收尾：向 stdin 写 q 让 FFmpeg 写完 mp4 尾部，超时才强杀。
/// 强杀意味着尾部可能不完整，此时把文件移入恢复区并登记 manifest。
fn finish_recording(
    handle: RecordHandle,
    recovery_dir: &std::path::Path,
) -> Result<RecordOutcome, String> {
    use std::io::Write;
    let mut handle = handle;
    if let Some(mut stdin) = handle.stdin.take() {
        let _ = stdin.write_all(b"q");
        let _ = stdin.flush();
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(8);
    let mut forced_kill = false;
    loop {
        match handle.child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = handle.child.kill();
                    let _ = handle.child.wait();
                    forced_kill = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => break,
        }
    }
    if forced_kill {
        let source = std::path::Path::new(&handle.path);
        if source.exists() {
            let name = source
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| "orbit_rec_recovered.mp4".to_string());
            std::fs::create_dir_all(recovery_dir).map_err(|error| error.to_string())?;
            let dest = recovery_dir.join(&name);
            let moved = std::fs::rename(source, &dest).is_ok()
                || (std::fs::copy(source, &dest).is_ok() && std::fs::remove_file(source).is_ok());
            if moved {
                let item = RecoveryItem {
                    path: dest.to_string_lossy().to_string(),
                    reason: "FFmpeg 收尾超时，MP4 尾部可能不完整".to_string(),
                    created_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs().to_string())
                        .unwrap_or_default(),
                    size_bytes: std::fs::metadata(&dest).map(|meta| meta.len()).unwrap_or(0),
                };
                register_recovery_item(recovery_dir, item)?;
                return Ok(RecordOutcome {
                    path: dest.to_string_lossy().to_string(),
                    recovered: true,
                });
            }
        }
    }
    Ok(RecordOutcome {
        path: handle.path,
        recovered: false,
    })
}

pub(crate) async fn record_stop_impl(app: &AppHandle) -> Result<Option<String>, String> {
    let recording = app
        .state::<RecordState>()
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "没有正在录制的会话".to_string())?;
    let paused = recording.paused;
    set_main_visible(app, true);

    emit_record_state(app, "saving", None, None, paused);
    let mut segments = recording.segments;
    if let Some(handle) = recording.current {
        let dir = recovery_dir(app)
            .unwrap_or_else(|_| std::env::temp_dir().join("orbit-record-recovery"));
        let outcome = tauri::async_runtime::spawn_blocking(move || finish_recording(handle, &dir))
            .await
            .map_err(|error| format!("录屏停止线程异常退出：{error}"))??;
        segments.push(std::path::PathBuf::from(outcome.path.clone()));
        if outcome.recovered {
            emit_record_state(
                app,
                "recovering",
                Some(outcome.path.clone()),
                Some("录屏收尾超时，文件已移入恢复区".to_string()),
                false,
            );
            return Ok(Some(outcome.path));
        }
    }
    let final_path = recording.final_path.clone();
    match merge_segments(&segments, &final_path, &recording.ffmpeg) {
        Ok(()) => {
            emit_record_state(
                app,
                "saved",
                Some(final_path.to_string_lossy().to_string()),
                None,
                false,
            );
            Ok(Some(final_path.to_string_lossy().to_string()))
        }
        Err(error) => {
            emit_record_state(app, "recovering", None, Some(error.clone()), false);
            Err(error)
        }
    }
}

/// 把所有分段拼接成单个 MP4：单段直接重命名，多段用 concat demuxer 复制流。
/// 各段参数一致（同一 gdigrab 命令行），-fflags +genpts 重新生成时间戳，
/// 最终时长 = 各段之和，暂停期间不计入时间轴。
fn merge_segments(
    segments: &[std::path::PathBuf],
    final_path: &std::path::Path,
    ffmpeg: &std::path::Path,
) -> Result<(), String> {
    if segments.is_empty() {
        return Err("没有可拼接的录制片段。".to_string());
    }
    if segments.len() == 1 {
        let src = &segments[0];
        if src.exists() {
            if final_path.exists() {
                let _ = std::fs::remove_file(final_path);
            }
            std::fs::rename(src, final_path)
                .or_else(|_| std::fs::copy(src, final_path).and_then(|_| std::fs::remove_file(src)))
                .map_err(|e| format!("单段重命名失败：{e}"))?;
        }
        return Ok(());
    }
    let list_path = std::env::temp_dir().join(format!(
        "orbit_concat_{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let mut list_content = String::new();
    for seg in segments {
        let p = seg.to_string_lossy().replace('\\', "/");
        list_content.push_str(&format!("file '{}'\n", p));
    }
    std::fs::write(&list_path, list_content).map_err(|e| e.to_string())?;
    let status = std::process::Command::new(ffmpeg)
        .no_window()
        .args([
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-fflags",
            "+genpts",
            "-i",
            &list_path.to_string_lossy(),
            "-c",
            "copy",
            &final_path.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("FFmpeg 拼接失败：{e}"))?;
    let _ = std::fs::remove_file(&list_path);
    if !status.success() {
        return Err("FFmpeg 拼接分段失败。".to_string());
    }
    for seg in segments {
        let _ = std::fs::remove_file(seg);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ResolvedRegion, ffmpeg_record_args, list_recovery_items, register_recovery_item};

    #[test]
    fn ffmpeg_record_args_support_optional_region() {
        let full = ffmpeg_record_args("out.mp4", None);
        assert!(!full.iter().any(|arg| arg == "-offset_x"));
        assert!(full.windows(2).any(|pair| pair[0] == "-f" && pair[1] == "gdigrab"));
        let region = ResolvedRegion {
            abs_x: 120,
            abs_y: 80,
            width: 320,
            height: 240,
        };
        let args = ffmpeg_record_args("out.mp4", Some(&region));
        let flag = |name: &str| -> String {
            let index = args
                .iter()
                .position(|arg| arg == name)
                .unwrap_or_else(|| panic!("缺少 {name} 参数"));
            args[index + 1].clone()
        };
        assert_eq!(flag("-offset_x"), "120");
        assert_eq!(flag("-offset_y"), "80");
        assert_eq!(flag("-video_size"), "320x240");
        assert_eq!(args.last().map(String::as_str), Some("out.mp4"));
    }

    #[test]
    fn recovery_manifest_round_trips_items() {
        let dir = std::env::temp_dir().join(format!(
            "orbit-recovery-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let item = orbit_core::RecoveryItem {
            path: "recovered.mp4".to_string(),
            reason: "ffmpeg 收尾超时".to_string(),
            created_at: "2026-11-11T00:00:00Z".to_string(),
            size_bytes: 42,
        };
        assert!(list_recovery_items(&dir).unwrap().is_empty());
        register_recovery_item(&dir, item.clone()).unwrap();
        register_recovery_item(&dir, item.clone()).unwrap();
        let items = list_recovery_items(&dir).unwrap();
        assert_eq!(items, vec![item.clone(), item]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 录屏相关的最小实机冒烟测试（原 lib.rs smoke 模块中录屏部分，纯移动）。
#[cfg(all(test, feature = "smoke"))]
mod smoke {
    use super::*;
    use xcap::Monitor;

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

    fn ffprobe_available() -> bool {
        std::process::Command::new("ffprobe")
            .arg("-version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// 用 ffprobe 反读产物，验证 mp4 尾部完整且视频流元数据可读。
    fn ffprobe_video_metadata(path: &std::path::Path) -> serde_json::Value {
        let output = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,width,height:format=duration",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .expect("启动 ffprobe 失败");
        assert!(
            output.status.success(),
            "ffprobe 无法读取录屏文件：{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("ffprobe 输出不是合法 JSON")
    }

    fn record_and_probe(
        path: &std::path::Path,
        region: Option<ResolvedRegion>,
    ) -> serde_json::Value {
        let path_str = path.to_string_lossy().to_string();
        let recovery = temp_path("record-recovery-smoke");
        let handle =
            spawn_ffmpeg_recording(std::path::Path::new("ffmpeg"), &path_str, region.as_ref())
                .expect("启动录制失败");
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let outcome = finish_recording(handle, &recovery).expect("停止录制失败");
        assert_eq!(outcome.path, path_str);
        assert!(!outcome.recovered, "正常收尾不应进入恢复区");
        assert!(
            std::fs::metadata(path).expect("录制文件不存在").len() > 0,
            "录制文件为空"
        );
        ffprobe_video_metadata(path)
    }

    #[test]
    fn record_start_stop_produces_probeable_video() {
        if !ffmpeg_available() || !ffprobe_available() {
            eprintln!("跳过：未找到 ffmpeg / ffprobe");
            return;
        }
        let path = temp_path("record-smoke.mp4");
        let meta = record_and_probe(&path, None);
        let stream = &meta["streams"][0];
        assert_eq!(stream["codec_name"].as_str(), Some("h264"));
        assert!(
            stream["width"].as_u64().unwrap_or(0) > 0,
            "ffprobe 缺少视频宽度：{meta}"
        );
        assert!(
            meta["format"]["duration"].as_str().is_some(),
            "ffprobe 缺少时长，mp4 尾部可能不完整：{meta}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn record_region_video_keeps_requested_dimensions() {
        if !ffmpeg_available() || !ffprobe_available() {
            eprintln!("跳过：未找到 ffmpeg / ffprobe");
            return;
        }
        let monitors = Monitor::all().expect("枚举显示器失败");
        let monitor = monitors.first().expect("没有可用显示器");
        let region = ResolvedRegion {
            abs_x: monitor.x().unwrap_or(0),
            abs_y: monitor.y().unwrap_or(0),
            width: 320,
            height: 240,
        };
        let path = temp_path("record-region-smoke.mp4");
        let meta = record_and_probe(&path, Some(region));
        let stream = &meta["streams"][0];
        assert_eq!(stream["width"].as_u64(), Some(320));
        assert_eq!(stream["height"].as_u64(), Some(240));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn merge_segments_concatenates_into_single_mp4() {
        if !ffmpeg_available() || !ffprobe_available() {
            eprintln!("跳过：未找到 ffmpeg / ffprobe");
            return;
        }
        // 两段各录制约 1.5s，模拟「录一段 → 暂停 → 继续录一段」的分段产物。
        let seg0 = temp_path("merge-seg0.mp4");
        let seg1 = temp_path("merge-seg1.mp4");
        let final_path = temp_path("merge-final.mp4");
        let _ = record_and_probe(&seg0, None);
        let _ = record_and_probe(&seg1, None);
        merge_segments(
            &[seg0.clone(), seg1.clone()],
            &final_path,
            &std::path::PathBuf::from("ffmpeg"),
        )
        .expect("分段拼接失败");
        assert!(final_path.exists(), "拼接后应生成单个 MP4");
        assert!(
            !seg0.exists() && !seg1.exists(),
            "拼接后应清理临时分段"
        );
        let meta = ffprobe_video_metadata(&final_path);
        let duration: f64 = meta["format"]["duration"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        assert!(
            duration > 2.5 && duration < 4.0,
            "拼接后时长应≈两段之和≈3s（暂停期间不计入），实际 {duration}"
        );
        let _ = std::fs::remove_file(&final_path);
    }
}
