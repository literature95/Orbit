//! 原生 Windows.Media.Ocr 后端：通过 `windows` crate 直接调用 WinRT，
//! 不再为每次识别启动 PowerShell 进程（冷启动 200–800 ms 的根因）。
//!
//! 语义与 PowerShell 后端保持一致：`text` + `lines/words` + `bbox = [x1,y1,x2,y2]`
//! （WinRT `BoundingRect` 的 X/Y/Width/Height 换算），`textAngle` 可空。

use orbit_core::{OcrLine, OcrResult, OcrWord};
use windows::Graphics::Imaging::BitmapDecoder;
use windows::Media::Ocr::OcrEngine;
use windows::Storage::{FileAccessMode, StorageFile};
use windows::core::HSTRING;

/// 确保当前线程有 COM 套间。MTA 已初始化或已被其他套间占用都视为可用：
/// 后者返回 RPC_E_CHANGED_MODE，不影响 WinRT 调用。
pub(crate) fn ensure_apartment() -> Result<(), String> {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_ok() {
        return Ok(());
    }
    // RPC_E_CHANGED_MODE：线程已有别的套间，继续用即可。
    const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;
    if hr.0 == RPC_E_CHANGED_MODE {
        return Ok(());
    }
    Err(format!("COM 初始化失败：{hr:?}"))
}

pub(crate) fn recognize(image_path: &str) -> Result<OcrResult, String> {
    ensure_apartment()?;

    let path = HSTRING::from(image_path);
    let file = StorageFile::GetFileFromPathAsync(&path)
        .map_err(|error| format!("打开图片失败：{error}"))?
        .join()
        .map_err(|error| format!("打开图片失败：{error}"))?;
    let stream = file
        .OpenAsync(FileAccessMode::Read)
        .map_err(|error| format!("读取图片失败：{error}"))?
        .join()
        .map_err(|error| format!("读取图片失败：{error}"))?;
    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(|error| format!("解码图片失败：{error}"))?
        .join()
        .map_err(|error| format!("解码图片失败：{error}"))?;
    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(|error| format!("转换位图失败：{error}"))?
        .join()
        .map_err(|error| format!("转换位图失败：{error}"))?;

    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|error| {
        format!("OCR 引擎不可用（可能需要安装系统语言包/OCR 组件）：{error}")
    })?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|error| format!("OCR 识别失败：{error}"))?
        .join()
        .map_err(|error| format!("OCR 识别失败：{error}"))?;

    convert(result)
}

/// WinRT 结果 → 契约结构。行 bbox 取行内 word 包围盒的并集，
/// 与 PowerShell 后端完全一致（空行给全零框）。
fn convert(result: windows::Media::Ocr::OcrResult) -> Result<OcrResult, String> {
    let text = result
        .Text()
        .map_err(|error| format!("读取 OCR 文本失败：{error}"))?
        .to_string();
    let text_angle = result
        .TextAngle()
        .ok()
        .and_then(|reference| reference.Value().ok());

    let lines_view = result
        .Lines()
        .map_err(|error| format!("读取 OCR 行失败：{error}"))?;
    let count = lines_view
        .Size()
        .map_err(|error| format!("读取 OCR 行数失败：{error}"))?;
    let mut lines = Vec::with_capacity(count as usize);
    for index in 0..count {
        let line = lines_view
            .GetAt(index)
            .map_err(|error| format!("读取第 {index} 行失败：{error}"))?;
        let line_text = line
            .Text()
            .map_err(|error| format!("读取行文本失败：{error}"))?
            .to_string();
        let words_view = line
            .Words()
            .map_err(|error| format!("读取行内词失败：{error}"))?;
        let word_count = words_view
            .Size()
            .map_err(|error| format!("读取词数失败：{error}"))?;

        let mut words = Vec::with_capacity(word_count as usize);
        let (mut x1, mut y1) = (f64::INFINITY, f64::INFINITY);
        let (mut x2, mut y2) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for word_index in 0..word_count {
            let word = words_view
                .GetAt(word_index)
                .map_err(|error| format!("读取第 {word_index} 个词失败：{error}"))?;
            let rect = word
                .BoundingRect()
                .map_err(|error| format!("读取词包围盒失败：{error}"))?;
            let (left, top) = (f64::from(rect.X), f64::from(rect.Y));
            let (right, bottom) = (
                left + f64::from(rect.Width),
                top + f64::from(rect.Height),
            );
            x1 = x1.min(left);
            y1 = y1.min(top);
            x2 = x2.max(right);
            y2 = y2.max(bottom);
            words.push(OcrWord {
                text: word
                    .Text()
                    .map_err(|error| format!("读取词文本失败：{error}"))?
                    .to_string(),
                bbox: [left, top, right, bottom],
            });
        }
        if words.is_empty() {
            x1 = 0.0;
            y1 = 0.0;
            x2 = 0.0;
            y2 = 0.0;
        }
        lines.push(OcrLine { text: line_text, bbox: [x1, y1, x2, y2], words });
    }

    Ok(OcrResult { text, lines, text_angle })
}
