//! IMAP 连接自检：连上服务器 → 可选 STARTTLS → 登录 → 数文件夹 → 选收件箱 → 退出。
//!
//! 安全约定：授权码只用于拼 LOGIN 命令；错误信息与日志里必须先脱敏。

use std::time::Duration;

use mail_domain::account::Security;
use mail_domain::error::ConnectionError;
use mail_domain::proxy::{ProxyRoute, Secret};
use mail_net::io::{read_crlf_line, write_crlf_line};
use mail_net::{connect_tcp, tls_wrap, Stream};

/// 自检需要的全部信息。
#[derive(Clone, PartialEq, Eq)]
pub struct ProbeRequest {
    /// 收件服务器地址。
    pub host: String,
    /// 收件服务器端口。
    pub port: u16,
    /// 加密方式。
    pub security: Security,
    /// 登录名（多数邮箱就是邮箱地址）。
    pub username: String,
    /// 授权码或密码；只用于登录命令，绝不出现在日志与错误里。
    pub password: Secret,
    /// 整个自检的超时时间。
    pub timeout: Duration,
}

/// 自检通过后的观察结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// 能看到的文件夹数量（至少应有收件箱）。
    pub folder_count: usize,
}

/// 一条服务器命令的应答。
struct CommandReply {
    status: String,
    detail: String,
    lines: Vec<String>,
}

/// 执行一次 INBOX 连接自检。
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
        return Err(ConnectionError::protocol("收件服务器地址为空"));
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

    let greeting = read_crlf_line(&mut stream, 65536).await?;
    let greeting_upper = greeting.to_ascii_uppercase();
    let mut already_authenticated = false;
    if greeting_upper.starts_with("* PREAUTH") {
        already_authenticated = true;
    } else if greeting_upper.starts_with("* OK") {
        // 正常欢迎语，继续登录流程。
    } else if greeting_upper.starts_with("* BYE") {
        return Err(ConnectionError::rejected("服务器拒绝连接，并主动断开了会话"));
    } else {
        return Err(ConnectionError::protocol(
            "服务器欢迎语无法识别：请确认收件地址与端口是否正确",
        ));
    }

    if request.security == Security::StartTls {
        let reply = send_command(&mut stream, "a001", "STARTTLS").await?;
        if reply.status != "OK" {
            return Err(ConnectionError::tls(
                "服务器没有接受 STARTTLS 升级请求，请改用 SSL/TLS 或检查端口",
            ));
        }
        stream = stream.wrap_tls(&request.host).await?;
    }

    if !already_authenticated {
        let user = quote_imap_string(&request.username)?;
        let password = quote_imap_string(request.password.expose())?;
        let reply = send_command(&mut stream, "a002", &format!("LOGIN {user} {password}")).await?;
        if reply.status != "OK" {
            let cleaned = mail_net::error::redact(&reply.detail, &[request.password.expose()]);
            tracing::debug!(reply = %cleaned, "IMAP 登录未通过");
            return Err(ConnectionError::auth(
                "登录被服务器拒绝：请检查登录名与授权码（多数邮箱需要单独申请授权码）",
            ));
        }
    }

    let reply = send_command(&mut stream, "a003", "LIST \"\" \"*\"").await?;
    if reply.status != "OK" {
        return Err(ConnectionError::protocol("读取文件夹列表失败"));
    }
    let folder_count = reply
        .lines
        .iter()
        .filter(|line| line.to_ascii_uppercase().starts_with("* LIST"))
        .count();

    // 规格要求自检必须真的能打开收件箱，只数文件夹不算过。
    let reply = send_command(&mut stream, "a004", "SELECT \"INBOX\"").await?;
    if reply.status != "OK" {
        return Err(ConnectionError::rejected(
            "打开收件箱失败：服务器不允许选择 INBOX",
        ));
    }

    let _ = send_command(&mut stream, "a005", "LOGOUT").await;

    Ok(ProbeReport { folder_count })
}

async fn send_command(
    stream: &mut Stream,
    tag: &str,
    command: &str,
) -> Result<CommandReply, ConnectionError> {
    write_crlf_line(stream, &format!("{tag} {command}")).await?;
    let prefix = format!("{tag} ");
    let mut lines = Vec::new();
    loop {
        let line = read_crlf_line(stream, 65536).await?;
        if let Some(rest) = line.strip_prefix(&prefix) {
            let status = rest
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            return Ok(CommandReply {
                status,
                detail: rest.to_string(),
                lines,
            });
        }
        lines.push(line);
    }
}

