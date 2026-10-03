//! mail-oauth：OAuth2 授权层。
//!
//! 职责：127.0.0.1 随机端口回环回调 + PKCE 换取令牌、刷新令牌。
//! 令牌只存系统凭据库（keyring），不得写入数据库或日志。
//!
//! Wave 0 仅提供骨架；具体实现自 Wave 6（OAuth 与打包）起填充。

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "OAuth2 授权与令牌刷新";
