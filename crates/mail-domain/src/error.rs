//! 共享错误类型：可读连接错误与输入校验错误。
//!
//! 约定：面向用户的错误必须分类清楚、可读、且不含任何凭据；内部细节只进日志，
//! 不允许把原始库/网络报错直接抛给界面。

use std::fmt;

/// 连接或自检失败的大类，用来决定给用户什么提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionErrorKind {
    /// 服务器拒绝登录：用户名或授权码不对。
    AuthFailed,
    /// 网络不可达：域名解析失败、连接被拒等。
    NetworkUnreachable,
    /// 超时：在规定时间内没等到服务器回应。
    Timeout,
    /// 加密握手失败：证书校验不过或服务器不支持所选的加密方式。
    TlsFailure,
    /// 代理相关失败：代理不可达、代理认证失败或配置无效。
    ProxyFailure,
    /// 协议错误：服务器返回的内容不符合预期。
    Protocol,
    /// 服务器明确拒绝（非认证类的拒绝）。
    Rejected,
    /// 其它未归类的失败。
    Unknown,
}

impl ConnectionErrorKind {
    /// 分类名，用于界面标签。
    pub fn label(self) -> &'static str {
        match self {
            Self::AuthFailed => "认证失败",
            Self::NetworkUnreachable => "网络不可达",
            Self::Timeout => "连接超时",
            Self::TlsFailure => "加密握手失败",
            Self::ProxyFailure => "代理失败",
            Self::Protocol => "服务器响应异常",
            Self::Rejected => "服务器拒绝",
            Self::Unknown => "未知错误",
        }
    }

    /// 处理建议，帮助用户自己排查。
    pub fn hint(self) -> &'static str {
        match self {
            Self::AuthFailed => "请检查登录名与授权码；注意多数邮箱要求使用「授权码」而不是网页登录密码。",
            Self::NetworkUnreachable => "请检查网络连接，或确认该邮箱在企业网络/防火墙下是否需要代理。",
            Self::Timeout => "请稍后重试；若长期超时，请检查网络或代理是否稳定。",
            Self::TlsFailure => {
                "请确认服务器地址与加密方式匹配（993/465 通常是 SSL/TLS，587 通常是 STARTTLS）。"
            }
            Self::ProxyFailure => "请检查代理地址、端口与账号密码，或改用「直连」再试。",
            Self::Protocol => "服务器返回了无法识别的内容，可能是地址或端口填错了。",
            Self::Rejected => "服务器拒绝了本次请求，请核对服务器地址与端口。",
            Self::Unknown => "请重试；若仍失败，可查看本地日志排查。",
        }
    }
}

impl fmt::Display for ConnectionErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// 一条对用户可读的连接错误。
///
/// `message` 必须是脱敏后的中文描述，禁止携带授权码、密码或令牌。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionError {
    /// 错误大类。
    pub kind: ConnectionErrorKind,
    /// 脱敏后的可读描述。
    pub message: String,
}

impl ConnectionError {
    /// 构造一条错误。
    pub fn new(kind: ConnectionErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// 认证失败的快捷构造。
    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ConnectionErrorKind::AuthFailed, message)
    }

    /// 网络不可达的快捷构造。
    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ConnectionErrorKind::NetworkUnreachable, message)
    }

    /// 超时的快捷构造。
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ConnectionErrorKind::Timeout, message)
    }

    /// 加密握手失败的快捷构造。
    pub fn tls(message: impl Into<String>) -> Self {
        Self::new(ConnectionErrorKind::TlsFailure, message)
    }

    /// 代理失败的快捷构造。
    pub fn proxy(message: impl Into<String>) -> Self {
        Self::new(ConnectionErrorKind::ProxyFailure, message)
    }

    /// 协议错误的快捷构造。
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(ConnectionErrorKind::Protocol, message)
    }

    /// 服务器拒绝的快捷构造。
    pub fn rejected(message: impl Into<String>) -> Self {
        Self::new(ConnectionErrorKind::Rejected, message)
    }
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}：{}", self.kind.label(), self.message)
    }
}

impl std::error::Error for ConnectionError {}

/// 输入校验错误：一次收集多条问题，方便界面一次性提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// 全部问题描述（中文）。
    pub messages: Vec<String>,
}

impl ValidationError {
    /// 用问题列表构造；`messages` 不应为空。
    pub fn new(messages: Vec<String>) -> Self {
        Self { messages }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.messages.join("；"))
    }
}

impl std::error::Error for ValidationError {}

#[cfg(test)]
mod tests {
    use super::{ConnectionError, ConnectionErrorKind};

    #[test]
    fn readable_error_has_label_and_hint() {
        let err = ConnectionError::auth("用户名或授权码不正确");
        assert_eq!(err.kind, ConnectionErrorKind::AuthFailed);
        assert_eq!(err.kind.label(), "认证失败");
        assert!(!err.kind.hint().is_empty());
        assert!(err.to_string().contains("认证失败"));
    }
}
