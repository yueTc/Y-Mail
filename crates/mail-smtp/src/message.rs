//! 外发邮件的 MIME 组装（Wave 5）。
//!
//! 结构：有附件时 `multipart/mixed` 里先放 `multipart/alternative`（纯文本 + HTML），
//! 再逐个放附件；没有附件时直接 `multipart/alternative`。
//! 非 ASCII 的主题、显示名与附件名按 RFC 2047 / RFC 2231 编码；
//! 所有内容用 base64 传输（不受 8BITMIME 支持与否影响）。
//!
//! 这里只做纯计算：不联网、不读文件；附件字节由调用方读好后传进来。

use std::sync::atomic::{AtomicU64, Ordering};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use mail_domain::dates::civil_from_days;

/// 单封邮件上限（含附件），与 IMAP APPEND 的上限保持一致。
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];

static MESSAGE_SEQ: AtomicU64 = AtomicU64::new(0);

/// 一个收件人 / 发件人。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    /// 显示名，可为空。
    pub name: String,
    /// 邮箱地址。
    pub address: String,
}

impl Mailbox {
    /// 用地址构造一个没有显示名的收件人。
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            name: String::new(),
            address: address.into(),
        }
    }
}

/// 一个准备随信发出的附件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingAttachment {
    /// 展示给收件人的文件名。
    pub filename: String,
    /// 内容类型，例如 `application/pdf`。
    pub mime_type: String,
    /// 文件字节。
    pub bytes: Vec<u8>,
}

/// 一封待组装的外发邮件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingMessage {
    /// 发件人显示名。
    pub from_name: String,
    /// 发件人邮箱。
    pub from_address: String,
    /// 收件人。
    pub to: Vec<Mailbox>,
    /// 抄送。
    pub cc: Vec<Mailbox>,
    /// 密送（只进信封，不写进信头）。
    pub bcc: Vec<Mailbox>,
    /// 主题。
    pub subject: String,
    /// 纯文本正文。
    pub body_text: String,
    /// HTML 正文。
    pub body_html: String,
    /// 回复的原始 Message-ID。
    pub in_reply_to: Option<String>,
    /// References 头的 Message-ID 列表。
    pub references: Vec<String>,
    /// 附件。
    pub attachments: Vec<OutgoingAttachment>,
    /// 生成时间（Unix 秒）；由调用方传入，方便测试。
    pub date_unix: i64,
}

/// 组装结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltMessage {
    /// 本次生成的 Message-ID（不含尖括号）。
    pub message_id: String,
    /// 完整 MIME 字节（CRLF 行尾）。
    pub raw: Vec<u8>,
    /// 规整后的信封发件人地址。
    pub from: String,
    /// 信封收件人（to + cc + bcc，已去重并去掉尖括号）。
    pub recipients: Vec<String>,
}

/// 组装失败。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MessageError {
    /// 字段不合法（地址为空、格式不对等）。
    #[error("{0}")]
    Invalid(String),
    /// 邮件（含附件）超过上限。
    #[error("邮件太大（超过 {limit} 字节），请减少附件后重试")]
    TooLarge {
        /// 上限字节数。
        limit: usize,
    },
}

