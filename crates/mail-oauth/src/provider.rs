//! 内置授权服务商元数据：Gmail 与 Microsoft。

/// 支持的授权服务商。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// 谷歌 Gmail。
    Gmail,
    /// 微软 Outlook / 企业邮箱。
    Microsoft,
}

impl ProviderKind {
    /// 入库用的稳定标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::Microsoft => "microsoft",
        }
    }

    /// 从入库文本还原；认不出来返回 `None`。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "gmail" | "google" => Some(Self::Gmail),
            "microsoft" | "outlook" | "office365" | "hotmail" => Some(Self::Microsoft),
            _ => None,
        }
    }

    /// 界面展示名。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Gmail => "谷歌 Gmail",
            Self::Microsoft => "微软 Outlook",
        }
    }
}

/// 一个服务商的固定端点与权限范围。
#[derive(Debug, Clone, Copy)]
pub struct ProviderMeta {
    /// 服务商标识。
    pub kind: ProviderKind,
    /// 授权页地址。
    pub auth_endpoint: &'static str,
    /// 换令牌地址。
    pub token_endpoint: &'static str,
    /// 申请的权限范围。
    pub scopes: &'static [&'static str],
}

/// 取某个服务商的固定元数据。
pub fn provider_meta(kind: ProviderKind) -> ProviderMeta {
    match kind {
        ProviderKind::Gmail => ProviderMeta {
            kind,
            auth_endpoint: "https://accounts.google.com/o/oauth2/v2/auth",
            token_endpoint: "https://oauth2.googleapis.com/token",
            scopes: &["https://mail.google.com/"],
        },
        ProviderKind::Microsoft => ProviderMeta {
            kind,
            auth_endpoint: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            token_endpoint: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            scopes: &[
                "https://outlook.office.com/IMAP.AccessAsUser.All",
                "https://outlook.office.com/SMTP.Send",
                "offline_access",
            ],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 标识可往返且兼容常见别名() {
        for kind in [ProviderKind::Gmail, ProviderKind::Microsoft] {
            assert_eq!(ProviderKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(ProviderKind::parse("Google"), Some(ProviderKind::Gmail));
        assert_eq!(ProviderKind::parse("Outlook"), Some(ProviderKind::Microsoft));
        assert_eq!(ProviderKind::parse("yahoo"), None);
    }

    #[test]
    fn 谷歌只申请全量邮箱权限() {
        let meta = provider_meta(ProviderKind::Gmail);
        assert_eq!(meta.scopes, &["https://mail.google.com/"]);
        assert!(meta.auth_endpoint.starts_with("https://"));
        assert!(meta.token_endpoint.starts_with("https://"));
    }

    #[test]
    fn 微软权限覆盖收发与离线刷新() {
        let meta = provider_meta(ProviderKind::Microsoft);
        assert!(meta
            .scopes
            .contains(&"https://outlook.office.com/IMAP.AccessAsUser.All"));
        assert!(meta.scopes.contains(&"https://outlook.office.com/SMTP.Send"));
        assert!(meta.scopes.contains(&"offline_access"));
    }
}
