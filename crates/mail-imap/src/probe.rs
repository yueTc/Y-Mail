//! IMAP 连接自检：连上服务器 → 可选 STARTTLS → 登录 → 报身份 → 数文件夹 → 选收件箱 → 退出。
//!
//! 安全约定：授权码只用于拼 LOGIN 命令；错误信息与日志里必须先脱敏。
//! 兼容约定：网易 163/126 这类 Coremail 服务器要求登录后先用 ID 命令自报身份（RFC 2971），
//! 否则会以「Unsafe Login」为由拒绝打开收件箱；服务器广告了 ID 能力就主动发送。

use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use mail_domain::account::Security;
use mail_domain::auth::{xoauth2_sasl, AuthMaterial};
use mail_domain::error::ConnectionError;
use mail_domain::proxy::ProxyRoute;
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
    /// 认证材料：授权码或 OAuth2 访问令牌；绝不进日志与错误信息。
    pub auth: AuthMaterial,
    /// 整个自检的超时时间。
    pub timeout: Duration,
}

/// 自检通过后的观察结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// 能看到的文件夹数量（至少应有收件箱）。
    pub folder_count: usize,
}

/// 自报身份时用的客户端名字，出现在 IMAP ID 命令里。
const IMAP_CLIENT_NAME: &str = "em-master";

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
    if request.auth.is_empty() {
        let hint = if request.auth.is_bearer() {
            "该账号还没有可用的登录令牌，请重新授权"
        } else {
            "请先填写授权码"
        };
        return Err(ConnectionError::auth(hint));
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
        match &request.auth {
            AuthMaterial::Password(_) => {
                let user = quote_imap_string(&request.username)?;
                let password = quote_imap_string(request.auth.expose())?;
                let reply = send_command(&mut stream, "a002", &format!("LOGIN {user} {password}")).await?;
                if reply.status != "OK" {
                    let cleaned = mail_net::error::redact(&reply.detail, &[request.auth.expose()]);
                    tracing::debug!(reply = %cleaned, "IMAP 登录未通过");
                    return Err(ConnectionError::auth(
                        "登录被服务器拒绝：请检查登录名与授权码（多数邮箱需要单独申请授权码）",
                    ));
                }
            }
            AuthMaterial::Bearer(_) => {
                let reply =
                    xoauth2_login(&mut stream, "a002", &request.username, request.auth.expose()).await?;
                if reply.status != "OK" {
                    let cleaned = mail_net::error::redact(&reply.detail, &[request.auth.expose()]);
                    tracing::debug!(reply = %cleaned, "IMAP XOAUTH2 登录未通过");
                    return Err(ConnectionError::auth(
                        "OAuth2 授权被服务器拒绝：授权可能已过期，请重新授权",
                    ));
                }
            }
        }
    }

    // 先问能力：广告了 ID 才发，普通服务器不受影响。
    let capability = send_command(&mut stream, "a003", "CAPABILITY").await?;
    // 命令编号按实际发出的命令顺延；跳过的命令不占号。
    let mut next_tag = 4u32;
    if capability.status == "OK" {
        let supports_id = capability.lines.iter().any(|line| {
            line.to_ascii_uppercase()
                .strip_prefix("* CAPABILITY")
                .is_some_and(|rest| rest.split_whitespace().any(|token| token == "ID"))
        });
        if supports_id {
            let id = format!(
                "ID (\"name\" \"{}\" \"version\" \"{}\" \"vendor\" \"{}\")",
                IMAP_CLIENT_NAME,
                env!("CARGO_PKG_VERSION"),
                IMAP_CLIENT_NAME
            );
            match send_command(&mut stream, &format!("a{next_tag:03}"), &id).await {
                Ok(reply) if reply.status == "OK" => {}
                Ok(reply) => {
                    // 服务器不收 ID 不致命，继续往下走，让真正的问题自己暴露。
                    tracing::debug!(status = %reply.status, "IMAP ID 命令未被接受");
                }
                Err(err) => return Err(err),
            }
            next_tag += 1;
        }
    }

    let reply = send_command(&mut stream, &format!("a{next_tag:03}"), "LIST \"\" \"*\"").await?;
    next_tag += 1;
    if reply.status != "OK" {
        let detail = sanitize_detail(&reply.detail, request.auth.expose());
        return Err(ConnectionError::protocol(format!("读取文件夹列表失败：{detail}")));
    }
    let folder_count = reply
        .lines
        .iter()
        .filter(|line| line.to_ascii_uppercase().starts_with("* LIST"))
        .count();

    // 规格要求自检必须真的能打开收件箱，只数文件夹不算过。
    let reply = send_command(&mut stream, &format!("a{next_tag:03}"), "SELECT \"INBOX\"").await?;
    next_tag += 1;
    if reply.status != "OK" {
        let detail = sanitize_detail(&reply.detail, request.auth.expose());
        return Err(ConnectionError::rejected(format!("打开收件箱失败：{detail}")));
    }

    let _ = send_command(&mut stream, &format!("a{next_tag:03}"), "LOGOUT").await;

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

