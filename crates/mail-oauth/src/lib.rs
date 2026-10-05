//! mail-oauth：OAuth2 授权层。
//!
//! 职责：127.0.0.1 随机端口回环回调 + PKCE（S256）换取令牌、刷新令牌。
//! 令牌只存系统凭据库（keyring），不写数据库、不写日志、不进错误消息。
//!
//! 支持 Gmail 与 Microsoft 两家；换令牌的 HTTP 请求通过 mail-net 走代理分层。

mod error;
mod flow;
mod http;
mod pkce;
mod provider;
mod token;

pub use error::OAuthError;
pub use flow::{bind_loopback, build_authorize_request, AuthorizeRequest, Loopback, CALLBACK_PATH};
pub use pkce::{challenge_for, generate_pkce, random_urlsafe, Pkce};
pub use provider::{default_client_id, provider_meta, resolve_client_id, ProviderKind, ProviderMeta};
pub use token::{exchange_code, exchange_code_at, now_unix, refresh, refresh_at, TokenSet};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "OAuth2 授权与令牌刷新";
