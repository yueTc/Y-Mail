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
    use super::redact;

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
}
