//! 内嵌图片（cid:）安全处理。
//!
//! 这里只做纯计算：Content-ID 规范化与安全校验、可内联的 MIME 白名单、
//! 图片字节嗅探与受控 data URL 构造。不联网、不落盘、不执行任何动作。
//!
//! 安全口径：
//! - Content-ID 一律按白名单字符校验，拒绝路径分隔符、`..`、控制字符与非 ASCII；
//! - 只允许常见光栅图（SVG 不内联），且声明的类型必须与文件头一致；
//! - 单张图片有字节上限，超过就拒绝内联。

use base64::Engine as _;

/// 单张内嵌图片允许内联渲染的最大字节数（2 MiB）。
pub const MAX_INLINE_IMAGE_BYTES: usize = 2 * 1024 * 1024;

/// Content-ID 允许的最大长度（字符数）。
pub const MAX_CONTENT_ID_LEN: usize = 255;

/// 内联图片被拒绝的原因。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InlineImageError {
    /// 图片字节为空。
    #[error("内嵌图片内容为空")]
    Empty,
    /// 超过单张上限。
    #[error("内嵌图片超过 {limit} 字节上限（实际 {actual} 字节）")]
    TooLarge {
        /// 上限字节数。
        limit: usize,
        /// 实际字节数。
        actual: usize,
    },
    /// 类型不在允许内联的白名单里（例如 SVG 或非图片）。
    #[error("内嵌图片类型 {0} 不允许内联显示")]
    UnsupportedType(String),
    /// 文件头不是已知的图片格式。
    #[error("内嵌图片的字节不是已知图片格式")]
    NotAnImage,
    /// 声明的类型与文件头不一致。
    #[error("内嵌图片声明的类型 {declared} 与文件头 {sniffed} 不一致")]
    Mismatch {
        /// 声明类型。
        declared: String,
        /// 嗅探出的类型。
        sniffed: String,
    },
}

/// 判断字符是否属于 Content-ID 的安全字符集。
///
/// 允许字母数字与 `@ . _ - + = ~`；其余（含 `/`、`\`、`:`、`%`、引号、
/// 空白、控制字符、非 ASCII）一律拒绝。
fn is_safe_content_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '@' | '.' | '_' | '-' | '+' | '=' | '~')
}

/// 规范化并校验 Content-ID：去尖括号、去空白、限制字符集与长度。
///
/// 不合法（可能被用来做路径穿越或注入）时返回 None。合法时统一转小写，
/// 方便与正文里的 `cid:` 引用做大小写不敏感匹配。
pub fn normalize_content_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let stripped = trimmed.strip_prefix('<').unwrap_or(trimmed);
    let stripped = stripped.strip_suffix('>').unwrap_or(stripped);
    let collapsed: String = stripped.chars().filter(|ch| !ch.is_whitespace()).collect();

    if collapsed.is_empty() || collapsed.chars().count() > MAX_CONTENT_ID_LEN {
        return None;
    }
    if collapsed.contains("..") || collapsed.starts_with('.') || collapsed.ends_with('.') {
        return None;
    }
    if collapsed.contains('/') || collapsed.contains('\\') {
        return None;
    }
    if !collapsed.chars().all(is_safe_content_id_char) {
        return None;
    }
    Some(collapsed.to_ascii_lowercase())
}

/// 该 MIME 类型是否允许内联渲染。
///
/// 只放行常见光栅图；`image/svg+xml` 与一切非 `image/*` 都返回 false。
pub fn is_renderable_inline_image_mime(mime_type: &str) -> bool {
    matches!(
        mime_type.trim().to_ascii_lowercase().as_str(),
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp" | "image/bmp" | "image/avif"
    )
}

/// 按文件头嗅探图片类型；不是已知光栅图时返回 None。
pub fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"BM") {
        return Some("image/bmp");
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"avif" || brand == b"avis" {
            return Some("image/avif");
        }
    }
    None
}

/// 声明类型与嗅探类型是否一致（`image/jpg` 视为 `image/jpeg`）。
fn mime_matches(declared: &str, sniffed: &str) -> bool {
    let declared = declared.trim().to_ascii_lowercase();
    let declared = if declared == "image/jpg" {
        "image/jpeg".to_string()
    } else {
        declared
    };
    declared == sniffed
}

