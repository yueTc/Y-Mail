//! 代理相关的纯类型与优先级判断。
//!
//! 优先级（规格 4.2 D3）：账号级 > 全局自定义 > 跟随系统 > 直连。
//! 这里只做「给定账号设置、全局设置、系统代理后的决策」，不读数据库、不读注册表、不建连接，
//! 因此可以在没有 I/O 的情况下把全部组合测清楚。

use std::fmt;

use crate::account::AccountProxyMode;
use crate::error::{ConnectionError, ValidationError};

/// 代理记录的主键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProxyId(pub i64);

/// 代理类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyKind {
    /// 袜子五（SOCKS5），支持用户名密码认证。
    Socks5,
    /// HTTP 隧道（CONNECT 方法），支持基础认证。
    Http,
}

impl ProxyKind {
    /// 存库用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Socks5 => "socks5",
            Self::Http => "http",
        }
    }

    /// 从存库字符串还原。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "socks5" => Some(Self::Socks5),
            "http" => Some(Self::Http),
            _ => None,
        }
    }

    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::Socks5 => "袜子五（SOCKS5）",
            Self::Http => "HTTP 隧道（CONNECT）",
        }
    }
}

/// 一个代理的配置；**不含密码**。
///
/// 密码单独存进系统凭据管理器，需要连接时再由上层取回，避免密码随配置到处流动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    /// 主键；新建但尚未入库时为 `None`。
    pub id: Option<ProxyId>,
    /// 便于识别的名字，可空。
    pub label: String,
    /// 代理类型。
    pub kind: ProxyKind,
    /// 代理主机。
    pub host: String,
    /// 代理端口。
    pub port: u16,
    /// 代理登录名，可空（无认证代理）。
    pub username: String,
}

impl ProxyConfig {
    /// 校验主机、端口等基本字段。
    pub fn validate(&self) -> Result<(), ValidationError> {
        let mut problems = Vec::new();
        if self.host.trim().is_empty() {
            problems.push("代理地址不能为空".to_string());
        } else if self.host.chars().any(char::is_whitespace) {
            problems.push("代理地址不能包含空白字符".to_string());
        }
        if self.port == 0 {
            problems.push("代理端口必须在 1 到 65535 之间".to_string());
        }
        if self.username.trim().is_empty() && !self.username.is_empty() {
            problems.push("代理用户名不能只填空白".to_string());
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(ValidationError::new(problems))
        }
    }
}

/// 明文敏感值（密码 / 授权码 / 令牌）。
///
/// `Debug` 被刻意实现为只打印 `***`，防止它随结构体调试输出或日志泄漏。
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// 包裹一个敏感值。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 取出原文，仅限真正需要发起认证时调用。
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// 连接时真正要用的代理：配置 + 可选的认证密码。
#[derive(Clone, PartialEq, Eq)]
pub struct ProxyRoute {
    /// 代理配置。
    pub config: ProxyConfig,
    /// 代理密码；无认证代理为 `None`。
    pub password: Option<Secret>,
}

impl fmt::Debug for ProxyRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 只暴露配置，密码永远不进调试输出。
        f.debug_struct("ProxyRoute")
            .field("config", &self.config)
            .field("has_password", &self.password.is_some())
            .finish()
    }
}

/// 全局代理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalProxyMode {
    /// 跟随系统（读取 Windows 系统代理设置）。
    System,
    /// 直连。
    Direct,
    /// 使用某个自定义代理。
    Custom(ProxyId),
}

impl GlobalProxyMode {
    /// 存库用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Direct => "direct",
            Self::Custom(_) => "custom",
        }
    }

    /// 从存库字符串还原；`custom` 需要带上代理主键。
    pub fn parse(value: &str, proxy_id: Option<i64>) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "direct" => Some(Self::Direct),
            "custom" => proxy_id.map(|id| Self::Custom(ProxyId(id))),
            _ => None,
        }
    }

    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Direct => "直连",
            Self::Custom(_) => "自定义代理",
        }
    }
}

/// 实际生效的代理来自哪一层，用于界面说明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxySource {
    /// 账号自己指定的代理。
    Account,
    /// 全局自定义代理。
    Global,
    /// 系统代理。
    System,
    /// 直连。
    Direct,
}

impl ProxySource {
    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::Account => "账号指定代理",
            Self::Global => "全局自定义代理",
            Self::System => "跟随系统",
            Self::Direct => "直连",
        }
    }
}

/// 代理决策结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyDecision {
    /// 要走代理时给出，直连为 `None`。
    pub route: Option<ProxyRoute>,
    /// 决策来源。
    pub source: ProxySource,
}

impl ProxyDecision {
    /// 直连。
    pub fn direct() -> Self {
        Self {
            route: None,
            source: ProxySource::Direct,
        }
    }
}

