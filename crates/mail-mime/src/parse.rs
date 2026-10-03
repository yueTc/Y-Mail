//! MIME 解析：把不可信的原始邮件字节拆成正文与附件元数据。
//!
//! 安全约定：
//! - 本模块只做纯计算，不联网、不写库、不发信；
//! - HTML 一律先过白名单清洗，远程图片默认改写成待放行属性；
//! - 解析失败返回可读错误，不把原始内容塞进错误信息。

use std::collections::HashSet;

use mail_parser::{MessageParser, MimeHeaders, PartType};

use crate::sanitize::{sanitize_html, REMOTE_SRC_ATTRIBUTE};

/// 解析一封邮件时接受的原文上限（32 MiB，与 IMAP 层一致）。
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

/// 解析失败的原因。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    /// 原始字节为空。
    #[error("邮件原文为空，无法解析")]
    Empty,
    /// 原文超过上限。
    #[error("邮件原文超过 {limit} 字节上限，已拒绝解析")]
    TooLarge {
        /// 上限字节数。
        limit: usize,
    },
    /// 结构无法识别。
    #[error("邮件原文无法解析为 MIME 结构")]
    Unrecognized,
    /// 找不到指定分片（附件下载时用）。
    #[error("邮件里找不到编号 {0} 的内容分片")]
    PartNotFound(u32),
}

/// 一封解析出来的附件元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAttachment {
    /// 在 MIME 结构里的分片下标，下载原件时按它定位。
    pub part_index: u32,
    /// 文件名（可能为空，例如内嵌图片）。
    pub filename: String,
    /// MIME 类型（如 `application/pdf`）；取不到时为空串。
    pub mime_type: String,
    /// 解码后的字节数。
    pub size: u64,
    /// Content-ID（内嵌图片引用用），没有则为 None。
    pub content_id: Option<String>,
    /// 是否内嵌展示（如正文里的图片）。
    pub is_inline: bool,
}

/// 一封解析完成的邮件正文与附件清单。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedMessage {
    /// 纯文本正文；没有则为 None。
    pub text_plain: Option<String>,
    /// 清洗后的 HTML 正文；没有则为 None。
    pub html_sanitized: Option<String>,
    /// 被拦截的远程图片数量（默认拦截，前端可提示）。
    pub blocked_remote_images: usize,
    /// 附件元数据（按分片下标升序）。
    pub attachments: Vec<ParsedAttachment>,
}

impl ParsedMessage {
    /// 是否含任何附件分片。
    pub fn has_attachments(&self) -> bool {
        !self.attachments.is_empty()
    }
}

/// 解析原始邮件字节。正文与附件都只做读取，不产生任何副作用。
pub fn parse_message(raw: &[u8]) -> Result<ParsedMessage, ParseError> {
    if raw.is_empty() {
        return Err(ParseError::Empty);
    }
    if raw.len() > MAX_MESSAGE_BYTES {
        return Err(ParseError::TooLarge {
            limit: MAX_MESSAGE_BYTES,
        });
    }

    let message = MessageParser::default()
        .parse(raw)
        .ok_or(ParseError::Unrecognized)?;

    let text_plain = message.body_text(0).map(|text| text.into_owned());
    let (html_sanitized, blocked_remote_images) = match message.body_html(0) {
        Some(html) => {
            let (cleaned, blocked) = sanitize_html(html.as_ref());
            (Some(cleaned), blocked)
        }
        None => (None, 0),
    };

    let attachments = collect_attachments(&message);

    Ok(ParsedMessage {
        text_plain,
        html_sanitized,
        blocked_remote_images,
        attachments,
    })
}

/// 取某个 MIME 分片的解码字节（附件下载用）。
///
/// 只做纯计算；分片下标来自本地附件表，找不到就给出可读错误。
pub fn attachment_content(raw: &[u8], part_index: u32) -> Result<Vec<u8>, ParseError> {
    if raw.is_empty() {
        return Err(ParseError::Empty);
    }
    if raw.len() > MAX_MESSAGE_BYTES {
        return Err(ParseError::TooLarge {
            limit: MAX_MESSAGE_BYTES,
        });
    }
    let message = MessageParser::default()
        .parse(raw)
        .ok_or(ParseError::Unrecognized)?;
    let part = message
        .part(part_index)
        .ok_or(ParseError::PartNotFound(part_index))?;
    Ok(part.contents().to_vec())
}

/// 按分片下标收集附件元数据。
fn collect_attachments<'a>(message: &'a mail_parser::Message<'a>) -> Vec<ParsedAttachment> {
    let known: HashSet<u32> = message.attachments.iter().copied().collect();
    let mut out = Vec::new();
    for index in 0..message.parts.len() as u32 {
        if !known.contains(&index) {
            continue;
        }
        let Some(part) = message.part(index) else {
            continue;
        };
        let is_inline = matches!(part.body, PartType::InlineBinary(_))
            || part
                .content_disposition()
                .is_some_and(|disposition| disposition.is_inline());
        let mime_type = part
            .content_type()
            .map(|content_type| match content_type.subtype() {
                Some(subtype) => format!("{}/{}", content_type.ctype(), subtype),
                None => content_type.ctype().to_string(),
            })
            .unwrap_or_default();
        let content_id = part
            .content_id()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        out.push(ParsedAttachment {
            part_index: index,
            filename: part.attachment_name().unwrap_or_default().trim().to_string(),
            mime_type,
            size: part.contents().len() as u64,
            content_id,
            is_inline,
        });
    }
    out
}

