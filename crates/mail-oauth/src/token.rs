//! 令牌交换与刷新。
//!
//! 令牌只在上层写入系统保险箱；本模块的调试输出和错误里都不含令牌明文。

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mail_domain::proxy::ProxyRoute;
use serde::Deserialize;

use crate::error::OAuthError;
use crate::http::post_form;
use crate::provider::{provider_meta, ProviderKind};

/// 一组令牌。
#[derive(Clone, PartialEq, Eq)]
pub struct TokenSet {
    /// 访问令牌。
    pub access_token: String,
    /// 刷新令牌；首次授权才有，刷新时可能沿用旧的。
    pub refresh_token: Option<String>,
    /// 到期时间（Unix 秒）。
    pub expires_at: Option<i64>,
    /// 服务器返回的权限范围。
    pub scope: Option<String>,
}

impl fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &"<隐藏>")
            .field("refresh_token", &self.refresh_token.is_some())
            .field("expires_at", &self.expires_at)
            .field("scope", &self.scope)
            .finish()
    }
}

impl TokenSet {
    /// 用在 XOAUTH2 里的访问令牌。
    pub fn bearer(&self) -> &str {
        &self.access_token
    }

    /// 是否已到期；`skew_secs` 是提前量。
    pub fn is_expired(&self, now_unix: i64, skew_secs: i64) -> bool {
        match self.expires_at {
            Some(at) => at - skew_secs <= now_unix,
            None => false,
        }
    }
}

/// 当前 Unix 秒。
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
    error: Option<String>,
}

/// 授权码换令牌。
pub async fn exchange_code(
    route: Option<&ProxyRoute>,
    kind: ProviderKind,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    timeout: Duration,
) -> Result<TokenSet, OAuthError> {
    let meta = provider_meta(kind);
    exchange_code_with_endpoint(
        meta.token_endpoint,
        route,
        client_id,
        code,
        verifier,
        redirect_uri,
        timeout,
    )
    .await
}

/// 与上同，但允许指定令牌地址（测试用本地假服务器）。
pub(crate) async fn exchange_code_with_endpoint(
    endpoint: &str,
    route: Option<&ProxyRoute>,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    timeout: Duration,
) -> Result<TokenSet, OAuthError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
    ];
    request_token(route, endpoint, &form, timeout, None).await
}

/// 用刷新令牌换新的访问令牌。
pub async fn refresh(
    route: Option<&ProxyRoute>,
    kind: ProviderKind,
    client_id: &str,
    refresh_token: &str,
    timeout: Duration,
) -> Result<TokenSet, OAuthError> {
    let meta = provider_meta(kind);
    refresh_with_endpoint(meta.token_endpoint, route, client_id, refresh_token, timeout).await
}

/// 与上同，但允许指定令牌地址（测试用本地假服务器）。
pub(crate) async fn refresh_with_endpoint(
    endpoint: &str,
    route: Option<&ProxyRoute>,
    client_id: &str,
    refresh_token: &str,
    timeout: Duration,
) -> Result<TokenSet, OAuthError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    request_token(route, endpoint, &form, timeout, Some(refresh_token)).await
}

