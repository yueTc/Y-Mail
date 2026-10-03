//! mail-domain：领域模型与共享类型。
//!
//! 本 crate 是依赖图的根节点：不依赖任何其它 crate，不做 I/O，不认识数据库与网络。
//! 其它所有 crate 都可以依赖它；它不依赖任何人。
//!
//! Wave 1 起提供账号、代理与「可读连接错误」的纯类型；代理优先级判断也放在这里，
//! 因为它不涉及 I/O，可以在没有数据库和网络的情况下完整测试。

pub mod account;
pub mod error;
pub mod proxy;

pub use account::{Account, AccountDraft, AccountId, AccountProxyMode, AuthType, Security, ServerConfig};
pub use error::{ConnectionError, ConnectionErrorKind, ValidationError};
pub use proxy::{
    decide_proxy, GlobalProxyMode, ProxyConfig, ProxyDecision, ProxyId, ProxyKind, ProxyRoute, ProxySource,
    Secret,
};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "领域模型与共享类型（无 I/O）";

#[cfg(test)]
mod tests {
    use super::CRATE_PURPOSE;

    #[test]
    fn purpose_is_not_empty() {
        assert!(!CRATE_PURPOSE.is_empty());
    }
}
