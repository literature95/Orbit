//! OCR 引擎抽象层。
//!
//! 命令层只依赖 `OcrEngine`，具体后端由 `default_engine()` 提供。当前唯一后端
//! 是 Windows.Media.Ocr（PowerShell 调用实现位于 `lib.rs` 的
//! `ocr_image_blocking`）。后续接入 PaddleOCR HTTP sidecar 时，只需新增一个
//! 实现并替换工厂返回值，命令契约与调用点无需改动。

use orbit_core::OcrResult;

/// 所有 OCR 后端的最小契约：给定本地图片路径，返回结构化识别结果。
/// 调用是同步阻塞的，调用方负责放入阻塞线程池，避免占用 UI 事件循环。
pub(crate) trait OcrEngine: Send + Sync {
    fn recognize(&self, image_path: &str) -> Result<OcrResult, String>;
}

/// Windows 内置后端：Windows.Media.Ocr。
pub(crate) struct WindowsMediaOcr;

impl OcrEngine for WindowsMediaOcr {
    fn recognize(&self, image_path: &str) -> Result<OcrResult, String> {
        crate::ocr_image_blocking(image_path)
    }
}

static WINDOWS_MEDIA_OCR: WindowsMediaOcr = WindowsMediaOcr;

/// 返回当前启用的引擎。`'static` 引用使命令总线的闭包无需额外持有所有权。
pub(crate) fn default_engine() -> &'static dyn OcrEngine {
    &WINDOWS_MEDIA_OCR
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubEngine;

    impl OcrEngine for StubEngine {
        fn recognize(&self, _image_path: &str) -> Result<OcrResult, String> {
            Ok(OcrResult {
                text: "stub".to_string(),
                lines: vec![],
                text_angle: None,
            })
        }
    }

    /// 通过 `&dyn OcrEngine` 调用，证明替换后端不需要改动命令层代码。
    #[test]
    fn engine_can_be_swapped_without_changing_callers() {
        fn recognize_via(engine: &dyn OcrEngine) -> OcrResult {
            engine.recognize("any.png").expect("stub 不应失败")
        }
        assert_eq!(recognize_via(&StubEngine).text, "stub");
    }

    /// trait object 必须在跨线程的命令总线中可用。
    #[test]
    fn engine_object_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn OcrEngine>();
        let _engine: &'static dyn OcrEngine = default_engine();
    }
}