async fn request_token(
    route: Option<&ProxyRoute>,
    endpoint: &str,
    form: &[(&str, &str)],
    timeout: Duration,
    fallback_refresh: Option<&str>,
) -> Result<TokenSet, OAuthError> {
    let body = post_form(route, endpoint, form, timeout).await?;
    let parsed: TokenResponse = serde_json::from_str(&body).map_err(|_| OAuthError::Protocol)?;
    if let Some(error) = parsed.error.as_deref() {
        // 只取错误码字段，响应正文其余内容一律丢弃。
        return Err(OAuthError::server_error(error));
    }
    let access_token = parsed
        .access_token
        .filter(|value| !value.trim().is_empty())
        .ok_or(OAuthError::Protocol)?;
    let refresh_token = parsed
        .refresh_token
        .filter(|value| !value.trim().is_empty())
        .or_else(|| fallback_refresh.map(str::to_string));
    let expires_at = parsed.expires_in.map(|secs| now_unix() + secs);
    Ok(TokenSet {
        access_token,
        refresh_token,
        expires_at,
        scope: parsed.scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn fake_server(status: &'static str, payload: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定成功");
        let port = listener.local_addr().expect("取地址").port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接");
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await.expect("读取请求");
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(response.as_bytes()).await.expect("写响应");
        });
        port
    }

    #[tokio::test]
    async fn 换令牌能解析访问与刷新令牌() {
        let port = fake_server(
            "200 OK",
            r#"{"access_token":"at-1","refresh_token":"rt-1","expires_in":3600,"scope":"mail"}"#,
        )
        .await;
        let endpoint = format!("http://127.0.0.1:{port}/token");
        let tokens = exchange_code_with_endpoint(
            &endpoint,
            None,
            "client-1",
            "code-1",
            "verifier-1",
            "http://127.0.0.1:1234/oauth/callback",
            Duration::from_secs(5),
        )
        .await
        .expect("换取成功");
        assert_eq!(tokens.bearer(), "at-1");
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt-1"));
        assert!(tokens.expires_at.expect("有到期时间") > now_unix());
        assert_eq!(tokens.scope.as_deref(), Some("mail"));
    }

    #[tokio::test]
    async fn 刷新时不返回新刷新令牌会沿用旧的() {
        let port = fake_server("200 OK", r#"{"access_token":"at-2","expires_in":600}"#).await;
        let endpoint = format!("http://127.0.0.1:{port}/token");
        let tokens = refresh_with_endpoint(&endpoint, None, "client-1", "rt-old", Duration::from_secs(5))
            .await
            .expect("刷新成功");
        assert_eq!(tokens.bearer(), "at-2");
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt-old"));
    }

    #[tokio::test]
    async fn 失效授权码映射成重新授权() {
        let port = fake_server(
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"token revoked"}"#,
        )
        .await;
        let endpoint = format!("http://127.0.0.1:{port}/token");
        let error = refresh_with_endpoint(&endpoint, None, "client-1", "rt", Duration::from_secs(5))
            .await
            .expect_err("应报失效");
        assert!(error.needs_reauth(), "invalid_grant 应触发重新授权");
    }

    #[tokio::test]
    async fn 错误信息不携带响应里的令牌原文() {
        let port = fake_server(
            "400 Bad Request",
            r#"{"error":"invalid_request","access_token":"leaked-token-value"}"#,
        )
        .await;
        let endpoint = format!("http://127.0.0.1:{port}/token");
        let error = exchange_code_with_endpoint(
            &endpoint,
            None,
            "client-1",
            "code",
            "verifier",
            "http://127.0.0.1:1/cb",
            Duration::from_secs(5),
        )
        .await
        .expect_err("应报错");
        let text = format!("{error} {error:?}");
        assert!(!text.contains("leaked-token-value"), "错误里不能出现令牌原文");
    }

    #[tokio::test]
    async fn 调试输出隐藏令牌原文() {
        let tokens = TokenSet {
            access_token: "at-secret".to_string(),
            refresh_token: Some("rt-secret".to_string()),
            expires_at: Some(1),
            scope: None,
        };
        let text = format!("{tokens:?}");
        assert!(!text.contains("at-secret"));
        assert!(!text.contains("rt-secret"));
        assert!(text.contains("<隐藏>"));
    }

    #[test]
    fn 到期判断带提前量() {
        let tokens = TokenSet {
            access_token: "at".to_string(),
            refresh_token: None,
            expires_at: Some(1000),
            scope: None,
        };
        assert!(tokens.is_expired(1000, 0));
        assert!(tokens.is_expired(950, 60), "提前 60 秒就该判到期");
        assert!(!tokens.is_expired(900, 60));
    }

    #[test]
    fn 没有到期时间时保守地视为未过期() {
        let tokens = TokenSet {
            access_token: "at".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        };
        assert!(!tokens.is_expired(i64::MAX, 0));
    }
}
