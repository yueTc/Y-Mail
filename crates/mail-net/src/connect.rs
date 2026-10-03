//! 建立 TCP 连接：直连，或穿过 SOCKS5 / HTTP 隧道代理。
//!
//! 代理握手只借用密码，握手完成后密码不再参与传输；错误信息里不含密码。

use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use mail_domain::error::ConnectionError;
use mail_domain::proxy::{ProxyKind, ProxyRoute, Secret};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::error::describe_io;

/// 默认的单步超时时间。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// 连接目标服务器；`route` 为 `None` 时直连。
pub async fn connect_tcp(
    host: &str,
    port: u16,
    route: Option<&ProxyRoute>,
    timeout: Duration,
) -> Result<TcpStream, ConnectionError> {
    match tokio::time::timeout(timeout, connect_inner(host, port, route)).await {
        Ok(result) => result,
        Err(_) => Err(ConnectionError::timeout("连接超时：限定时间内没有完成连接")),
    }
}

async fn connect_inner(
    host: &str,
    port: u16,
    route: Option<&ProxyRoute>,
) -> Result<TcpStream, ConnectionError> {
    match route {
        None => TcpStream::connect((host, port))
            .await
            .map_err(|err| crate::error::classify_direct(&err)),
        Some(route) => {
            let proxy = &route.config;
            let tcp = TcpStream::connect((proxy.host.as_str(), proxy.port))
                .await
                .map_err(|err| {
                    ConnectionError::proxy(format!("无法连接代理服务器：{}", describe_io(&err)))
                })?;
            match proxy.kind {
                ProxyKind::Socks5 => socks5_connect(tcp, host, port, route).await,
                ProxyKind::Http => http_connect(tcp, host, port, route).await,
            }
        }
    }
}

fn socks5_reply_error(code: u8) -> ConnectionError {
    match code {
        0x01 => ConnectionError::proxy("代理报告内部错误"),
        0x02 => ConnectionError::proxy("代理规则不允许连接该地址"),
        0x03 => ConnectionError::network("代理报告：网络不可达"),
        0x04 => ConnectionError::network("代理报告：目标主机不可达"),
        0x05 => ConnectionError::network("代理报告：目标服务器拒绝连接"),
        0x06 => ConnectionError::network("代理报告：连接已过期"),
        0x07 => ConnectionError::proxy("代理不支持该连接命令"),
        0x08 => ConnectionError::proxy("代理不支持该地址类型"),
        _ => ConnectionError::proxy("代理返回了未知错误"),
    }
}

async fn socks5_auth(tcp: &mut TcpStream, username: &str, password: &str) -> Result<(), ConnectionError> {
    if username.len() > 255 || password.len() > 255 {
        return Err(ConnectionError::proxy("代理用户名或密码过长"));
    }
    let mut request = Vec::with_capacity(3 + username.len() + password.len());
    request.push(0x01);
    request.push(username.len() as u8);
    request.extend_from_slice(username.as_bytes());
    request.push(password.len() as u8);
    request.extend_from_slice(password.as_bytes());
    tcp.write_all(&request)
        .await
        .map_err(|err| ConnectionError::proxy(format!("发送代理认证失败：{}", describe_io(&err))))?;
    let mut response = [0u8; 2];
    tcp.read_exact(&mut response)
        .await
        .map_err(|err| ConnectionError::proxy(format!("读取代理应答失败：{}", describe_io(&err))))?;
    if response[1] != 0x00 {
        return Err(ConnectionError::proxy("代理认证失败：请检查代理用户名与密码"));
    }
    Ok(())
}

