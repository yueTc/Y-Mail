//! AI 层的错误类型。
//!
//! 规矩：错误消息里只允许出现固定中文提示与错误码，绝不携带 CDKey、邮件正文
//! 或模型返回的原文片段。

use thiserror::Error;

/// AI / 翻译调用可能出现的失败。
#[derive(Debug, Error)]
pub enum AiError {
    /// 站点地址不合法（协议、主机名、端口）。
    #[error("站点地址不合法：{0}")]
    Config(&'static str),
    /// 非本机站点必须用 HTTPS。
    #[error("非本机站点必须使用 HTTPS：{0}")]
    InsecureEndpoint(String),
    /// 还没填 CDKey。
    #[error("还没有填写该站点的密钥（CDKey / API Key）")]
    MissingKey,
    /// 模型编号为空。
    #[error("还没有选择模型")]
    MissingModel,
    /// 网络或 TLS 失败。
    #[error("连接 AI 站点失败")]
    Network(#[source] mail_domain::error::ConnectionError),
    /// 请求超时。
    #[error("AI 站点响应超时，请稍后重试")]
    Timeout,
    /// 对方响应不是能识别的格式。
    #[error("AI 站点响应无法识别")]
    Protocol,
    /// 对方返回了错误状态（只保留状态码与清洗过的错误码）。
    #[error("AI 站点返回错误（HTTP {status}）：{detail}")]
    Status {
        /// HTTP 状态码。
        status: u16,
        /// 清洗后的错误说明。
        detail: String,
    },
    /// 该站点不支持这个功能（例如 DeepL 只能翻译）。
    #[error("当前站点不支持这个功能：{0}")]
    Unsupported(&'static str),
    /// 模型不支持思考程度参数，已经自动降级重试。
    #[error("该模型不支持所选思考程度，已按默认方式调用")]
    ThinkingDowngraded,
    /// 段落对齐的译文条数与原文对不上。
    #[error("翻译结果与原文段落数对不上，请重试")]
    SegmentMismatch,
}

impl AiError {
    /// 把服务器返回的错误正文清洗成一句可读说明。
    ///
    /// 只保留错误码与短消息，去掉括号里的原始内容，避免把请求正文带出来。
    pub(crate) fn status_detail(status: u16, body: &str) -> String {
        let cleaned = sanitize_error_body(body);
        if cleaned.is_empty() {
            format!("站点拒绝了这次调用（状态码 {status}）")
        } else {
            cleaned
        }
    }

    /// 是否是「思考程度参数不被支持」类的错误（用来决定要不要降级重试）。
    pub(crate) fn looks_like_thinking_unsupported(status: u16, body: &str) -> bool {
        if status != 400 && status != 422 {
            return false;
        }
        let lower = body.to_ascii_lowercase();
        lower.contains("reasoning_effort")
            || lower.contains("reasoning effort")
            || lower.contains("unsupported parameter")
            || lower.contains("unknown parameter")
            || lower.contains("unrecognized")
    }
}

/// 从服务器错误正文里只挑出错误码与短消息，其余一律丢弃。
fn sanitize_error_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // 优先按 JSON 取 error.message / error.code；取不到就退回「只保留短标识」。
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(error) = value.get("error") {
            if let Some(message) = error.get("message").and_then(|item| item.as_str()) {
                return shorten(&scrub_secret_like(message));
            }
            if let Some(code) = error.get("code").and_then(|item| item.as_str()) {
                return shorten(&scrub_secret_like(code));
            }
            if let Some(text) = error.as_str() {
                return shorten(&scrub_secret_like(text));
            }
        }
    }
    // 非 JSON：先把可能回显的密钥长串洗掉，再只留 ASCII 字母数字、下划线、点与短横。
    let scrubbed = scrub_secret_like(trimmed);
    let filtered: String = scrubbed
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
        .take(80)
        .collect();
    if filtered.is_empty() {
        String::new()
    } else {
        format!("错误码 {filtered}")
    }
}

/// 把一条消息截短，避免把整段正文带回界面。
fn shorten(message: &str) -> String {
    let single_line: String = message.chars().take(120).collect();
    single_line.replace('\n', " ").trim().to_string()
}

/// 判断一个字节是不是「密钥形态」里允许出现的字符。
fn is_secret_run_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')
}

