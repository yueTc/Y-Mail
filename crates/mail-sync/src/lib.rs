//! mail-sync：GitHub 登录与 Gist 设置同步的底座。
//!
//! 职责：口令派生与加解密（[`crypto`]）、出网包格式（[`envelope`]）、通用 HTTP
//! 客户端（[`http`]）、GitHub 设备码登录与资料（[`github`]），以及 Gist 读写
//! （`gist`）。这里只做「字节进、字节出」，不认识账号、代理、签名这些业务表，
//! 也不碰数据库。
//!
//! 依赖方向：mail-domain ← mail-sync，mail-net ← mail-sync。mail-sync 不依赖
//! mail-store、不依赖 mail-core，保持在底层，不制造环路。

pub mod crypto;
pub mod envelope;
mod error;
pub mod github;
pub mod http;

pub use error::SyncError;

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "GitHub 登录与 Gist 设置同步底座：口令派生、加解密、出网包格式与 GitHub 接口";