async fn socks5_connect(
    mut tcp: TcpStream,
    host: &str,
    port: u16,
    route: &ProxyRoute,
) -> Result<TcpStream, ConnectionError> {
    if host.is_empty() || host.len() > 255 {
        return Err(ConnectionError::proxy("目标服务器地址为空或过长"));
    }
    let username = route.config.username.trim();
    let password = route.password.as_ref().map(Secret::expose).unwrap_or_default();
    let use_auth = !username.is_empty();

    let greeting: &[u8] = if use_auth {
        &[0x05, 0x02, 0x00, 0x02]
    } else {
        &[0x05, 0x01, 0x00]
    };
    tcp.write_all(greeting)
        .await
        .map_err(|err| ConnectionError::proxy(format!("发送代理握手失败：{}", describe_io(&err))))?;
    let mut response = [0u8; 2];
    tcp.read_exact(&mut response)
        .await
        .map_err(|err| ConnectionError::proxy(format!("读取代理应答失败：{}", describe_io(&err))))?;
    if response[0] != 0x05 {
        return Err(ConnectionError::proxy("代理响应不符合 SOCKS5 协议"));
    }
    match response[1] {
        0x00 => {}
        0x02 => {
            if !use_auth {
                return Err(ConnectionError::proxy("代理想用用户名密码认证，但当前没有填写"));
            }
            socks5_auth(&mut tcp, username, password).await?;
        }
        0xFF => {
            return Err(ConnectionError::proxy("代理不接受任何可用的登录方式"));
        }
        _ => {
            return Err(ConnectionError::proxy("代理要求了不支持的登录方式"));
        }
    }

    let mut request = Vec::with_capacity(7 + host.len());
    request.extend_from_slice(&[0x05, 0x01, 0x00, 0x03]);
    request.push(host.len() as u8);
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    tcp.write_all(&request)
        .await
        .map_err(|err| ConnectionError::proxy(format!("发送代理请求失败：{}", describe_io(&err))))?;

    let mut head = [0u8; 4];
    tcp.read_exact(&mut head)
        .await
        .map_err(|err| ConnectionError::proxy(format!("读取代理应答失败：{}", describe_io(&err))))?;
    if head[0] != 0x05 {
        return Err(ConnectionError::proxy("代理响应不符合 SOCKS5 协议"));
    }
    if head[1] != 0x00 {
        return Err(socks5_reply_error(head[1]));
    }
    match head[3] {
        0x01 => skip_bytes(&mut tcp, 4 + 2).await?,
        0x03 => {
            let mut length = [0u8; 1];
            tcp.read_exact(&mut length)
                .await
                .map_err(|err| ConnectionError::proxy(format!("读取代理应答失败：{}", describe_io(&err))))?;
            skip_bytes(&mut tcp, usize::from(length[0]) + 2).await?;
        }
        0x04 => skip_bytes(&mut tcp, 16 + 2).await?,
        _ => return Err(ConnectionError::proxy("代理返回了不支持的地址类型")),
    }
    Ok(tcp)
}

async fn skip_bytes(tcp: &mut TcpStream, count: usize) -> Result<(), ConnectionError> {
    let mut buffer = vec![0u8; count];
    tcp.read_exact(&mut buffer)
        .await
        .map_err(|err| ConnectionError::proxy(format!("读取代理应答失败：{}", describe_io(&err))))?;
    Ok(())
}