/// 清洗后的 HTML 里是否还有被拦下的远程图片。
/// 交由前端在用户放行后用 `restore_remote_images` 还原。
pub fn contains_blocked_remote_images(html: &str) -> bool {
    html.contains(REMOTE_SRC_ATTRIBUTE)
}

#[cfg(test)]
mod tests {
    use super::{parse_message, ParseError};

    const PLAIN_MESSAGE: &str = concat!(
        "From: 张三 <zhang@example.com>\r\n",
        "To: 李四 <li@example.com>\r\n",
        "Subject: 你好\r\n",
        "Message-ID: <m1@example.com>\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "Content-Transfer-Encoding: 8bit\r\n",
        "\r\n",
        "这是正文。\r\n",
        "第二行。\r\n",
    );

    #[test]
    fn 纯文本正文能解析出来() {
        let parsed = parse_message(PLAIN_MESSAGE.as_bytes()).expect("应解析成功");
        assert_eq!(parsed.text_plain.as_deref(), Some("这是正文。\r\n第二行。\r\n"));
        let html = parsed.html_sanitized.clone().expect("纯文本应能转成安全 HTML");
        assert!(html.contains("这是正文"));
        assert!(!html.contains("<script"));
        assert_eq!(parsed.blocked_remote_images, 0);
        assert!(!parsed.has_attachments());
    }

    #[test]
    fn 空原文与超限原文会被拒绝() {
        assert_eq!(parse_message(b"").expect_err("空原文应失败"), ParseError::Empty);
        let huge = vec![b'x'; super::MAX_MESSAGE_BYTES + 1];
        assert!(matches!(
            parse_message(&huge).expect_err("超限应失败"),
            ParseError::TooLarge { .. }
        ));
    }

    #[test]
    fn html正文会清掉脚本并拦截远程图片() {
        let raw = concat!(
            "From: a@example.com\r\n",
            "Subject: html\r\n",
            "Content-Type: text/html; charset=utf-8\r\n",
            "\r\n",
            "<p onclick=\"alert(1)\">你好</p><script>alert(1)</script>",
            "<img src=\"https://tracker.example/pixel.gif\" alt=\"像素\">",
        );
        let parsed = parse_message(raw.as_bytes()).expect("应解析成功");
        let html = parsed.html_sanitized.expect("应有 HTML");
        assert!(!html.contains("<script"));
        assert!(!html.contains("onclick"));
        assert!(html.contains("你好"));
        assert!(!html.contains("<img src=\"https://tracker.example"));
        assert!(html.contains("data-em-original-src"));
        assert_eq!(parsed.blocked_remote_images, 1);
    }

    #[test]
    fn 附件元数据能取到文件名类型大小与内嵌标记() {
        let raw = concat!(
            "From: a@example.com\r\n",
            "Subject: 带附件\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=\"BOUND\"\r\n",
            "\r\n",
            "--BOUND\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n",
            "\r\n",
            "正文。\r\n",
            "--BOUND\r\n",
            "Content-Type: application/pdf; name=\"报告.pdf\"\r\n",
            "Content-Disposition: attachment; filename=\"报告.pdf\"\r\n",
            "Content-Transfer-Encoding: base64\r\n",
            "\r\n",
            "aGVsbG8=\r\n",
            "--BOUND\r\n",
            "Content-Type: image/png; name=\"图.png\"\r\n",
            "Content-Disposition: inline; filename=\"图.png\"\r\n",
            "Content-ID: <img-1@example.com>\r\n",
            "Content-Transfer-Encoding: base64\r\n",
            "\r\n",
            "aGVsbG8=\r\n",
            "--BOUND--\r\n",
        );
        let parsed = parse_message(raw.as_bytes()).expect("应解析成功");
        assert_eq!(parsed.attachments.len(), 2);

        let pdf = &parsed.attachments[0];
        assert_eq!(pdf.filename, "报告.pdf");
        assert_eq!(pdf.mime_type, "application/pdf");
        assert_eq!(pdf.size, 5);
        assert!(!pdf.is_inline);

        let image = &parsed.attachments[1];
        assert_eq!(image.filename, "图.png");
        assert_eq!(image.mime_type, "image/png");
        assert!(image.is_inline);
        assert_eq!(image.content_id.as_deref(), Some("img-1@example.com"));
        assert!(image.part_index > pdf.part_index);

        let bytes = super::attachment_content(raw.as_bytes(), pdf.part_index).expect("取附件字节");
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn 找不到分片时给出可读错误() {
        let err = super::attachment_content(PLAIN_MESSAGE.as_bytes(), 999).expect_err("应失败");
        assert!(err.to_string().contains("999"), "错误应含分片号：{err}");
    }
}
