//! 最小 HTTP 客户端：只给 GitHub 与 Gist 用。
//!
//! 传输复用 `mail-net`，因此代理分层（账号级 > 全局自定义 > 系统代理）与收发邮件、
//! AI 外发走的是同一套选路逻辑。
//!
//! 规矩：请求头（尤其是授权头）绝不进日志，所以 [`HttpRequest`] 的 `Debug` 只打印
//! 头名不打印头值；错误信息不带请求头与响应正文。

use std::time::Duration;

use mail_domain::proxy::ProxyRoute;
use mail_net::Stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use crate::error::SyncError;

/// HTTP 请求方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// 读取。
    Get,
    /// 新建。
    Post,
    /// 局部更新。
    Patch,
    /// 删除。
    Delete,
}

impl Method {
    /// 拼请求行用的方法名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

/// 一个待发送的请求。
///
/// 刻意不派生 `Debug`：头里的授权令牌不能随调试输出泄漏。
#[derive(Clone)]
pub struct HttpRequest {
    /// 请求方法。
    pub method: Method,
    /// 完整地址（含协议、主机、路径与查询串）。
    pub url: String,
    /// 自定义请求头，按加入顺序排在 `Host` 之后。
    pub headers: Vec<(String, String)>,
    /// 请求体；`None` 表示没有正文。
    pub body: Option<Vec<u8>>,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("header_names", &names)
            .field("body_len", &self.body.as_ref().map(Vec::len))
            .finish()
    }
}

impl HttpRequest {
    /// 建一个空请求。
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// `GET` 请求。
    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::Get, url)
    }

    /// `POST`，正文是 JSON，自动带上 `Content-Type`。
    pub fn post_json(url: impl Into<String>, body: &serde_json::Value) -> Result<Self, SyncError> {
        Self::json_body(Method::Post, url, body)
    }

    /// `PATCH`，正文是 JSON，自动带上 `Content-Type`。
    pub fn patch_json(url: impl Into<String>, body: &serde_json::Value) -> Result<Self, SyncError> {
        Self::json_body(Method::Patch, url, body)
    }

    /// `DELETE` 请求。
    pub fn delete(url: impl Into<String>) -> Self {
        Self::new(Method::Delete, url)
    }

    fn json_body(
        method: Method,
        url: impl Into<String>,
        body: &serde_json::Value,
    ) -> Result<Self, SyncError> {
        let bytes = serde_json::to_vec(body).map_err(|_| SyncError::Malformed)?;
        Ok(Self::new(method, url)
            .with_header("Content-Type", "application/json")
            .with_body(bytes))
    }

    /// 挂一个自定义请求头。
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// 挂上 `Authorization: Bearer <令牌>`。
    pub fn with_bearer(self, token: &str) -> Self {
        self.with_header("Authorization", format!("Bearer {token}"))
    }

    /// 设置请求体。
    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }

    /// 把表单字段编码成 `application/x-www-form-urlencoded` 请求体。
    pub fn with_form(self, pairs: &[(&str, &str)]) -> Self {
        let body = form_encode(pairs);
        self.with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_body(body.into_bytes())
    }
}

/// 把键值对按表单规则编码。
pub fn form_encode(pairs: &[(&str, &str)]) -> String {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.extend_pairs(pairs.iter().copied());
    serializer.finish()
}

/// 响应：状态码加原始正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP 状态码。
    pub status: u16,
    /// 原始响应正文。
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// 状态码是不是 2xx。
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// 正文按 UTF-8 宽松解码成字符串。
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// 正文按 JSON 解析。
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, SyncError> {
        serde_json::from_slice(&self.body).map_err(|_| SyncError::Protocol)
    }
}

/// 发一个请求，拿回状态码与正文。
///
/// 非 2xx 不算传输失败——状态码原样交给上层判断（GitHub 的错误码常放在响应体里）。
pub async fn send(
    route: Option<&ProxyRoute>,
    request: &HttpRequest,
    timeout: Duration,
) -> Result<HttpResponse, SyncError> {
    let target = Target::parse(&request.url)?;
    let raw = build_request(request, &target)?;

    let exchange = async {
        let tcp = mail_net::connect_tcp(&target.host, target.port, route, timeout)
            .await
            .map_err(SyncError::Network)?;
        let mut stream = if target.secure {
            mail_net::tls_wrap(tcp, &target.host)
                .await
                .map_err(SyncError::Network)?
        } else {
            Stream::plain(tcp)
        };
        stream.write_all(&raw).await.map_err(|_| SyncError::Protocol)?;
        stream.flush().await.map_err(|_| SyncError::Protocol)?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .map_err(|_| SyncError::Protocol)?;
        Ok::<Vec<u8>, SyncError>(response)
    };

    let response = tokio::time::timeout(timeout, exchange)
        .await
        .map_err(|_| SyncError::Timeout)??;
    parse_response(&response)
}

/// 解析出来的目标地址。
struct Target {
    host: String,
    port: u16,
    secure: bool,
    path: String,
}

