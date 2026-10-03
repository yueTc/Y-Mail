//! SMTP 连接自检：连上服务器 → EHLO → 可选 STARTTLS → 认证 → 退出。
//!
//! 安全约定：授权码只用于拼认证命令；错误信息与日志里必须先脱敏。

use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use mail_domain::account::Security;
use mail_domain::error::ConnectionError;
use mail_domain::proxy::{ProxyRoute, Secret};
use mail_net::io::{read_crlf_line, write_crlf_line};
use mail_net::{connect_tcp, tls_wrap, Stream};

/// 自检需要的全部信息。
#[derive(Clone, PartialEq, Eq)]
pub struct ProbeRequest {
    /// 发件服务器地址。
    pub host: String,
    /// 发件服务器端口。
    pub port: u16,
    /// 加密方式。
    pub security: Security,
    /// 登录名（多数邮箱就是邮箱地址）。
    pub username: String,
    /// 授权码或密码；只用于认证命令，绝不出现在日志与错误里。
    pub password: Secret,
    /// 整个自检的超时时间。
    pub timeout: Duration,
}

/// 自检通过后的观察结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// 实际使用的认证方式（PLAIN 或 LOGIN）。
    pub mechanism: String,
}

/// 一条服务器应答。
struct Reply {
    code: u16,
    lines: Vec<String>,
}

impl Reply {
    /// 把应答拼成一行，用于内部排查（调用方必须先脱敏）。
    fn detail(&self) -> String {
        self.lines.join(" ")
    }
}

/// 执行一次发件服务器连接自检。
pub async fn probe(
    request: &ProbeRequest,
    route: Option<&ProxyRoute>,
) -> Result<ProbeReport, ConnectionError> {
    validate(request)?;
    match tokio::time::timeout(request.timeout, run(request, route)).await {
        Ok(result) => result,
        Err(_) => Err(ConnectionError::timeout(
            "连接超时：服务器在规定时间内没有完成应答",
        )),
    }
}

fn validate(request: &ProbeRequest) -> Result<(), ConnectionError> {
    if request.host.trim().is_empty() {
        return Err(ConnectionError::protocol("发件服务器地址为空"));
    }
    if request.username.trim().is_empty() {
        return Err(ConnectionError::protocol("登录名为空"));
    }
    if request.password.is_empty() {
        return Err(ConnectionError::auth("请先填写授权码"));
    }
    Ok(())
}

async fn run(request: &ProbeRequest, route: Option<&ProxyRoute>) -> Result<ProbeReport, ConnectionError> {
    let tcp = connect_tcp(&request.host, request.port, route, request.timeout).await?;
    let mut stream = match request.security {
        Security::Tls => tls_wrap(tcp, &request.host).await?,
        Security::StartTls | Security::Plain => Stream::plain(tcp),
    };

    let greeting = read_reply(&mut stream).await?;
    if greeting.code != 220 {
        return Err(ConnectionError::rejected(format!(
            "服务器没有正常欢迎连接（返回码 {}）",
            greeting.code
        )));
    }

    let mut ehlo = send_ehlo(&mut stream).await?;

    if request.security == Security::StartTls {
        let reply = send_command(&mut stream, "STARTTLS").await?;
        if reply.code != 220 {
            return Err(ConnectionError::tls(
                "服务器没有接受 STARTTLS 升级请求，请改用 SSL/TLS 或检查端口",
            ));
        }
        stream = stream.wrap_tls(&request.host).await?;
        ehlo = send_ehlo(&mut stream).await?;
    }

    let mut mechanisms = parse_auth_mechanisms(&ehlo);
    if mechanisms.is_empty() {
        // 少数服务器不主动广告认证方式，仍按最常见的 PLAIN 试一次。
        mechanisms.push("PLAIN".to_string());
    }

    let mechanism = if mechanisms.iter().any(|item| item == "PLAIN") {
        auth_plain(&mut stream, request).await?;
        "PLAIN"
    } else if mechanisms.iter().any(|item| item == "LOGIN") {
        auth_login(&mut stream, request).await?;
        "LOGIN"
    } else {
        return Err(ConnectionError::auth(
            "服务器没有提供受支持的认证方式（仅支持 PLAIN 或 LOGIN）",
        ));
    };

    let reply = send_command(&mut stream, "QUIT").await?;
    if reply.code != 221 {
        tracing::debug!(code = reply.code, "退出命令没有得到预期应答");
    }

    Ok(ProbeReport {
        mechanism: mechanism.to_string(),
    })
}

async fn send_ehlo(stream: &mut Stream) -> Result<Reply, ConnectionError> {
    let reply = send_command(stream, "EHLO em-master.local").await?;
    if reply.code != 250 {
        return Err(ConnectionError::protocol(format!(
            "服务器不接受 EHLO 问候（返回码 {}）",
            reply.code
        )));
    }
    Ok(reply)
}

