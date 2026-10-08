//! mail-ai：AI 与翻译旁路层（默认关闭）。
//!
//! 安全边界：本 crate 没有邮箱操作权限，只能返回纯文本；任何发送、跳转、写库动作都必须由用户显式点击触发。
//! 外发前必须由上层展示目标（站点域名 / 模型 / 是否本地）并取得用户授权。

mod client;
mod error;
mod http;
pub mod prompt;
mod provider;
mod segment;
pub mod verify;

pub use client::{chat, list_models, translate, ChatOutcome, Endpoint, TranslationOutcome};
pub use error::AiError;
pub use provider::{is_local_host, join_url, normalize_base_url, ProviderKind, ThinkingLevel};
pub use segment::{anchor_targets, normalize_whitespace, split_html, split_text, Segment};
pub use verify::{parse_verification, sanitize_code, sanitize_link, VerificationFinding};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "AI 与翻译（默认关闭，逐次授权）";
