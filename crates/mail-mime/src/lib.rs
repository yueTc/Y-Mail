//! mail-mime：邮件解析层。
//!
//! 职责：把不可信的原始邮件字节解析为结构化内容（正文、附件元数据），并为 HTML 提供白名单清洗。
//! 邮件正文一律视为不可信输入，解析层不得据此触发任何动作。

mod inline;
mod parse;
mod sanitize;

pub use inline::{
    inline_image_data_url, is_renderable_inline_image_mime, normalize_content_id, sniff_image_mime,
    InlineImageError, MAX_CONTENT_ID_LEN, MAX_INLINE_IMAGE_BYTES,
};
pub use parse::{
    attachment_content, contains_blocked_remote_images, parse_message, ParseError, ParsedAttachment,
    ParsedMessage, MAX_MESSAGE_BYTES,
};
pub use sanitize::{is_remote_image_url, restore_remote_images, sanitize_html, REMOTE_SRC_ATTRIBUTE};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "MIME 解析与 HTML 清洗";

#[cfg(test)]
mod tests {
    use super::CRATE_PURPOSE;

    #[test]
    fn 用途标识不为空() {
        assert!(!CRATE_PURPOSE.is_empty());
    }
}
