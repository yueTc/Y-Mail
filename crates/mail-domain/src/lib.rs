//! mail-domain：领域模型与共享类型。
//!
//! 本 crate 是依赖图的根节点：不依赖任何其它 crate，不做 I/O，不认识数据库与网络。
//! 其它所有 crate 都可以依赖它；它不依赖任何人。
//!
//! Wave 1 起提供账号、代理与「可读连接错误」的纯类型；代理优先级判断也放在这里，
//! 因为它不涉及 I/O，可以在没有数据库和网络的情况下完整测试。
//! Wave 2 起补上同步相关的纯逻辑：文件夹归类、历史范围、退避计算与线程键。

pub mod account;
pub mod auth;
pub mod dates;
pub mod error;
pub mod proxy;
pub mod sync;

pub use account::{
    Account, AccountDraft, AccountId, AccountProxyMode, AuthType, OAuthProvider, Security, ServerConfig,
};
pub use auth::{xoauth2_sasl, AuthMaterial};
pub use dates::{civil_from_days, days_from_civil, format_imap_date, format_iso8601_utc, parse_mail_date};
pub use error::{ConnectionError, ConnectionErrorKind, ValidationError};
pub use proxy::{
    decide_proxy, GlobalProxyMode, ProxyConfig, ProxyDecision, ProxyId, ProxyKind, ProxyRoute, ProxySource,
    Secret,
};
pub use sync::{
    backoff_delay, normalize_subject, thread_key, FolderKind, HistoryRange, SyncJobKind, SyncJobState,
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
