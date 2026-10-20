//! 最小 HTTP 客户端：只为 AI 站点调用服务。
//!
//! 传输复用 mail-net，因此代理分层（账号级 > 全局自定义 > 系统代理）与收发邮件、
//! OAuth 换令牌走的是同一套选路逻辑。
//!
//! 这里的实现刻意保持「一次请求一条连接」：发完就 `Connection: close`，
//! 省掉连接池与长连接状态机，调用量本来也不大。

use std::time::Duration;

use mail_domain::proxy::ProxyRoute;
use mail_net::Stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use crate::error::AiError;
use crate::provider::is_local_host;

/// 一次 HTTP 响应的状态码与正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HttpResponse {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应正文；非法 UTF-8 用替换字符兜底。
    pub body: String,
}

/// 请求头里的应用标识。
///
/// 不少站点（TypeSafe 前面的 Cloudflare 就是）会对没有 User-Agent 的请求直接回
/// 403 挑战页，正文是一整段网页；这里固定带一个可识别的名字，别让请求像爬虫。
const USER_AGENT: &str = concat!("Y-Mail/", env!("CARGO_PKG_VERSION"));

/// 临时拥塞最多额外重试两次；其它错误不重试。
const MAX_TRANSIENT_RETRIES: usize = 2;
const RETRY_BASE_DELAY_MS: u64 = 200;

/// 发一次请求。`body` 为 `None` 表示不带正文（GET）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn request(
    route: Option<&ProxyRoute>,
    method: &str,
    endpoint: &str,
    headers: &[(&str, String)],
    body: Option<&str>,
    timeout: Duration,
) -> Result<HttpResponse, AiError> {
    let url = Url::parse(endpoint).map_err(|_| AiError::Config("站点地址无法解析"))?;
    let host = url
        .host_str()
        .ok_or(AiError::Config("站点地址缺少主机名"))?
        .to_string();
    let secure = match url.scheme() {
        "https" => true,
        "http" => {
            // 请求层再兜一道底：就算上游漏了校验，也绝不向非本机明文发送。
            if !is_local_host(&host) {
                return Err(AiError::InsecureEndpoint(host));
            }
            false
        }
        _ => return Err(AiError::Config("站点地址协议不受支持")),
    };
    let port = url
        .port_or_known_default()
        .ok_or(AiError::Config("站点地址缺少端口"))?;

    // 路径带上查询串：有些中转站把参数放在 query 里。
    let mut target = if url.path().is_empty() {
        "/".to_string()
    } else {
        url.path().to_string()
    };
    if let Some(query) = url.query() {
        target.push('?');
        target.push_str(query);
    }

    let mut head = format!(
        "{method} {target} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {USER_AGENT}\r\nAccept: application/json\r\nAccept-Language: zh-CN,zh;q=0.9,en;q=0.8\r\n"
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    let payload = body.unwrap_or("");
    if !payload.is_empty() {
        head.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    }
    head.push_str("Connection: close\r\n\r\n");

    for attempt in 0..=MAX_TRANSIENT_RETRIES {
        let response = request_once(route, &host, port, secure, &head, payload, timeout).await?;
        if !matches!(response.status, 429 | 529) || attempt == MAX_TRANSIENT_RETRIES {
            return Ok(response);
        }
        let delay = Duration::from_millis(RETRY_BASE_DELAY_MS * (1_u64 << attempt));
        tokio::time::sleep(delay).await;
    }
    unreachable!("重试循环至少有返回分支")
}

async fn request_once(
    route: Option<&ProxyRoute>,
    host: &str,
    port: u16,
    secure: bool,
    head: &str,
    payload: &str,
    timeout: Duration,
) -> Result<HttpResponse, AiError> {
    let exchange = async {
        let tcp = mail_net::connect_tcp(host, port, route, timeout)
            .await
            .map_err(AiError::Network)?;
        let mut stream = if secure {
            mail_net::tls_wrap(tcp, host).await.map_err(AiError::Network)?
        } else {
            Stream::plain(tcp)
        };
        stream
            .write_all(head.as_bytes())
            .await
            .map_err(|_| AiError::Protocol)?;
        if !payload.is_empty() {
            stream
                .write_all(payload.as_bytes())
                .await
                .map_err(|_| AiError::Protocol)?;
        }
        stream.flush().await.map_err(|_| AiError::Protocol)?;
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .await
            .map_err(|_| AiError::Protocol)?;
        Ok::<Vec<u8>, AiError>(raw)
    };

    let raw = tokio::time::timeout(timeout, exchange)
        .await
        .map_err(|_| AiError::Timeout)??;
    parse_response(&raw)
}
/// 拆状态行、头部与正文；正文支持定长与分块两种传输方式。
fn parse_response(raw: &[u8]) -> Result<HttpResponse, AiError> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(AiError::Protocol)?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let body = &raw[split + 4..];
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(AiError::Protocol)?;
    let chunked = head.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    });
    let decoded = if chunked { dechunk(body)? } else { body.to_vec() };
    Ok(HttpResponse {
        status,
        body: String::from_utf8_lossy(&decoded).to_string(),
    })
}

