//! mail-core：引擎门面（facade）。
//!
//! 职责：对桌面外壳与外部接入只暴露干净接口，内部编排存储与协议层；
//! 未来可整体抽为独立后台进程（daemon），UI 层无需改写。
//!
//! Wave 0 提供骨架与初始化入口；Wave 1 起加入账号、代理与连接自检编排。

pub mod accounts;
pub mod ai;
pub mod checks;
pub mod compose;
pub mod engine;
pub mod inbox;
pub mod mcp;
pub mod oauth;
pub mod paths;
pub mod proxies;
pub mod reading;
pub mod search;
pub mod secrets;
pub mod sync;

pub use ai::{
    AiAuthorizationPreview, AiProviderInput, AiProviderView, AiTarget, AiTextOutcome, AiTranslation,
};
pub use checks::ConnectionReport;
pub use compose::{ComposeAttachment, ComposeParticipant, DraftSeed, OutboxItem, SendOutcome};
pub use engine::{EngineError, EngineInit, MailEngine, KEYRING_SERVICE};
pub use inbox::{InboxMessagePage, InboxThreadPage};
pub use mcp::{
    McpAccountView, McpAttachmentView, McpBodyView, McpDraftInput, McpError, McpFolderView,
    McpMessageDetailView, McpMessageView, McpRecipient, McpSearchPageView, McpStatus, McpThreadView,
    McpToolInfo, MCP_BODY_MAX_CHARS, MCP_READ_TOOLS, MCP_SEARCH_MAX_LIMIT, MCP_SNIPPET_MAX_CHARS,
    MCP_THREAD_MAX_LIMIT, MCP_UNTRUSTED_NOTICE, MCP_WRITE_TOOLS,
};
pub use oauth::{OAuthAuthorization, OAuthOutcome, OAuthStatus};
pub use reading::MessageBodyView;
pub use search::DeepSearchPage;

// 存储层的收件箱类型在这里重新导出，外壳无需直接依赖 mail-store。
pub use mail_store::{
    AccountInboxSummary, AiFunction, AiModelMapEntry, AiProviderKind, AiThinkingLevel, AttachmentState,
    BodyState, InboxFolder, InboxMessage, InboxQuery, InboxThread, McpAuditRecord, MessageLocation,
    NewAiAudit, NewAiProvider, NewOutbox, OutboxKind, OutboxState, SearchHit, SearchPage, SearchQuery,
    SnippetSegment, StoredAiAudit, StoredAiCache, StoredAiProvider, StoredAttachment, StoredContact,
    StoredOutbox, StoredSignature,
};
pub use paths::SqlitePaths;
pub use secrets::{KeyringSecretStore, MemorySecretStore, SecretStore, SecretStoreError};
pub use sync::{AccountSyncStatus, SyncConfig, SyncService, SyncState};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "引擎门面（唯一对外接口）";
