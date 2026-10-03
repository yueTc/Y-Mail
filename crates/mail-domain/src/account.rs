//! 邮箱账号的领域模型。
//!
//! 这里只描述「账号是什么」，不负责落库与联网。凭据本体（授权码）不在本模块出现，
//! 数据库里只保存一个指向系统凭据管理器的引用键。

use std::fmt;

use crate::error::ValidationError;
use crate::proxy::ProxyId;

/// 账号主键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccountId(pub i64);

/// 认证方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthType {
    /// 授权码 / 密码登录（Wave 1 使用）。
    Password,
    /// OAuth2（Gmail / Outlook，Wave 6 使用）。
    OAuth2,
}

impl AuthType {
    /// 存库用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::OAuth2 => "oauth2",
        }
    }

    /// 从存库字符串还原。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "password" => Some(Self::Password),
            "oauth2" => Some(Self::OAuth2),
            _ => None,
        }
    }

    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::Password => "授权码",
            Self::OAuth2 => "OAuth2 授权",
        }
    }
}

/// 传输加密方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// 从建立连接起就是加密的（隐式 SSL/TLS，常见端口 993 / 465）。
    Tls,
    /// 先明文连接，再用 STARTTLS 升级为加密（常见端口 143 / 587）。
    StartTls,
    /// 不加密（只建议本机自建服务器或调试时使用）。
    Plain,
}

impl Security {
    /// 存库用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tls => "tls",
            Self::StartTls => "starttls",
            Self::Plain => "plain",
        }
    }

    /// 从存库字符串还原。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "tls" => Some(Self::Tls),
            "starttls" => Some(Self::StartTls),
            "plain" => Some(Self::Plain),
            _ => None,
        }
    }

    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::Tls => "SSL/TLS（隐式加密）",
            Self::StartTls => "STARTTLS（先明文后升级）",
            Self::Plain => "不加密",
        }
    }
}

/// 一台服务器的地址、端口与加密方式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// 服务器主机名，例如 `imap.qq.com`。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 加密方式。
    pub security: Security,
}

impl ServerConfig {
    /// 校验主机与端口；问题追加进 `problems`，`name` 用于拼出「收件服务器」之类的前缀。
    pub fn validate(&self, name: &str, problems: &mut Vec<String>) {
        if self.host.trim().is_empty() {
            problems.push(format!("{name}地址不能为空"));
        } else if self.host.chars().any(char::is_whitespace) {
            problems.push(format!("{name}地址不能包含空白字符"));
        }
        if self.port == 0 {
            problems.push(format!("{name}端口必须在 1 到 65535 之间"));
        }
    }
}

/// 账号级代理策略（规格 R2 的账号级覆盖）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountProxyMode {
    /// 跟随全局策略。
    InheritGlobal,
    /// 强制直连。
    Direct,
    /// 使用某个指定代理。
    Custom(ProxyId),
}

impl AccountProxyMode {
    /// 存库用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InheritGlobal => "inherit",
            Self::Direct => "direct",
            Self::Custom(_) => "custom",
        }
    }

    /// 从存库字符串还原；`custom` 需要带上代理主键。
    pub fn parse(value: &str, proxy_id: Option<i64>) -> Option<Self> {
        match value {
            "inherit" => Some(Self::InheritGlobal),
            "direct" => Some(Self::Direct),
            "custom" => proxy_id.map(|id| Self::Custom(ProxyId(id))),
            _ => None,
        }
    }

    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::InheritGlobal => "跟随全局",
            Self::Direct => "直连",
            Self::Custom(_) => "指定代理",
        }
    }
}

/// 新建或修改账号时由界面填写的草稿；不含凭据本体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountDraft {
    /// 显示名。
    pub display_name: String,
    /// 邮箱地址。
    pub email: String,
    /// 认证方式。
    pub auth_type: AuthType,
    /// 登录名（很多邮箱就是邮箱地址本身）。
    pub username: String,
    /// 收件服务器。
    pub imap: ServerConfig,
    /// 发件服务器。
    pub smtp: ServerConfig,
    /// 账号级代理策略。
    pub proxy: AccountProxyMode,
    /// 界面色标，可空。
    pub color: String,
    /// 是否启用。
    pub enabled: bool,
}