/// 解分块传输编码。
fn dechunk(mut body: &[u8]) -> Result<Vec<u8>, AiError> {
    let mut out = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or(AiError::Protocol)?;
        let size_text = String::from_utf8_lossy(&body[..line_end]);
        let size = usize::from_str_radix(size_text.trim().split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| AiError::Protocol)?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if body.len() < size + 2 {
            return Err(AiError::Protocol);
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn 解析定长响应带状态码() {
        let raw = b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\n{}";
        let parsed = parse_response(raw).expect("可解析");
        assert_eq!(parsed.status, 201);
        assert_eq!(parsed.body, "{}");
    }

    #[test]
    fn 解析分块响应() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n";
        let parsed = parse_response(raw).expect("可解析");
        assert_eq!(parsed.body, "{}");
    }

    #[test]
    fn 缺少状态行直接报错() {
        assert!(parse_response(b"not http").is_err());
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        let head_end = loop {
            let read = socket.read(&mut buf).await.expect("读取请求");
            assert!(read > 0, "连接在请求结束前关闭");
            request.extend_from_slice(&buf[..read]);
            if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let head = String::from_utf8_lossy(&request[..head_end]).to_string();
        let content_length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        while request.len() < head_end + content_length {
            let read = socket.read(&mut buf).await.expect("读取请求正文");
            assert!(read > 0, "连接在请求正文结束前关闭");
            request.extend_from_slice(&buf[..read]);
        }
        String::from_utf8_lossy(&request).to_string()
    }

    /// 手动联调：确认加了 User-Agent 后能穿过 TypeSafe 前面的防护系统。
    ///
    /// 需要联网，所以默认忽略；跑法：
    /// `cargo test -p mail-ai typesafe_真实站点 -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "需要联网，手动验证防护系统不再拦请求"]
    async fn typesafe_真实站点不再被防护系统拦() {
        let response = request(
            None,
            "POST",
            "https://api.typesafe.ai/v1/systemone",
            &[
                ("Authorization", "Bearer fake-key-for-check".to_string()),
                ("Content-Type", "application/json".to_string()),
            ],
            Some(r#"{"state":"test","model":"jev-latest","questions":[]}"#),
            Duration::from_secs(20),
        )
        .await
        .expect("请求应能到达站点");
        let body = response.body.trim_start();
        assert!(
            !body.starts_with('<'),
            "被防护系统拦成了网页（状态 {}）：{}",
            response.status,
            &body[..body.len().min(200)]
        );
        assert!(
            body.contains("error_type") || body.contains("error"),
            "应该是接口自己的 JSON 错误：{}",
            &body[..body.len().min(200)]
        );
    }

    async fn fake_status_sequence(
        statuses: Vec<&'static str>,
    ) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定成功");
        let port = listener.local_addr().expect("取地址").port();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        tokio::spawn(async move {
            for status in statuses {
                let (mut socket, _) = listener.accept().await.expect("接受连接");
                let _ = read_request(&mut socket).await;
                seen.fetch_add(1, Ordering::SeqCst);
                let payload = "{}";
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                socket.write_all(response.as_bytes()).await.expect("写响应");
            }
        });
        (port, count)
    }

    #[tokio::test]
    async fn 非本机明文请求在联网前被拒绝() {
        let error = request(
            None,
            "POST",
            "http://example.invalid/v1/decisions",
            &[],
            Some("{}"),
            Duration::from_secs(1),
        )
        .await
        .expect_err("非本机明文请求必须被拒");
        assert!(matches!(error, AiError::InsecureEndpoint(_)), "{error}");
    }

    #[tokio::test]
    async fn 网关繁忙时只对429和529最多重试两次() {
        use std::sync::atomic::Ordering;

        let (port, count) = fake_status_sequence(vec!["429 Too Many Requests", "529 Busy", "200 OK"]).await;
        let endpoint = format!("http://127.0.0.1:{port}/v1/decisions");
        let response = request(None, "POST", &endpoint, &[], Some("{}"), Duration::from_secs(5))
            .await
            .expect("最终应成功");
        assert_eq!(response.status, 200);
        assert_eq!(count.load(Ordering::SeqCst), 3, "应额外重试两次");

        for (status, expected) in [
            ("401 Unauthorized", 401),
            ("403 Forbidden", 403),
            ("404 Not Found", 404),
            ("422 Bad Request", 422),
        ] {
            let (never_port, never_count) = fake_status_sequence(vec![status]).await;
            let never_endpoint = format!("http://127.0.0.1:{never_port}/v1/decisions");
            let response = request(
                None,
                "POST",
                &never_endpoint,
                &[],
                Some("{}"),
                Duration::from_secs(5),
            )
            .await
            .expect("普通错误也要返回响应");
            assert_eq!(response.status, expected);
            assert_eq!(never_count.load(Ordering::SeqCst), 1, "普通错误不能重试");
        }
    }

    #[tokio::test]
    async fn post_json能带上正文与自定义头() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定成功");
        let addr = listener.local_addr().expect("取地址");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接");
            let request = read_request(&mut socket).await;
            assert!(
                request.starts_with("POST /v1/chat/completions HTTP/1.1"),
                "{request}"
            );
            assert!(request.contains("Authorization: Bearer sk-test"), "{request}");
            assert!(
                request.contains("User-Agent: Y-Mail/"),
                "必须带应用 User-Agent，否则会被防护系统当爬虫拦掉：{request}"
            );
            assert!(request.contains(r#""model":"m1""#), "{request}");
            let response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: application/json\r\n",
                "Content-Length: 11\r\n",
                "Connection: close\r\n",
                "\r\n",
                "{\"ok\":true}",
            );
            socket.write_all(response.as_bytes()).await.expect("写响应");
            let _ = socket.shutdown().await;
        });

        let endpoint = format!("http://127.0.0.1:{}/v1/chat/completions", addr.port());
        let response = request(
            None,
            "POST",
            &endpoint,
            &[
                ("Authorization", "Bearer sk-test".to_string()),
                ("Content-Type", "application/json".to_string()),
            ],
            Some(r#"{"model":"m1"}"#),
            Duration::from_secs(5),
        )
        .await
        .expect("请求成功");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "{\"ok\":true}");
    }
}
