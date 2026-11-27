//! 格式注册表：格式 id、分类、扩展名与常用度。
//!
//! 借鉴 Filee 的 FormatRegistry：格式是路由图的节点，引擎的边引用这里的 id。
//! 格式识别按扩展名归一化；未知扩展名不参与规划，避免猜错格式。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormatCategory {
    Image,
    Video,
    Audio,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FormatInfo {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) category: FormatCategory,
    pub(crate) extensions: &'static [&'static str],
    /// 是否出现在圆盘默认格式列表（其余格式仍可通过 convert.plan 规划）。
    pub(crate) common: bool,
}

use FormatCategory::{Audio, Image, Video};

pub(crate) const FORMATS: &[FormatInfo] = &[
    // 图片
    FormatInfo { id: "png",  label: "PNG 图片",  category: Image, extensions: &["png"], common: true },
    FormatInfo { id: "jpg",  label: "JPG 图片",  category: Image, extensions: &["jpg", "jpeg"], common: true },
    FormatInfo { id: "webp", label: "WebP 图片", category: Image, extensions: &["webp"], common: true },
    FormatInfo { id: "gif",  label: "GIF 动图",  category: Image, extensions: &["gif"], common: true },
    FormatInfo { id: "bmp",  label: "BMP 图片",  category: Image, extensions: &["bmp"], common: true },
    FormatInfo { id: "tiff", label: "TIFF 图片", category: Image, extensions: &["tiff", "tif"], common: false },
    FormatInfo { id: "ico",  label: "ICO 图标",  category: Image, extensions: &["ico"], common: false },
    FormatInfo { id: "tga",  label: "TGA 图片",  category: Image, extensions: &["tga"], common: false },
    // 视频
    FormatInfo { id: "mp4",  label: "MP4 视频",  category: Video, extensions: &["mp4", "m4v"], common: true },
    FormatInfo { id: "mov",  label: "MOV 视频",  category: Video, extensions: &["mov"], common: false },
    FormatInfo { id: "mkv",  label: "MKV 视频",  category: Video, extensions: &["mkv"], common: false },
    FormatInfo { id: "avi",  label: "AVI 视频",  category: Video, extensions: &["avi"], common: false },
    FormatInfo { id: "webm", label: "WebM 视频", category: Video, extensions: &["webm"], common: true },
    // 音频
    FormatInfo { id: "mp3",  label: "MP3 音频",  category: Audio, extensions: &["mp3"], common: true },
    FormatInfo { id: "wav",  label: "WAV 音频",  category: Audio, extensions: &["wav"], common: true },
    FormatInfo { id: "flac", label: "FLAC 音频", category: Audio, extensions: &["flac"], common: false },
    FormatInfo { id: "aac",  label: "AAC 音频",  category: Audio, extensions: &["aac"], common: false },
    FormatInfo { id: "ogg",  label: "OGG 音频",  category: Audio, extensions: &["ogg"], common: false },
    FormatInfo { id: "m4a",  label: "M4A 音频",  category: Audio, extensions: &["m4a"], common: false },
];

/// 按扩展名或 id 归一化到注册格式。
pub(crate) fn normalize(input: &str) -> Option<&'static str> {
    let lower = input.trim_start_matches('.').to_ascii_lowercase();
    FORMATS
        .iter()
        .find(|format| format.id == lower || format.extensions.iter().any(|ext| *ext == lower))
        .map(|format| format.id)
}

/// 圆盘默认展示的目标格式。
pub(crate) fn common_targets() -> Vec<String> {
    FORMATS.iter().filter(|f| f.common).map(|f| f.id.to_string()).collect()
}

/// 前端标签（tiff/ico 这类非默认格式从 convert.formats 返回时也需要名字）。
pub(crate) fn label(id: &str) -> Option<&'static str> {
    FORMATS.iter().find(|f| f.id == id).map(|f| f.label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_extensions_and_aliases() {
        assert_eq!(normalize("JPEG"), Some("jpg"));
        assert_eq!(normalize(".tif"), Some("tiff"));
        assert_eq!(normalize("mp4"), Some("mp4"));
        assert_eq!(normalize("unknown"), None);
    }

    #[test]
    fn common_targets_are_a_manageable_subset() {
        let targets = common_targets();
        assert!(targets.len() >= 8 && targets.len() <= 12);
        assert!(targets.contains(&"png".to_string()));
        assert!(targets.contains(&"mp3".to_string()));
    }
}
