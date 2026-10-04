//! SMTP 投递（Wave 5）。
//!
//! 流程：建连 → EHLO → 可选 STARTTLS → 认证 → MAIL FROM → 逐个 RCPT TO
//! → DATA（正文做点号转义）→ QUIT。
//!
//! 错误分类只有四类：临时（4xx，可重试）、永久（5xx，不重试）、认证、网络。
//! 重试策略不在本 crate：投递本身不重发，由 mail-core 的 outbox 队列统一控制，
//! 避免网络抖动时重复投递同一封信。

use std::time::Duration;

use mail_domain::account::Security;
use mail_domain::error::{ConnectionError, ConnectionErrorKind};
use mail_domain::proxy::{ProxyRoute, Secret};
use mail_net::Stream;
use tokio::io::AsyncWriteExt;

use crate::protocol::{authenticate, connect_greeted, read_reply, send_command, start_session};

/// 单封邮件上限；与 MIME 组装层保持一致。
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

/// 投递需要的全部信息。
#[derive(Clone, PartialEq, Eq)]
pub struct SendRequest {
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
    /// 整个投递的超时时间。
    pub timeout: Duration,
    /// 信封发件人。
    pub mail_from: String,
    /// 信封收件人（发件人自己不在其中）。
    pub recipients: Vec<String>,
    /// 已组装好的 MIME 字节（CRLF 行尾）。
    pub raw: Vec<u8>,
}

impl std::fmt::Debug for SendRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 授权码不进 Debug 输出，避免被上层日志间接带出去。
        f.debug_struct("SendRequest")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("username", &self.username)
            .field("mail_from", &self.mail_from)
            .field("recipients", &self.recipients)
            .field("raw_bytes", &self.raw.len())
            .finish_non_exhaustive()
    }
}

/// 投递结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReport {
    /// 服务器接受的收件人数量。
    pub accepted_recipients: usize,
}

/// 投递失败的大类，决定 outbox 是退回队列还是标记永久失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendErrorKind {
    /// 临时失败（4xx、网络抖动、超时），可以稍后重试。
    Temporary,
    /// 永久失败（5xx、地址被拒），重试没有意义。
    Permanent,
    /// 认证失败，需要用户检查授权码。
    Auth,
    /// 连接阶段失败（DNS、TCP、代理、加密握手），当作临时失败重试。
    Network,
}

impl SendErrorKind {
    /// 是否值得自动重试。
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Temporary | Self::Network)
    }
}

/// 一条面向用户的投递错误；`message` 已脱敏，可直接进 outbox.last_error。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendError {
    /// 失败大类。
    pub kind: SendErrorKind,
    /// 服务器返回码（连接阶段失败时为 None）。
    pub code: Option<u16>,
    /// 脱敏后的中文描述。
    pub message: String,
}

impl SendError {
    fn new(kind: SendErrorKind, code: Option<u16>, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
        }
    }

    /// 由连接阶段错误转换。
    fn from_connection(error: ConnectionError) -> Self {
        let kind = match error.kind {
            ConnectionErrorKind::AuthFailed => SendErrorKind::Auth,
            _ => SendErrorKind::Network,
        };
        Self::new(kind, None, error.message)
    }

    /// 由服务器返回码转换：4xx 临时，5xx 永久。
    fn from_reply(stage: &str, code: u16, summary: &str) -> Self {
        let kind = if code >= 500 {
            SendErrorKind::Permanent
        } else {
            SendErrorKind::Temporary
        };
        let detail = if summary.is_empty() {
            String::new()
        } else {
            format!("：{summary}")
        };
        Self::new(
            kind,
            Some(code),
            format!("服务器在{stage}阶段拒绝了本次投递（返回码 {code}）{detail}"),
        )
    }
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SendError {}

/// 投递一封已经组装好的邮件。不做重试。
pub async fn send(request: &SendRequest, route: Option<&ProxyRoute>) -> Result<SendReport, SendError> {
    validate(request)?;
    match tokio::time::timeout(request.timeout, run(request, route)).await {
        Ok(result) => result,
        Err(_) => Err(SendError::new(
            SendErrorKind::Temporary,
            None,
            "投递超时：服务器在规定时间内没有完成应答",
        )),
    }
}

