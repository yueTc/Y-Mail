//! 最小 HTTP 客户端：只为 OAuth 换令牌服务。
//!
//! 传输复用 mail-net，因此代理分层（账号级 > 全局自定义 > 系统代理）
//! 与收发邮件走的是同一套选路逻辑。

use std::time::Duration;

use mail_domain::proxy::ProxyRoute;
use mail_net::Stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use crate::error::OAuthError;

/// 表单 POST，返回响应正文（不区分状态码，交由上层解析错误码）。
pub async fn post_form(
    route: Option<&ProxyRoute>,
    endpoint: &str,
    form: &[(&str, &str)],
    timeout: Duration,
) -> Result<String, OAuthError> {
    let url = Url::parse(endpoint).map_err(|_| OAuthError::Config("令牌地址无效"))?;
    let host = url
        .host_str()
        .ok_or(OAuthError::Config("令牌地址缺少主机名"))?
        .to_string();
    let secure = match url.scheme() {
        "https" => true,
        "http" => false,
        _ => return Err(OAuthError::Config("令牌地址协议不受支持")),
    };
    let port = url
        .port_or_known_default()
        .ok_or(OAuthError::Config("令牌地址缺少端口"))?;
    let path = if url.path().is_empty() { "/" } else { url.path() };
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form.iter().copied())
        .finish();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    let exchange = async {
        let tcp = mail_net::connect_tcp(&host, port, route, timeout)
            .await
            .map_err(OAuthError::Network)?;
        let mut stream = if secure {
            mail_net::tls_wrap(tcp, &host)
                .await
                .map_err(OAuthError::Network)?
        } else {
            Stream::plain(tcp)
        };
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|_| OAuthError::Protocol)?;
        stream.flush().await.map_err(|_| OAuthError::Protocol)?;
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .await
            .map_err(|_| OAuthError::Protocol)?;
        Ok::<Vec<u8>, OAuthError>(raw)
    };

    let raw = tokio::time::timeout(timeout, exchange)
        .await
        .map_err(|_| OAuthError::Timeout)??;
    parse_response(&raw)
}

/// 拆状态行、头部与正文，正文支持定长和分块两种传输方式。
fn parse_response(raw: &[u8]) -> Result<String, OAuthError> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(OAuthError::Protocol)?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let body = &raw[split + 4..];
    if !head.starts_with("HTTP/") {
        return Err(OAuthError::Protocol);
    }
    let chunked = head.lines().any(|line| {
        line.to_ascii_lowercase().starts_with("transfer-encoding:")
            && line.to_ascii_lowercase().contains("chunked")
    });
    let decoded = if chunked { dechunk(body)? } else { body.to_vec() };
    Ok(String::from_utf8_lossy(&decoded).to_string())
}

/// 解分块传输编码。
fn dechunk(mut body: &[u8]) -> Result<Vec<u8>, OAuthError> {
    let mut out = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or(OAuthError::Protocol)?;
        let size_text = String::from_utf8_lossy(&body[..line_end]);
        let size = usize::from_str_radix(size_text.trim().split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| OAuthError::Protocol)?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if body.len() < size + 2 {
            return Err(OAuthError::Protocol);
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
    fn 解析定长响应() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(parse_response(raw).expect("可解析"), "{}");
    }

    #[test]
    fn 解析分块响应() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw).expect("可解析"), "{}");
    }

    #[test]
    fn 缺少响应头分隔符直接报错() {
        assert!(parse_response(b"HTTP/1.1 200 OK").is_err());
    }

    #[tokio::test]
    async fn 表单提交能拿到响应正文() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定成功");
        let addr = listener.local_addr().expect("取地址");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接");
            let mut buf = vec![0u8; 2048];
            let read = socket.read(&mut buf).await.expect("读取请求");
            let request = String::from_utf8_lossy(&buf[..read]).to_string();
            assert!(request.starts_with("POST /token HTTP/1.1"));
            assert!(request.contains("grant_type=authorization_code"));
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
                )
                .await
                .expect("写响应");
        });

        let endpoint = format!("http://127.0.0.1:{}/token", addr.port());
        let body = post_form(
            None,
            &endpoint,
            &[("grant_type", "authorization_code"), ("code", "abc")],
            Duration::from_secs(5),
        )
        .await
        .expect("请求成功");
        assert_eq!(body, "{\"ok\":true}");
    }
}