impl AccountDraft {
    /// 校验草稿；一次返回全部问题。
    pub fn validate(&self) -> Result<(), ValidationError> {
        let mut problems = Vec::new();

        if self.email.trim().is_empty() {
            problems.push("邮箱地址不能为空".to_string());
        } else if !looks_like_email(&self.email) {
            problems.push("邮箱地址格式不正确".to_string());
        }

        if self.username.trim().is_empty() {
            problems.push("登录名不能为空".to_string());
        }

        self.imap.validate("收件服务器", &mut problems);
        self.smtp.validate("发件服务器", &mut problems);

        if problems.is_empty() {
            Ok(())
        } else {
            Err(ValidationError::new(problems))
        }
    }

    /// 返回一份「显示名已填好」的副本：显示名留空时回落到邮箱地址。
    pub fn normalized(&self) -> Self {
        let mut draft = self.clone();
        if draft.display_name.trim().is_empty() {
            draft.display_name = draft.email.trim().to_string();
        }
        draft
    }
}

/// 一个已保存的账号；凭据本体不在这里，只保留系统凭据管理器里的引用键。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// 主键。
    pub id: AccountId,
    /// 显示名。
    pub display_name: String,
    /// 邮箱地址。
    pub email: String,
    /// 认证方式。
    pub auth_type: AuthType,
    /// 登录名。
    pub username: String,
    /// 收件服务器。
    pub imap: ServerConfig,
    /// 发件服务器。
    pub smtp: ServerConfig,
    /// 账号级代理策略。
    pub proxy: AccountProxyMode,
    /// 界面色标。
    pub color: String,
    /// 是否启用。
    pub enabled: bool,
    /// 系统凭据管理器里的引用键；未保存凭据时为 `None`。
    pub credential_key: Option<String>,
    /// 创建时间（UTC）。
    pub created_at: String,
    /// 更新时间（UTC）。
    pub updated_at: String,
}

impl Account {
    /// 从已保存账号提取一份草稿（编辑表单用）。
    pub fn draft(&self) -> AccountDraft {
        AccountDraft {
            display_name: self.display_name.clone(),
            email: self.email.clone(),
            auth_type: self.auth_type,
            username: self.username.clone(),
            imap: self.imap.clone(),
            smtp: self.smtp.clone(),
            proxy: self.proxy,
            color: self.color.clone(),
            enabled: self.enabled,
        }
    }
}

/// 极简邮箱格式判断：有且仅有一个 `@`，两侧非空且不含空白。
fn looks_like_email(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.chars().any(char::is_whitespace) {
        return false;
    }
    let mut parts = trimmed.split('@');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(local), Some(domain), None) => !local.is_empty() && !domain.is_empty() && domain.contains('.'),
        _ => false,
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{AccountDraft, AccountProxyMode, AuthType, Security, ServerConfig};

    fn draft() -> AccountDraft {
        AccountDraft {
            display_name: "我的邮箱".to_string(),
            email: "someone@example.com".to_string(),
            auth_type: AuthType::Password,
            username: "someone@example.com".to_string(),
            imap: ServerConfig {
                host: "imap.example.com".to_string(),
                port: 993,
                security: Security::Tls,
            },
            smtp: ServerConfig {
                host: "smtp.example.com".to_string(),
                port: 465,
                security: Security::Tls,
            },
            proxy: AccountProxyMode::InheritGlobal,
            color: String::new(),
            enabled: true,
        }
    }

    #[test]
    fn 合法草稿校验通过() {
        draft().validate().expect("应通过校验");
    }

    #[test]
    fn 显示名可留空并回落邮箱() {
        let mut blank = draft();
        blank.display_name = "   ".to_string();
        blank.validate().expect("显示名留空不应拦住校验");
        assert_eq!(blank.normalized().display_name, "someone@example.com");

        let kept = draft().normalized();
        assert_eq!(kept.display_name, "我的邮箱");
    }

    #[test]
    fn 邮箱格式错误会被拦住() {
        let mut bad = draft();
        bad.email = "不是邮箱".to_string();
        let err = bad.validate().expect_err("应校验失败");
        assert!(err.to_string().contains("邮箱地址格式"));
    }

    #[test]
    fn 端口为零会被拦住() {
        let mut bad = draft();
        bad.imap.port = 0;
        let err = bad.validate().expect_err("应校验失败");
        assert!(err.to_string().contains("收件服务器端口"));
    }

    #[test]
    fn 字符串往返解析一致() {
        assert_eq!(AuthType::parse("password"), Some(AuthType::Password));
        assert_eq!(
            Security::parse(Security::StartTls.as_str()),
            Some(Security::StartTls)
        );
        assert_eq!(
            AccountProxyMode::parse("custom", Some(3)),
            Some(AccountProxyMode::Custom(crate::proxy::ProxyId(3)))
        );
        assert_eq!(AccountProxyMode::parse("custom", None), None);
    }
}
