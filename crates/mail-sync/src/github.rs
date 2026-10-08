//! GitHub 设备码登录与用户资料。
//!
//! 设备码流程（规格 D8）：程序先要一组「设备码 + 用户码」，把用户码显示给用户、
//! 让他去浏览器确认；程序拿设备码按间隔轮询，换到访问令牌为止。
//!
//! 规矩：设备码与访问令牌都只经内存回到上层，由上层写系统保险箱；日志只记状态不记令牌。
//! [`DeviceCode`] 的 `Debug` 刻意把设备码本体掩掉。

use std::time::Duration;

use mail_domain::proxy::ProxyRoute;

use crate::error::SyncError;
use crate::http::{self, HttpRequest, Method};

/// 内置的 GitHub 授权应用客户端编号（OAuth App；设备码流程不需要 secret）。
///
/// 用户可以在设置里填自己的编号覆盖它。
pub const DEFAULT_CLIENT_ID: &str = "Ov23li0xyhQRo7UJipmD";

/// GitHub 要求自报家门，缺了用户代理会被接口拒掉。
pub const USER_AGENT: &str = "Y-Mail";

/// 设备码换令牌用的授权类型。
pub const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// 登录（只为头像与昵称）够用的权限。
pub const SCOPE_LOGIN: &[&str] = &["read:user"];

/// 开启设置同步时再要的一次权限（读用户 + 管 Gist）。
pub const SCOPE_SYNC: &[&str] = &["read:user", "gist"];

/// 轮询换令牌时，单次请求的超时。
const POLL_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// GitHub 的三个接口地址。
///
/// 做成结构体是为了测试能指向本机假服务器；生产用 [`Endpoints::github`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// 申请设备码。
    pub device_code: String,
    /// 轮询换访问令牌。
    pub access_token: String,
    /// 取登录用户资料。
    pub user: String,
}

impl Endpoints {
    /// GitHub 的正式地址。
    pub fn github() -> Self {
        Self {
            device_code: "https://github.com/login/device/code".to_string(),
            access_token: "https://github.com/login/oauth/access_token".to_string(),
            user: "https://api.github.com/user".to_string(),
        }
    }
}

/// 一组设备码与用户码。
///
/// 用户码是要显示给用户的那串字母数字；设备码是程序轮询用的凭据，不外露。
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceCode {
    /// 程序轮询用的设备码（敏感，不进日志）。
    pub device_code: String,
    /// 显示给用户输入的短码。
    pub user_code: String,
    /// 让用户打开的确认页地址。
    pub verification_uri: String,
    /// 这组码的有效秒数。
    pub expires_in: u64,
    /// GitHub 要求的轮询间隔（秒）。
    pub interval: u64,
}

impl std::fmt::Debug for DeviceCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceCode")
            .field("device_code", &"***")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

/// 登录用户的资料：登录名、昵称、头像地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubProfile {
    /// 登录名（账号名）。
    pub login: String,
    /// 昵称；用户可以没设。
    pub name: Option<String>,
    /// 头像地址；用户可以没设。
    pub avatar_url: Option<String>,
}

impl GitHubProfile {
    /// 界面上显示时优先用昵称，没有就退回登录名。
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&self.login)
    }

    /// 登录名非空才算真拿到了资料。
    pub fn is_authenticated(&self) -> bool {
        !self.login.trim().is_empty()
    }

    /// 从接口返回的 JSON 取字段；缺字段就用兜底值，不因一个空昵称整段失败。
    fn from_json(value: &serde_json::Value) -> Self {
        let text = |key: &str| {
            value
                .get(key)
                .and_then(|field| field.as_str())
                .map(str::trim)
                .filter(|field| !field.is_empty())
                .map(str::to_string)
        };
        Self {
            login: text("login").unwrap_or_default(),
            name: text("name"),
            avatar_url: text("avatar_url"),
        }
    }
}