/// 从 EHLO 应答里挑出认证方式，例如 `250-AUTH PLAIN LOGIN`。
fn parse_auth_mechanisms(reply: &Reply) -> Vec<String> {
    let mut mechanisms = Vec::new();
    for line in &reply.lines {
        let upper = line.to_ascii_uppercase();
        let Some(index) = upper.find("AUTH") else {
            continue;
        };
        let rest = &upper[index + 4..];
        // 形如「AUTH=PLAIN LOGIN」时先去掉等号。
        let rest = rest.trim_start().trim_start_matches('=');
        for item in rest.split_whitespace() {
            if item == "PLAIN" || item == "LOGIN" {
                mechanisms.push(item.to_string());
            }
        }
    }
    if mechanisms.iter().any(|item| item == "PLAIN") {
        return vec!["PLAIN".to_string()];
    }
    if mechanisms.iter().any(|item| item == "LOGIN") {
        return vec!["LOGIN".to_string()];
    }
    mechanisms
}

fn plain_token(username: &str, password: &str) -> String {
    let mut raw = Vec::with_capacity(username.len() + password.len() + 2);
    raw.push(0);
    raw.extend_from_slice(username.as_bytes());
    raw.push(0);
    raw.extend_from_slice(password.as_bytes());
    STANDARD.encode(raw)
}

async fn auth_plain(stream: &mut Stream, request: &ProbeRequest) -> Result<(), ConnectionError> {
    let token = plain_token(&request.username, request.password.expose());
    let reply = send_command(stream, &format!("AUTH PLAIN {token}")).await?;
    check_auth_reply(&reply, request, "PLAIN")
}

async fn auth_login(stream: &mut Stream, request: &ProbeRequest) -> Result<(), ConnectionError> {
    let reply = send_command(stream, "AUTH LOGIN").await?;
    if reply.code != 334 {
        return Err(auth_error(&reply, request));
    }
    let reply = send_command(stream, &STANDARD.encode(&request.username)).await?;
    if reply.code != 334 {
        return Err(auth_error(&reply, request));
    }
    let reply = send_command(stream, &STANDARD.encode(request.password.expose())).await?;
    check_auth_reply(&reply, request, "LOGIN")
}

fn check_auth_reply(reply: &Reply, request: &ProbeRequest, mechanism: &str) -> Result<(), ConnectionError> {
    if reply.code == 235 {
        return Ok(());
    }
    tracing::debug!(
        reply = %mail_net::error::redact(&reply.detail(), &[request.password.expose()]),
        mechanism,
        "SMTP 认证未通过"
    );
    Err(auth_error(reply, request))
}

fn auth_error(reply: &Reply, _request: &ProbeRequest) -> ConnectionError {
    ConnectionError::auth(format!(
        "登录被服务器拒绝（返回码 {}）：请检查登录名与授权码",
        reply.code
    ))
}

async fn send_command(stream: &mut Stream, command: &str) -> Result<Reply, ConnectionError> {
    write_crlf_line(stream, command).await?;
    read_reply(stream).await
}

