//! SMTP 协议公共部分：建连、EHLO、STARTTLS 与认证。
//!
//! 连接自检（`probe`）与真实投递（`send`）共用这里，避免两套实现各写一遍。
//! 安全约定：授权码只用于拼认证命令；认证失败时服务器应答正文一律不写日志，
//! 因为部分服务器会在应答里回显 base64 后的账号与授权码。

use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use mail_domain::account::Security;
use mail_domain::auth::{xoauth2_sasl, AuthMaterial};
use mail_domain::error::ConnectionError;
use mail_domain::proxy::ProxyRoute;
use mail_net::io::{read_crlf_line, write_crlf_line};
use mail_net::{connect_tcp, tls_wrap, Stream};

/// 一条服务器应答（可能是多行）。
#[derive(Debug, Clone)]
pub(crate) struct Reply {
    /// 应答码（多行取第一行的码）。
    pub code: u16,
    /// 每行的原始文本（含应答码与分隔符）。
    pub lines: Vec<String>,
}

impl Reply {
    /// 应答正文：去掉每行前四个字符（应答码与分隔符）后拼成一句。
    pub fn summary(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.chars().skip(4).collect::<String>())
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// 建连并检查 220 欢迎语；隐式 TLS 在这一步就套上。
pub(crate) async fn connect_greeted(
    host: &str,
    port: u16,
    security: Security,
    route: Option<&ProxyRoute>,
    timeout: Duration,
) -> Result<Stream, ConnectionError> {
    let tcp = connect_tcp(host, port, route, timeout).await?;
    let mut stream = match security {
        Security::Tls => tls_wrap(tcp, host).await?,
        Security::StartTls | Security::Plain => Stream::plain(tcp),
    };
    let greeting = read_reply(&mut stream).await?;
    if greeting.code != 220 {
        return Err(ConnectionError::rejected(format!(
            "服务器没有正常欢迎连接（返回码 {}）",
            greeting.code
        )));
    }
    Ok(stream)
}

/// 发 EHLO；STARTTLS 时升级加密并重新 EHLO，返回升级后的连接与最后一次 EHLO 应答。
pub(crate) async fn start_session(
    mut stream: Stream,
    security: Security,
    host: &str,
) -> Result<(Stream, Reply), ConnectionError> {
    let mut ehlo = send_ehlo(&mut stream).await?;
    if security == Security::StartTls {
        let reply = send_command(&mut stream, "STARTTLS").await?;
        if reply.code != 220 {
            return Err(ConnectionError::tls(
                "服务器没有接受 STARTTLS 升级请求，请改用 SSL/TLS 或检查端口",
            ));
        }
        stream = stream.wrap_tls(host).await?;
        ehlo = send_ehlo(&mut stream).await?;
    }
    Ok((stream, ehlo))
}

/// 按服务器广告的方式认证；返回实际使用的方式名。
///
/// 普通密码走 PLAIN 优先、退 LOGIN；OAuth2 令牌只走 XOAUTH2，
/// 绝不会把令牌当成密码塞进 PLAIN / LOGIN。
pub(crate) async fn authenticate(
    stream: &mut Stream,
    username: &str,
    auth: &AuthMaterial,
    ehlo: &Reply,
) -> Result<&'static str, ConnectionError> {
    let mechanisms = parse_auth_mechanisms(ehlo);
    if auth.is_bearer() {
        if mechanisms.iter().any(|item| item == "XOAUTH2") {
            auth_xoauth2(stream, username, auth.expose()).await?;
            Ok("XOAUTH2")
        } else {
            Err(ConnectionError::auth(
                "服务器没有广告 XOAUTH2，无法用 OAuth2 令牌登录",
            ))
        }
    } else if mechanisms.iter().any(|item| item == "PLAIN") {
        auth_plain(stream, username, auth.expose()).await?;
        Ok("PLAIN")
    } else if mechanisms.iter().any(|item| item == "LOGIN") {
        auth_login(stream, username, auth.expose()).await?;
        Ok("LOGIN")
    } else if mechanisms.is_empty() {
        // 少数服务器不主动广告认证方式，仍按最常见的 PLAIN 试一次。
        auth_plain(stream, username, auth.expose()).await?;
        Ok("PLAIN")
    } else {
        Err(ConnectionError::auth(
            "服务器没有提供受支持的认证方式（仅支持 PLAIN、LOGIN 或 XOAUTH2）",
        ))
    }
}