/// 把本地已缓存的图片字节构造为受控的 data URL。
///
/// 校验顺序：非空 → 不超上限 → 类型白名单 → 文件头可识别 → 声明与文件头一致。
/// 任何一步不过都返回错误，绝不产生可执行内容。
pub fn inline_image_data_url(declared_mime: &str, bytes: &[u8]) -> Result<String, InlineImageError> {
    if bytes.is_empty() {
        return Err(InlineImageError::Empty);
    }
    if bytes.len() > MAX_INLINE_IMAGE_BYTES {
        return Err(InlineImageError::TooLarge {
            limit: MAX_INLINE_IMAGE_BYTES,
            actual: bytes.len(),
        });
    }
    if !is_renderable_inline_image_mime(declared_mime) {
        return Err(InlineImageError::UnsupportedType(
            declared_mime.trim().to_string(),
        ));
    }
    let sniffed = sniff_image_mime(bytes).ok_or(InlineImageError::NotAnImage)?;
    if !mime_matches(declared_mime, sniffed) {
        return Err(InlineImageError::Mismatch {
            declared: declared_mime.trim().to_string(),
            sniffed: sniffed.to_string(),
        });
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{sniffed};base64,{encoded}"))
}

#[cfg(test)]
mod tests {
    use super::{
        inline_image_data_url, is_renderable_inline_image_mime, normalize_content_id, sniff_image_mime,
        InlineImageError, MAX_INLINE_IMAGE_BYTES,
    };

    const PNG_HEADER: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    fn png_bytes(len: usize) -> Vec<u8> {
        let mut bytes = PNG_HEADER.to_vec();
        bytes.resize(len.max(PNG_HEADER.len()), 0);
        bytes
    }

    #[test]
    fn 规范化会去掉尖括号与空白() {
        assert_eq!(
            normalize_content_id("  <Image-1@Example.COM>  ").as_deref(),
            Some("image-1@example.com")
        );
        assert_eq!(
            normalize_content_id("<img-1@example.com>").as_deref(),
            Some("img-1@example.com")
        );
        assert_eq!(
            normalize_content_id("ii_1234+ab@mail.example").as_deref(),
            Some("ii_1234+ab@mail.example")
        );
    }

    #[test]
    fn 规范化会拒绝路径穿越与控制字符() {
        assert!(normalize_content_id("../../etc/passwd").is_none());
        assert!(normalize_content_id("a/../b@x").is_none());
        assert!(normalize_content_id("a\\..\\b@x").is_none());
        assert!(normalize_content_id("a/ b@x").is_none());
        assert_eq!(normalize_content_id("a\tb@x").as_deref(), Some("ab@x"));
        assert!(normalize_content_id("bad\"id<x>").is_none());
        assert!(normalize_content_id("id%2e%2e@x").is_none());
        assert!(normalize_content_id("带中文@x").is_none());
        assert!(normalize_content_id("").is_none());
        assert!(normalize_content_id("<>").is_none());
        assert!(normalize_content_id(".hidden@x").is_none());
        assert!(normalize_content_id("a..b@x").is_none());
    }

    #[test]
    fn 规范化会限制长度() {
        let long = format!("{}@x", "a".repeat(super::MAX_CONTENT_ID_LEN));
        assert!(normalize_content_id(&long).is_none());
    }

    #[test]
    fn 类型白名单排除svg与非图片() {
        assert!(is_renderable_inline_image_mime("image/png"));
        assert!(is_renderable_inline_image_mime("IMAGE/JPEG"));
        assert!(!is_renderable_inline_image_mime("image/svg+xml"));
        assert!(!is_renderable_inline_image_mime("application/pdf"));
        assert!(!is_renderable_inline_image_mime("text/html"));
        assert!(!is_renderable_inline_image_mime(""));
    }

    #[test]
    fn 文件头嗅探认得常见光栅图() {
        assert_eq!(sniff_image_mime(&png_bytes(16)), Some("image/png"));
        assert_eq!(sniff_image_mime(&[0xFF, 0xD8, 0xFF, 0x00]), Some("image/jpeg"));
        assert_eq!(sniff_image_mime(b"GIF89a...."), Some("image/gif"));
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(sniff_image_mime(&webp), Some("image/webp"));
        assert_eq!(sniff_image_mime(b"BMxxxxxx"), Some("image/bmp"));
        let mut avif = b"\x00\x00\x00\x20".to_vec();
        avif.extend_from_slice(b"ftypavif");
        assert_eq!(sniff_image_mime(&avif), Some("image/avif"));
        assert_eq!(sniff_image_mime(b"<html><body>hi"), None);
    }

    #[test]
    fn data_url只接受白名单内且文件头一致的图片() {
        let url = inline_image_data_url("image/png", &png_bytes(32)).expect("png 应通过");
        assert!(url.starts_with("data:image/png;base64,"));

        assert!(matches!(
            inline_image_data_url("application/pdf", &png_bytes(32)),
            Err(InlineImageError::UnsupportedType(_))
        ));
        assert!(matches!(
            inline_image_data_url("image/svg+xml", b"<svg onload=alert(1)></svg>"),
            Err(InlineImageError::UnsupportedType(_))
        ));
        assert!(matches!(
            inline_image_data_url("image/png", b"<html>not an image</html>"),
            Err(InlineImageError::NotAnImage)
        ));
        assert!(matches!(
            inline_image_data_url("image/png", &[0xFF, 0xD8, 0xFF, 0x00]),
            Err(InlineImageError::Mismatch { .. })
        ));
        assert!(matches!(
            inline_image_data_url("image/png", &[]),
            Err(InlineImageError::Empty)
        ));
    }

    #[test]
    fn 超大图会被拒绝() {
        let huge = png_bytes(MAX_INLINE_IMAGE_BYTES + 1);
        assert!(matches!(
            inline_image_data_url("image/png", &huge),
            Err(InlineImageError::TooLarge { .. })
        ));
    }
}