/// 把错误正文里可能回显的密钥洗成 `***`。
///
/// 两道防线：
/// 1. `sk-` / `lo-` / `Bearer ` 这类前缀后面跟着的整段都算敏感；
/// 2. 连续 16 位以上的字母数字（含 `_`、`.`、`-`）也按可疑密钥处理。
///
/// 这里只处理 ASCII 边界，不会切坏中文。
fn scrub_secret_like(text: &str) -> String {
    let bytes = text.as_bytes();
    let lowered = text.to_ascii_lowercase();
    let mut spans: Vec<(usize, usize)> = Vec::new();

    // 前缀形态：服务商常只回显带前缀的密钥。
    for prefix in ["sk-", "lo-", "bearer ", "deepl-auth-key "] {
        let mut from = 0;
        while let Some(pos) = lowered[from..].find(prefix) {
            let start = from + pos;
            let mut end = start + prefix.len();
            while end < bytes.len() && is_secret_run_byte(bytes[end]) {
                end += 1;
            }
            if end - start > prefix.len() + 3 {
                spans.push((start, end));
            }
            from = end.max(start + prefix.len());
            if from >= lowered.len() {
                break;
            }
        }
    }

    // 通用形态：超长不可读串。
    let mut run_start: Option<usize> = None;
    for (index, byte) in bytes.iter().enumerate() {
        if is_secret_run_byte(*byte) {
            run_start.get_or_insert(index);
        } else if let Some(start) = run_start.take() {
            if index - start >= 16 {
                spans.push((start, index));
            }
        }
    }
    if let Some(start) = run_start {
        if bytes.len() - start >= 16 {
            spans.push((start, bytes.len()));
        }
    }

    if spans.is_empty() {
        return text.to_string();
    }
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in spans {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end) in merged {
        out.push_str(&text[cursor..start]);
        out.push_str("***");
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 错误正文里的正文片段不会被带出来() {
        let body = r#"{"error":{"message":"bad key","prompt":"这是一封邮件正文的秘密内容"}}"#;
        let detail = AiError::status_detail(401, body);
        assert!(detail.contains("bad key"));
        assert!(!detail.contains("秘密内容"), "不能把正文片段带出来：{detail}");
    }

    #[test]
    fn 非json错误正文只保留短标识() {
        let detail = AiError::status_detail(500, "<html><body>Internal Error!</body></html>");
        assert!(!detail.contains('<'), "{detail}");
        assert!(detail.len() < 120);
    }

    #[test]
    fn 空错误正文给固定提示() {
        let detail = AiError::status_detail(400, "   ");
        assert!(detail.contains("400"));
    }

    #[test]
    fn 能认出思考程度不被支持的报错() {
        let body = r#"{"error":{"message":"Unsupported parameter: reasoning_effort"}}"#;
        assert!(AiError::looks_like_thinking_unsupported(400, body));
        assert!(!AiError::looks_like_thinking_unsupported(500, body));
        let other = r#"{"error":{"message":"invalid api key"}}"#;
        assert!(!AiError::looks_like_thinking_unsupported(400, other));
    }

    #[test]
    fn 回显的密钥前缀形态会被洗掉() {
        let body = r#"{"error":{"message":"Incorrect API key provided: sk-proj-AbCdEfGhIjKlMnOpQrStUvWx"}}"#;
        let detail = AiError::status_detail(401, body);
        assert!(!detail.contains("sk-proj"), "{detail}");
        assert!(!detail.contains("AbCdEfGh"), "{detail}");
        assert!(detail.contains("***"), "{detail}");
    }

    #[test]
    fn 超长不可读串按密钥洗掉但短错误码保留() {
        let body = r#"{"error":{"code":"invalid_api_key"}}"#;
        let detail = AiError::status_detail(401, body);
        assert!(detail.contains("invalid_api_key"), "{detail}");

        let long = r#"{"error":{"message":"bad credential: ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"}}"#;
        let detail = AiError::status_detail(401, long);
        assert!(!detail.contains("ABCDEFGHIJ"), "{detail}");
        assert!(detail.contains("bad credential"), "{detail}");
    }

    #[test]
    fn 非json错误正文里的密钥也不会漏出来() {
        let body = "Unauthorized: sk-live-ZZZZZZZZZZZZZZZZZZZZZZZZZZ";
        let detail = AiError::status_detail(401, body);
        assert!(!detail.contains("sk-live"), "{detail}");
        assert!(!detail.contains("ZZZZZZZZ"), "{detail}");
    }
}