/// 向 GitHub 申请一组设备码与用户码。
///
/// `scope` 用 [`SCOPE_LOGIN`]（只要头像昵称）或 [`SCOPE_SYNC`]（再加管 Gist）。
pub async fn request_device_code(
    route: Option<&ProxyRoute>,
    endpoints: &Endpoints,
    client_id: &str,
    scope: &[&str],
    timeout: Duration,
) -> Result<DeviceCode, SyncError> {
    let scope_text = scope.join(" ");
    let request = HttpRequest::new(Method::Post, endpoints.device_code.clone())
        .with_header("Accept", "application/json")
        .with_header("User-Agent", USER_AGENT)
        .with_form(&[("client_id", client_id), ("scope", scope_text.as_str())]);
    let response = http::send(route, &request, timeout).await?;
    if !response.is_success() {
        return Err(SyncError::Http {
            status: response.status,
        });
    }
    let value: serde_json::Value = response.json()?;
    let device_code = text_field(&value, "device_code")?;
    let user_code = text_field(&value, "user_code").unwrap_or_default();
    let verification_uri = text_field(&value, "verification_uri")
        .or_else(|_| text_field(&value, "verification_uri_complete"))
        .unwrap_or_default();
    Ok(DeviceCode {
        device_code: device_code.to_string(),
        user_code: user_code.to_string(),
        verification_uri: verification_uri.to_string(),
        expires_in: value
            .get("expires_in")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(900),
        interval: value
            .get("interval")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(5),
    })
}

/// 轮询一次，看用户确认了没有。
///
/// 返回访问令牌；「还没确认」「慢一点」这两种情况用对应的 [`SyncError`] 表达，
/// 交给 [`poll_for_token`] 决定要不要接着等。
pub async fn poll_token_once(
    route: Option<&ProxyRoute>,
    endpoints: &Endpoints,
    client_id: &str,
    device_code: &str,
    timeout: Duration,
) -> Result<String, SyncError> {
    let request = HttpRequest::new(Method::Post, endpoints.access_token.clone())
        .with_header("Accept", "application/json")
        .with_header("User-Agent", USER_AGENT)
        .with_form(&[
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", DEVICE_GRANT_TYPE),
        ]);
    let response = http::send(route, &request, timeout).await?;
    if !response.is_success() {
        return Err(SyncError::Http {
            status: response.status,
        });
    }
    let value: serde_json::Value = response.json()?;
    if let Some(code) = value.get("error").and_then(|field| field.as_str()) {
        return Err(map_token_error(code));
    }
    match text_field(&value, "access_token") {
        Ok(token) => Ok(token.to_string()),
        Err(_) => Err(SyncError::Protocol),
    }
}

/// 轮询换令牌的节奏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollOptions {
    /// 两次轮询之间等多久。
    pub interval: Duration,
    /// GitHub 说「慢一点」时，间隔再加多少（官方要求至少 5 秒）。
    pub slow_down_penalty: Duration,
    /// 整体最多等多久。
    pub budget: Duration,
}

impl Default for PollOptions {
    /// GitHub 给的常规节奏：5 秒一次，最多等 15 分钟。
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(5),
            slow_down_penalty: Duration::from_secs(5),
            budget: Duration::from_secs(900),
        }
    }
}

/// 按节奏反复轮询，直到拿到令牌或撞上整体时限。
///
/// 「慢一点」会把间隔拉长；「已过期」「用户拒绝」立刻返回，不再等。
pub async fn poll_for_token(
    route: Option<&ProxyRoute>,
    endpoints: &Endpoints,
    client_id: &str,
    device_code: &str,
    options: PollOptions,
) -> Result<String, SyncError> {
    let started = tokio::time::Instant::now();
    // 间隔为 0 会变成忙等，兜一个下限。
    let mut interval = if options.interval.is_zero() {
        Duration::from_secs(1)
    } else {
        options.interval
    };
    loop {
        match poll_token_once(route, endpoints, client_id, device_code, POLL_REQUEST_TIMEOUT).await {
            Ok(token) => return Ok(token),
            Err(SyncError::AuthorizationPending) => {}
            Err(SyncError::SlowDown) => {
                interval = interval.saturating_add(options.slow_down_penalty);
            }
            Err(other) => return Err(other),
        }
        if started.elapsed() >= options.budget {
            return Err(SyncError::Timeout);
        }
        tokio::time::sleep(interval).await;
    }
}

