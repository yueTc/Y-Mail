//! mail-smtp：SMTP 协议层。
//!
//! 职责：把 outbox 队列中的邮件投递出去，并区分临时失败与永久失败。
//! 写操作（投递）不重试，重试策略由 mail-core 的队列统一控制，避免重复发送。
//!
//! Wave 0 仅提供骨架；具体实现自 Wave 5（写信与发送）起填充。

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "SMTP 协议客户端";