/// 把字符串包成 IMAP 的引号形式，转义反斜杠与引号。
fn quote_imap_string(value: &str) -> Result<String, ConnectionError> {
    if value.contains(['\r', '\n']) {
        return Err(ConnectionError::protocol("登录名或授权码包含不支持的换行符"));
    }
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!("\"{escaped}\""))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mail_domain::account::Security;
    use mail_domain::error::ConnectionErrorKind;
    use mail_domain::proxy::Secret;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    use super::{probe, quote_imap_string, ProbeRequest};

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
    async fn 自检成功时能数出文件夹() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("接受连接失败");
            let mut reader = BufReader::new(socket);
            reader
                .get_mut()
                .write_all(b"* OK IMAP4rev1 ready\r\n")
                .await
                .expect("写入失败");
            let login = read_line(&mut reader).await;
            assert_eq!(login, "a002 LOGIN \"user@example.com\" \"pw-secret\"");
            reader
                .get_mut()
                .write_all(b"a002 OK LOGIN completed\r\n")
                .await
                .expect("写入失败");
            let list = read_line(&mut reader).await;
            assert_eq!(list, "a003 LIST \"\" \"*\"");
            reader
                .get_mut()
                .write_all(
                    b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n* LIST (\\HasNoChildren) \"/\" \"Sent\"\r\na003 OK LIST completed\r\n",
                )
                .await
                .expect("写入失败");
            let select = read_line(&mut reader).await;
            assert_eq!(select, "a004 SELECT \"INBOX\"");
            reader
                .get_mut()
                .write_all(b"* 12 EXISTS\r\na004 OK SELECT completed\r\n")
                .await
                .expect("写入失败");
            let logout = read_line(&mut reader).await;
            assert_eq!(logout, "a005 LOGOUT");
            reader
                .get_mut()
                .write_all(b"* BYE\r\na005 OK\r\n")
                .await
                .expect("写入失败");
        });

        let report = probe(&request(addr.port(), Security::Plain), None)
            .await
            .expect("自检应通过");
        assert_eq!(report.folder_count, 2);
    }

    #[tokio::test]
    async fn 无法打开收件箱时归类为服务器拒绝() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("接受连接失败");
            let mut reader = BufReader::new(socket);
            reader
                .get_mut()
                .write_all(b"* OK IMAP4rev1 ready\r\n")
                .await
                .expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"a002 OK LOGIN completed\r\n")
                .await
                .expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"a003 OK LIST completed\r\n")
                .await
                .expect("写入失败");
            let select = read_line(&mut reader).await;
            assert_eq!(select, "a004 SELECT \"INBOX\"");
            reader
                .get_mut()
                .write_all(b"a004 NO SELECT failed\r\n")
                .await
                .expect("写入失败");
        });

        let err = probe(&request(addr.port(), Security::Plain), None)
            .await
            .expect_err("应打开失败");
        assert_eq!(err.kind, ConnectionErrorKind::Rejected);
        assert!(err.message.contains("打开收件箱失败"));
    }

    #[tokio::test]
    async fn 登录被拒时归类为认证失败且不泄露授权码() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("接受连接失败");
            let mut reader = BufReader::new(socket);
            reader
                .get_mut()
                .write_all(b"* OK ready\r\n")
                .await
                .expect("写入失败");
            let login = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(format!("a002 NO login failed for {login}\r\n").as_bytes())
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
    async fn 服务器不接受starttls时归类为加密失败() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("接受连接失败");
            let mut reader = BufReader::new(socket);
            reader
                .get_mut()
                .write_all(b"* OK ready\r\n")
                .await
                .expect("写入失败");
            let starttls = read_line(&mut reader).await;
            assert_eq!(starttls, "a001 STARTTLS");
            reader
                .get_mut()
                .write_all(b"a001 NO STARTTLS not supported\r\n")
                .await
                .expect("写入失败");
        });

        let err = probe(&request(addr.port(), Security::StartTls), None)
            .await
            .expect_err("应升级失败");
        assert_eq!(err.kind, ConnectionErrorKind::TlsFailure);
    }

    #[tokio::test]
    async fn tls握手失败时归类为加密失败() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接失败");
            socket.write_all(b"not a tls server\r\n").await.expect("写入失败");
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let err = probe(&request(addr.port(), Security::Tls), None)
            .await
            .expect_err("应握手失败");
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

    #[test]
    fn 引号转义正确且换行被拒绝() {
        assert_eq!(quote_imap_string("a\"b\\c").expect("应成功"), "\"a\\\"b\\\\c\"");
        assert!(quote_imap_string("a\r\nb").is_err());
    }
}
