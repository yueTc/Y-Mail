//! mail-smtp：SMTP 协议层。
//!
//! 职责：把 outbox 队列中的邮件投递出去，并区分临时失败与永久失败。
//! 写操作（投递）不重试，重试策略由 mail-core 的队列统一控制，避免重复发送。
//!
//! Wave 1 提供「连接自检」（连上、EHLO、可选 STARTTLS、认证）；完整投递自 Wave 5 起填充。

pub mod probe;

pub use probe::{probe, ProbeReport, ProbeRequest};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "SMTP 协议客户端";
