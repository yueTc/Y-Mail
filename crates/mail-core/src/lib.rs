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
pub mod contacts;
pub mod download;
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
pub mod sync_settings;

pub use ai::{
    AiAuthorizationPreview, AiProviderInput, AiProviderView, AiTarget, AiTextOutcome, AiTranslation,
};
pub use checks::ConnectionReport;
pub use compose::{ComposeAttachment, ComposeParticipant, DraftSeed, OutboxItem, SendOutcome};
pub use contacts::{
    ContactCounts, ContactExport, ContactExportKind, ContactImportEntry, ContactImportMode,
    ContactImportOutcome, ContactImportPreview, ContactProblem, CONTACT_IMPORT_MAX_BYTES,
    CONTACT_IMPORT_MAX_ENTRIES, CONTACT_IMPORT_MAX_PROBLEMS,
};
pub use download::SavedExternalAttachment;
pub use engine::{EngineError, EngineInit, MailEngine, KEYRING_SERVICE};
pub use inbox::{InboxMessagePage, InboxThreadPage};
pub use mail_ai::{sanitize_code, sanitize_link, VerificationFinding};
pub use mcp::{
    McpAccountView, McpAttachmentView, McpBodyView, McpDraftInput, McpError, McpFolderView,
    McpMessageDetailView, McpMessageView, McpRecipient, McpSearchPageView, McpStatus, McpThreadView,
    McpToolInfo, MCP_BODY_MAX_CHARS, MCP_READ_TOOLS, MCP_SEARCH_MAX_LIMIT, MCP_SNIPPET_MAX_CHARS,
    MCP_THREAD_MAX_LIMIT, MCP_UNTRUSTED_NOTICE, MCP_WRITE_TOOLS,
};
pub use oauth::{OAuthAuthorization, OAuthOutcome, OAuthStatus};
pub use reading::{InlineImageState, InlineImageView, MessageBodyView};
pub use search::DeepSearchPage;

// 存储层的收件箱类型在这里重新导出，外壳无需直接依赖 mail-store。
/// 邮件头里的 RFC 2047 编码字解码；外壳组装展示字段时用，避免直接依赖 mail-mime。
pub use mail_mime::decode_encoded_words;
pub use mail_store::{
    AccountInboxSummary, AiFunction, AiModelMapEntry, AiProviderKind, AiThinkingLevel, AttachmentState,
    BodyState, ContactDraft, ContactImportRow, ContactImportStats, ContactScope, ContactSource, InboxFolder,
    InboxMessage, InboxQuery, InboxThread, McpAuditRecord, MessageLocation, NewAiAudit, NewAiProvider,
    NewOutbox, OutboxKind, OutboxState, SearchHit, SearchPage, SearchQuery, SnippetSegment, Store,
    StoredAiAudit, StoredAiCache, StoredAiProvider, StoredAttachment, StoredContact, StoredContactGroup,
    StoredOutbox, StoredSignature,
};
pub use paths::SqlitePaths;
pub use secrets::{ChunkedSecretStore, KeyringSecretStore, MemorySecretStore, SecretStore, SecretStoreError};
pub use sync::{AccountSyncStatus, SyncConfig, SyncService, SyncState};
pub use sync_settings::{
    account_logical_key, ai_logical_key, proxy_logical_key, GitHubLoginView, SettingsSnapshot,
    SnapshotAccount, SnapshotAiModelMap, SnapshotAiProvider, SnapshotGlobalProxy, SnapshotProxy,
    SnapshotProxyRef, SnapshotServer, SnapshotSettings, SnapshotSignature, GITHUB_AVATAR_KEY,
    GITHUB_LOGIN_KEY, GITHUB_NAME_KEY, GITHUB_TOKEN_KEY, SNAPSHOT_SCHEMA_VERSION,
};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "引擎门面（唯一对外接口）";
