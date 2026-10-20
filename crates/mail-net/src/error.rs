//! 错误归类与脱敏小工具。
//!
//! 约定：给用户看的错误必须是中文、可读、不含凭据；底层英文报错只进开发日志。

use std::io;

use mail_domain::error::ConnectionError;

/// 把系统网络错误翻成一句中文（不携带底层英文原文）。
pub fn describe_io(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::TimedOut => "等待响应超时",
        io::ErrorKind::ConnectionRefused => "对方拒绝连接",
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted => "连接被中途断开",
        io::ErrorKind::NotFound | io::ErrorKind::AddrNotAvailable => "找不到该地址",
        io::ErrorKind::PermissionDenied => "系统不允许建立该连接",
        io::ErrorKind::AddrInUse => "本地端口已被占用",
        io::ErrorKind::WouldBlock => "网络暂时不可用",
        _ => "网络通信出错",
    }
}

/// 直连失败时的归类。
pub fn classify_direct(error: &io::Error) -> ConnectionError {
    if error.kind() == io::ErrorKind::TimedOut {
        ConnectionError::timeout("连接服务器超时")
    } else {
        ConnectionError::network(format!("无法连接服务器：{}", describe_io(error)))
    }
}

/// 加密握手失败时，优先识别底层的连接中断和超时；未知原因仍归为加密失败。
///
/// 只返回固定描述，底层错误中的远端内容不进入用户提示。
pub(crate) fn classify_tls(error: &(dyn std::error::Error + 'static)) -> ConnectionError {
    let mut current = Some(error);
    while let Some(cause) = current {
        if let Some(io_error) = cause.downcast_ref::<io::Error>() {
            if let Some(classified) = classify_tls_io(io_error.kind()) {
                return classified;
            }
        }
        current = cause.source();
    }

    // 部分 TLS 实现或系统证书平台没有把原始 io::Error 暴露到错误链中。
    // 这里只匹配稳定的英文描述和系统错误编号，不依赖本地化文本。
    let text = error.to_string().to_ascii_lowercase();
    if text == "unexpected eof during handshake"
        || text.ends_with("(os error 10053)")
        || text.ends_with("(os error 10054)")
    {
        return tls_interrupted();
    }
    if text.ends_with("(os error 10060)") {
        return ConnectionError::timeout("建立加密连接时等待响应超时，请检查网络或代理线路");
    }

    ConnectionError::tls("未能建立加密连接，可能与服务器证书、加密方式或网络中断有关")
}

fn classify_tls_io(kind: io::ErrorKind) -> Option<ConnectionError> {
    match kind {
        io::ErrorKind::TimedOut => Some(ConnectionError::timeout(
            "建立加密连接时等待响应超时，请检查网络或代理线路",
        )),
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::NotConnected => Some(tls_interrupted()),
        _ => None,
    }
}

fn tls_interrupted() -> ConnectionError {
    ConnectionError::network("建立加密连接时连接被中途断开，请检查网络或代理线路是否能连接邮件服务器")
}

/// 把文本里出现的敏感值替换成 `***`；长的先替换，避免部分遮挡。
pub fn redact(text: &str, secrets: &[&str]) -> String {
    let mut secrets: Vec<&str> = secrets
        .iter()
        .copied()
        .filter(|secret| !secret.is_empty())
        .collect();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    let mut result = text.to_string();
    for secret in secrets {
        if result.contains(secret) {
            result = result.replace(secret, "***");
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use std::io;

    use mail_domain::error::ConnectionErrorKind;

    use super::{classify_tls, redact};

    #[test]
    fn 脱敏会把授权码替换掉() {
        let text = "登录失败：密码 secret-123 不正确";
        let clean = redact(text, &["secret-123", ""]);
        assert_eq!(clean, "登录失败：密码 *** 不正确");
        assert!(!clean.contains("secret-123"));
    }

    #[test]
    fn 空密码不会把文本全部替换() {
        let clean = redact("普通文本", &[""]);
        assert_eq!(clean, "普通文本");
    }

    #[test]
    fn 多个敏感值按长度优先替换() {
        let clean = redact("值 abcdef 与 abc", &["abc", "abcdef"]);
        assert_eq!(clean, "值 *** 与 ***");
    }

    #[test]
    fn 握手中断与超时保留真实分类且不回显底层内容() {
        for (kind, expected) in [
            (
                io::ErrorKind::UnexpectedEof,
                ConnectionErrorKind::NetworkUnreachable,
            ),
            (
                io::ErrorKind::ConnectionReset,
                ConnectionErrorKind::NetworkUnreachable,
            ),
            (
                io::ErrorKind::ConnectionAborted,
                ConnectionErrorKind::NetworkUnreachable,
            ),
            (io::ErrorKind::BrokenPipe, ConnectionErrorKind::NetworkUnreachable),
            (
                io::ErrorKind::NotConnected,
                ConnectionErrorKind::NetworkUnreachable,
            ),
            (io::ErrorKind::TimedOut, ConnectionErrorKind::Timeout),
        ] {
            let raw = io::Error::new(kind, "untrusted-remote-content");
            let error = classify_tls(&raw);
            assert_eq!(error.kind, expected);
            assert!(error.message.contains("代理"));
            assert!(!error.message.contains("untrusted-remote-content"));
            assert!(!error.kind.hint().contains("993/465"));
        }
    }

    #[test]
    fn 加密库包装后的网络错误仍能归类() {
        #[derive(Debug, thiserror::Error)]
        #[error("加密组件包装错误")]
        struct Wrapped(#[source] io::Error);

        let wrapped = Wrapped(io::Error::new(io::ErrorKind::ConnectionReset, "remote-detail"));
        let error = classify_tls(&wrapped);
        assert_eq!(error.kind, ConnectionErrorKind::NetworkUnreachable);
        assert!(!error.message.contains("remote-detail"));
    }

    #[test]
    fn 识别未暴露错误链的握手中断和超时() {
        for (text, expected) in [
            (
                "unexpected EOF during handshake",
                ConnectionErrorKind::NetworkUnreachable,
            ),
            (
                "系统中止了连接 (os error 10053)",
                ConnectionErrorKind::NetworkUnreachable,
            ),
            (
                "连接被重置 (os error 10054)",
                ConnectionErrorKind::NetworkUnreachable,
            ),
            ("连接超时 (os error 10060)", ConnectionErrorKind::Timeout),
        ] {
            let error = classify_tls(&io::Error::other(text));
            assert_eq!(error.kind, expected, "{text}");
        }
    }

    #[test]
    fn 未确定原因的握手错误不误判成网络中断() {
        for text in [
            "certificate verify failed: untrusted-remote-content",
            "hostname mismatch: untrusted-remote-content",
            "wrong version number: untrusted-remote-content",
            "未知加密错误 untrusted-remote-content",
        ] {
            let error = classify_tls(&io::Error::other(text));
            assert_eq!(error.kind, ConnectionErrorKind::TlsFailure);
            assert!(error.message.contains("证书"));
            assert!(!error.message.contains("untrusted-remote-content"));
        }
    }
}
