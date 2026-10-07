//! mail-smtp：SMTP 协议层。
//!
//! 职责：把 outbox 队列中的邮件投递出去，并区分临时失败与永久失败。
//! 写操作（投递）不重试，重试策略由 mail-core 的队列统一控制，避免重复发送。
//!
//! Wave 1 提供「连接自检」（连上、EHLO、可选 STARTTLS、认证）；
//! Wave 5 加入 MIME 组装与真实投递（`message` / `send`）。

mod protocol;

pub mod message;
pub mod probe;
pub mod send;

pub use message::{
    build_message, guess_mime_type, BuiltMessage, Mailbox, MessageError, OutgoingAttachment,
    OutgoingInlineImage, OutgoingMessage,
};
pub use probe::{probe, ProbeReport, ProbeRequest};
pub use send::{send, SendError, SendErrorKind, SendReport, SendRequest};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "SMTP 协议客户端";

#[cfg(test)]
mod tests {
    use super::CRATE_PURPOSE;

    #[test]
    fn 用途标识不为空() {
        assert!(!CRATE_PURPOSE.is_empty());
    }
}