pub(crate) async fn send_ehlo(stream: &mut Stream) -> Result<Reply, ConnectionError> {
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
///
/// 只保留我们支持的三种，重复广告只记一次；顺序按服务器广告的先后。
fn parse_auth_mechanisms(reply: &Reply) -> Vec<String> {
    let mut mechanisms: Vec<String> = Vec::new();
    for line in &reply.lines {
        let upper = line.to_ascii_uppercase();
        let Some(index) = upper.find("AUTH") else {
            continue;
        };
        let rest = &upper[index + 4..];
        // 形如「AUTH=PLAIN LOGIN」时先去掉等号。
        let rest = rest.trim_start().trim_start_matches('=');
        for item in rest.split_whitespace() {
            if matches!(item, "PLAIN" | "LOGIN" | "XOAUTH2") && !mechanisms.iter().any(|seen| seen == item) {
                mechanisms.push(item.to_string());
            }
        }
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

async fn auth_plain(stream: &mut Stream, username: &str, password: &str) -> Result<(), ConnectionError> {
    let token = plain_token(username, password);
    let reply = send_command(stream, &format!("AUTH PLAIN {token}")).await?;
    check_auth_reply(&reply, "PLAIN")
}

async fn auth_login(stream: &mut Stream, username: &str, password: &str) -> Result<(), ConnectionError> {
    let reply = send_command(stream, "AUTH LOGIN").await?;
    if reply.code != 334 {
        return Err(auth_error(&reply, "LOGIN"));
    }
    let reply = send_command(stream, &STANDARD.encode(username)).await?;
    if reply.code != 334 {
        return Err(auth_error(&reply, "LOGIN"));
    }
    let reply = send_command(stream, &STANDARD.encode(password)).await?;
    check_auth_reply(&reply, "LOGIN")
}

/// XOAUTH2：一条命令带上 SASL 串。部分服务器失败时先回 334 挑战，
/// 客户端需要回一个空行，服务器才给最终应答。
async fn auth_xoauth2(stream: &mut Stream, username: &str, token: &str) -> Result<(), ConnectionError> {
    let sasl = STANDARD.encode(xoauth2_sasl(username, token));
    let reply = send_command(stream, &format!("AUTH XOAUTH2 {sasl}")).await?;
    if reply.code == 334 {
        let reply = send_command(stream, "").await?;
        return check_auth_reply(&reply, "XOAUTH2");
    }
    check_auth_reply(&reply, "XOAUTH2")
}

fn check_auth_reply(reply: &Reply, mechanism: &str) -> Result<(), ConnectionError> {
    if reply.code == 235 {
        return Ok(());
    }
    // 服务器应答正文可能回显 base64 后的账号与授权码，认证失败时整段不写日志。
    tracing::debug!(code = reply.code, mechanism, "SMTP 认证未通过，响应正文不记录");
    Err(auth_error(reply, mechanism))
}

fn auth_error(reply: &Reply, mechanism: &str) -> ConnectionError {
    let hint = if mechanism == "XOAUTH2" {
        "请到账号设置里重新授权"
    } else {
        "请检查登录名与授权码"
    };
    ConnectionError::auth(format!("登录被服务器拒绝（返回码 {}）：{hint}", reply.code))
}

pub(crate) async fn send_command(stream: &mut Stream, command: &str) -> Result<Reply, ConnectionError> {
    write_crlf_line(stream, command).await?;
    read_reply(stream).await
}

/// 读一条（可能是多行的）SMTP 应答，例如 `250-XXX` 连续多行后以 `250 YYY` 结束。
pub(crate) async fn read_reply(stream: &mut Stream) -> Result<Reply, ConnectionError> {
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