/// 把一封结构化邮件组装成 MIME 字节。
pub fn build_message(message: &OutgoingMessage) -> Result<BuiltMessage, MessageError> {
    let from_address = normalize_address(&message.from_address)
        .ok_or_else(|| MessageError::Invalid("发件人地址为空或不合法".to_string()))?;
    let recipients = collect_recipients(message)?;
    if recipients.is_empty() {
        return Err(MessageError::Invalid("至少需要一位收件人".to_string()));
    }

    let message_id = make_message_id(&from_address);
    let mut out = String::with_capacity(2048);
    push_headers(&mut out, message, &from_address, &message_id);

    let alt_boundary = make_boundary("alt");
    let text_part = text_part(&alt_boundary, message);
    if message.attachments.is_empty() {
        out.push_str(&format!(
            "Content-Type: multipart/alternative; boundary=\"{alt_boundary}\"\r\n\r\n"
        ));
        out.push_str(&text_part);
    } else {
        let mixed_boundary = make_boundary("mix");
        out.push_str(&format!(
            "Content-Type: multipart/mixed; boundary=\"{mixed_boundary}\"\r\n\r\n"
        ));
        out.push_str(&format!("--{mixed_boundary}\r\n"));
        out.push_str(&format!(
            "Content-Type: multipart/alternative; boundary=\"{alt_boundary}\"\r\n\r\n"
        ));
        out.push_str(&text_part);
        for attachment in &message.attachments {
            out.push_str(&format!("--{mixed_boundary}\r\n"));
            out.push_str(&attachment_part(attachment)?);
        }
        out.push_str(&format!("--{mixed_boundary}--\r\n"));
    }

    let raw = out.into_bytes();
    if raw.len() > MAX_MESSAGE_BYTES {
        return Err(MessageError::TooLarge {
            limit: MAX_MESSAGE_BYTES,
        });
    }

    Ok(BuiltMessage {
        message_id,
        raw,
        from: from_address,
        recipients,
    })
}

/// 信头：From / To / Cc / Subject / Date / Message-ID / 回复线索 / MIME 版本。
fn push_headers(out: &mut String, message: &OutgoingMessage, from_address: &str, message_id: &str) {
    let from = Mailbox {
        name: message.from_name.clone(),
        address: from_address.to_string(),
    };
    out.push_str(&format!("Date: {}\r\n", rfc5322_date(message.date_unix)));
    out.push_str(&format!("From: {}\r\n", format_mailbox(&from)));
    out.push_str(&format!("To: {}\r\n", format_mailbox_list(&message.to)));
    if !message.cc.is_empty() {
        out.push_str(&format!("Cc: {}\r\n", format_mailbox_list(&message.cc)));
    }
    out.push_str(&format!("Subject: {}\r\n", encode_unstructured(&message.subject)));
    out.push_str(&format!("Message-ID: <{message_id}>\r\n"));
    if let Some(value) = message.in_reply_to.as_deref() {
        let cleaned = sanitize_header(value);
        if !cleaned.is_empty() {
            out.push_str(&format!("In-Reply-To: {}\r\n", ensure_angle(&cleaned)));
        }
    }
    if !message.references.is_empty() {
        let refs: Vec<String> = message
            .references
            .iter()
            .map(|value| ensure_angle(&sanitize_header(value)))
            .filter(|value| value.len() > 2)
            .collect();
        if !refs.is_empty() {
            out.push_str(&format!("References: {}\r\n", refs.join(" ")));
        }
    }
    out.push_str("MIME-Version: 1.0\r\n");
}

/// `multipart/alternative` 的两个正文分片。
fn text_part(boundary: &str, message: &OutgoingMessage) -> String {
    let mut out = String::new();
    out.push_str(&format!("--{boundary}\r\n"));
    out.push_str("Content-Type: text/plain; charset=\"UTF-8\"\r\n");
    out.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
    out.push_str(&base64_block(message.body_text.as_bytes()));
    out.push_str(&format!("--{boundary}\r\n"));
    out.push_str("Content-Type: text/html; charset=\"UTF-8\"\r\n");
    out.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
    out.push_str(&base64_block(message.body_html.as_bytes()));
    out.push_str(&format!("--{boundary}--\r\n"));
    out
}

