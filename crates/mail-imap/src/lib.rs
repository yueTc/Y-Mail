//! mail-imap：IMAP 协议层。
//!
//! 职责：连接与认证（授权码 / XOAUTH2）、文件夹列表、UID 增量同步、IDLE 实时推送。
//! 网络错误与协议错误必须区分，供上层做故障隔离与重试决策。
//!
//! Wave 0 仅提供骨架；具体实现自 Wave 2（同步引擎）起填充。

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "IMAP 协议客户端";
