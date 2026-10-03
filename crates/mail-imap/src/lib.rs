//! mail-imap：IMAP 协议层。
//!
//! 职责：连接与认证（授权码，XOAUTH2 留待 Wave 6）、文件夹列表、UID 增量同步、
//! 元数据抓取与 IDLE 实时推送。网络错误与协议错误必须区分，供上层做故障隔离与重试决策。
//!
//! Wave 1 提供「连接自检」（连上、登录、数文件夹）；Wave 2 起提供完整客户端
//! [`client::ImapClient`]，自检走同一套协议实现。

pub mod client;
pub mod probe;

pub use client::{
    Address, ClientConfig, Envelope, FolderInfo, IdleOutcome, ImapClient, MailboxStatus, MessageMeta,
};
pub use probe::{probe, ProbeReport, ProbeRequest};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "IMAP 协议客户端";