/// 一个附件分片。
fn attachment_part(attachment: &OutgoingAttachment) -> Result<String, MessageError> {
    let mime_type = sanitize_mime_type(&attachment.mime_type);
    let filename = sanitize_header(&attachment.filename);
    let filename = if filename.is_empty() {
        "attachment".to_string()
    } else {
        filename
    };
    let mut out = String::new();
    if filename.is_ascii() && !filename.contains('"') {
        out.push_str(&format!("Content-Type: {mime_type}; name=\"{filename}\"\r\n"));
    } else {
        out.push_str(&format!(
            "Content-Type: {mime_type}; name=\"{}\"\r\n",
            encode_unstructured(&filename)
        ));
    }
    out.push_str("Content-Transfer-Encoding: base64\r\n");
    if filename.is_ascii() && !filename.contains('"') {
        out.push_str(&format!(
            "Content-Disposition: attachment; filename=\"{filename}\"\r\n\r\n"
        ));
    } else {
        out.push_str(&format!(
            "Content-Disposition: attachment; filename=\"{}\"\r\n\r\n",
            encode_unstructured(&filename)
        ));
    }
    out.push_str(&base64_block(&attachment.bytes));
    Ok(out)
}

/// 把收件人拼成一行；返回空行前先做地址合法性检查。
fn collect_recipients(message: &OutgoingMessage) -> Result<Vec<String>, MessageError> {
    let mut seen = Vec::new();
    let mut recipients = Vec::new();
    for mailbox in message
        .to
        .iter()
        .chain(message.cc.iter())
        .chain(message.bcc.iter())
    {
        let address = normalize_address(&mailbox.address)
            .ok_or_else(|| MessageError::Invalid(format!("收件人地址不合法：{}", mailbox.address)))?;
        if seen
            .iter()
            .any(|item: &String| item.eq_ignore_ascii_case(&address))
        {
            continue;
        }
        seen.push(address.clone());
        recipients.push(address);
    }
    Ok(recipients)
}

/// 地址规整：去掉尖括号与空白，拒绝注入字符，要求包含 `@`。
fn normalize_address(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_start_matches('<').trim_end_matches('>').trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.contains('@')
        || trimmed
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '<' | '>' | ',' | ';'))
    {
        return None;
    }
    Some(trimmed.to_string())
}

/// 把发件人 / 收件人列表拼成信头。
fn format_mailbox_list(list: &[Mailbox]) -> String {
    let items: Vec<String> = list.iter().map(format_mailbox).collect();
    if items.is_empty() {
        "undisclosed-recipients:;".to_string()
    } else {
        items.join(", ")
    }
}

fn format_mailbox(mailbox: &Mailbox) -> String {
    let address = mailbox.address.trim();
    let name = sanitize_header(&mailbox.name);
    if name.is_empty() {
        format!("<{address}>")
    } else {
        format!("{} <{address}>", encode_phrase(&name))
    }
}

/// 显示名：纯 ASCII 用带转义的引号，其余走 RFC 2047。
fn encode_phrase(value: &str) -> String {
    if value.is_ascii() {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        encode_unstructured(value)
    }
}

/// 非结构化信头（主题、显示名）：纯 ASCII 原样，含非 ASCII 走 RFC 2047 编码词。
fn encode_unstructured(value: &str) -> String {
    let cleaned = sanitize_header(value);
    if cleaned.is_ascii() {
        return cleaned;
    }
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_bytes = 0usize;
    for ch in cleaned.chars() {
        if current_bytes + ch.len_utf8() > 45 && !current.is_empty() {
            words.push(rfc2047_word(&current));
            current.clear();
            current_bytes = 0;
        }
        current.push(ch);
        current_bytes += ch.len_utf8();
    }
    if !current.is_empty() {
        words.push(rfc2047_word(&current));
    }
    words.join("\r\n ")
}

fn rfc2047_word(value: &str) -> String {
    format!("=?UTF-8?B?{}?=", STANDARD.encode(value.as_bytes()))
}