/// 用 XOAUTH2 发一条登录命令；服务器回 `+` 挑战时补一个空行再等最终应答。
async fn xoauth2_login(
    stream: &mut Stream,
    tag: &str,
    username: &str,
    token: &str,
) -> Result<CommandReply, ConnectionError> {
    let payload = STANDARD.encode(xoauth2_sasl(username, token));
    write_crlf_line(stream, &format!("{tag} AUTHENTICATE XOAUTH2 {payload}")).await?;
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
        if line.starts_with('+') {
            write_crlf_line(stream, "").await?;
            continue;
        }
        lines.push(line);
    }
}

/// 服务器原文先脱敏再展示，避免任何角落回显授权码。
fn sanitize_detail(detail: &str, secret: &str) -> String {
    let cleaned = mail_net::error::redact(detail, &[secret]);
    cleaned.trim().to_string()
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

    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    use mail_domain::account::Security;
    use mail_domain::auth::AuthMaterial;
    use mail_domain::error::ConnectionErrorKind;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    use super::{probe, quote_imap_string, ProbeRequest};

    fn request(port: u16, security: Security) -> ProbeRequest {
        ProbeRequest {
            host: "127.0.0.1".to_string(),
            port,
            security,
            username: "user@example.com".to_string(),
            auth: AuthMaterial::password("pw-secret"),
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
            let capability = read_line(&mut reader).await;
            assert_eq!(capability, "a003 CAPABILITY");
            reader
                .get_mut()
                .write_all(b"* CAPABILITY IMAP4rev1 ID\r\na003 OK CAPABILITY completed\r\n")
                .await
                .expect("写入失败");
            let id = read_line(&mut reader).await;
            assert!(id.starts_with("a004 ID ("), "应发出 ID 命令：{id}");
            assert!(id.contains("\"name\" \"em-master\""));
            reader
                .get_mut()
                .write_all(b"* ID (\"name\" \"em-master\")\r\na004 OK ID completed\r\n")
                .await
                .expect("写入失败");
            let list = read_line(&mut reader).await;
            assert_eq!(list, "a005 LIST \"\" \"*\"");
            reader
                .get_mut()
                .write_all(
                    b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n* LIST (\\HasNoChildren) \"/\" \"Sent\"\r\na005 OK LIST completed\r\n",
                )
                .await
                .expect("写入失败");
            let select = read_line(&mut reader).await;
            assert_eq!(select, "a006 SELECT \"INBOX\"");
            reader
                .get_mut()
                .write_all(b"* 12 EXISTS\r\na006 OK SELECT completed\r\n")
                .await
                .expect("写入失败");
            let logout = read_line(&mut reader).await;
            assert_eq!(logout, "a007 LOGOUT");
            reader
                .get_mut()
                .write_all(b"* BYE\r\na007 OK\r\n")
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
            let _login = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"a002 OK LOGIN completed\r\n")
                .await
                .expect("写入失败");
            let capability = read_line(&mut reader).await;
            assert_eq!(capability, "a003 CAPABILITY");
            reader
                .get_mut()
                .write_all(b"* CAPABILITY IMAP4rev1\r\na003 OK CAPABILITY completed\r\n")
                .await
                .expect("写入失败");
            let list = read_line(&mut reader).await;
            assert_eq!(list, "a004 LIST \"\" \"*\"");
            reader
                .get_mut()
                .write_all(b"a004 OK LIST completed\r\n")
                .await
                .expect("写入失败");
            let select = read_line(&mut reader).await;
            assert_eq!(select, "a005 SELECT \"INBOX\"");
            reader
                .get_mut()
                .write_all(b"a005 NO Unsafe Login, please contact kefu@188.com\r\n")
                .await
                .expect("写入失败");
        });

        let err = probe(&request(addr.port(), Security::Plain), None)
            .await
            .expect_err("应打开失败");
        assert_eq!(err.kind, ConnectionErrorKind::Rejected);
        assert!(err.message.contains("打开收件箱失败"));
        assert!(
            err.message.contains("Unsafe Login"),
            "错误文案应带上服务器原文：{}",
            err.message
        );
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

    #[tokio::test]
    async fn 授权账号走xoauth2登录() {
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
            let auth = read_line(&mut reader).await;
            let prefix = "a002 AUTHENTICATE XOAUTH2 ";
            assert!(auth.starts_with(prefix), "应发出 XOAUTH2 命令：{auth}");
            let payload = STANDARD.decode(&auth[prefix.len()..]).expect("base64 应能解码");
            let mut expected = Vec::new();
            expected.extend_from_slice(b"user=user@example.com");
            expected.push(0x01);
            expected.extend_from_slice(b"auth=Bearer tok-abc");
            expected.push(0x01);
            expected.push(0x01);
            assert_eq!(payload, expected, "SASL 初始响应应拼装正确");
            reader
                .get_mut()
                .write_all(b"a002 OK AUTHENTICATE completed\r\n")
                .await
                .expect("写入失败");
            let capability = read_line(&mut reader).await;
            assert_eq!(capability, "a003 CAPABILITY");
            reader
                .get_mut()
                .write_all(b"* CAPABILITY IMAP4rev1\r\na003 OK CAPABILITY completed\r\n")
                .await
                .expect("写入失败");
            let list = read_line(&mut reader).await;
            assert_eq!(list, "a004 LIST \"\" \"*\"");
            reader
                .get_mut()
                .write_all(b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\na004 OK LIST completed\r\n")
                .await
                .expect("写入失败");
            let select = read_line(&mut reader).await;
            assert_eq!(select, "a005 SELECT \"INBOX\"");
            reader
                .get_mut()
                .write_all(b"a005 OK SELECT completed\r\n")
                .await
                .expect("写入失败");
            let logout = read_line(&mut reader).await;
            assert_eq!(logout, "a006 LOGOUT");
            reader
                .get_mut()
                .write_all(b"a006 OK\r\n")
                .await
                .expect("写入失败");
        });

        let mut req = request(addr.port(), Security::Plain);
        req.auth = AuthMaterial::bearer("tok-abc");
        let report = probe(&req, None).await.expect("自检应通过");
        assert_eq!(report.folder_count, 1);
    }

    #[tokio::test]
    async fn 服务器不广告id时跳过并完成自检() {
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
            let _login = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"a002 OK LOGIN completed\r\n")
                .await
                .expect("写入失败");
            let capability = read_line(&mut reader).await;
            assert_eq!(capability, "a003 CAPABILITY");
            reader
                .get_mut()
                .write_all(b"* CAPABILITY IMAP4rev1 AUTH=PLAIN\r\na003 OK CAPABILITY completed\r\n")
                .await
                .expect("写入失败");
            // 没有 ID 能力，下一个命令必须直接是 LIST，绝不能是 ID。
            let list = read_line(&mut reader).await;
            assert_eq!(list, "a004 LIST \"\" \"*\"");
            reader
                .get_mut()
                .write_all(b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\na004 OK LIST completed\r\n")
                .await
                .expect("写入失败");
            let select = read_line(&mut reader).await;
            assert_eq!(select, "a005 SELECT \"INBOX\"");
            reader
                .get_mut()
                .write_all(b"a005 OK SELECT completed\r\n")
                .await
                .expect("写入失败");
            let logout = read_line(&mut reader).await;
            assert_eq!(logout, "a006 LOGOUT");
            reader
                .get_mut()
                .write_all(b"a006 OK\r\n")
                .await
                .expect("写入失败");
        });

        let report = probe(&request(addr.port(), Security::Plain), None)
            .await
            .expect("自检应通过");
        assert_eq!(report.folder_count, 1);
    }

    #[tokio::test]
    async fn id被拒也不影响自检通过() {
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
            let _login = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"a002 OK LOGIN completed\r\n")
                .await
                .expect("写入失败");
            let capability = read_line(&mut reader).await;
            assert_eq!(capability, "a003 CAPABILITY");
            reader
                .get_mut()
                .write_all(b"* CAPABILITY IMAP4rev1 ID\r\na003 OK CAPABILITY completed\r\n")
                .await
                .expect("写入失败");
            let id = read_line(&mut reader).await;
            assert!(id.starts_with("a004 ID ("), "应发出 ID 命令：{id}");
            reader
                .get_mut()
                .write_all(b"a004 BAD ID not allowed\r\n")
                .await
                .expect("写入失败");
            let list = read_line(&mut reader).await;
            assert_eq!(list, "a005 LIST \"\" \"*\"");
            reader
                .get_mut()
                .write_all(b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\na005 OK LIST completed\r\n")
                .await
                .expect("写入失败");
            let select = read_line(&mut reader).await;
            assert_eq!(select, "a006 SELECT \"INBOX\"");
            reader
                .get_mut()
                .write_all(b"a006 OK SELECT completed\r\n")
                .await
                .expect("写入失败");
            let logout = read_line(&mut reader).await;
            assert_eq!(logout, "a007 LOGOUT");
            reader
                .get_mut()
                .write_all(b"a007 OK\r\n")
                .await
                .expect("写入失败");
        });

        let report = probe(&request(addr.port(), Security::Plain), None)
            .await
            .expect("ID 被拒也不该影响自检");
        assert_eq!(report.folder_count, 1);
    }
    #[test]
    fn 引号转义正确且换行被拒绝() {
        assert_eq!(quote_imap_string("a\"b\\c").expect("应成功"), "\"a\\\"b\\\\c\"");
        assert!(quote_imap_string("a\r\nb").is_err());
    }
}
