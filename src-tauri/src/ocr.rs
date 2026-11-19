//! OCR 引擎抽象层。
//!
//! 命令层只依赖 `OcrEngine`，具体后端由 `default_engine()` 提供：
//!
//! 1. `NativeOcr`（默认）：`windows` crate 直连 WinRT `Windows.Media.Ocr`，
//!    无子进程、无 PowerShell 冷启动（实现见 `ocr_native.rs`）；
//! 2. `PowerShellOcr`（回退）：原 PowerShell 调用（`ocr_backend.rs`），
//!    在原生路径失败时兜底，保证命令契约与可用性不退化。
//!
//! 后续接入 PaddleOCR HTTP sidecar 时，只需新增一个 `OcrEngine` 实现并调整
//! 工厂返回顺序，命令契约与调用点无需改动。

use orbit_core::OcrResult;

/// 所有 OCR 后端的最小契约：给定本地图片路径，返回结构化识别结果。
/// 调用是同步阻塞的，调用方负责放入阻塞线程池，避免占用 UI 事件循环。
pub(crate) trait OcrEngine: Send + Sync {
    fn recognize(&self, image_path: &str) -> Result<OcrResult, String>;
}

/// 原生后端：WinRT 直连，失败时回退到 PowerShell 实现。
pub(crate) struct NativeOcr;

impl OcrEngine for NativeOcr {
    fn recognize(&self, image_path: &str) -> Result<OcrResult, String> {
        orbit_ocr::recognize(image_path)
    }
}

/// 纯 PowerShell 后端（保留给原生路径不可用的环境与对照测试）。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct PowerShellOcr;

impl OcrEngine for PowerShellOcr {
    fn recognize(&self, image_path: &str) -> Result<OcrResult, String> {
        orbit_ocr::recognize_with_powershell(image_path)
    }
}

static NATIVE_OCR: NativeOcr = NativeOcr;

/// 返回当前启用的引擎。`'static` 引用使命令总线的闭包无需额外持有所有权。
pub(crate) fn default_engine() -> &'static dyn OcrEngine {
    &NATIVE_OCR
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

    /// 两个真实后端都实现了同一契约（编译期检查）。
    #[test]
    fn backend_implementations_satisfy_the_trait() {
        fn assert_engine<T: OcrEngine>() {}
        assert_engine::<NativeOcr>();
        assert_engine::<PowerShellOcr>();
    }
}