/// 信头安全：去掉换行等控制字符（防头注入），保留可读文本。
fn sanitize_header(value: &str) -> String {
    value
        .chars()
        .filter_map(|ch| match ch {
            '\r' | '\n' | '\t' => Some(' '),
            _ if ch.is_control() => None,
            _ => Some(ch),
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn sanitize_mime_type(value: &str) -> String {
    let cleaned = sanitize_header(value);
    let token = cleaned.split(';').next().unwrap_or("").trim();
    if token.is_empty() || token.contains(char::is_whitespace) {
        "application/octet-stream".to_string()
    } else {
        token.to_string()
    }
}

/// base64 分块：每行不超过 76 个字符，行尾 CRLF。
fn base64_block(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "\r\n".to_string();
    }
    let encoded = STANDARD.encode(bytes);
    let mut out = String::with_capacity(encoded.len() + encoded.len() / 76 * 2 + 2);
    for chunk in encoded.as_bytes().chunks(76) {
        if let Ok(text) = std::str::from_utf8(chunk) {
            out.push_str(text);
        }
        out.push_str("\r\n");
    }
    out
}

fn ensure_angle(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with('<') && trimmed.ends_with('>') {
        trimmed.to_string()
    } else {
        format!("<{trimmed}>")
    }
}

fn make_message_id(from_address: &str) -> String {
    let domain = from_address
        .split('@')
        .nth(1)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("em-master.local");
    let seq = MESSAGE_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    format!("{nanos}.{seq}@{domain}")
}

fn make_boundary(kind: &str) -> String {
    let seq = MESSAGE_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    format!("=_em-{kind}-{nanos}-{seq}")
}

/// RFC 5322 日期（UTC，`+0000`）。
fn rfc5322_date(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let weekday = WEEKDAYS[(days.rem_euclid(7)) as usize];
    format!(
        "{weekday}, {day:02} {} {year:04} {:02}:{:02}:{:02} +0000",
        MONTHS[(month - 1) as usize],
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// 按扩展名猜内容类型；猜不出按二进制流处理。
pub fn guess_mime_type(filename: &str) -> String {
    let ext = filename.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let guessed = match ext.as_str() {
        "txt" | "log" | "md" => "text/plain",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    };
    guessed.to_string()
}
#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;

    use super::{build_message, guess_mime_type, Mailbox, MessageError, OutgoingAttachment, OutgoingMessage};

    fn message() -> OutgoingMessage {
        OutgoingMessage {
            from_name: "张三".to_string(),
            from_address: "zhangsan@example.com".to_string(),
            to: vec![Mailbox {
                name: "李四".to_string(),
                address: "lisi@example.com".to_string(),
            }],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "会议纪要".to_string(),
            body_text: "正文".to_string(),
            body_html: "<p>正文</p>".to_string(),
            in_reply_to: None,
            references: Vec::new(),
            attachments: Vec::new(),
            date_unix: 1_700_000_000,
        }
    }

    fn decode_blocks(text: &str) -> String {
        let encoded: String = text
            .lines()
            .skip_while(|line| !line.is_empty())
            .skip(1)
            .take_while(|line| !line.starts_with("--"))
            .collect::<Vec<_>>()
            .join("");
        let cleaned: String = encoded
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '='))
            .collect();
        String::from_utf8(STANDARD.decode(cleaned).expect("base64")).expect("utf8")
    }

    #[test]
    fn 纯文本邮件按替代视图组装() {
        let built = build_message(&message()).expect("组装");
        let raw = String::from_utf8(built.raw.clone()).expect("文本");
        assert!(raw.contains("From: =?UTF-8?B?"));
        assert!(raw.contains("To: =?UTF-8?B?"));
        assert!(raw.contains("Subject: =?UTF-8?B?"));
        assert!(raw.contains("MIME-Version: 1.0"));
        assert!(raw.contains("multipart/alternative"));
        assert!(!raw.contains("multipart/mixed"));
        assert!(raw.contains("charset=\"UTF-8\""));
        assert!(built.message_id.contains('@'));
        assert_eq!(built.recipients, vec!["lisi@example.com"]);
    }

    #[test]
    fn 密送只进信封不写信头() {
        let mut draft = message();
        draft.cc = vec![Mailbox::new("cc@example.com")];
        draft.bcc = vec![Mailbox::new("bcc@example.com")];
        let built = build_message(&draft).expect("组装");
        let raw = String::from_utf8(built.raw.clone()).expect("文本");
        assert!(!raw.contains("bcc@example.com"), "密送地址不得出现在信头");
        assert!(raw.contains("cc@example.com"));
        assert_eq!(
            built.recipients,
            vec![
                "lisi@example.com".to_string(),
                "cc@example.com".to_string(),
                "bcc@example.com".to_string()
            ]
        );
    }

    #[test]
    fn 收件人去重忽略大小写() {
        let mut draft = message();
        draft.cc = vec![Mailbox::new("LISI@example.com")];
        let built = build_message(&draft).expect("组装");
        assert_eq!(built.recipients.len(), 1);
    }

    #[test]
    fn 带附件时用混合视图且附件名编码() {
        let mut draft = message();
        draft.attachments = vec![OutgoingAttachment {
            filename: "会议记录.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            bytes: b"hello".to_vec(),
        }];
        let built = build_message(&draft).expect("组装");
        let raw = String::from_utf8(built.raw.clone()).expect("文本");
        assert!(raw.contains("multipart/mixed"));
        assert!(raw.contains("application/pdf"));
        assert!(raw.contains("=?UTF-8?B?"));
        assert!(raw.contains("Content-Transfer-Encoding: base64"));
    }

    #[test]
    fn 主题里的换行不能造成头注入() {
        let mut draft = message();
        draft.subject = "标题\r\nBcc: evil@example.com".to_string();
        let built = build_message(&draft).expect("组装");
        let raw = String::from_utf8(built.raw).expect("文本");
        assert!(!raw.contains("\r\nBcc:"), "换行应被清洗掉：{raw}");
    }

    #[test]
    fn 非法收件人与超限被拒() {
        let mut draft = message();
        draft.to = vec![Mailbox::new("not-an-address")];
        let err = build_message(&draft).expect_err("应拒绝");
        assert!(matches!(err, MessageError::Invalid(_)));

        let mut draft = message();
        draft.attachments = vec![OutgoingAttachment {
            filename: "big.bin".to_string(),
            mime_type: "application/octet-stream".to_string(),
            bytes: vec![0u8; super::MAX_MESSAGE_BYTES + 1],
        }];
        let err = build_message(&draft).expect_err("应超限");
        assert!(matches!(err, MessageError::TooLarge { .. }));
    }

    #[test]
    fn 附件内容可被正确解码() {
        let mut draft = message();
        draft.attachments = vec![OutgoingAttachment {
            filename: "a.txt".to_string(),
            mime_type: "text/plain".to_string(),
            bytes: "附件内容".as_bytes().to_vec(),
        }];
        let built = build_message(&draft).expect("组装");
        let raw = String::from_utf8(built.raw).expect("文本");
        let attachment = raw
            .split("Content-Disposition: attachment")
            .nth(1)
            .expect("附件段");
        assert_eq!(decode_blocks(attachment), "附件内容");
    }

    #[test]
    fn 日期符合rfc5322格式() {
        let draft = message();
        let built = build_message(&draft).expect("组装");
        let raw = String::from_utf8(built.raw).expect("文本");
        let date = raw
            .lines()
            .find(|line| line.starts_with("Date: "))
            .expect("日期头");
        assert!(date.contains("+0000"), "{date}");
        assert!(date.ends_with("+0000"), "{date}");
        assert!(date.contains("Nov 2023"), "{date}");
    }

    #[test]
    fn 按扩展名猜内容类型() {
        assert_eq!(guess_mime_type("a.PDF"), "application/pdf");
        assert_eq!(
            guess_mime_type("a.docx"),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
        assert_eq!(guess_mime_type("unknown.xyz"), "application/octet-stream");
        assert_eq!(guess_mime_type("noext"), "application/octet-stream");
    }
}
