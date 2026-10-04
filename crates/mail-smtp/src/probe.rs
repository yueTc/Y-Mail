//! SMTP 连接自检：连上服务器 → EHLO → 可选 STARTTLS → 认证 → 退出。
//!
//! 安全约定：授权码只用于拼认证命令；错误信息与日志里必须先脱敏。

use std::time::Duration;

use mail_domain::account::Security;
use mail_domain::error::ConnectionError;
use mail_domain::proxy::{ProxyRoute, Secret};

use crate::protocol::{authenticate, connect_greeted, send_command, start_session};

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
    let stream = connect_greeted(
        &request.host,
        request.port,
        request.security,
        route,
        request.timeout,
    )
    .await?;
    let (mut stream, ehlo) = start_session(stream, request.security, &request.host).await?;

    let mechanism = authenticate(&mut stream, &request.username, request.password.expose(), &ehlo).await?;

    let reply = send_command(&mut stream, "QUIT").await?;
    if reply.code != 221 {
        tracing::debug!(code = reply.code, "退出命令没有得到预期应答");
    }

    Ok(ProbeReport {
        mechanism: mechanism.to_string(),
    })
}
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, OnceLock};
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

    /// 把 tracing 输出收进内存，用来断言日志里没有敏感内容。
    #[derive(Clone, Default)]
    struct LogCapture(Arc<Mutex<Vec<u8>>>);

    /// 全局订阅器只装一次，之后每条测试都把日志写进同一份缓冲区。
    ///
    /// 不能改用线程级订阅器：tracing 的调用点兴趣缓存是进程级的，一个
    /// 没有订阅器的线程先跑到某个调用点，就会把它缓存成「永不记录」，
    /// 别的线程再也收不到那条日志。
    fn log_buffer() -> &'static Arc<Mutex<Vec<u8>>> {
        static BUFFER: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
        BUFFER.get_or_init(|| {
            let logs = Arc::new(Mutex::new(Vec::new()));
            let subscriber = tracing_subscriber::fmt()
                .with_writer({
                    let logs = logs.clone();
                    move || LogCapture(logs.clone())
                })
                .with_max_level(tracing::Level::DEBUG)
                .finish();
            // 同进程里只可能装成功一次，失败说明已经有别的订阅器在兜底。
            let _ = tracing::subscriber::set_global_default(subscriber);
            logs
        })
    }

    impl std::io::Write for LogCapture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("日志锁").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
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
    async fn 认证被拒时归类为认证失败且日志不泄露授权码() {
        let expected = {
            let mut raw = vec![0];
            raw.extend_from_slice(b"user@example.com");
            raw.push(0);
            raw.extend_from_slice(b"pw-secret");
            STANDARD.encode(raw)
        };
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

        let logs = log_buffer();

        let err = probe(&request(addr.port(), Security::Plain), None)
            .await
            .expect_err("应认证失败");

        assert_eq!(err.kind, ConnectionErrorKind::AuthFailed);
        assert!(!err.message.contains("pw-secret"));
        let text = String::from_utf8(logs.lock().expect("日志锁").clone()).expect("日志应是文本");
        assert!(text.contains("SMTP 认证未通过"), "应记录失败日志：{text}");
        assert!(!text.contains(&expected), "日志不得包含 base64 认证令牌");
        assert!(!text.contains("pw-secret"), "日志不得包含明文授权码");
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
