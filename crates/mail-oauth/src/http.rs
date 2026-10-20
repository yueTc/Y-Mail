//! 最小 HTTP 客户端：只为 OAuth 换令牌服务。
//!
//! 传输复用 mail-net，因此代理分层（账号级 > 全局自定义 > 系统代理）
//! 与收发邮件走的是同一套选路逻辑。

use std::time::Duration;

use mail_domain::proxy::ProxyRoute;
use mail_net::Stream;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use url::Url;

use crate::error::OAuthError;

/// 表单 POST，返回响应正文（不区分状态码，交由上层解析错误码）。
pub async fn post_form(
    route: Option<&ProxyRoute>,
    endpoint: &str,
    form: &[(&str, &str)],
    timeout: Duration,
) -> Result<String, OAuthError> {
    let url = Url::parse(endpoint).map_err(|_| OAuthError::Config("令牌地址无效".to_string()))?;
    let host = url
        .host_str()
        .ok_or(OAuthError::Config("令牌地址缺少主机名".to_string()))?
        .to_string();
    let secure = match url.scheme() {
        "https" => true,
        "http" => false,
        _ => return Err(OAuthError::Config("令牌地址协议不受支持".to_string())),
    };
    let port = url
        .port_or_known_default()
        .ok_or(OAuthError::Config("令牌地址缺少端口".to_string()))?;
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
            .map_err(transport_error)?;
        stream.flush().await.map_err(transport_error)?;
        read_response(&mut stream).await
    };

    tokio::time::timeout(timeout, exchange)
        .await
        .map_err(|_| OAuthError::Timeout)?
}

/// 令牌回应很小，限制正文及传输元数据，避免无界读取远端内容。
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_METADATA_BYTES: usize = 64 * 1024;

/// 按 HTTP 正文边界读取，收到完整回应即返回，不依赖加密连接的关闭方式。
async fn read_response<S: AsyncRead + Unpin>(stream: &mut S) -> Result<String, OAuthError> {
    let mut reader = BufReader::new(stream);
    let mut metadata_left = MAX_METADATA_BYTES;
    let (length, chunked) = loop {
        let status_line = response_line(&mut reader, &mut metadata_left).await?;
        let status_line = std::str::from_utf8(&status_line).map_err(|_| OAuthError::Protocol)?;
        let mut parts = status_line.split_whitespace();
        if !matches!(parts.next(), Some("HTTP/1.0" | "HTTP/1.1")) {
            return Err(OAuthError::Protocol);
        }
        let status = parts
            .next()
            .filter(|code| code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|code| code.parse::<u16>().ok())
            .filter(|code| (100..=599).contains(code))
            .ok_or(OAuthError::Protocol)?;
        let mut length = None;
        let mut chunked = false;
        loop {
            let line = response_line(&mut reader, &mut metadata_left).await?;
            if line.is_empty() {
                break;
            }
            let line = std::str::from_utf8(&line).map_err(|_| OAuthError::Protocol)?;
            let (name, value) = line.split_once(':').ok_or(OAuthError::Protocol)?;
            if name.eq_ignore_ascii_case("content-length") {
                let value = value.trim();
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(OAuthError::Protocol);
                }
                let size = value.parse::<usize>().map_err(|_| OAuthError::Protocol)?;
                if size > MAX_BODY_BYTES || length.is_some_and(|prior| prior != size) {
                    return Err(OAuthError::Protocol);
                }
                length = Some(size);
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                if chunked || !value.trim().eq_ignore_ascii_case("chunked") {
                    return Err(OAuthError::Protocol);
                }
                chunked = true;
            }
        }
        if chunked && length.is_some() {
            return Err(OAuthError::Protocol);
        }
        if status == 101 {
            return Err(OAuthError::Protocol);
        }
        if !(100..200).contains(&status) {
            break (length, chunked);
        }
    };

    let body = if chunked {
        read_chunked(&mut reader, &mut metadata_left).await?
    } else if let Some(size) = length {
        let mut body = vec![0; size];
        reader.read_exact(&mut body).await.map_err(transport_error)?;
        body
    } else {
        // 无长度、无分块标记时，只有正常读到连接末尾才能确定回应完整。
        let mut body = Vec::new();
        reader
            .take(MAX_BODY_BYTES as u64 + 1)
            .read_to_end(&mut body)
            .await
            .map_err(transport_error)?;
        if body.len() > MAX_BODY_BYTES {
            return Err(OAuthError::Protocol);
        }
        body
    };
    String::from_utf8(body).map_err(|_| OAuthError::Protocol)
}

/// 只接受完整行，按总预算限制响应头、分块元数据与尾部字段。
async fn response_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    remaining: &mut usize,
) -> Result<Vec<u8>, OAuthError> {
    let mut line = Vec::new();
    let count = reader
        .take(*remaining as u64 + 1)
        .read_until(b'\n', &mut line)
        .await
        .map_err(transport_error)?;
    if count > *remaining {
        return Err(OAuthError::Protocol);
    }
    *remaining -= count;
    if !line.ends_with(b"\r\n") {
        return Err(transport_error(std::io::Error::from(
            std::io::ErrorKind::UnexpectedEof,
        )));
    }
    line.truncate(line.len() - 2);
    Ok(line)
}

