//! 内置授权服务商元数据：Gmail 与 Microsoft。
//!
//! 桌面端走 PKCE 公共客户端流程，客户端编号不是秘密，可以随程序一起发给用户。
//! 编号来源有三处：源码里写死的常量（发布版随包带走）、编译时注入的环境变量
//! （打包脚本用）、运行时环境变量（本机调试用）。

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

/// 随源码写死的内置客户端编号（发布版带着走，普通用户零配置）。
///
/// 桌面端走 PKCE 公共客户端，编号按 OAuth 规范不是秘密，可以随程序一起分发。
/// 留空字符串表示这一类没内置。
const BUILTIN_GMAIL_CLIENT_ID: &str =
    "199049238202-5j8kdrdoguhsf1h8jnmnuh3anrvip3t6.apps.googleusercontent.com";
/// 微软 Outlook 的内置编号；对应在 Microsoft Entra 里注册的桌面应用。
const BUILTIN_MICROSOFT_CLIENT_ID: &str = "09746a15-e75b-4eee-a361-d82a3e9d8c0d";

/// 软件内置的开发者应用编号（客户端编号）。
///
/// 取值顺序：编译时注入 > 运行时环境变量 > 源码常量。
/// - 谷歌：`YMAIL_GMAIL_CLIENT_ID`
/// - 微软：`YMAIL_MICROSOFT_CLIENT_ID`
///
/// 三处都没配就返回 `None`，界面会提示用户去「高级设置」里自己填。
pub fn default_client_id(kind: ProviderKind) -> Option<String> {
    let (compile_time, runtime_key, embedded) = match kind {
        ProviderKind::Gmail => (
            option_env!("YMAIL_GMAIL_CLIENT_ID"),
            "YMAIL_GMAIL_CLIENT_ID",
            BUILTIN_GMAIL_CLIENT_ID,
        ),
        ProviderKind::Microsoft => (
            option_env!("YMAIL_MICROSOFT_CLIENT_ID"),
            "YMAIL_MICROSOFT_CLIENT_ID",
            BUILTIN_MICROSOFT_CLIENT_ID,
        ),
    };
    let runtime = std::env::var(runtime_key).ok();
    pick_client_id(compile_time, runtime.as_deref(), embedded)
}

/// 按优先级挑一个非空编号：编译期 > 运行期 > 源码内置。
fn pick_client_id(compile_time: Option<&str>, runtime: Option<&str>, embedded: &str) -> Option<String> {
    clean_client_id(compile_time)
        .or_else(|| clean_client_id(runtime))
        .or_else(|| clean_client_id(Some(embedded)))
}

/// 去掉首尾空白；空串和纯空白都当「没配」。
fn clean_client_id(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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
    fn 编号取值顺序是编译期优先运行期次之内置兜底() {
        let compile = pick_client_id(Some(" compile "), Some("runtime"), "embedded");
        assert_eq!(compile.as_deref(), Some("compile"));
        let runtime = pick_client_id(None, Some(" runtime "), "embedded");
        assert_eq!(runtime.as_deref(), Some("runtime"));
        let embedded = pick_client_id(None, None, "embedded");
        assert_eq!(embedded.as_deref(), Some("embedded"));
        let blank = pick_client_id(Some("  "), Some(""), "  ");
        assert_eq!(blank, None);
    }

    #[test]
    fn 谷歌编号已经写进源码() {
        let id = BUILTIN_GMAIL_CLIENT_ID.trim();
        assert!(
            id.ends_with(".apps.googleusercontent.com"),
            "谷歌应用编号应以 .apps.googleusercontent.com 结尾：{id}"
        );
        assert!(id.contains("-"), "谷歌应用编号应带项目号前缀：{id}");
    }

    #[test]
    fn 微软编号已经写进源码() {
        let id = BUILTIN_MICROSOFT_CLIENT_ID.trim();
        assert_eq!(id.len(), 36, "微软应用编号是 36 位：{id}");
        assert_eq!(id.matches('-').count(), 4, "微软应用编号带 4 个连字号：{id}");
        assert!(id.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-'));
    }

    #[test]
    fn 内置编号能直接支撑授权() {
        for kind in [ProviderKind::Gmail, ProviderKind::Microsoft] {
            let resolved = resolve_client_id(kind, "")
                .unwrap_or_else(|_| panic!("源码里已经内置了{}编号", kind.display_name()));
            assert!(!resolved.trim().is_empty());
        }
    }

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
