//! PowerShell 回退后端：调用 `powershell.exe` 内的 Windows.Media.Ocr。
//!
//! 保留原因：原生 WinRT 路径在极少数机器上可能因 COM/语言包问题失败，
//! 该路径能在 PowerShell 环境可用的机器上兜底，输出契约与原生后端一致。

use orbit_core::OcrResult;
use std::process::{Command, Stdio};

/// 解析 PowerShell 输出：UTF-8、BOM、空输出与非 JSON 都要给出明确错误。
pub(crate) fn parse_ocr_output(stdout: &[u8]) -> Result<OcrResult, String> {
    let raw = String::from_utf8(stdout.to_vec())
        .map_err(|error| format!("OCR 输出不是 UTF-8：{error}"))?;
    let payload = raw.trim_start_matches('\u{feff}').trim();
    if payload.is_empty() {
        return Err("OCR 未返回任何内容。".to_string());
    }
    serde_json::from_str(payload).map_err(|error| format!("OCR 结果解析失败：{error}"))
}

const POWERSHELL_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Storage.StorageFile, Windows.Storage, ContentType=WindowsRuntime]
$null = [Windows.Storage.Streams.IRandomAccessStream, Windows.Storage.Streams, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.SoftwareBitmap, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Media.Ocr.OcrEngine, Windows.Media.Ocr, ContentType=WindowsRuntime]
$null = [Windows.Media.Ocr.OcrResult, Windows.Media.Ocr, ContentType=WindowsRuntime]

$asTaskOpen = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
  $_.Name -eq 'AsTask' -and $_.IsGenericMethod -and $_.GetGenericArguments().Count -eq 1 -and
  $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -like 'IAsyncOperation*'
}
function Await($op, $resultType) {
  $task = $asTaskOpen.MakeGenericMethod($resultType).Invoke($null, @($op))
  $task.Wait() | Out-Null
  return $task.Result
}
function ToBBox($rect) {
  return @(
    [double]$rect.X,
    [double]$rect.Y,
    [double]($rect.X + $rect.Width),
    [double]($rect.Y + $rect.Height)
  )
}

$path = $env:ORBIT_OCR_PATH
$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($path)) ([Windows.Storage.StorageFile])
$ras = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($ras)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
$result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])

$lines = @()
foreach ($line in $result.Lines) {
  $words = @()
  $x1 = [double]::PositiveInfinity
  $y1 = [double]::PositiveInfinity
  $x2 = [double]::NegativeInfinity
  $y2 = [double]::NegativeInfinity
  foreach ($word in $line.Words) {
    $box = ToBBox $word.BoundingRect
    $words += [ordered]@{ text = [string]$word.Text; bbox = @($box) }
    if ($box[0] -lt $x1) { $x1 = $box[0] }
    if ($box[1] -lt $y1) { $y1 = $box[1] }
    if ($box[2] -gt $x2) { $x2 = $box[2] }
    if ($box[3] -gt $y2) { $y2 = $box[3] }
  }
  if ($words.Count -eq 0) { $x1 = 0; $y1 = 0; $x2 = 0; $y2 = 0 }
  $lines += [ordered]@{
    text = [string]$line.Text
    bbox = @($x1, $y1, $x2, $y2)
    words = @($words)
  }
}

$angle = $null
try { if ($null -ne $result.TextAngle) { $angle = [double]$result.TextAngle } } catch { $angle = $null }
$payload = [ordered]@{
  text = [string]$result.Text
  lines = @($lines)
  textAngle = $angle
}
ConvertTo-Json -InputObject $payload -Compress -Depth 8
"#;

/// 用 PowerShell 调用 Windows.Media.Ocr 识别一张本地图片。
pub fn recognize_with_powershell(image_path: &str) -> Result<OcrResult, String> {
    // 路径经环境变量传入，不做字符串内插，避免引号破坏脚本或注入。
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", POWERSHELL_SCRIPT])
        .env("ORBIT_OCR_PATH", image_path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("PowerShell 启动失败：{error}"))?;

    if output.status.success() {
        parse_ocr_output(&output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("PowerShell OCR 失败：{stderr}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_utf8_bom_and_compact_json() {
        let raw = "\u{feff}{\"text\":\"hello\",\"lines\":[],\"textAngle\":null}";
        let result = parse_ocr_output(raw.as_bytes()).expect("应解析成功");
        assert_eq!(result.text, "hello");
        assert!(result.lines.is_empty());
        assert!(result.text_angle.is_none());
    }

    #[test]
    fn parse_rejects_empty_output() {
        assert!(parse_ocr_output(b"   \n").is_err());
        assert!(parse_ocr_output("\u{feff}".as_bytes()).is_err());
    }

    #[test]
    fn parse_rejects_non_json() {
        let error = parse_ocr_output(b"not json").unwrap_err();
        assert!(error.contains("解析失败"), "错误信息应说明解析失败：{error}");
    }
}
