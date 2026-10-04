//! 站点类型与思考程度抽象。
//!
//! 站点分三类：OpenAI 兼容（第三方中转站 / 自建网关）、DeepL（只做翻译）、
//! 本机 Ollama（零外传）。思考程度是四档抽象，由这里映射成各家参数。

use crate::error::AiError;

/// AI 站点类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// OpenAI 兼容站点。
    OpenAiCompatible,
    /// DeepL 翻译接口；只做翻译，不做摘要与润色。
    DeepL,
    /// 本机 Ollama；地址与本机判断走同一套规则。
    Ollama,
}

impl ProviderKind {
    /// 存库用的稳定标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai_compatible",
            Self::DeepL => "deepl",
            Self::Ollama => "ollama",
        }
    }

    /// 从存库文本还原；认不出来返回 `None`。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai_compatible" | "openai" | "compatible" => Some(Self::OpenAiCompatible),
            "deepl" => Some(Self::DeepL),
            "ollama" => Some(Self::Ollama),
            _ => None,
        }
    }

    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "OpenAI 兼容站点",
            Self::DeepL => "DeepL 翻译",
            Self::Ollama => "本机 Ollama",
        }
    }

    /// 新建站点时预填的地址。
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "https://api.openai.com/v1",
            Self::DeepL => "https://api-free.deepl.com",
            Self::Ollama => "http://127.0.0.1:11434/v1",
        }
    }

    /// 是不是只能翻译。
    pub fn translation_only(self) -> bool {
        matches!(self, Self::DeepL)
    }
}

/// 思考程度：关闭 / 低 / 中 / 高。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThinkingLevel {
    /// 关闭，不传任何思考参数。
    #[default]
    Off,
    /// 低。
    Low,
    /// 中。
    Medium,
    /// 高。
    High,
}

impl ThinkingLevel {
    /// 存库用的稳定标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// 从存库文本还原；认不出来按「关闭」处理。
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Self::Low,
            "medium" | "mid" => Self::Medium,
            "high" => Self::High,
            _ => Self::Off,
        }
    }

    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "关闭",
            Self::Low => "低",
            Self::Medium => "中",
            Self::High => "高",
        }
    }

    /// 映射成 OpenAI 兼容接口的 `reasoning_effort`；关闭时不带这个参数。
    pub fn reasoning_effort(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
        }
    }
}

/// 这个主机名是不是本机。本机允许明文 HTTP，其它一律要 HTTPS。
pub fn is_local_host(host: &str) -> bool {
    let host = host
        .trim()
        .trim_matches(|ch| ch == '[' || ch == ']')
        .to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "0.0.0.0" {
        return true;
    }
    // 整个 127.0.0.0/8 都算本机。
    if let Some(rest) = host.strip_prefix("127.") {
        return rest.split('.').all(|part| part.parse::<u8>().is_ok());
    }
    false
}

/// 校验并规整站点地址：去掉末尾斜杠；非本机必须 HTTPS。
pub fn normalize_base_url(raw: &str, kind: ProviderKind) -> Result<String, AiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AiError::Config("站点地址不能为空"));
    }
    let url = url::Url::parse(trimmed).map_err(|_| AiError::Config("站点地址无法解析"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AiError::Config(
            "站点地址里不能带账号或密码，请把密钥填在密钥框里",
        ));
    }
    // 有些站点会把密钥塞进查询串；这种地址一旦入库就等于密钥落库，必须拦住。
    for (name, _) in url.query_pairs() {
        let lowered = name.trim().to_ascii_lowercase();
        if matches!(
            lowered.as_str(),
            "key" | "api_key" | "apikey" | "cdkey" | "token" | "access_token" | "secret" | "password"
        ) {
            return Err(AiError::Config(
                "站点地址里不能带账号或密码，请把密钥填在密钥框里",
            ));
        }
    }
    let host = url
        .host_str()
        .ok_or(AiError::Config("站点地址缺少主机名"))?
        .to_string();
    match url.scheme() {
        "https" => {}
        "http" => {
            if !is_local_host(&host) {
                return Err(AiError::InsecureEndpoint(host));
            }
        }
        _ => return Err(AiError::Config("站点地址协议只支持 http 或 https")),
    }
    if kind == ProviderKind::Ollama && !is_local_host(&host) {
        // Ollama 设计上就跑在本机；远程地址按普通兼容站点处理也允许，只是提醒用户自己确认。
        tracing::debug!(host = %host, "Ollama 站点指向了非本机地址");
    }
    Ok(trimmed.trim_end_matches('/').to_string())
}

