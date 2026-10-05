//! 内置授权服务商元数据：Gmail 与 Microsoft。
//!
//! 桌面端走 PKCE 公共客户端流程，客户端编号不是秘密，可以随程序一起发给用户。
//! 编号来源有两处：编译时写死的值（发布打包用）、运行时环境变量（本机调试用）。

use crate::error::OAuthError;

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

/// 软件内置的开发者应用编号（客户端编号）。
///
/// 优先取编译时写死的值，其次取运行时环境变量：
/// - 谷歌：`EM_MASTER_GMAIL_CLIENT_ID`
/// - 微软：`EM_MASTER_MICROSOFT_CLIENT_ID`
///
/// 两处都没配就返回 `None`，界面会提示用户去「高级设置」里自己填。
pub fn default_client_id(kind: ProviderKind) -> Option<String> {
    let (built_in, runtime_key) = match kind {
        ProviderKind::Gmail => (
            option_env!("EM_MASTER_GMAIL_CLIENT_ID"),
            "EM_MASTER_GMAIL_CLIENT_ID",
        ),
        ProviderKind::Microsoft => (
            option_env!("EM_MASTER_MICROSOFT_CLIENT_ID"),
            "EM_MASTER_MICROSOFT_CLIENT_ID",
        ),
    };
    if let Some(value) = built_in.map(str::trim).filter(|value| !value.is_empty()) {
        return Some(value.to_string());
    }
    std::env::var(runtime_key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 决定这次授权用哪个客户端编号：用户填了就用用户的，没填就用内置的。
///
/// 内置编号也没有时给一句能看懂的中文提示，指明去哪儿填。
pub fn resolve_client_id(kind: ProviderKind, provided: &str) -> Result<String, OAuthError> {
    let provided = provided.trim();
    if !provided.is_empty() {
        return Ok(provided.to_string());
    }
    default_client_id(kind).ok_or_else(|| {
        OAuthError::Config(format!(
            "还没有内置{}的登录编号；请在账号表单的「高级设置」里填一个客户端编号，或让软件提供者补上内置编号",
            kind.display_name()
        ))
    })
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
    fn 用户填了编号就优先用用户的() {
        let resolved = resolve_client_id(ProviderKind::Microsoft, " my-client ").expect("应能解析");
        assert_eq!(resolved, "my-client");
    }

    #[test]
    fn 没填编号时要么用内置要么给可读提示() {
        match resolve_client_id(ProviderKind::Microsoft, "") {
            Ok(value) => assert!(!value.trim().is_empty(), "内置编号不能是空串"),
            Err(error) => {
                let text = error.to_string();
                assert!(text.contains("登录编号"), "应提示登录编号：{text}");
                assert!(
                    !text.to_lowercase().contains("token"),
                    "提示里不能带令牌字样：{text}"
                );
            }
        }
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