/// 读一条（可能是多行的）SMTP 应答，例如 `250-XXX` 连续多行后以 `250 YYY` 结束。
async fn read_reply(stream: &mut Stream) -> Result<Reply, ConnectionError> {
    let mut lines = Vec::new();
    let mut code = 0u16;
    loop {
        let line = read_crlf_line(stream, 8192).await?;
        if line.len() < 3 {
            return Err(ConnectionError::protocol("服务器应答格式无法识别"));
        }
        let parsed: u16 = line[..3]
            .parse()
            .map_err(|_| ConnectionError::protocol("服务器应答格式无法识别"))?;
        if code == 0 {
            code = parsed;
        }
        let separator = line.as_bytes().get(3).copied().unwrap_or(b' ');
        lines.push(line);
        if separator != b'-' {
            break;
        }
    }
    Ok(Reply { code, lines })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    use mail_domain::account::Security;
    use mail_domain::error::ConnectionErrorKind;
    use mail_domain::proxy::Secret;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    use super::{probe, ProbeRequest};

    fn request(port: u16, security: Security) -> ProbeRequest {
        ProbeRequest {
            host: "127.0.0.1".to_string(),
            port,
            security,
            username: "user@example.com".to_string(),
            password: Secret::new("pw-secret"),
            timeout: Duration::from_secs(5),
        }
    }

    async fn read_line(reader: &mut BufReader<TcpStream>) -> String {
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("读取失败");
        line.trim_end().to_string()
    }

    #[tokio::test]
    async fn 自检成功并使用明文认证() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        let expected = {
            let mut raw = vec![0];
            raw.extend_from_slice(b"user@example.com");
            raw.push(0);
            raw.extend_from_slice(b"pw-secret");
            STANDARD.encode(raw)
        };
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("接受连接失败");
            let mut reader = BufReader::new(socket);
            reader
                .get_mut()
                .write_all(b"220 smtp.example.com ready\r\n")
                .await
                .expect("写入失败");
            let ehlo = read_line(&mut reader).await;
            assert!(ehlo.starts_with("EHLO "));
            reader
                .get_mut()
                .write_all(b"250-smtp.example.com\r\n250-AUTH PLAIN LOGIN\r\n250 SIZE 1000000\r\n")
                .await
                .expect("写入失败");
            let auth = read_line(&mut reader).await;
            assert_eq!(auth, format!("AUTH PLAIN {expected}"));
            reader
                .get_mut()
                .write_all(b"235 2.7.0 Authentication successful\r\n")
                .await
                .expect("写入失败");
            let quit = read_line(&mut reader).await;
            assert_eq!(quit, "QUIT");
            reader
                .get_mut()
                .write_all(b"221 bye\r\n")
                .await
                .expect("写入失败");
        });

        let report = probe(&request(addr.port(), Security::Plain), None)
            .await
            .expect("自检应通过");
        assert_eq!(report.mechanism, "PLAIN");
    }

    #[tokio::test]
    async fn 认证被拒时归类为认证失败且不泄露授权码() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("接受连接失败");
            let mut reader = BufReader::new(socket);
            reader
                .get_mut()
                .write_all(b"220 ready\r\n")
                .await
                .expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"250-AUTH PLAIN\r\n250 OK\r\n")
                .await
                .expect("写入失败");
            let auth = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(format!("535 5.7.8 bad credentials ({auth})\r\n").as_bytes())
                .await
                .expect("写入失败");
        });

        let err = probe(&request(addr.port(), Security::Plain), None)
            .await
            .expect_err("应认证失败");
        assert_eq!(err.kind, ConnectionErrorKind::AuthFailed);
        assert!(!err.message.contains("pw-secret"));
    }

    #[tokio::test]
    async fn 服务器不广告认证方式时仍尝试明文认证() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("接受连接失败");
            let mut reader = BufReader::new(socket);
            reader
                .get_mut()
                .write_all(b"220 ready\r\n")
                .await
                .expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"250 smtp.example.com\r\n")
                .await
                .expect("写入失败");
            let auth = read_line(&mut reader).await;
            assert!(auth.starts_with("AUTH PLAIN "));
            reader.get_mut().write_all(b"235 ok\r\n").await.expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"221 bye\r\n")
                .await
                .expect("写入失败");
        });

        let report = probe(&request(addr.port(), Security::Plain), None)
            .await
            .expect("自检应通过");
        assert_eq!(report.mechanism, "PLAIN");
    }

    #[tokio::test]
    async fn 服务器只支持登录式认证时走login流程() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("接受连接失败");
            let mut reader = BufReader::new(socket);
            reader
                .get_mut()
                .write_all(b"220 ready\r\n")
                .await
                .expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"250-AUTH LOGIN\r\n250 OK\r\n")
                .await
                .expect("写入失败");
            let auth = read_line(&mut reader).await;
            assert_eq!(auth, "AUTH LOGIN");
            reader
                .get_mut()
                .write_all(b"334 VXNlcm5hbWU6\r\n")
                .await
                .expect("写入失败");
            let user = read_line(&mut reader).await;
            assert_eq!(user, STANDARD.encode("user@example.com"));
            reader
                .get_mut()
                .write_all(b"334 UGFzc3dvcmQ6\r\n")
                .await
                .expect("写入失败");
            let password = read_line(&mut reader).await;
            assert_eq!(password, STANDARD.encode("pw-secret"));
            reader.get_mut().write_all(b"235 ok\r\n").await.expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"221 bye\r\n")
                .await
                .expect("写入失败");
        });

        let report = probe(&request(addr.port(), Security::Plain), None)
            .await
            .expect("自检应通过");
        assert_eq!(report.mechanism, "LOGIN");
    }

    #[tokio::test]
    async fn 服务器不接受starttls时归类为加密失败() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("接受连接失败");
            let mut reader = BufReader::new(socket);
            reader
                .get_mut()
                .write_all(b"220 ready\r\n")
                .await
                .expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"250 smtp.example.com\r\n")
                .await
                .expect("写入失败");
            let starttls = read_line(&mut reader).await;
            assert_eq!(starttls, "STARTTLS");
            reader
                .get_mut()
                .write_all(b"454 TLS not available\r\n")
                .await
                .expect("写入失败");
        });

        let err = probe(&request(addr.port(), Security::StartTls), None)
            .await
            .expect_err("应升级失败");
        assert_eq!(err.kind, ConnectionErrorKind::TlsFailure);
    }

    #[tokio::test]
    async fn 网络不可达时归类为网络错误() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        drop(listener);

        let err = probe(&request(addr.port(), Security::Plain), None)
            .await
            .expect_err("应连不上");
        assert_eq!(err.kind, ConnectionErrorKind::NetworkUnreachable);
    }
}