fn validate(request: &SendRequest) -> Result<(), SendError> {
    if request.host.trim().is_empty() {
        return Err(SendError::new(
            SendErrorKind::Permanent,
            None,
            "发件服务器地址为空，请到账号设置里检查",
        ));
    }
    if request.username.trim().is_empty() {
        return Err(SendError::new(
            SendErrorKind::Auth,
            None,
            "登录名为空，请到账号设置里检查",
        ));
    }
    if request.password.is_empty() {
        return Err(SendError::new(
            SendErrorKind::Auth,
            None,
            "账号没有可用的授权码，请到账号设置里重新填写",
        ));
    }
    if request.mail_from.trim().is_empty() {
        return Err(SendError::new(
            SendErrorKind::Permanent,
            None,
            "发件人地址为空，无法投递",
        ));
    }
    if request.recipients.is_empty() {
        return Err(SendError::new(
            SendErrorKind::Permanent,
            None,
            "没有收件人，无法投递",
        ));
    }
    if request.raw.is_empty() {
        return Err(SendError::new(
            SendErrorKind::Permanent,
            None,
            "邮件内容为空，无法投递",
        ));
    }
    if request.raw.len() > MAX_MESSAGE_BYTES {
        return Err(SendError::new(
            SendErrorKind::Permanent,
            None,
            format!("邮件太大（超过 {MAX_MESSAGE_BYTES} 字节），请减少附件后重试"),
        ));
    }
    Ok(())
}

async fn run(request: &SendRequest, route: Option<&ProxyRoute>) -> Result<SendReport, SendError> {
    let stream = connect_greeted(
        &request.host,
        request.port,
        request.security,
        route,
        request.timeout,
    )
    .await
    .map_err(SendError::from_connection)?;
    let (mut stream, ehlo) = start_session(stream, request.security, &request.host)
        .await
        .map_err(SendError::from_connection)?;

    authenticate(&mut stream, &request.username, request.password.expose(), &ehlo)
        .await
        .map_err(SendError::from_connection)?;

    let from = sanitize_path(&request.mail_from);
    expect_reply(&mut stream, &format!("MAIL FROM:<{from}>"), "发件人").await?;

    for recipient in &request.recipients {
        let address = sanitize_path(recipient);
        let command = format!("RCPT TO:<{address}>");
        let reply = send_command(&mut stream, &command)
            .await
            .map_err(SendError::from_connection)?;
        if reply.code != 250 && reply.code != 251 {
            return Err(SendError::from_reply("收件人", reply.code, &reply.summary()));
        }
    }

    let reply = send_command(&mut stream, "DATA")
        .await
        .map_err(SendError::from_connection)?;
    if reply.code != 354 {
        return Err(SendError::from_reply("正文", reply.code, &reply.summary()));
    }
    write_data(&mut stream, &request.raw)
        .await
        .map_err(SendError::from_connection)?;
    let reply = read_reply(&mut stream)
        .await
        .map_err(SendError::from_connection)?;
    if reply.code != 250 {
        return Err(SendError::from_reply("投递", reply.code, &reply.summary()));
    }

    // 退出失败不影响「已经投递成功」的结论。
    match send_command(&mut stream, "QUIT").await {
        Ok(reply) if reply.code != 221 => {
            tracing::debug!(code = reply.code, "退出命令没有得到预期应答");
        }
        Ok(_) => {}
        Err(error) => {
            tracing::debug!(kind = %error.kind.label(), "退出命令失败，不影响投递结果");
        }
    }

    Ok(SendReport {
        accepted_recipients: request.recipients.len(),
    })
}

/// 发一条期望 250 的命令（MAIL FROM）。
async fn expect_reply(stream: &mut Stream, command: &str, stage: &str) -> Result<(), SendError> {
    let reply = send_command(stream, command)
        .await
        .map_err(SendError::from_connection)?;
    if reply.code != 250 {
        return Err(SendError::from_reply(stage, reply.code, &reply.summary()));
    }
    Ok(())
}