/// 读 HTTP 应答头（读到空行为止）；逐字节读，避免多读走隧道里的数据。
async fn read_http_head(tcp: &mut TcpStream) -> Result<String, ConnectionError> {
    let mut buffer = Vec::with_capacity(512);
    loop {
        if buffer.len() >= 8192 {
            return Err(ConnectionError::proxy("代理返回的应答过长"));
        }
        let mut byte = [0u8; 1];
        let read = tcp
            .read(&mut byte)
            .await
            .map_err(|err| ConnectionError::proxy(format!("读取代理应答失败：{}", describe_io(&err))))?;
        if read == 0 {
            return Err(ConnectionError::proxy("代理在没有应答的情况下关闭了连接"));
        }
        buffer.push(byte[0]);
        if buffer.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(buffer).map_err(|_| ConnectionError::proxy("代理返回的内容不是有效文本"))
}

async fn http_connect(
    mut tcp: TcpStream,
    host: &str,
    port: u16,
    route: &ProxyRoute,
) -> Result<TcpStream, ConnectionError> {
    let authority = format!("{host}:{port}");
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if !route.config.username.is_empty() {
        let password = route.password.as_ref().map(Secret::expose).unwrap_or_default();
        let token = STANDARD.encode(format!("{}:{}", route.config.username, password));
        request.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    request.push_str("\r\n");
    tcp.write_all(request.as_bytes())
        .await
        .map_err(|err| ConnectionError::proxy(format!("发送代理请求失败：{}", describe_io(&err))))?;

    let response = read_http_head(&mut tcp).await?;
    let first_line = response.lines().next().unwrap_or_default();
    let code = first_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok());
    match code {
        Some(200..=299) => Ok(tcp),
        Some(407) => Err(ConnectionError::proxy("代理认证失败：请检查代理用户名与密码")),
        Some(403) => Err(ConnectionError::proxy("代理拒绝建立到该地址的隧道")),
        Some(code) => Err(ConnectionError::proxy(format!(
            "代理返回了失败状态（状态码 {code}）"
        ))),
        None => Err(ConnectionError::proxy("代理返回的内容无法识别")),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mail_domain::error::ConnectionErrorKind;
    use mail_domain::proxy::{ProxyConfig, ProxyKind, ProxyRoute, Secret};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::connect_tcp;

    fn route(
        addr: std::net::SocketAddr,
        kind: ProxyKind,
        username: &str,
        password: Option<&str>,
    ) -> ProxyRoute {
        ProxyRoute {
            config: ProxyConfig {
                id: None,
                label: "测试代理".to_string(),
                kind,
                host: addr.ip().to_string(),
                port: addr.port(),
                username: username.to_string(),
            },
            password: password.map(Secret::new),
        }
    }

    async fn read_head(socket: &mut TcpStream) -> String {
        let mut buffer = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            socket.read_exact(&mut byte).await.expect("读取请求失败");
            buffer.push(byte[0]);
            if buffer.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8(buffer).expect("请求不是文本")
    }

    #[tokio::test]
    async fn 直连可以连上本机服务器() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接失败");
            let mut buffer = [0u8; 4];
            socket.read_exact(&mut buffer).await.expect("读取失败");
            assert_eq!(&buffer, b"ping");
            socket.write_all(b"pong").await.expect("写入失败");
        });

        let mut tcp = connect_tcp("127.0.0.1", addr.port(), None, Duration::from_secs(5))
            .await
            .expect("应能连上");
        tcp.write_all(b"ping").await.expect("发送失败");
        let mut buffer = [0u8; 4];
        tcp.read_exact(&mut buffer).await.expect("读取失败");
        assert_eq!(&buffer, b"pong");
    }

    #[tokio::test]
    async fn 直连被拒绝时归类为网络不可达() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        drop(listener);

        let err = connect_tcp("127.0.0.1", addr.port(), None, Duration::from_secs(5))
            .await
            .expect_err("应连接失败");
        assert_eq!(err.kind, ConnectionErrorKind::NetworkUnreachable);
        assert!(err.message.contains("无法连接服务器"));
    }

    #[tokio::test]
    async fn 袜子五代理握手成功() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接失败");
            let mut greeting = [0u8; 3];
            socket.read_exact(&mut greeting).await.expect("读取问候失败");
            assert_eq!(greeting, [0x05, 0x01, 0x00]);
            socket.write_all(&[0x05, 0x00]).await.expect("回应失败");
            let mut head = [0u8; 5];
            socket.read_exact(&mut head).await.expect("读取请求失败");
            assert_eq!(&head[..4], &[0x05, 0x01, 0x00, 0x03]);
            let mut rest = vec![0u8; usize::from(head[4]) + 2];
            socket.read_exact(&mut rest).await.expect("读取地址失败");
            socket
                .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await
                .expect("回应失败");
            socket.write_all(b"pong").await.expect("写入失败");
        });

        let route = route(addr, ProxyKind::Socks5, "", None);
        let mut tcp = connect_tcp("mail.example.com", 993, Some(&route), Duration::from_secs(5))
            .await
            .expect("应穿过代理连上");
        let mut buffer = [0u8; 4];
        tcp.read_exact(&mut buffer).await.expect("读取失败");
        assert_eq!(&buffer, b"pong");
    }

    #[tokio::test]
    async fn 袜子五代理认证成功后密码不出现在错误里() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接失败");
            let mut greeting = [0u8; 4];
            socket.read_exact(&mut greeting).await.expect("读取问候失败");
            assert_eq!(greeting, [0x05, 0x02, 0x00, 0x02]);
            socket.write_all(&[0x05, 0x02]).await.expect("回应失败");
            let mut head = [0u8; 2];
            socket.read_exact(&mut head).await.expect("读取认证头失败");
            let mut username = vec![0u8; usize::from(head[1])];
            socket.read_exact(&mut username).await.expect("读取用户名失败");
            assert_eq!(username, b"user");
            let mut length = [0u8; 1];
            socket.read_exact(&mut length).await.expect("读取长度失败");
            let mut password = vec![0u8; usize::from(length[0])];
            socket.read_exact(&mut password).await.expect("读取密码失败");
            assert_eq!(password, b"pw-secret");
            socket.write_all(&[0x01, 0x00]).await.expect("认证通过失败");
            let mut request = [0u8; 4];
            socket.read_exact(&mut request).await.expect("读取请求失败");
            let mut rest = vec![0u8; 64];
            let read = socket.read(&mut rest).await.expect("读取请求失败");
            socket
                .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await
                .expect("回应失败");
            let _ = read;
        });

        let route = route(addr, ProxyKind::Socks5, "user", Some("pw-secret"));
        let tcp = connect_tcp("mail.example.com", 993, Some(&route), Duration::from_secs(5))
            .await
            .expect("应认证成功");
        drop(tcp);
    }

    #[tokio::test]
    async fn 袜子五代理认证失败时给出可读错误() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接失败");
            let mut greeting = [0u8; 4];
            socket.read_exact(&mut greeting).await.expect("读取问候失败");
            socket.write_all(&[0x05, 0x02]).await.expect("回应失败");
            let mut head = [0u8; 2];
            socket.read_exact(&mut head).await.expect("读取认证头失败");
            let mut username = vec![0u8; usize::from(head[1])];
            socket.read_exact(&mut username).await.expect("读取用户名失败");
            let mut length = [0u8; 1];
            socket.read_exact(&mut length).await.expect("读取长度失败");
            let mut password = vec![0u8; usize::from(length[0])];
            socket.read_exact(&mut password).await.expect("读取密码失败");
            socket.write_all(&[0x01, 0x01]).await.expect("认证失败回应");
        });

        let route = route(addr, ProxyKind::Socks5, "user", Some("pw-secret"));
        let err = connect_tcp("mail.example.com", 993, Some(&route), Duration::from_secs(5))
            .await
            .expect_err("应认证失败");
        assert_eq!(err.kind, ConnectionErrorKind::ProxyFailure);
        assert!(err.message.contains("代理认证失败"));
        assert!(!err.message.contains("pw-secret"));
    }

    #[tokio::test]
    async fn http_隧道代理握手成功() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接失败");
            let head = read_head(&mut socket).await;
            assert!(head.starts_with("CONNECT mail.example.com:993 HTTP/1.1"));
            socket
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await
                .expect("回应失败");
            socket.write_all(b"pong").await.expect("写入失败");
        });

        let route = route(addr, ProxyKind::Http, "", None);
        let mut tcp = connect_tcp("mail.example.com", 993, Some(&route), Duration::from_secs(5))
            .await
            .expect("应穿过代理连上");
        let mut buffer = [0u8; 4];
        tcp.read_exact(&mut buffer).await.expect("读取失败");
        assert_eq!(&buffer, b"pong");
    }

    #[tokio::test]
    async fn http_隧道代理带认证时发送认证头() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接失败");
            let head = read_head(&mut socket).await;
            assert!(head.contains("Proxy-Authorization: Basic dXNlcjpwdw=="));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\n\r\n")
                .await
                .expect("回应失败");
        });

        let route = route(addr, ProxyKind::Http, "user", Some("pw"));
        let tcp = connect_tcp("mail.example.com", 993, Some(&route), Duration::from_secs(5))
            .await
            .expect("应认证成功");
        drop(tcp);
    }

    #[tokio::test]
    async fn http_隧道代理认证失败时给出可读错误() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接失败");
            let _ = read_head(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
                .await
                .expect("回应失败");
        });

        let route = route(addr, ProxyKind::Http, "user", Some("pw-secret"));
        let err = connect_tcp("mail.example.com", 993, Some(&route), Duration::from_secs(5))
            .await
            .expect_err("应认证失败");
        assert_eq!(err.kind, ConnectionErrorKind::ProxyFailure);
        assert!(err.message.contains("代理认证失败"));
        assert!(!err.message.contains("pw-secret"));
    }

    #[tokio::test]
    async fn 代理解析失败时给出代理错误() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        drop(listener);

        let route = route(addr, ProxyKind::Socks5, "", None);
        let err = connect_tcp("mail.example.com", 993, Some(&route), Duration::from_secs(5))
            .await
            .expect_err("应连不上代理");
        assert_eq!(err.kind, ConnectionErrorKind::ProxyFailure);
        assert!(err.message.contains("无法连接代理服务器"));
    }
}
