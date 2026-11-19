//! Orbit 共享 OCR 引擎。
//!
//! 桌面端（命令 `ocr.image` / `ocr.current_screen`）与服务端（`orbit-ocr-server`）
//! 共用同一套实现，避免两处各写一份、结果语义漂移：
//!
//! - `recognize_native`：`windows` crate 直连 WinRT `Windows.Media.Ocr`，无子进程；
//! - `recognize_with_powershell`：回退实现（原桌面端路径），PowerShell 内调同一 API；
//! - `recognize`：原生优先、失败回退，返回 `orbit_core::OcrResult`。

#[cfg(target_os = "windows")]
mod native;
mod powershell;

pub use powershell::recognize_with_powershell;

/// 原生后端（仅 Windows）。失败时调用方应回退到 PowerShell 实现。
#[cfg(target_os = "windows")]
pub fn recognize_native(image_path: &str) -> Result<orbit_core::OcrResult, String> {
    native::recognize(image_path)
}

#[cfg(not(target_os = "windows"))]
pub fn recognize_native(_image_path: &str) -> Result<orbit_core::OcrResult, String> {
    Err("当前平台没有原生 OCR 后端".to_string())
}

/// 默认识别入口：原生优先，失败回退 PowerShell（仅 Windows 可用）。
pub fn recognize(image_path: &str) -> Result<orbit_core::OcrResult, String> {
    match recognize_native(image_path) {
        Ok(result) => Ok(result),
        Err(error) => {
            log::warn!("原生 OCR 失败，回退 PowerShell 后端：{error}");
            recognize_with_powershell(image_path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 公开 API 的返回类型必须与命令契约一致。
    #[test]
    fn recognize_signature_matches_contract() {
        fn assert_signature(
            _f: fn(&str) -> Result<orbit_core::OcrResult, String>,
        ) {
        }
        assert_signature(recognize);
        assert_signature(recognize_native);
        assert_signature(recognize_with_powershell);
    }
}