/// 写 DATA 内容：点号转义 + 结束标记 `\r\n.\r\n`。
async fn write_data(stream: &mut Stream, raw: &[u8]) -> Result<(), ConnectionError> {
    let mut payload = Vec::with_capacity(raw.len() + 16);
    // 逐行处理；行首的点号要双写，这是 SMTP 的转义规则。
    let mut start = 0usize;
    while start < raw.len() {
        let end = raw[start..]
            .windows(2)
            .position(|pair| pair == b"\r\n")
            .map(|offset| start + offset + 2)
            .unwrap_or(raw.len());
        let line = &raw[start..end];
        if line.first() == Some(&b'.') {
            payload.push(b'.');
        }
        payload.extend_from_slice(line);
        start = end;
    }
    if !payload.ends_with(b"\r\n") {
        payload.extend_from_slice(b"\r\n");
    }
    payload.extend_from_slice(b".\r\n");

    stream
        .write_all(&payload)
        .await
        .map_err(|error| ConnectionError::network(format!("发送邮件正文失败：{error}")))?;
    stream
        .flush()
        .await
        .map_err(|error| ConnectionError::network(format!("发送邮件正文失败：{error}")))
}

/// 去掉地址里的尖括号与首尾空白，防止把命令参数拼坏。
fn sanitize_path(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    use mail_domain::account::Security;
    use mail_domain::proxy::Secret;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    use super::{send, SendErrorKind, SendRequest};

    fn request(port: u16) -> SendRequest {
        SendRequest {
            host: "127.0.0.1".to_string(),
            port,
            security: Security::Plain,
            username: "user@example.com".to_string(),
            password: Secret::new("pw-secret"),
            timeout: Duration::from_secs(5),
            mail_from: "user@example.com".to_string(),
            recipients: vec!["to@example.com".to_string()],
            raw: b"Subject: test\r\n\r\nline 1\r\n.line 2\r\n".to_vec(),
        }
    }

    async fn read_line(reader: &mut BufReader<TcpStream>) -> String {
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("读取失败");
        line.trim_end().to_string()
    }

    /// 假服务器：欢迎 → EHLO/AUTH → 投递命令；`data_reply` 控制最终应答。
    async fn serve(listener: TcpListener, final_reply: &'static str) {
        let (socket, _) = listener.accept().await.expect("接受连接失败");
        let mut reader = BufReader::new(socket);
        reader
            .get_mut()
            .write_all(b"220 ready\r\n")
            .await
            .expect("写入失败");
        let ehlo = read_line(&mut reader).await;
        assert!(ehlo.starts_with("EHLO "), "{ehlo}");
        reader
            .get_mut()
            .write_all(b"250-smtp.example.com\r\n250 AUTH PLAIN\r\n")
            .await
            .expect("写入失败");
        let auth = read_line(&mut reader).await;
        assert!(auth.starts_with("AUTH PLAIN "), "{auth}");
        reader.get_mut().write_all(b"235 ok\r\n").await.expect("写入失败");
        let mail = read_line(&mut reader).await;
        assert_eq!(mail, "MAIL FROM:<user@example.com>");
        reader.get_mut().write_all(b"250 ok\r\n").await.expect("写入失败");
        let rcpt = read_line(&mut reader).await;
        assert_eq!(rcpt, "RCPT TO:<to@example.com>");
        reader.get_mut().write_all(b"250 ok\r\n").await.expect("写入失败");
        let data = read_line(&mut reader).await;
        assert_eq!(data, "DATA");
        reader
            .get_mut()
            .write_all(b"354 go ahead\r\n")
            .await
            .expect("写入失败");
        // 读到单独一行的点号为止，检查点号转义。
        let mut body = String::new();
        loop {
            let line = read_line(&mut reader).await;
            body.push_str(&line);
            body.push('\n');
            if line == "." {
                break;
            }
        }
        assert!(body.contains("..line 2"), "行首点号应双写：{body}");
        reader
            .get_mut()
            .write_all(final_reply.as_bytes())
            .await
            .expect("写入失败");
        if final_reply.starts_with("250") {
            let quit = read_line(&mut reader).await;
            assert_eq!(quit, "QUIT");
            reader
                .get_mut()
                .write_all(b"221 bye\r\n")
                .await
                .expect("写入失败");
        }
    }

    #[tokio::test]
    async fn 投递成功并按顺序走完命令() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(serve(listener, "250 2.0.0 queued as ABC\r\n"));

        let report = send(&request(addr.port()), None).await.expect("应投递成功");
        assert_eq!(report.accepted_recipients, 1);
    }

    #[tokio::test]
    async fn 认证被拒归类为认证失败且错误不含密码() {
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
                .write_all(b"250 AUTH PLAIN\r\n")
                .await
                .expect("写入失败");
            let auth = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(format!("535 bad credentials ({auth})\r\n").as_bytes())
                .await
                .expect("写入失败");
        });

        let err = send(&request(addr.port()), None).await.expect_err("应失败");
        assert_eq!(err.kind, SendErrorKind::Auth);
        assert!(!err.message.contains("pw-secret"));
        let encoded = {
            let mut raw = vec![0];
            raw.extend_from_slice(b"user@example.com");
            raw.push(0);
            raw.extend_from_slice(b"pw-secret");
            STANDARD.encode(raw)
        };
        assert!(!err.message.contains(&encoded));
    }

    #[tokio::test]
    async fn 收件人被临时拒绝归为可重试() {
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
                .write_all(b"250 AUTH PLAIN\r\n")
                .await
                .expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader.get_mut().write_all(b"235 ok\r\n").await.expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader.get_mut().write_all(b"250 ok\r\n").await.expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"450 mailbox busy\r\n")
                .await
                .expect("写入失败");
        });

        let err = send(&request(addr.port()), None).await.expect_err("应失败");
        assert_eq!(err.kind, SendErrorKind::Temporary);
        assert!(err.kind.is_retryable());
        assert_eq!(err.code, Some(450));
    }

    #[tokio::test]
    async fn 收件人被永久拒绝归为不可重试() {
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
                .write_all(b"250 AUTH PLAIN\r\n")
                .await
                .expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader.get_mut().write_all(b"235 ok\r\n").await.expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader.get_mut().write_all(b"250 ok\r\n").await.expect("写入失败");
            let _ = read_line(&mut reader).await;
            reader
                .get_mut()
                .write_all(b"550 no such user\r\n")
                .await
                .expect("写入失败");
        });

        let err = send(&request(addr.port()), None).await.expect_err("应失败");
        assert_eq!(err.kind, SendErrorKind::Permanent);
        assert!(!err.kind.is_retryable());
        assert_eq!(err.code, Some(550));
    }

    #[tokio::test]
    async fn 正文阶段临时拒绝可重试() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        tokio::spawn(serve(listener, "451 try later\r\n"));

        let err = send(&request(addr.port()), None).await.expect_err("应失败");
        assert_eq!(err.kind, SendErrorKind::Temporary);
        assert_eq!(err.code, Some(451));
    }

    #[tokio::test]
    async fn 连不上时归为网络类可重试() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("取地址失败");
        drop(listener);

        let err = send(&request(addr.port()), None).await.expect_err("应失败");
        assert_eq!(err.kind, SendErrorKind::Network);
        assert!(err.kind.is_retryable());
    }

    #[tokio::test]
    async fn 没有收件人时不连网直接拒绝() {
        let mut request = request(1);
        request.recipients.clear();
        let err = send(&request, None).await.expect_err("应失败");
        assert_eq!(err.kind, SendErrorKind::Permanent);
    }

    #[tokio::test]
    async fn 调试输出不包含授权码() {
        let request = request(1);
        let text = format!("{request:?}");
        assert!(!text.contains("pw-secret"), "{text}");
    }
}