impl Target {
    fn parse(url: &str) -> Result<Self, SyncError> {
        let parsed = Url::parse(url).map_err(|_| SyncError::Config("请求地址无效".to_string()))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| SyncError::Config("请求地址缺少主机名".to_string()))?
            .to_string();
        let secure = match parsed.scheme() {
            "https" => true,
            "http" => false,
            _ => return Err(SyncError::Config("请求地址协议不受支持".to_string())),
        };
        let port = parsed
            .port_or_known_default()
            .ok_or_else(|| SyncError::Config("请求地址缺少端口".to_string()))?;
        let mut path = if parsed.path().is_empty() {
            "/".to_string()
        } else {
            parsed.path().to_string()
        };
        if let Some(query) = parsed.query() {
            path.push('?');
            path.push_str(query);
        }
        Ok(Self {
            host,
            port,
            secure,
            path,
        })
    }

    /// 首部 `Host` 的取值：非默认端口才带上端口号。
    fn host_header(&self) -> String {
        let default = if self.secure { 443 } else { 80 };
        if self.port == default {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// 把请求拼成 HTTP/1.1 报文字节。
fn build_request(request: &HttpRequest, target: &Target) -> Result<Vec<u8>, SyncError> {
    let body = request.body.as_deref().unwrap_or(&[]);
    let mut head = String::new();
    head.push_str(request.method.as_str());
    head.push(' ');
    head.push_str(&target.path);
    head.push_str(" HTTP/1.1\r\n");
    head.push_str(&format!("Host: {}\r\n", target.host_header()));
    for (name, value) in &request.headers {
        // 头名与头值里出现换行就是请求头注入，直接拒绝。
        if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
            return Err(SyncError::Config("请求头不合法".to_string()));
        }
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    let has_length = request
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
    if !has_length {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("Accept-Encoding: identity\r\nConnection: close\r\n\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    Ok(out)
}

/// 拆状态行、头部与正文；正文支持定长与分块两种传输方式。
fn parse_response(raw: &[u8]) -> Result<HttpResponse, SyncError> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(SyncError::Protocol)?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or(SyncError::Protocol)?;
    let status = parse_status(status_line)?;
    let mut chunked = false;
    let mut content_length: Option<usize> = None;
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("transfer-encoding:") {
            if rest.contains("chunked") {
                chunked = true;
            }
        } else if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
    }
    let raw_body = &raw[split + 4..];
    let body = if chunked {
        dechunk(raw_body)?
    } else if let Some(len) = content_length {
        raw_body[..len.min(raw_body.len())].to_vec()
    } else {
        raw_body.to_vec()
    };
    Ok(HttpResponse { status, body })
}

/// 从状态行里取状态码。
fn parse_status(line: &str) -> Result<u16, SyncError> {
    if !line.starts_with("HTTP/") {
        return Err(SyncError::Protocol);
    }
    line.split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(SyncError::Protocol)
}

/// 解分块传输编码。
fn dechunk(mut body: &[u8]) -> Result<Vec<u8>, SyncError> {
    let mut out = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or(SyncError::Protocol)?;
        let size_text = String::from_utf8_lossy(&body[..line_end]);
        let size = usize::from_str_radix(size_text.trim().split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| SyncError::Protocol)?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if body.len() < size + 2 {
            return Err(SyncError::Protocol);
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// 起一个只服务一次的假服务器；返回端口与「收全请求后」的任务句柄。
    async fn spawn_server(response: Vec<u8>) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定成功");
        let port = listener.local_addr().expect("取地址").port();
        let handle = tokio::spawn(async move {
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
            String::from_utf8_lossy(&buf).to_string()
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

    fn ok_response(body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn get请求行与自定义头拼装正确() {
        let (port, server) = spawn_server(ok_response("{}")).await;
        let url = format!("http://127.0.0.1:{port}/user");
        let request = HttpRequest::get(url).with_header("Accept", "application/vnd.github+json");
        let response = send(None, &request, Duration::from_secs(5))
            .await
            .expect("请求成功");
        assert_eq!(response.status, 200);
        assert_eq!(response.body_text(), "{}");

        let raw = server.await.expect("服务器任务");
        assert!(raw.starts_with("GET /user HTTP/1.1\r\n"), "实际：{raw}");
        assert!(raw.contains(&format!("Host: 127.0.0.1:{port}\r\n")));
        assert!(raw.contains("Accept: application/vnd.github+json\r\n"));
        assert!(raw.contains("Connection: close\r\n"));
    }

    #[tokio::test]
    async fn post_json带正文与内容类型() {
        let (port, server) = spawn_server(ok_response("{}")).await;
        let url = format!("http://127.0.0.1:{port}/gists");
        let body = serde_json::json!({ "public": false, "files": { "ymail-sync.json": { "content": "x" } } });
        let request = HttpRequest::post_json(url, &body).expect("拼请求");
        send(None, &request, Duration::from_secs(5))
            .await
            .expect("请求成功");

        let raw = server.await.expect("服务器任务");
        assert!(raw.starts_with("POST /gists HTTP/1.1\r\n"), "实际：{raw}");
        assert!(raw.contains("Content-Type: application/json\r\n"));
        let (_, payload) = raw.split_once("\r\n\r\n").expect("有正文");
        let parsed: serde_json::Value = serde_json::from_str(payload).expect("正文是 JSON");
        assert_eq!(parsed["public"], serde_json::json!(false));
    }

    #[tokio::test]
    async fn patch与delete方法拼装正确() {
        let (port, server) = spawn_server(ok_response("{}")).await;
        let url = format!("http://127.0.0.1:{port}/gists/1");
        let body = serde_json::json!({ "description": "改" });
        let request = HttpRequest::patch_json(url, &body).expect("拼请求");
        send(None, &request, Duration::from_secs(5))
            .await
            .expect("请求成功");
        let raw = server.await.expect("服务器任务");
        assert!(raw.starts_with("PATCH /gists/1 HTTP/1.1\r\n"), "实际：{raw}");

        let (port, server) = spawn_server(ok_response("")).await;
        let url = format!("http://127.0.0.1:{port}/gists/1");
        let request = HttpRequest::delete(url);
        send(None, &request, Duration::from_secs(5))
            .await
            .expect("请求成功");
        let raw = server.await.expect("服务器任务");
        assert!(raw.starts_with("DELETE /gists/1 HTTP/1.1\r\n"), "实际：{raw}");
    }

    #[tokio::test]
    async fn 查询串会带进请求行() {
        let (port, server) = spawn_server(ok_response("[]")).await;
        let url = format!("http://127.0.0.1:{port}/gists?per_page=100&page=2");
        let request = HttpRequest::get(url);
        send(None, &request, Duration::from_secs(5))
            .await
            .expect("请求成功");
        let raw = server.await.expect("服务器任务");
        assert!(
            raw.starts_with("GET /gists?per_page=100&page=2 HTTP/1.1\r\n"),
            "实际：{raw}"
        );
    }

    #[tokio::test]
    async fn 四零四能取回状态码与正文() {
        let body = r#"{"message":"Not Found"}"#;
        let response = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (port, _server) = spawn_server(response.into_bytes()).await;
        let url = format!("http://127.0.0.1:{port}/missing");
        let response = send(None, &HttpRequest::get(url), Duration::from_secs(5))
            .await
            .expect("传输成功");
        assert_eq!(response.status, 404);
        assert!(!response.is_success());
        assert!(response.body_text().contains("Not Found"));
    }

    #[tokio::test]
    async fn 五零零能取回状态码与正文() {
        let body = "server exploded";
        let response = format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (port, _server) = spawn_server(response.into_bytes()).await;
        let url = format!("http://127.0.0.1:{port}/boom");
        let response = send(None, &HttpRequest::get(url), Duration::from_secs(5))
            .await
            .expect("传输成功");
        assert_eq!(response.status, 500);
        assert_eq!(response.body_text(), "server exploded");
    }

    #[test]
    fn 定长响应按长度截取() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}tail";
        let response = parse_response(raw).expect("可解析");
        assert_eq!(response.status, 200);
        assert_eq!(response.body_text(), "{}");
    }

    #[test]
    fn 分块响应能还原() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n";
        let response = parse_response(raw).expect("可解析");
        assert_eq!(response.body_text(), "{}");
    }

    #[test]
    fn 缺少响应头分隔符直接报错() {
        assert!(parse_response(b"HTTP/1.1 200 OK").is_err());
    }

    #[test]
    fn 状态行不对直接报错() {
        assert!(parse_response(b"200 OK\r\n\r\n").is_err());
    }

    #[test]
    fn 请求头注入被拒绝() {
        let target = Target::parse("https://api.github.com/user").expect("解析地址");
        let request = HttpRequest::get("https://api.github.com/user").with_header("X-Bad", "a\r\nX-Evil: 1");
        assert!(build_request(&request, &target).is_err());
    }

    #[test]
    fn 调试输出不泄露授权头() {
        let request = HttpRequest::get("https://api.github.com/user").with_bearer("gho_secret_value");
        let debug = format!("{request:?}");
        assert!(!debug.contains("gho_secret_value"), "调试输出泄漏了令牌：{debug}");
        assert!(debug.contains("Authorization"), "头名应当保留：{debug}");
    }

    #[test]
    fn 表单字段被正确编码() {
        assert_eq!(
            form_encode(&[("scope", "read:user gist"), ("client_id", "a b")]),
            "scope=read%3Auser+gist&client_id=a+b"
        );
    }

    #[test]
    fn 主机首部非默认端口才带端口() {
        let secure = Target::parse("https://api.github.com/user").expect("解析");
        assert_eq!(secure.host_header(), "api.github.com");
        let custom = Target::parse("http://127.0.0.1:8080/x").expect("解析");
        assert_eq!(custom.host_header(), "127.0.0.1:8080");
    }
}
