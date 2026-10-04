//! mail-store：本地存储层，SQLite 的唯一写入口。
//!
//! 约定：其它 crate 不得直接访问数据库，只能经本 crate 的接口读写。
//! Wave 0 实现连接（WAL）与迁移机制；Wave 1 加入账号与代理表；
//! Wave 2 加入文件夹、邮件与同步任务表；Wave 3 加入统一收件箱只读查询；
//! Wave 4 加入正文缓存与附件下载记录；Wave 5 加入全文搜索、发件队列、
//! 地址簿与签名；Wave 7 加入 AI 站点、模型映射、缓存与审计。

pub mod accounts;
pub mod ai;
pub mod compose;
pub mod connection;
pub mod error;
pub mod inbox;
pub mod migrations;
pub mod proxies;
pub mod reading;
pub mod search;
pub mod sync;

pub use ai::{
    AiFunction, AiModelMapEntry, AiProviderKind, AiThinkingLevel, NewAiAudit, NewAiProvider, StoredAiAudit,
    StoredAiCache, StoredAiProvider,
};
pub use compose::{
    ComposeSource, NewOutbox, OutboxKind, OutboxState, StoredContact, StoredOutbox, StoredSignature,
};
pub use connection::Store;
pub use error::StoreError;
pub use inbox::{AccountInboxSummary, InboxFolder, InboxMessage, InboxQuery, InboxThread};
pub use migrations::{MigrationOutcome, MigrationReport};
pub use proxies::StoredProxy;
pub use reading::{
    AttachmentState, BodyState, MessageLocation, NewAttachment, StoredAttachment, StoredMessageBody,
};
pub use search::{SearchHit, SearchPage, SearchQuery, SnippetSegment};
pub use sync::{NewMessage, StoredFolder, StoredSyncJob};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "SQLite 存储与迁移（唯一写库者）";
