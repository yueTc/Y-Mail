//! 授权流程：拼授权地址、在本机开回环端口等回调。

use std::fmt;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use crate::error::OAuthError;
use crate::pkce::{generate_pkce, random_urlsafe, Pkce};
use crate::provider::{provider_meta, ProviderKind};

/// 本机回调使用的固定路径。
pub const CALLBACK_PATH: &str = "/oauth/callback";

/// 一次授权请求：浏览器该打开的地址 + 本地校验材料。
pub struct AuthorizeRequest {
    /// 用系统浏览器打开的授权页地址。
    pub url: String,
    /// 回调校验用的随机串。
    pub state: String,
    /// PKCE 材料，换取授权码时要用 verifier。
    pub pkce: Pkce,
    /// 回调地址，必须与授权码换令牌时完全一致。
    pub redirect_uri: String,
}

impl fmt::Debug for AuthorizeRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 授权地址里带着 state，调试输出里把它抹掉。
        let masked_url = self.url.replace(&self.state, "<隐藏>");
        f.debug_struct("AuthorizeRequest")
            .field("url", &masked_url)
            .field("state", &"<隐藏>")
            .field("pkce", &self.pkce)
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

/// 生成授权地址；`client_id` 由用户配置，不能为空。
pub fn build_authorize_request(
    kind: ProviderKind,
    client_id: &str,
    port: u16,
    login_hint: Option<&str>,
) -> Result<AuthorizeRequest, OAuthError> {
    let client_id = client_id.trim();
    if client_id.is_empty() {
        return Err(OAuthError::Config("缺少 client_id"));
    }
    if port == 0 {
        return Err(OAuthError::Config("回调端口无效"));
    }
    let pkce = generate_pkce()?;
    let state = random_urlsafe(32)?;
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let meta = provider_meta(kind);
    let mut url = Url::parse(meta.auth_endpoint).map_err(|_| OAuthError::Config("授权地址无效"))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("client_id", client_id);
        query.append_pair("response_type", "code");
        query.append_pair("redirect_uri", &redirect_uri);
        query.append_pair("scope", &meta.scopes.join(" "));
        query.append_pair("state", &state);
        query.append_pair("code_challenge", &pkce.challenge);
        query.append_pair("code_challenge_method", "S256");
        // 谷歌默认不给刷新令牌，必须显式要求离线访问并强制同意页。
        if kind == ProviderKind::Gmail {
            query.append_pair("access_type", "offline");
            query.append_pair("prompt", "consent");
        }
        if let Some(hint) = login_hint.map(str::trim).filter(|hint| !hint.is_empty()) {
            query.append_pair("login_hint", hint);
        }
    }
    Ok(AuthorizeRequest {
        url: url.to_string(),
        state,
        pkce,
        redirect_uri,
    })
}

/// 已经绑好端口的本机回调服务。
pub struct Loopback {
    listener: TcpListener,
    /// 实际绑定的端口。
    pub port: u16,
}

impl fmt::Debug for Loopback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Loopback").field("port", &self.port).finish()
    }
}

/// 在 127.0.0.1 上绑一个随机端口。
pub async fn bind_loopback() -> Result<Loopback, OAuthError> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|_| OAuthError::Bind)?;
    let port = listener.local_addr().map_err(|_| OAuthError::Bind)?.port();
    Ok(Loopback { listener, port })
}

impl Loopback {
    /// 等一次回调：校验 state 后返回授权码；超时或状态不符都会拒绝。
    pub async fn wait_for_code(self, expected_state: &str, timeout: Duration) -> Result<String, OAuthError> {
        match tokio::time::timeout(timeout, self.accept_once(expected_state)).await {
            Ok(result) => result,
            Err(_) => Err(OAuthError::CallbackTimeout),
        }
    }

    async fn accept_once(self, expected_state: &str) -> Result<String, OAuthError> {
        loop {
            let (mut socket, _) = self.listener.accept().await.map_err(|_| OAuthError::Bind)?;
            let mut buf = vec![0u8; 8192];
            let read = match socket.read(&mut buf).await {
                Ok(read) => read,
                Err(_) => continue,
            };
            let request = String::from_utf8_lossy(&buf[..read]);
            let target = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            if !target.starts_with('/') {
                let _ = respond(&mut socket, PAGE_NOT_FOUND).await;
                continue;
            }
            let parsed = match Url::parse(&format!("http://127.0.0.1{target}")) {
                Ok(parsed) => parsed,
                Err(_) => {
                    let _ = respond(&mut socket, PAGE_NOT_FOUND).await;
                    continue;
                }
            };
            if parsed.path() != CALLBACK_PATH {
                let _ = respond(&mut socket, PAGE_NOT_FOUND).await;
                continue;
            }

            let mut code = None;
            let mut state = None;
            let mut error = None;
            for (key, value) in parsed.query_pairs() {
                match key.as_ref() {
                    "code" => code = Some(value.to_string()),
                    "state" => state = Some(value.to_string()),
                    "error" => error = Some(value.to_string()),
                    _ => {}
                }
            }
            let outcome = if let Some(reason) = error {
                Err(OAuthError::Denied(sanitize_reason(&reason)))
            } else if state.as_deref() != Some(expected_state) {
                Err(OAuthError::StateMismatch)
            } else {
                code.ok_or(OAuthError::Protocol)
            };
            let page = if outcome.is_ok() {
                PAGE_SUCCESS
            } else {
                PAGE_FAILURE
            };
            let _ = respond(&mut socket, page).await;
            return outcome;
        }
    }
}