/// 把站点地址与路径拼起来。
pub fn join_url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 站点标识可往返() {
        for kind in [
            ProviderKind::OpenAiCompatible,
            ProviderKind::DeepL,
            ProviderKind::Ollama,
        ] {
            assert_eq!(ProviderKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(
            ProviderKind::parse("OpenAI"),
            Some(ProviderKind::OpenAiCompatible)
        );
        assert_eq!(ProviderKind::parse("anthropic"), None);
    }

    #[test]
    fn 只有deepl是纯翻译站点() {
        assert!(ProviderKind::DeepL.translation_only());
        assert!(!ProviderKind::Ollama.translation_only());
        assert!(!ProviderKind::OpenAiCompatible.translation_only());
    }

    #[test]
    fn 思考程度可往返且关闭时不带参数() {
        for level in [
            ThinkingLevel::Off,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ] {
            assert_eq!(ThinkingLevel::parse(level.as_str()), level);
        }
        assert_eq!(ThinkingLevel::parse("看不懂"), ThinkingLevel::Off);
        assert_eq!(ThinkingLevel::Off.reasoning_effort(), None);
        assert_eq!(ThinkingLevel::High.reasoning_effort(), Some("high"));
    }

    #[test]
    fn 本机地址判断() {
        assert!(is_local_host("localhost"));
        assert!(is_local_host("127.0.0.1"));
        assert!(is_local_host("127.1.2.3"));
        assert!(is_local_host("[::1]"));
        assert!(!is_local_host("api.openai.com"));
        assert!(!is_local_host("1270.0.0.1"));
    }

    #[test]
    fn 非本机明文地址被拦住() {
        let error = normalize_base_url("http://api.example.com/v1", ProviderKind::OpenAiCompatible)
            .expect_err("非本机明文应被拦住");
        assert!(matches!(error, AiError::InsecureEndpoint(_)));
        let ok =
            normalize_base_url("http://127.0.0.1:11434/v1/", ProviderKind::Ollama).expect("本机明文允许");
        assert_eq!(ok, "http://127.0.0.1:11434/v1");
    }

    #[test]
    fn 地址拼接不重复斜杠() {
        assert_eq!(
            join_url("https://api.example.com/v1/", "/chat/completions"),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn 站点地址里不能夹带账号密码() {
        let error = normalize_base_url(
            "https://user:secret-pass@api.example.com/v1",
            ProviderKind::OpenAiCompatible,
        )
        .expect_err("带账号密码的地址必须被拒绝");
        assert!(matches!(error, AiError::Config(_)));
        let text = error.to_string();
        assert!(!text.contains("secret-pass"), "{text}");
        assert!(text.contains("密钥框"), "{text}");
    }

    #[test]
    fn 站点地址里不能夹带密钥查询参数() {
        let error = normalize_base_url(
            "https://api.example.com/v1?key=sk-live-abcdef123456",
            ProviderKind::OpenAiCompatible,
        )
        .expect_err("查询串里带密钥必须被拒绝");
        let text = error.to_string();
        assert!(!text.contains("sk-live"), "{text}");

        // 普通查询参数不该被误伤。
        let ok = normalize_base_url(
            "https://api.example.com/v1?api-version=2024-02-01",
            ProviderKind::OpenAiCompatible,
        )
        .expect("普通查询参数应保留");
        assert!(ok.contains("api-version"), "{ok}");
    }
}
