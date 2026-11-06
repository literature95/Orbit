use std::borrow::Cow;

use orbit_core::{ScreenshotMode, ScreenshotRequest};
use serde_json::json;
use xcap::{Monitor, image::RgbaImage};

const MAX_CAPTURE_PIXELS: u64 = 64 * 1024 * 1024;

pub fn capture_to_clipboard(request: ScreenshotRequest) -> Result<serde_json::Value, String> {
    let monitors = Monitor::all().map_err(|error| error.to_string())?;
    if monitors.is_empty() {
        return Err("系统未检测到可截图的显示器。".to_string());
    }

    let (image, monitor_count) = match request.mode {
        ScreenshotMode::Fullscreen => capture_fullscreen(&monitors, request.monitor_id)?,
        ScreenshotMode::Region => (capture_region(&monitors, &request)?, 1),
    };
    let width = image.width();
    let height = image.height();
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    clipboard
        .set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: Cow::Owned(image.into_raw()),
        })
        .map_err(|error| error.to_string())?;

    Ok(json!({
        "width": width,
        "height": height,
        "monitorCount": monitor_count
    }))
}

fn capture_fullscreen(
    monitors: &[Monitor],
    monitor_id: Option<u32>,
) -> Result<(RgbaImage, usize), String> {
    let selected: Vec<&Monitor> = if let Some(id) = monitor_id {
        vec![find_monitor(monitors, id)?]
    } else {
        monitors.iter().collect()
    };
    let bounds = selected
        .iter()
        .map(|monitor| {
            Ok((
                monitor.x().map_err(|error| error.to_string())?,
                monitor.y().map_err(|error| error.to_string())?,
                monitor.width().map_err(|error| error.to_string())?,
                monitor.height().map_err(|error| error.to_string())?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let min_x = bounds
        .iter()
        .map(|(x, _, _, _)| *x)
        .min()
        .unwrap_or_default();
    let min_y = bounds
        .iter()
        .map(|(_, y, _, _)| *y)
        .min()
        .unwrap_or_default();
    let max_x = bounds
        .iter()
        .map(|(x, _, width, _)| i64::from(*x) + i64::from(*width))
        .max()
        .ok_or_else(|| "系统未检测到可截图的显示器。".to_string())?;
    let max_y = bounds
        .iter()
        .map(|(_, y, _, height)| i64::from(*y) + i64::from(*height))
        .max()
        .ok_or_else(|| "系统未检测到可截图的显示器。".to_string())?;
    let width = u32::try_from(max_x - min_x as i64)
        .map_err(|_| "显示器布局宽度超出支持范围。".to_string())?;
    let height = u32::try_from(max_y - min_y as i64)
        .map_err(|_| "显示器布局高度超出支持范围。".to_string())?;
    validate_capture_size(width, height)?;

    let mut combined = RgbaImage::new(width, height);
    let monitor_count = selected.len();
    for (monitor, (x, y, _, _)) in selected.iter().zip(bounds) {
        let image = monitor.capture_image().map_err(|error| error.to_string())?;
        let offset_x = i64::from(x) - i64::from(min_x);
        let offset_y = i64::from(y) - i64::from(min_y);
        if offset_x + i64::from(image.width()) > i64::from(width)
            || offset_y + i64::from(image.height()) > i64::from(height)
        {
            return Err("截图期间显示器布局发生变化，请重试。".to_string());
        }
        xcap::image::imageops::replace(&mut combined, &image, offset_x, offset_y);
    }
    Ok((combined, monitor_count))
}

fn capture_region(monitors: &[Monitor], request: &ScreenshotRequest) -> Result<RgbaImage, String> {
    let monitor = match request.monitor_id {
        Some(id) => find_monitor(monitors, id)?,
        None => primary_or_first(monitors)?,
    };
    let x = request
        .x
        .ok_or_else(|| "区域截图缺少 x 坐标。".to_string())?;
    let y = request
        .y
        .ok_or_else(|| "区域截图缺少 y 坐标。".to_string())?;
    let width = request
        .width
        .filter(|value| *value > 0)
        .ok_or_else(|| "区域截图宽度必须大于 0。".to_string())?;
    let height = request
        .height
        .filter(|value| *value > 0)
        .ok_or_else(|| "区域截图高度必须大于 0。".to_string())?;
    validate_capture_size(width, height)?;
    monitor
        .capture_region(x, y, width, height)
        .map_err(|error| error.to_string())
}

fn find_monitor(monitors: &[Monitor], id: u32) -> Result<&Monitor, String> {
    for monitor in monitors {
        if monitor.id().map_err(|error| error.to_string())? == id {
            return Ok(monitor);
        }
    }
    Err(format!("未找到显示器 {id}。"))
}

fn primary_or_first(monitors: &[Monitor]) -> Result<&Monitor, String> {
    for monitor in monitors {
        if monitor.is_primary().map_err(|error| error.to_string())? {
            return Ok(monitor);
        }
    }
    monitors
        .first()
        .ok_or_else(|| "系统未检测到可截图的显示器。".to_string())
}

fn validate_capture_size(width: u32, height: u32) -> Result<(), String> {
    let pixels = u64::from(width) * u64::from(height);
    if width == 0 || height == 0 || pixels > MAX_CAPTURE_PIXELS {
        return Err("截图尺寸无效或超出内存安全上限。".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_capture_size;

    #[test]
    fn rejects_empty_capture_dimensions() {
        assert!(validate_capture_size(0, 100).is_err());
        assert!(validate_capture_size(100, 0).is_err());
    }

    #[test]
    fn rejects_capture_sizes_above_the_memory_limit() {
        assert!(validate_capture_size(8193, 8192).is_err());
    }
}