/// 读取完整的分块正文；零长度块和尾部空行都收到后才算完成。
async fn read_chunked<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    metadata_left: &mut usize,
) -> Result<Vec<u8>, OAuthError> {
    let mut body = Vec::new();
    loop {
        let line = response_line(reader, metadata_left).await?;
        let size_text = std::str::from_utf8(&line).map_err(|_| OAuthError::Protocol)?;
        let size_text = size_text.split(';').next().unwrap_or("");
        if size_text.is_empty() || !size_text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(OAuthError::Protocol);
        }
        let size = usize::from_str_radix(size_text, 16).map_err(|_| OAuthError::Protocol)?;
        if size == 0 {
            loop {
                let trailer = response_line(reader, metadata_left).await?;
                if trailer.is_empty() {
                    return Ok(body);
                }
                if !trailer.contains(&b':') {
                    return Err(OAuthError::Protocol);
                }
            }
        }
        let end = body
            .len()
            .checked_add(size)
            .filter(|end| *end <= MAX_BODY_BYTES)
            .ok_or(OAuthError::Protocol)?;
        let start = body.len();
        body.resize(end, 0);
        reader
            .read_exact(&mut body[start..])
            .await
            .map_err(transport_error)?;
        let mut separator = [0; 2];
        reader.read_exact(&mut separator).await.map_err(transport_error)?;
        if separator != *b"\r\n" {
            return Err(OAuthError::Protocol);
        }
    }
}

/// 不回显网络库的原文，避免远端回应或凭据进入错误消息。
fn transport_error(error: std::io::Error) -> OAuthError {
    if error.kind() == std::io::ErrorKind::TimedOut {
        OAuthError::Timeout
    } else {
        OAuthError::Network(mail_domain::error::ConnectionError::network(
            "与授权服务器的连接中断或响应未收全，请检查网络或代理后重新授权",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn 解析定长响应() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(read_response(&mut &raw[..]).await.expect("可解析"), "{}");
    }

    #[tokio::test]
    async fn 解析分块响应() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n";
        assert_eq!(read_response(&mut &raw[..]).await.expect("可解析"), "{}");
    }

    #[tokio::test]
    async fn 缺少响应头分隔符直接报错() {
        assert!(read_response(&mut &b"HTTP/1.1 200 OK"[..]).await.is_err());
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

    /// 模拟服务器发完回应后，下一次读取就返回连接收尾错误。
    struct FailsOnClose {
        response: Vec<u8>,
        position: usize,
        fragment: usize,
    }

    impl AsyncRead for FailsOnClose {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let this = self.get_mut();
            if this.position == this.response.len() {
                return std::task::Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "untrusted-token-must-not-leak",
                )));
            }
            let size = buffer
                .remaining()
                .min(this.fragment)
                .min(this.response.len() - this.position);
            buffer.put_slice(&this.response[this.position..this.position + size]);
            this.position += size;
            std::task::Poll::Ready(Ok(()))
        }
    }

    fn broken_stream(response: String, fragment: usize) -> FailsOnClose {
        FailsOnClose {
            response: response.into_bytes(),
            position: 0,
            fragment,
        }
    }

    #[tokio::test]
    async fn 收齐定长或分块回应后不再读取连接收尾() {
        let payload = r#"{"access_token":"at-test"}"#;
        for response in [
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{payload}", payload.len()),
            format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x};ext=ok\r\n{payload}\r\n0\r\n\r\n", payload.len()),
            format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{payload}\r\n0\r\nX-Trailer: value\r\n\r\n", payload.len()),
        ] {
            for fragment in [1, 7, usize::MAX] {
                let mut stream = broken_stream(response.clone(), fragment);
                assert_eq!(read_response(&mut stream).await.expect("完整回应不能被收尾错误丢弃"), payload);
            }
        }
    }

    #[tokio::test]
    async fn 回应未收全时不能接受看似完整的令牌正文() {
        let payload = r#"{"access_token":"at-test"}"#;
        for response in [
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{payload}",
                payload.len() + 1
            ),
            format!(
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{payload}\r\n",
                payload.len()
            ),
            format!(
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{payload}\r\n0\r\n",
                payload.len()
            ),
        ] {
            let mut stream = broken_stream(response, 7);
            let error = read_response(&mut stream)
                .await
                .expect_err("缺少完整边界必须失败");
            assert!(matches!(error, OAuthError::Network(_)));
            assert!(!format!("{error} {error:?}").contains("untrusted-token-must-not-leak"));
            assert!(!format!("{error} {error:?}").contains("at-test"));
        }
    }

    #[tokio::test]
    async fn 无长度与分块标记时必须等正常关闭() {
        let response = "HTTP/1.1 200 OK\r\n\r\n{}";
        assert_eq!(
            read_response(&mut response.as_bytes()).await.expect("正常结束"),
            "{}"
        );
        let error = read_response(&mut broken_stream(response.to_string(), 7))
            .await
            .expect_err("不能把中断当作完整回应");
        assert!(matches!(error, OAuthError::Network(_)));
    }

    #[tokio::test]
    async fn 拒绝歧义长度非法分块和超大响应() {
        for response in [
            "HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 3\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}xx0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffffffffffffffffffff\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n100001\r\n",
        ] {
            assert!(matches!(
                read_response(&mut response.as_bytes()).await,
                Err(OAuthError::Protocol)
            ));
        }
        let too_large = format!(
            "HTTP/1.1 200 OK\r\nX-Long: {}\r\n\r\n",
            "x".repeat(MAX_METADATA_BYTES)
        );
        assert!(matches!(
            read_response(&mut too_large.as_bytes()).await,
            Err(OAuthError::Protocol)
        ));
    }

    #[tokio::test]
    async fn 临时响应之后继续读取最终正文() {
        let response = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}";
        assert_eq!(read_response(&mut &response[..]).await.expect("最终回应"), "{}");
    }
}
