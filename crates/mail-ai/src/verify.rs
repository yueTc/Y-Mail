//! 验证码 / 验证链接的本地解析与白名单校验。
//!
//! 安全前提：模型输出与邮件正文一样是不可信内容。这里只做「提取候选 + 本地校验」，
//! 绝不因为模型怎么说就去复制、跳转或执行任何动作；真正的复制与打开只由用户点击按钮触发。
//! 校验通过的值也不会写日志、写库。

use serde::Deserialize;

/// 从一封邮件里识别出的验证信息；字段都经过本地校验，没识别到就是 `None`。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerificationFinding {
    /// 校验通过的验证码。
    pub code: Option<String>,
    /// 校验通过的验证链接。
    pub link: Option<String>,
}

impl VerificationFinding {
    /// 两个字段都没有时算「没识别到」。
    pub fn is_empty(&self) -> bool {
        self.code.is_none() && self.link.is_none()
    }
}

/// 模型返回的原始片段；字段缺省按空串处理。
#[derive(Debug, Deserialize, Default)]
struct RawFinding {
    #[serde(default)]
    code: String,
    #[serde(default)]
    link: String,
}

/// 把模型输出解析成校验后的结果。
///
/// 模型可能不老实：这里先截取第一个 `{` 到最后一个 `}` 再解析，解析失败一律当没识别到。
pub fn parse_verification(raw: &str) -> VerificationFinding {
    let Some(json) = extract_json_object(raw) else {
        return VerificationFinding::default();
    };
    let Ok(parsed) = serde_json::from_str::<RawFinding>(json) else {
        return VerificationFinding::default();
    };
    VerificationFinding {
        code: sanitize_code(&parsed.code),
        link: sanitize_link(&parsed.link),
    }
}

/// 从一段文本里截出最外层的 JSON 对象；找不到返回 `None`。
fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    (end > start).then(|| &raw[start..=end])
}

/// 校验验证码：去掉连字符后是 4 到 12 位数字或字母数字，最多一个连字符，且不能在首尾。
pub fn sanitize_code(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut hyphen_count = 0usize;
    let mut alnum_count = 0usize;
    for (index, ch) in trimmed.char_indices() {
        if ch == '-' {
            if index == 0 || hyphen_count >= 1 {
                return None;
            }
            hyphen_count += 1;
        } else if ch.is_ascii_alphanumeric() {
            alnum_count += 1;
        } else {
            return None;
        }
    }
    if trimmed.ends_with('-') || !(4..=12).contains(&alnum_count) {
        return None;
    }
    Some(trimmed.to_string())
}

/// 校验验证链接：只放行绝对的 `http://` / `https://` 地址，其余协议一律拒绝。
pub fn sanitize_link(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 4096 {
        return None;
    }
    let parsed = url::Url::parse(trimmed).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.host_str()?;
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析正常返回的验证码和链接() {
        let finding = parse_verification(r#"{"code":"123456","link":"https://example.com/verify?t=abc"}"#);
        assert_eq!(finding.code.as_deref(), Some("123456"));
        assert_eq!(finding.link.as_deref(), Some("https://example.com/verify?t=abc"));
        assert!(!finding.is_empty());
    }

    #[test]
    fn 容忍代码块围栏和多余文字() {
        let raw = "好的，结果如下：\n```json\n{\"code\":\"AB-1234\",\"link\":\"\"}\n```";
        let finding = parse_verification(raw);
        assert_eq!(finding.code.as_deref(), Some("AB-1234"));
        assert!(finding.link.is_none());
    }

    #[test]
    fn 模型乱说的内容过不了本地校验() {
        let finding = parse_verification(r#"{"code":"<b>424242</b>","link":"javascript:alert(1)"}"#);
        assert!(finding.is_empty());
    }

    #[test]
    fn 非标准协议一律拒绝() {
        for link in [
            "file:///C:/windows/system32/calc.exe",
            "data:text/html,<script>alert(1)</script>",
            "javascript:alert(1)",
            "ftp://example.com/x",
        ] {
            assert!(sanitize_link(link).is_none(), "{link}");
        }
    }

    #[test]
    fn 验证码长度与字符受限() {
        assert_eq!(sanitize_code(" 1234 ").as_deref(), Some("1234"));
        assert_eq!(sanitize_code("abcd1234").as_deref(), Some("abcd1234"));
        assert_eq!(sanitize_code("12-34").as_deref(), Some("12-34"));
        assert!(sanitize_code("123").is_none(), "太短");
        assert!(sanitize_code("1234567890123").is_none(), "太长");
        assert!(sanitize_code("-1234").is_none(), "连字符不能在开头");
        assert!(sanitize_code("12-3-4").is_none(), "最多一个连字符");
        assert!(sanitize_code("12 34").is_none(), "不能有空格");
        assert!(sanitize_code("验证码123456").is_none(), "不能带别的内容");
    }

    #[test]
    fn 解析失败当没识别到() {
        assert!(parse_verification("没有验证码").is_empty());
        assert!(parse_verification("").is_empty());
    }
}