/// 拿令牌取一次登录用户的资料。
pub async fn fetch_profile(
    route: Option<&ProxyRoute>,
    endpoints: &Endpoints,
    token: &str,
    timeout: Duration,
) -> Result<GitHubProfile, SyncError> {
    let request = HttpRequest::get(endpoints.user.clone())
        .with_header("Accept", "application/vnd.github+json")
        .with_header("User-Agent", USER_AGENT)
        .with_bearer(token);
    let response = http::send(route, &request, timeout).await?;
    if !response.is_success() {
        return Err(SyncError::Http {
            status: response.status,
        });
    }
    let value: serde_json::Value = response.json()?;
    Ok(GitHubProfile::from_json(&value))
}

/// 把 GitHub 的令牌接口错误码翻成我们的错误。
fn map_token_error(code: &str) -> SyncError {
    match code {
        "authorization_pending" => SyncError::AuthorizationPending,
        "slow_down" => SyncError::SlowDown,
        "expired_token" => SyncError::DeviceCodeExpired,
        "access_denied" => SyncError::AccessDenied,
        _ => SyncError::Protocol,
    }
}

/// 取一个非空的字符串字段。
fn text_field<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str, SyncError> {
    value
        .get(key)
        .and_then(|field| field.as_str())
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .ok_or(SyncError::Protocol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// 起一个按剧本逐个回应的假服务器；返回端口与「收到的请求」任务句柄。
    async fn spawn_script(responses: Vec<Vec<u8>>) -> (u16, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定成功");
        let port = listener.local_addr().expect("取地址").port();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut socket, _) = listener.accept().await.expect("接受连接");
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let read = socket.read(&mut chunk).await.expect("读取请求");
                    if read == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..read]);
                    if request_complete(&buf) {
                        break;
                    }
                }
                socket.write_all(&response).await.expect("写响应");
                let _ = socket.shutdown().await;
                requests.push(String::from_utf8_lossy(&buf).to_string());
            }
            requests
        });
        (port, handle)
    }

    fn request_complete(buf: &[u8]) -> bool {
        let Some(split) = buf.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let head = String::from_utf8_lossy(&buf[..split]);
        let length = head
            .lines()
            .find_map(|line| {
                let lower = line.to_ascii_lowercase();
                lower
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap_or(0))
            })
            .unwrap_or(0);
        buf.len() >= split + 4 + length
    }

    /// 拼一个 200 响应。
    fn json_response(body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    /// 拼一个错误响应。
    fn status_response(status: u16, reason: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn endpoints(port: u16) -> Endpoints {
        Endpoints {
            device_code: format!("http://127.0.0.1:{port}/login/device/code"),
            access_token: format!("http://127.0.0.1:{port}/login/oauth/access_token"),
            user: format!("http://127.0.0.1:{port}/user"),
        }
    }

    #[tokio::test]
    async fn 设备码请求带上编号与权限并解析字段() {
        let body = r#"{"device_code":"dev-123","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#;
        let (port, server) = spawn_script(vec![json_response(body)]).await;
        let code = request_device_code(
            None,
            &endpoints(port),
            "client-1",
            SCOPE_SYNC,
            Duration::from_secs(5),
        )
        .await
        .expect("拿到设备码");
        assert_eq!(code.device_code, "dev-123");
        assert_eq!(code.user_code, "ABCD-1234");
        assert_eq!(code.expires_in, 900);
        assert_eq!(code.interval, 5);

        let requests = server.await.expect("服务器任务");
        let raw = &requests[0];
        assert!(
            raw.starts_with("POST /login/device/code HTTP/1.1\r\n"),
            "实际：{raw}"
        );
        assert!(raw.contains("client_id=client-1"));
        assert!(raw.contains("scope=read%3Auser+gist"), "实际：{raw}");
        assert!(raw.contains("User-Agent: Y-Mail\r\n"));
    }

    #[tokio::test]
    async fn 设备码非两百直接报状态码() {
        let body = r#"{"message":"Bad credentials"}"#;
        let (port, _server) = spawn_script(vec![status_response(401, "Unauthorized", body)]).await;
        let error = request_device_code(
            None,
            &endpoints(port),
            "client-1",
            SCOPE_LOGIN,
            Duration::from_secs(5),
        )
        .await
        .expect_err("应当报错");
        assert!(matches!(error, SyncError::Http { status: 401 }));
    }

    #[tokio::test]
    async fn 轮询等待后成功拿到令牌() {
        let pending = json_response(r#"{"error":"authorization_pending"}"#);
        let success =
            json_response(r#"{"access_token":"gho_token_abc","token_type":"bearer","scope":"gist"}"#);
        let (port, server) = spawn_script(vec![pending, success]).await;
        let token = poll_for_token(
            None,
            &endpoints(port),
            "client-1",
            "dev-123",
            PollOptions {
                interval: Duration::from_millis(10),
                slow_down_penalty: Duration::from_millis(1),
                budget: Duration::from_secs(5),
            },
        )
        .await
        .expect("换到令牌");
        assert_eq!(token, "gho_token_abc");
        let requests = server.await.expect("服务器任务");
        assert_eq!(requests.len(), 2, "应当轮询了两次");
    }

    #[tokio::test]
    async fn 慢一点会拉长间隔但继续等() {
        let slow = json_response(r#"{"error":"slow_down"}"#);
        let success = json_response(r#"{"access_token":"gho_token_xyz"}"#);
        let (port, server) = spawn_script(vec![slow, success]).await;
        let token = poll_for_token(
            None,
            &endpoints(port),
            "client-1",
            "dev-123",
            PollOptions {
                interval: Duration::from_millis(10),
                slow_down_penalty: Duration::from_millis(1),
                budget: Duration::from_secs(5),
            },
        )
        .await
        .expect("换到令牌");
        assert_eq!(token, "gho_token_xyz");
        let requests = server.await.expect("服务器任务");
        assert_eq!(requests.len(), 2);
    }

    #[tokio::test]
    async fn 设备码过期给出对应提示() {
        let expired = json_response(r#"{"error":"expired_token"}"#);
        let (port, _server) = spawn_script(vec![expired]).await;
        let error = poll_token_once(
            None,
            &endpoints(port),
            "client-1",
            "dev-123",
            Duration::from_secs(5),
        )
        .await
        .expect_err("应当报错");
        assert!(matches!(error, SyncError::DeviceCodeExpired));
        assert_eq!(error.to_string(), "设备码已过期，请重新发起登录");
    }

    #[tokio::test]
    async fn 用户拒绝给出对应提示() {
        let denied = json_response(r#"{"error":"access_denied"}"#);
        let (port, _server) = spawn_script(vec![denied]).await;
        let error = poll_token_once(
            None,
            &endpoints(port),
            "client-1",
            "dev-123",
            Duration::from_secs(5),
        )
        .await
        .expect_err("应当报错");
        assert!(matches!(error, SyncError::AccessDenied));
        assert_eq!(error.to_string(), "授权被拒绝");
    }

    #[tokio::test]
    async fn 没确认时单次轮询报等待中() {
        let pending = json_response(r#"{"error":"authorization_pending"}"#);
        let (port, _server) = spawn_script(vec![pending]).await;
        let error = poll_token_once(
            None,
            &endpoints(port),
            "client-1",
            "dev-123",
            Duration::from_secs(5),
        )
        .await
        .expect_err("应当报等待");
        assert!(matches!(error, SyncError::AuthorizationPending));
    }

    #[tokio::test]
    async fn 整段时间到点就放弃() {
        let pending = json_response(r#"{"error":"authorization_pending"}"#);
        let (port, _server) = spawn_script(vec![pending.clone(), pending]).await;
        let error = poll_for_token(
            None,
            &endpoints(port),
            "client-1",
            "dev-123",
            PollOptions {
                interval: Duration::from_millis(1),
                slow_down_penalty: Duration::from_millis(1),
                budget: Duration::ZERO,
            },
        )
        .await
        .expect_err("应当超时");
        assert!(matches!(error, SyncError::Timeout));
    }

    #[tokio::test]
    async fn 取资料解析完整字段() {
        let body = r#"{"login":"octocat","name":"The Octocat","avatar_url":"https://avatars.githubusercontent.com/u/1?v=4"}"#;
        let (port, server) = spawn_script(vec![json_response(body)]).await;
        let profile = fetch_profile(None, &endpoints(port), "gho_token_abc", Duration::from_secs(5))
            .await
            .expect("拿到资料");
        assert_eq!(profile.login, "octocat");
        assert_eq!(profile.name.as_deref(), Some("The Octocat"));
        assert_eq!(profile.display_name(), "The Octocat");

        let requests = server.await.expect("服务器任务");
        let raw = &requests[0];
        assert!(raw.starts_with("GET /user HTTP/1.1\r\n"), "实际：{raw}");
        assert!(raw.contains("Authorization: Bearer gho_token_abc\r\n"));
        assert!(raw.contains("Accept: application/vnd.github+json\r\n"));
    }

    #[tokio::test]
    async fn 取资料缺昵称与头像时退回登录名() {
        let body = r#"{"login":"octocat"}"#;
        let (port, _server) = spawn_script(vec![json_response(body)]).await;
        let profile = fetch_profile(None, &endpoints(port), "gho_token_abc", Duration::from_secs(5))
            .await
            .expect("拿到资料");
        assert_eq!(profile.login, "octocat");
        assert!(profile.name.is_none());
        assert!(profile.avatar_url.is_none());
        assert_eq!(profile.display_name(), "octocat");
        assert!(profile.is_authenticated());
    }

    #[tokio::test]
    async fn 取资料空昵称也退回登录名() {
        let body = r#"{"login":"octocat","name":"   ","avatar_url":""}"#;
        let (port, _server) = spawn_script(vec![json_response(body)]).await;
        let profile = fetch_profile(None, &endpoints(port), "gho_token_abc", Duration::from_secs(5))
            .await
            .expect("拿到资料");
        assert!(profile.name.is_none());
        assert!(profile.avatar_url.is_none());
        assert_eq!(profile.display_name(), "octocat");
    }

    #[tokio::test]
    async fn 取资料失败时错误里不带令牌() {
        let body = r#"{"message":"Bad credentials"}"#;
        let (port, _server) = spawn_script(vec![status_response(401, "Unauthorized", body)]).await;
        let error = fetch_profile(None, &endpoints(port), "gho_super_secret", Duration::from_secs(5))
            .await
            .expect_err("应当报错");
        let text = format!("{error} {error:?}");
        assert!(!text.contains("gho_super_secret"), "错误里泄漏了令牌：{text}");
    }

    #[test]
    fn 设备码调试输出掩掉设备码本体() {
        let code = DeviceCode {
            device_code: "dev-should-not-show".to_string(),
            user_code: "ABCD-1234".to_string(),
            verification_uri: "https://github.com/login/device".to_string(),
            expires_in: 900,
            interval: 5,
        };
        let debug = format!("{code:?}");
        assert!(
            !debug.contains("dev-should-not-show"),
            "调试输出泄漏设备码：{debug}"
        );
        assert!(debug.contains("ABCD-1234"));
    }

    #[test]
    fn 令牌错误码映射齐全() {
        assert!(matches!(
            map_token_error("authorization_pending"),
            SyncError::AuthorizationPending
        ));
        assert!(matches!(map_token_error("slow_down"), SyncError::SlowDown));
        assert!(matches!(
            map_token_error("expired_token"),
            SyncError::DeviceCodeExpired
        ));
        assert!(matches!(
            map_token_error("access_denied"),
            SyncError::AccessDenied
        ));
        assert!(matches!(map_token_error("whatever"), SyncError::Protocol));
    }
}
