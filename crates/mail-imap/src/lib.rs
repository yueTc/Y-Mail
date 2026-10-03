//! mail-imap：IMAP 协议层。
//!
//! 职责：连接与认证（授权码 / XOAUTH2）、文件夹列表、UID 增量同步、IDLE 实时推送。
//! 网络错误与协议错误必须区分，供上层做故障隔离与重试决策。
//!
//! Wave 1 提供「连接自检」（连上、登录、数文件夹）；完整同步自 Wave 2 起填充。

pub mod probe;

pub use probe::{probe, ProbeReport, ProbeRequest};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "IMAP 协议客户端";
