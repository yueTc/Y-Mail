//! OAuth2 错误类型。
//!
//! 规矩：错误消息里只允许出现错误码和固定中文提示，绝不允许携带授权码、
//! 访问令牌或刷新令牌的明文。

use mail_domain::error::ConnectionError;
use thiserror::Error;

/// OAuth2 流程里可读的错误。
#[derive(Debug, Error)]
pub enum OAuthError {
    /// 用户在浏览器里拒绝了授权。
    #[error("授权被拒绝：{0}")]
    Denied(String),
    /// 等回调超时。
    #[error("等待授权回调超时，请重新发起授权")]
    CallbackTimeout,
    /// 回调里的 state 与本次请求对不上，按跨站伪造处理。
    #[error("授权回调校验失败（state 不匹配），已拒绝本次回调")]
    StateMismatch,
    /// 授权服务器返回了错误码（只保留错误码本身）。
    #[error("授权服务器返回错误：{0}")]
    Server(String),
    /// 刷新令牌已失效，需要用户重新授权。
    #[error("授权已失效，请重新授权")]
    ReauthRequired,
    /// 网络或 TLS 失败。
    #[error("OAuth 网络请求失败")]
    Network(#[source] ConnectionError),
    /// 对方响应不符合预期。
    #[error("授权服务器响应无法识别")]
    Protocol,
    /// 请求超时。
    #[error("OAuth 请求超时，请稍后重试")]
    Timeout,
    /// 本地配置不完整。
    #[error("OAuth 配置不完整：{0}")]
    Config(&'static str),
    /// 本机回调端口无法绑定。
    #[error("无法在本机开启授权回调端口")]
    Bind,
}

impl OAuthError {
    /// 把服务器错误码清洗成短标识；顺带区分「要重新授权」。
    pub(crate) fn server_error(code: &str) -> Self {
        let cleaned: String = code
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .take(40)
            .collect();
        if cleaned == "invalid_grant" {
            return Self::ReauthRequired;
        }
        Self::Server(cleaned)
    }

    /// 是否属于「必须重新授权」的情况。
    pub fn needs_reauth(&self) -> bool {
        matches!(self, Self::ReauthRequired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 失效授权码统一映射成重新授权() {
        let error = OAuthError::server_error("invalid_grant");
        assert!(error.needs_reauth());
        assert_eq!(error.to_string(), "授权已失效，请重新授权");
    }

    #[test]
    fn 服务器错误码会去掉可疑字符并截断() {
        let error = OAuthError::server_error("bad\u{4e2d}code\u{20}secret=abc");
        let text = error.to_string();
        assert!(text.contains("badcode"));
        assert!(!text.contains('='), "错误码里的等号应被剔除：{text}");
        assert!(!text.contains('\u{4e2d}'));
    }

    #[test]
    fn 错误码过长会被截断() {
        let long = "x".repeat(500);
        let error = OAuthError::server_error(&long);
        assert!(error.to_string().len() < 80);
    }
}