/// 错误原因只保留短标识，避免把对方塞进来的长串带进界面。
fn sanitize_reason(reason: &str) -> String {
    let cleaned: String = reason
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .take(40)
        .collect();
    if cleaned.is_empty() {
        "未说明原因".to_string()
    } else {
        cleaned
    }
}

async fn respond(socket: &mut tokio::net::TcpStream, page: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await
}

const PAGE_SUCCESS: &str = "<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\"><title>授权成功</title><body><h1>授权成功</h1><p>可以回到客户端继续了，这个窗口可以直接关掉。</p></body></html>";
const PAGE_FAILURE: &str = "<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\"><title>授权未完成</title><body><h1>授权未完成</h1><p>请回到客户端重新发起授权，这个窗口可以直接关掉。</p></body></html>";
const PAGE_NOT_FOUND: &str = "<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\"><title>找不到页面</title><body><h1>找不到页面</h1></body></html>";

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpStream;

    #[test]
    fn 授权地址带齐必要参数且不含密钥() {
        let request = build_authorize_request(ProviderKind::Gmail, "client-123", 51234, Some("me@gmail.com"))
            .expect("参数齐全");
        assert!(request.url.contains("code_challenge_method=S256"));
        assert!(request.url.contains("code_challenge="));
        assert!(request.url.contains("access_type=offline"));
        assert!(request.url.contains("prompt=consent"));
        assert!(request.url.contains("login_hint=me%40gmail.com"));
        assert!(request
            .url
            .contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A51234%2Foauth%2Fcallback"));
        assert!(!request.url.contains("client_secret"));
        assert!(!request.url.contains(&request.pkce.verifier));
        assert_eq!(request.redirect_uri, "http://127.0.0.1:51234/oauth/callback");
    }

    #[test]
    fn 缺少客户端编号会被拦住() {
        let error = build_authorize_request(ProviderKind::Microsoft, "   ", 5000, None)
            .expect_err("空 client_id 应报错");
        assert!(error.to_string().contains("client_id"));
    }

    #[test]
    fn 微软授权地址不带谷歌专属参数() {
        let request =
            build_authorize_request(ProviderKind::Microsoft, "client-abc", 5001, None).expect("可生成");
        assert!(!request.url.contains("access_type=offline"));
        assert!(request.url.contains("offline_access"));
    }

    #[test]
    fn 调试输出隐藏授权校验串() {
        let request =
            build_authorize_request(ProviderKind::Gmail, "client-123", 51234, None).expect("可生成");
        let text = format!("{request:?}");
        assert!(!text.contains(&request.state));
        assert!(!text.contains(&request.pkce.verifier));
    }

    #[tokio::test]
    async fn 回调校验状态并返回授权码() {
        let server = bind_loopback().await.expect("绑定成功");
        let port = server.port;
        let waiting = tokio::spawn(server.wait_for_code("expected-state", Duration::from_secs(5)));
        let mut client = TcpStream::connect(("127.0.0.1", port)).await.expect("连接成功");
        client
            .write_all(
                b"GET /oauth/callback?code=abc123&state=expected-state HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            )
            .await
            .expect("写入成功");
        let code = waiting.await.expect("任务未崩溃").expect("拿到授权码");
        assert_eq!(code, "abc123");
    }

    #[tokio::test]
    async fn 状态不匹配会拒绝回调() {
        let server = bind_loopback().await.expect("绑定成功");
        let port = server.port;
        let waiting = tokio::spawn(server.wait_for_code("expected-state", Duration::from_secs(5)));
        let mut client = TcpStream::connect(("127.0.0.1", port)).await.expect("连接成功");
        client
            .write_all(b"GET /oauth/callback?code=abc123&state=forged HTTP/1.1\r\n\r\n")
            .await
            .expect("写入成功");
        let error = waiting.await.expect("任务未崩溃").expect_err("应拒绝");
        assert!(matches!(error, OAuthError::StateMismatch));
    }

    #[tokio::test]
    async fn 用户在浏览器拒绝时给出可读原因() {
        let server = bind_loopback().await.expect("绑定成功");
        let port = server.port;
        let waiting = tokio::spawn(server.wait_for_code("expected-state", Duration::from_secs(5)));
        let mut client = TcpStream::connect(("127.0.0.1", port)).await.expect("连接成功");
        client
            .write_all(b"GET /oauth/callback?error=access_denied&state=expected-state HTTP/1.1\r\n\r\n")
            .await
            .expect("写入成功");
        let error = waiting.await.expect("任务未崩溃").expect_err("应报拒绝");
        assert!(error.to_string().contains("access_denied"));
    }

    #[tokio::test]
    async fn 路径不对会跳过而不是直接失败() {
        let server = bind_loopback().await.expect("绑定成功");
        let port = server.port;
        let waiting = tokio::spawn(server.wait_for_code("right", Duration::from_secs(5)));
        let mut noise = TcpStream::connect(("127.0.0.1", port)).await.expect("连接成功");
        noise
            .write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n")
            .await
            .expect("写入成功");
        drop(noise);
        let mut client = TcpStream::connect(("127.0.0.1", port)).await.expect("连接成功");
        client
            .write_all(b"GET /oauth/callback?code=ok&state=right HTTP/1.1\r\n\r\n")
            .await
            .expect("写入成功");
        let code = waiting.await.expect("任务未崩溃").expect("拿到授权码");
        assert_eq!(code, "ok");
    }
}