/// 按优先级决定本次连接走哪个代理。
///
/// - `lookup`：按主键查代理（配置 + 密码），由上层从数据库与系统凭据管理器组装。
/// - `system_proxy`：上层读到的系统代理；没有配置时传 `None`，此时「跟随系统」等价于直连。
pub fn decide_proxy(
    account_mode: AccountProxyMode,
    global_mode: GlobalProxyMode,
    lookup: &dyn Fn(ProxyId) -> Option<ProxyRoute>,
    system_proxy: Option<ProxyRoute>,
) -> Result<ProxyDecision, ConnectionError> {
    if let AccountProxyMode::Direct = account_mode {
        return Ok(ProxyDecision::direct());
    }

    if let AccountProxyMode::Custom(id) = account_mode {
        let route = lookup(id).ok_or_else(|| {
            ConnectionError::proxy(format!("账号指定的代理不存在或已被删除（编号 {}）", id.0))
        })?;
        return Ok(ProxyDecision {
            route: Some(route),
            source: ProxySource::Account,
        });
    }

    // 账号设置是「跟随全局」。
    match global_mode {
        GlobalProxyMode::Direct => Ok(ProxyDecision::direct()),
        GlobalProxyMode::Custom(id) => {
            let route = lookup(id).ok_or_else(|| {
                ConnectionError::proxy(format!("全局指定的代理不存在或已被删除（编号 {}）", id.0))
            })?;
            Ok(ProxyDecision {
                route: Some(route),
                source: ProxySource::Global,
            })
        }
        GlobalProxyMode::System => match system_proxy {
            Some(route) => Ok(ProxyDecision {
                route: Some(route),
                source: ProxySource::System,
            }),
            None => Ok(ProxyDecision::direct()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decide_proxy, GlobalProxyMode, ProxyConfig, ProxyDecision, ProxyId, ProxyKind, ProxyRoute,
        ProxySource,
    };
    use crate::account::AccountProxyMode;

    fn route(id: i64) -> ProxyRoute {
        ProxyRoute {
            config: ProxyConfig {
                id: Some(ProxyId(id)),
                label: format!("代理{id}"),
                kind: ProxyKind::Socks5,
                host: "127.0.0.1".to_string(),
                port: 1080,
                username: String::new(),
            },
            password: None,
        }
    }

    fn lookup(id: ProxyId) -> Option<ProxyRoute> {
        match id.0 {
            1 => Some(route(1)),
            2 => Some(route(2)),
            _ => None,
        }
    }

    #[test]
    fn 账号指定代理优先于全局与系统() {
        let decision = decide_proxy(
            AccountProxyMode::Custom(ProxyId(1)),
            GlobalProxyMode::Custom(ProxyId(2)),
            &lookup,
            Some(route(2)),
        )
        .expect("决策应成功");
        assert_eq!(decision.source, ProxySource::Account);
        assert_eq!(decision.route.expect("应有代理").config.id, Some(ProxyId(1)));
    }

    #[test]
    fn 账号直连优先于全局自定义() {
        let decision = decide_proxy(
            AccountProxyMode::Direct,
            GlobalProxyMode::Custom(ProxyId(1)),
            &lookup,
            Some(route(1)),
        )
        .expect("决策应成功");
        assert_eq!(decision, ProxyDecision::direct());
    }

    #[test]
    fn 跟随全局时全局自定义生效() {
        let decision = decide_proxy(
            AccountProxyMode::InheritGlobal,
            GlobalProxyMode::Custom(ProxyId(2)),
            &lookup,
            Some(route(1)),
        )
        .expect("决策应成功");
        assert_eq!(decision.source, ProxySource::Global);
        assert_eq!(decision.route.expect("应有代理").config.id, Some(ProxyId(2)));
    }

    #[test]
    fn 跟随系统时使用系统代理() {
        let decision = decide_proxy(
            AccountProxyMode::InheritGlobal,
            GlobalProxyMode::System,
            &lookup,
            Some(route(1)),
        )
        .expect("决策应成功");
        assert_eq!(decision.source, ProxySource::System);
    }

    #[test]
    fn 跟随系统但系统未配代理时直连() {
        let decision = decide_proxy(
            AccountProxyMode::InheritGlobal,
            GlobalProxyMode::System,
            &lookup,
            None,
        )
        .expect("决策应成功");
        assert_eq!(decision, ProxyDecision::direct());
    }

    #[test]
    fn 全局直连时直连() {
        let decision = decide_proxy(
            AccountProxyMode::InheritGlobal,
            GlobalProxyMode::Direct,
            &lookup,
            Some(route(1)),
        )
        .expect("决策应成功");
        assert_eq!(decision, ProxyDecision::direct());
    }

    #[test]
    fn 代理被删除时给出可读代理错误() {
        let err = decide_proxy(
            AccountProxyMode::Custom(ProxyId(99)),
            GlobalProxyMode::Direct,
            &lookup,
            None,
        )
        .expect_err("应报错");
        assert_eq!(err.kind, crate::error::ConnectionErrorKind::ProxyFailure);
        assert!(err.message.contains("99"));
    }

    #[test]
    fn 代理配置校验能拦住空地址与端口() {
        let config = ProxyConfig {
            id: None,
            label: String::new(),
            kind: ProxyKind::Http,
            host: "  ".to_string(),
            port: 0,
            username: String::new(),
        };
        let err = config.validate().expect_err("应校验失败");
        assert!(err.messages.len() >= 2);
    }

    #[test]
    fn 敏感值调试输出不泄露原文() {
        let secret = super::Secret::new("abcdefg");
        let text = format!("{secret:?}");
        assert_eq!(text, "Secret(***)");
        assert!(!text.contains("abcdefg"));
    }
}
