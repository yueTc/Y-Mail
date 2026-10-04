//! 前端可调用的命令。
//!
//! 职责边界（规格 4.1）：外壳只做窗口、生命周期与命令转发；
//! 账号、代理、连接自检全部走 `mail-core` 门面。
//!
//! 安全约定：授权码与代理密码只从界面传入、写进系统凭据管理器；出参一律不含凭据本体，
//! 已保存的账号只回一个 `hasCredential` 布尔值。命令入参不写日志。

use mail_core::{
    AccountInboxSummary, ConnectionReport, EngineError, InboxFolder, InboxMessage, InboxQuery, InboxThread,
    NewOutbox, OutboxKind, SearchHit, SearchQuery, SnippetSegment, StoredAttachment, StoredContact,
    StoredOutbox, StoredSignature,
};
use mail_domain::account::{Account, AccountDraft, AccountProxyMode, AuthType, Security, ServerConfig};
use mail_domain::proxy::{GlobalProxyMode, ProxyConfig, ProxyId, ProxyKind, Secret};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

// ============================ 错误 ============================

/// 命令错误：给界面看的可读结构。
///
/// - `kind`：错误大类（认证失败 / 超时 / 输入有误……）；
/// - `message`：具体描述（已脱敏）；
/// - `hint`：排查建议；
/// - `details`：表单校验等问题清单。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    /// 具体描述。
    pub message: String,
    /// 错误大类；无分类时为 `None`。
    pub kind: Option<String>,
    /// 排查建议。
    pub hint: Option<String>,
    /// 问题清单（校验类错误）。
    pub details: Vec<String>,
}

impl CommandError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: None,
            hint: None,
            details: Vec::new(),
        }
    }

    fn input(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: Some("输入有误".to_string()),
            hint: None,
            details: Vec::new(),
        }
    }
}

impl From<EngineError> for CommandError {
    fn from(err: EngineError) -> Self {
        match err {
            EngineError::Connection(connection) => Self {
                message: connection.message.clone(),
                kind: Some(connection.kind.label().to_string()),
                hint: Some(connection.kind.hint().to_string()),
                details: Vec::new(),
            },
            EngineError::Validation(validation) => Self {
                message: "输入有误，请检查下面列出的问题".to_string(),
                kind: Some("输入有误".to_string()),
                hint: None,
                details: validation.messages.clone(),
            },
            other => Self::new(other.to_string()),
        }
    }
}

// ============================ 枚举 ============================

/// 认证方式（`password` / `oauth2`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthTypeDto {
    /// 授权码 / 密码（Wave 1）。
    Password,
    /// OAuth2（Wave 6）。
    OAuth2,
}

impl AuthTypeDto {
    fn to_domain(self) -> AuthType {
        match self {
            Self::Password => AuthType::Password,
            Self::OAuth2 => AuthType::OAuth2,
        }
    }

    fn from_domain(value: AuthType) -> Self {
        match value {
            AuthType::Password => Self::Password,
            AuthType::OAuth2 => Self::OAuth2,
        }
    }
}

/// 传输加密方式（`tls` / `starttls` / `plain`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecurityDto {
    /// 隐式 SSL/TLS。
    Tls,
    /// 先明文后升级。
    StartTls,
    /// 不加密。
    Plain,
}

impl SecurityDto {
    fn to_domain(self) -> Security {
        match self {
            Self::Tls => Security::Tls,
            Self::StartTls => Security::StartTls,
            Self::Plain => Security::Plain,
        }
    }

    fn from_domain(value: Security) -> Self {
        match value {
            Security::Tls => Self::Tls,
            Security::StartTls => Self::StartTls,
            Security::Plain => Self::Plain,
        }
    }
}

/// 账号级代理策略（`inherit` / `direct` / `custom`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountProxyModeKind {
    /// 跟随全局。
    Inherit,
    /// 强制直连。
    Direct,
    /// 指定代理。
    Custom,
}

/// 全局代理策略（`system` / `direct` / `custom`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GlobalProxyModeKind {
    /// 跟随系统。
    System,
    /// 直连。
    Direct,
    /// 自定义代理。
    Custom,
}

/// 代理类型（`socks5` / `http`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyKindDto {
    /// 袜子五。
    Socks5,
    /// HTTP 隧道。
    Http,
}

impl ProxyKindDto {
    fn to_domain(self) -> ProxyKind {
        match self {
            Self::Socks5 => ProxyKind::Socks5,
            Self::Http => ProxyKind::Http,
        }
    }

    fn from_domain(value: ProxyKind) -> Self {
        match value {
            ProxyKind::Socks5 => Self::Socks5,
            ProxyKind::Http => Self::Http,
        }
    }
}

// ============================ 账号 DTO ============================

/// 一台服务器的地址、端口与加密方式。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigDto {
    /// 主机名。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 加密方式。
    pub security: SecurityDto,
}

impl ServerConfigDto {
    fn to_domain(&self) -> ServerConfig {
        ServerConfig {
            host: self.host.clone(),
            port: self.port,
            security: self.security.to_domain(),
        }
    }

    fn from_domain(value: &ServerConfig) -> Self {
        Self {
            host: value.host.clone(),
            port: value.port,
            security: SecurityDto::from_domain(value.security),
        }
    }
}

/// 账号级代理选择。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProxyDto {
    /// 策略。
    pub mode: AccountProxyModeKind,
    /// 「指定代理」时必填。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_id: Option<i64>,
}

impl AccountProxyDto {
    fn to_domain(&self) -> Result<AccountProxyMode, CommandError> {
        match self.mode {
            AccountProxyModeKind::Inherit => Ok(AccountProxyMode::InheritGlobal),
            AccountProxyModeKind::Direct => Ok(AccountProxyMode::Direct),
            AccountProxyModeKind::Custom => self
                .proxy_id
                .map(|id| AccountProxyMode::Custom(ProxyId(id)))
                .ok_or_else(|| CommandError::input("选了「指定代理」就要挑一个具体代理")),
        }
    }

    fn from_domain(value: AccountProxyMode) -> Self {
        match value {
            AccountProxyMode::InheritGlobal => Self {
                mode: AccountProxyModeKind::Inherit,
                proxy_id: None,
            },
            AccountProxyMode::Direct => Self {
                mode: AccountProxyModeKind::Direct,
                proxy_id: None,
            },
            AccountProxyMode::Custom(id) => Self {
                mode: AccountProxyModeKind::Custom,
                proxy_id: Some(id.0),
            },
        }
    }
}

/// 新建 / 修改账号时界面提交的草稿；**不含授权码**。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDraftDto {
    /// 显示名。
    pub display_name: String,
    /// 邮箱地址。
    pub email: String,
    /// 认证方式。
    pub auth_type: AuthTypeDto,
    /// 登录名。
    pub username: String,
    /// 收件服务器。
    pub imap: ServerConfigDto,
    /// 发件服务器。
    pub smtp: ServerConfigDto,
    /// 账号级代理。
    pub proxy: AccountProxyDto,
    /// 界面色标。
    #[serde(default)]
    pub color: String,
    /// 是否启用。
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl AccountDraftDto {
    fn to_domain(&self) -> Result<AccountDraft, CommandError> {
        Ok(AccountDraft {
            display_name: self.display_name.clone(),
            email: self.email.clone(),
            auth_type: self.auth_type.to_domain(),
            username: self.username.clone(),
            imap: self.imap.to_domain(),
            smtp: self.smtp.to_domain(),
            proxy: self.proxy.to_domain()?,
            color: self.color.clone(),
            enabled: self.enabled,
        })
    }
}

/// 返回给界面的账号；**不含凭据引用键**，只给 `hasCredential`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    /// 主键。
    pub id: i64,
    /// 显示名。
    pub display_name: String,
    /// 邮箱地址。
    pub email: String,
    /// 认证方式。
    pub auth_type: AuthTypeDto,
    /// 登录名。
    pub username: String,
    /// 收件服务器。
    pub imap: ServerConfigDto,
    /// 发件服务器。
    pub smtp: ServerConfigDto,
    /// 账号级代理。
    pub proxy: AccountProxyDto,
    /// 界面色标。
    pub color: String,
    /// 是否启用。
    pub enabled: bool,
    /// 系统凭据管理器里是否已有授权码。
    pub has_credential: bool,
    /// 创建时间（UTC）。
    pub created_at: String,
    /// 更新时间（UTC）。
    pub updated_at: String,
}

impl AccountDto {
    fn from_domain(account: &Account) -> Self {
        Self {
            id: account.id.0,
            display_name: account.display_name.clone(),
            email: account.email.clone(),
            auth_type: AuthTypeDto::from_domain(account.auth_type),
            username: account.username.clone(),
            imap: ServerConfigDto::from_domain(&account.imap),
            smtp: ServerConfigDto::from_domain(&account.smtp),
            proxy: AccountProxyDto::from_domain(account.proxy),
            color: account.color.clone(),
            enabled: account.enabled,
            has_credential: account.credential_key.is_some(),
            created_at: account.created_at.clone(),
            updated_at: account.updated_at.clone(),
        }
    }
}

// ============================ 代理 DTO ============================

/// 新建 / 修改代理时界面提交的配置；**不含密码**。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfigDto {
    /// 主键；新建时省略。
    #[serde(default)]
    pub id: Option<i64>,
    /// 便于识别的名字。
    #[serde(default)]
    pub label: String,
    /// 代理类型。
    pub kind: ProxyKindDto,
    /// 代理主机。
    pub host: String,
    /// 代理端口。
    pub port: u16,
    /// 代理登录名。
    #[serde(default)]
    pub username: String,
}

impl ProxyConfigDto {
    fn to_domain(&self) -> ProxyConfig {
        ProxyConfig {
            id: self.id.map(ProxyId),
            label: self.label.clone(),
            kind: self.kind.to_domain(),
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
        }
    }
}

/// 返回给界面的代理；**不含密码与凭据引用键**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyDto {
    /// 主键。
    pub id: i64,
    /// 便于识别的名字。
    pub label: String,
    /// 代理类型。
    pub kind: ProxyKindDto,
    /// 代理主机。
    pub host: String,
    /// 代理端口。
    pub port: u16,
    /// 代理登录名。
    pub username: String,
    /// 是否已保存密码。
    pub has_password: bool,
}

impl ProxyDto {
    fn from_parts(config: &ProxyConfig, has_password: bool) -> Self {
        Self {
            id: config.id.map_or(0, |id| id.0),
            label: config.label.clone(),
            kind: ProxyKindDto::from_domain(config.kind),
            host: config.host.clone(),
            port: config.port,
            username: config.username.clone(),
            has_password,
        }
    }
}

/// 全局代理设置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalProxyDto {
    /// 策略。
    pub mode: GlobalProxyModeKind,
    /// 「自定义代理」时必填。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_id: Option<i64>,
}

impl GlobalProxyDto {
    fn to_domain(&self) -> Result<GlobalProxyMode, CommandError> {
        match self.mode {
            GlobalProxyModeKind::System => Ok(GlobalProxyMode::System),
            GlobalProxyModeKind::Direct => Ok(GlobalProxyMode::Direct),
            GlobalProxyModeKind::Custom => self
                .proxy_id
                .map(|id| GlobalProxyMode::Custom(ProxyId(id)))
                .ok_or_else(|| CommandError::input("全局选了「自定义代理」就要挑一个具体代理")),
        }
    }

    fn from_domain(value: GlobalProxyMode) -> Self {
        match value {
            GlobalProxyMode::System => Self {
                mode: GlobalProxyModeKind::System,
                proxy_id: None,
            },
            GlobalProxyMode::Direct => Self {
                mode: GlobalProxyModeKind::Direct,
                proxy_id: None,
            },
            GlobalProxyMode::Custom(id) => Self {
                mode: GlobalProxyModeKind::Custom,
                proxy_id: Some(id.0),
            },
        }
    }
}

// ============================ 自检报告 ============================

/// 连接自检结果，供界面显示「可用」及观察到的事实。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionReportDto {
    /// 收件服务器上能看到的文件夹数量。
    pub imap_folder_count: usize,
    /// 发件服务器实际使用的认证方式。
    pub smtp_mechanism: String,
}

impl ConnectionReportDto {
    fn from_report(report: &ConnectionReport) -> Self {
        Self {
            imap_folder_count: report.imap_folder_count,
            smtp_mechanism: report.smtp_mechanism.clone(),
        }
    }
}

// ============================ 数据库状态 ============================

/// 数据库状态快照，字段名与前端 TypeScript 类型保持一致。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbStatus {
    /// 数据库文件绝对路径。
    pub database_file: String,
    /// 日志目录。
    pub log_dir: String,
    /// 当前结构版本。
    pub schema_version: i64,
    /// 本次启动新应用的迁移条数。
    pub applied_count: usize,
    /// 已登记迁移的版本号（升序），直接读自数据库。
    pub applied_versions: Vec<i64>,
    /// bundled SQLite 是否带 FTS5。
    pub fts5_available: bool,
}

impl DbStatus {
    /// 由引擎初始化摘要与数据库登记行构造状态快照。
    pub fn from_init(init: &mail_core::EngineInit, applied_versions: Vec<i64>, log_dir: String) -> Self {
        Self {
            database_file: init.database_file.clone(),
            log_dir,
            schema_version: init.schema_version,
            applied_count: init.applied_count(),
            applied_versions,
            fts5_available: init.fts5_available,
        }
    }
}

/// 返回数据库初始化状态，供界面确认迁移链路已跑通。
#[tauri::command]
pub async fn db_status(state: tauri::State<'_, AppState>) -> Result<DbStatus, CommandError> {
    state.db_status().await.map_err(CommandError::new)
}

// ============================ 账号命令 ============================

/// 列出全部账号（不联网）。
#[tauri::command]
pub async fn list_accounts(state: tauri::State<'_, AppState>) -> Result<Vec<AccountDto>, CommandError> {
    let engine = state.engine().await;
    let accounts = engine.list_accounts()?;
    Ok(accounts.iter().map(AccountDto::from_domain).collect())
}

/// 用一份尚未保存的草稿做连接自检；不写库、不写保险箱。
#[tauri::command]
pub async fn test_account_connection(
    state: tauri::State<'_, AppState>,
    draft: AccountDraftDto,
    secret: String,
) -> Result<ConnectionReportDto, CommandError> {
    let draft = draft.to_domain()?;
    let engine = state.engine().await;
    let report = engine
        .test_account_connection(&draft, &Secret::new(secret))
        .await?;
    Ok(ConnectionReportDto::from_report(&report))
}

/// 新建账号：引擎会先连接自检，通过后才落库。
#[tauri::command]
pub async fn create_account(
    state: tauri::State<'_, AppState>,
    draft: AccountDraftDto,
    secret: String,
) -> Result<AccountDto, CommandError> {
    let draft = draft.to_domain()?;
    let engine = state.engine().await;
    let account = engine.create_account(&draft, &Secret::new(secret)).await?;
    Ok(AccountDto::from_domain(&account))
}

/// 修改账号；`secret` 省略或留空表示沿用已保存的授权码。
#[tauri::command]
pub async fn update_account(
    state: tauri::State<'_, AppState>,
    id: i64,
    draft: AccountDraftDto,
    secret: Option<String>,
) -> Result<AccountDto, CommandError> {
    let draft = draft.to_domain()?;
    let secret = secret.map(Secret::new);
    let engine = state.engine().await;
    let account = engine
        .update_account(mail_domain::AccountId(id), &draft, secret.as_ref())
        .await?;
    Ok(AccountDto::from_domain(&account))
}

/// 删除账号；凭据一并从系统凭据管理器删除。
#[tauri::command]
pub async fn delete_account(state: tauri::State<'_, AppState>, id: i64) -> Result<(), CommandError> {
    let engine = state.engine().await;
    engine.delete_account(mail_domain::AccountId(id))?;
    Ok(())
}

/// 对已保存的账号再做一次自检，授权码从系统凭据管理器取。
#[tauri::command]
pub async fn test_saved_account(
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<ConnectionReportDto, CommandError> {
    let engine = state.engine().await;
    let report = engine.test_saved_account(mail_domain::AccountId(id)).await?;
    Ok(ConnectionReportDto::from_report(&report))
}

// ============================ 代理命令 ============================

/// 列出全部代理（不含密码）。
#[tauri::command]
pub async fn list_proxies(state: tauri::State<'_, AppState>) -> Result<Vec<ProxyDto>, CommandError> {
    let engine = state.engine().await;
    let proxies = engine.list_proxies()?;
    Ok(proxies
        .iter()
        .map(|stored| ProxyDto::from_parts(&stored.config, stored.password_key.is_some()))
        .collect())
}

/// 新建或修改代理。
///
/// `password` 的语义：省略表示保持原密码；空串表示清空密码；非空表示换新密码。
#[tauri::command]
pub async fn save_proxy(
    state: tauri::State<'_, AppState>,
    config: ProxyConfigDto,
    password: Option<String>,
) -> Result<ProxyDto, CommandError> {
    let config = config.to_domain();
    let password = password.map(Secret::new);
    let engine = state.engine().await;
    let stored = engine.save_proxy(&config, password.as_ref())?;
    Ok(ProxyDto::from_parts(
        &stored.config,
        stored.password_key.is_some(),
    ))
}

/// 删除代理；引用它的账号会自动改回「跟随全局」。
#[tauri::command]
pub async fn delete_proxy(state: tauri::State<'_, AppState>, id: i64) -> Result<(), CommandError> {
    let engine = state.engine().await;
    engine.delete_proxy(ProxyId(id))?;
    Ok(())
}

/// 读取全局代理策略。
#[tauri::command]
pub async fn get_proxy_settings(state: tauri::State<'_, AppState>) -> Result<GlobalProxyDto, CommandError> {
    let engine = state.engine().await;
    let mode = engine.global_proxy_mode()?;
    Ok(GlobalProxyDto::from_domain(mode))
}

/// 保存全局代理策略。
#[tauri::command]
pub async fn set_proxy_settings(
    state: tauri::State<'_, AppState>,
    mode: GlobalProxyDto,
) -> Result<GlobalProxyDto, CommandError> {
    let mode = mode.to_domain()?;
    let engine = state.engine().await;
    engine.set_global_proxy_mode(mode)?;
    Ok(GlobalProxyDto::from_domain(engine.global_proxy_mode()?))
}

/// 测试一个代理能不能连到目标服务器。
///
/// `target` 省略或为空时用默认目标 `www.baidu.com:443`。
#[tauri::command]
pub async fn test_proxy(
    state: tauri::State<'_, AppState>,
    id: i64,
    target: Option<String>,
) -> Result<(), CommandError> {
    let target = parse_target(target)?;
    let engine = state.engine().await;
    engine.test_proxy(ProxyId(id), Some(target)).await?;
    Ok(())
}

/// 解析「主机:端口」形式的测试目标；空值回退到默认目标。
fn parse_target(target: Option<String>) -> Result<(String, u16), CommandError> {
    let Some(raw) = target else {
        return Ok(("www.baidu.com".to_string(), 443));
    };
    let value = raw.trim();
    if value.is_empty() {
        return Ok(("www.baidu.com".to_string(), 443));
    }
    let (host, port) = value
        .rsplit_once(':')
        .ok_or_else(|| CommandError::input("测试目标要写成「主机:端口」，例如 www.baidu.com:443"))?;
    let host = host.trim();
    if host.is_empty() {
        return Err(CommandError::input("测试目标的主机名不能为空"));
    }
    let port: u16 = port
        .trim()
        .parse()
        .map_err(|_| CommandError::input("测试目标的端口要在 1 到 65535 之间"))?;
    Ok((host.to_string(), port))
}

// ============================ 同步命令 ============================

/// 一个账号的同步状态；字段名与前端 TypeScript 类型保持一致。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusDto {
    /// 账号编号。
    pub account_id: i64,
    /// 邮箱地址。
    pub email: String,
    /// 状态标识（idle / connecting / syncing / backfilling / idle_waiting / error / needs_reauth / stopped）。
    pub state: String,
    /// 状态的中文名。
    pub state_label: String,
    /// 已处理条数。
    pub progress: i64,
    /// 本轮总条数；未知时为 0。
    pub total: i64,
    /// 给用户看的一句话说明。
    pub message: String,
    /// 是否需要用户重新填写授权码。
    pub needs_reauth: bool,
    /// 状态更新时间（UTC ISO-8601）。
    pub updated_at: String,
}

impl SyncStatusDto {
    /// 由引擎的状态快照转换。
    fn from_status(status: &mail_core::AccountSyncStatus) -> Self {
        Self {
            account_id: status.account_id,
            email: status.email.clone(),
            state: status.state.as_str().to_string(),
            state_label: status.state.label().to_string(),
            progress: status.progress,
            total: status.total,
            message: status.message.clone(),
            needs_reauth: status.needs_reauth,
            updated_at: status.updated_at.clone(),
        }
    }
}

/// 读取各账号的同步状态（只读内存快照，不联网）。
#[tauri::command]
pub async fn sync_status(state: tauri::State<'_, AppState>) -> Result<Vec<SyncStatusDto>, CommandError> {
    let engine = state.engine().await;
    Ok(engine
        .sync_statuses()
        .iter()
        .map(SyncStatusDto::from_status)
        .collect())
}

/// 启动同步：传入账号编号就只起这一个，省略就起全部启用账号。
#[tauri::command]
pub async fn start_sync(
    state: tauri::State<'_, AppState>,
    account_id: Option<i64>,
) -> Result<usize, CommandError> {
    let engine = state.engine().await;
    Ok(engine.start_sync(account_id)?)
}

/// 停止同步：传入账号编号就只停这一个，省略就全停。
#[tauri::command]
pub async fn stop_sync(
    state: tauri::State<'_, AppState>,
    account_id: Option<i64>,
) -> Result<(), CommandError> {
    let engine = state.engine().await;
    engine.stop_sync(account_id).await;
    Ok(())
}

// ============================ 统一收件箱命令（Wave 3） ============================

/// 收件箱默认每页条数。
const INBOX_DEFAULT_LIMIT: i64 = 200;
/// 收件箱每页条数上限（列表与线程展开共用）。
const INBOX_MAX_LIMIT: i64 = 500;

/// 统一收件箱查询条件（前端传入，全部字段可选）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InboxQueryDto {
    /// 只看某个账号；省略表示全部账号。
    pub account_id: Option<i64>,
    /// 只看某个文件夹；省略表示各账号的收件箱。
    pub folder_id: Option<i64>,
    /// 只看未读。
    pub unread_only: bool,
    /// 跳过条数。
    pub offset: i64,
    /// 最多返回条数；省略用默认值。
    pub limit: Option<i64>,
}

impl InboxQueryDto {
    /// 转成存储层查询；顺带把条数与偏移量夹在合法范围。
    fn to_query(&self) -> Result<InboxQuery, CommandError> {
        if self.offset < 0 {
            return Err(CommandError::input("分页偏移量不能是负数"));
        }
        let limit = self.limit.unwrap_or(INBOX_DEFAULT_LIMIT);
        if limit <= 0 {
            return Err(CommandError::input("每页条数要大于 0"));
        }
        if limit > INBOX_MAX_LIMIT {
            return Err(CommandError::input(format!("每页最多 {INBOX_MAX_LIMIT} 条")));
        }
        Ok(InboxQuery {
            account_id: self.account_id,
            folder_id: self.folder_id,
            unread_only: self.unread_only,
            offset: self.offset,
            limit,
        })
    }
}

/// 一个账号的收件箱汇总。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInboxDto {
    /// 账号编号。
    pub account_id: i64,
    /// 邮箱地址。
    pub email: String,
    /// 显示名。
    pub display_name: String,
    /// 色标。
    pub color: String,
    /// 是否启用。
    pub enabled: bool,
    /// 收件箱邮件总数。
    pub message_count: i64,
    /// 收件箱未读数。
    pub unread_count: i64,
}

impl AccountInboxDto {
    fn from_summary(summary: &AccountInboxSummary) -> Self {
        Self {
            account_id: summary.account_id,
            email: summary.email.clone(),
            display_name: summary.display_name.clone(),
            color: summary.color.clone(),
            enabled: summary.enabled,
            message_count: summary.message_count,
            unread_count: summary.unread_count,
        }
    }
}

/// 收件箱总览：各账号汇总 + 未读与邮件合计。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxSummaryDto {
    /// 每个账号的汇总（含未启用账号）。
    pub accounts: Vec<AccountInboxDto>,
    /// 所有账号的未读合计。
    pub total_unread: i64,
    /// 所有账号的邮件合计。
    pub total_messages: i64,
}

/// 收件箱里的一封邮件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxMessageDto {
    /// 邮件编号。
    pub id: i64,
    /// 所属账号。
    pub account_id: i64,
    /// 所属文件夹。
    pub folder_id: i64,
    /// 服务器 UID。
    pub uid: u32,
    /// 会话键。
    pub thread_key: String,
    /// 主题。
    pub subject: String,
    /// 发件人显示名。
    pub from_name: String,
    /// 发件人邮箱。
    pub from_addr: String,
    /// 日期（UTC）。
    pub date_utc: String,
    /// 大小（字节）。
    pub size: u32,
    /// 是否含附件。
    pub has_attachments: bool,
    /// 是否已读。
    pub is_read: bool,
    /// 是否星标。
    pub is_flagged: bool,
    /// 摘要。
    pub snippet: String,
    /// 账号邮箱。
    pub account_email: String,
    /// 账号显示名。
    pub account_name: String,
    /// 账号色标。
    pub account_color: String,
    /// 文件夹路径。
    pub folder_path: String,
}

impl InboxMessageDto {
    fn from_message(message: &InboxMessage) -> Self {
        Self {
            id: message.id,
            account_id: message.account_id,
            folder_id: message.folder_id,
            uid: message.uid,
            thread_key: message.thread_key.clone(),
            subject: message.subject.clone(),
            from_name: message.from_name.clone(),
            from_addr: message.from_addr.clone(),
            date_utc: message.date_utc.clone(),
            size: message.size,
            has_attachments: message.has_attachments,
            is_read: message.is_read,
            is_flagged: message.is_flagged,
            snippet: message.snippet.clone(),
            account_email: message.account_email.clone(),
            account_name: message.account_display_name.clone(),
            account_color: message.account_color.clone(),
            folder_path: message.folder_path.clone(),
        }
    }
}

/// 折叠后的一条会话线程。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxThreadDto {
    /// 所属账号。
    pub account_id: i64,
    /// 会话键。
    pub thread_key: String,
    /// 线程里的邮件条数。
    pub message_count: i64,
    /// 线程里的未读条数。
    pub unread_count: i64,
    /// 最新一封。
    pub latest: InboxMessageDto,
}

impl InboxThreadDto {
    fn from_thread(thread: &InboxThread) -> Self {
        Self {
            account_id: thread.account_id,
            thread_key: thread.thread_key.clone(),
            message_count: thread.message_count,
            unread_count: thread.unread_count,
            latest: InboxMessageDto::from_message(&thread.latest),
        }
    }
}

/// 一页邮件（含总数）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxMessagePageDto {
    /// 本页邮件。
    pub items: Vec<InboxMessageDto>,
    /// 符合条件的总条数。
    pub total: i64,
    /// 本页跳过的条数。
    pub offset: i64,
    /// 本页最大条数。
    pub limit: i64,
}

/// 一页线程（含总数）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxThreadPageDto {
    /// 本页线程。
    pub items: Vec<InboxThreadDto>,
    /// 符合条件的总行数。
    pub total: i64,
    /// 本页跳过的条数。
    pub offset: i64,
    /// 本页最大条数。
    pub limit: i64,
}

/// 一个账号下的文件夹（带本地邮件条数）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxFolderDto {
    /// 所属账号。
    pub account_id: i64,
    /// 文件夹编号。
    pub folder_id: i64,
    /// 服务器上的完整路径。
    pub full_path: String,
    /// 归类结果。
    pub kind: String,
    /// 本地邮件条数。
    pub message_count: i64,
    /// 本地未读条数。
    pub unread_count: i64,
}

impl InboxFolderDto {
    fn from_folder(folder: &InboxFolder) -> Self {
        Self {
            account_id: folder.account_id,
            folder_id: folder.folder_id,
            full_path: folder.full_path.clone(),
            kind: folder.kind.clone(),
            message_count: folder.message_count,
            unread_count: folder.unread_count,
        }
    }
}

/// 列出全部账号的文件夹（带本地条数，供左侧文件夹树）。
#[tauri::command]
pub async fn list_inbox_folders(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<InboxFolderDto>, CommandError> {
    let engine = state.engine().await;
    let folders = engine.inbox_folders()?;
    Ok(folders.iter().map(InboxFolderDto::from_folder).collect())
}
/// 收件箱总览：各账号未读与邮件合计。
#[tauri::command]
pub async fn inbox_summary(state: tauri::State<'_, AppState>) -> Result<InboxSummaryDto, CommandError> {
    let engine = state.engine().await;
    let accounts = engine.inbox_account_summary()?;
    let total_unread = accounts.iter().map(|s| s.unread_count).sum();
    let total_messages = accounts.iter().map(|s| s.message_count).sum();
    Ok(InboxSummaryDto {
        accounts: accounts.iter().map(AccountInboxDto::from_summary).collect(),
        total_unread,
        total_messages,
    })
}

/// 平铺模式：一页邮件（每封一行，时间倒序）。
#[tauri::command]
pub async fn list_inbox_messages(
    state: tauri::State<'_, AppState>,
    query: Option<InboxQueryDto>,
) -> Result<InboxMessagePageDto, CommandError> {
    let query = query.unwrap_or_default().to_query()?;
    let engine = state.engine().await;
    let page = engine.inbox_messages(&query)?;
    Ok(InboxMessagePageDto {
        items: page.items.iter().map(InboxMessageDto::from_message).collect(),
        total: page.total,
        offset: page.offset,
        limit: page.limit,
    })
}

/// 会话模式：一页线程（每个账号的同名主题折叠一行）。
#[tauri::command]
pub async fn list_inbox_threads(
    state: tauri::State<'_, AppState>,
    query: Option<InboxQueryDto>,
) -> Result<InboxThreadPageDto, CommandError> {
    let query = query.unwrap_or_default().to_query()?;
    let engine = state.engine().await;
    let page = engine.inbox_threads(&query)?;
    Ok(InboxThreadPageDto {
        items: page.items.iter().map(InboxThreadDto::from_thread).collect(),
        total: page.total,
        offset: page.offset,
        limit: page.limit,
    })
}

/// 展开一条会话：取该账号该线程的邮件（新的在前）。
#[tauri::command]
pub async fn list_thread_messages(
    state: tauri::State<'_, AppState>,
    account_id: i64,
    thread_key: String,
    limit: Option<i64>,
) -> Result<Vec<InboxMessageDto>, CommandError> {
    if thread_key.trim().is_empty() {
        return Err(CommandError::input("会话键不能为空"));
    }
    let limit = limit.unwrap_or(INBOX_DEFAULT_LIMIT);
    if limit <= 0 {
        return Err(CommandError::input("每页条数要大于 0"));
    }
    if limit > INBOX_MAX_LIMIT {
        return Err(CommandError::input(format!("每页最多 {INBOX_MAX_LIMIT} 条")));
    }
    let engine = state.engine().await;
    let messages = engine.thread_messages(account_id, thread_key.trim(), limit)?;
    Ok(messages.iter().map(InboxMessageDto::from_message).collect())
}

// ============================ 读信与附件（Wave 4） ============================

/// 一个附件的元数据与本地保存状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentDto {
    /// 附件编号。
    pub id: i64,
    /// 所属邮件。
    pub message_id: i64,
    /// MIME 分片下标。
    pub part_index: u32,
    /// 文件名。
    pub filename: String,
    /// MIME 类型。
    pub mime_type: String,
    /// 字节数。
    pub size: u64,
    /// Content-ID（内嵌图片引用用）。
    pub content_id: Option<String>,
    /// 是否内嵌展示。
    pub is_inline: bool,
    /// 本地保存路径；未下载时为 None。
    pub local_path: Option<String>,
    /// 下载状态：pending / downloading / downloaded / failed。
    pub state: String,
}

impl AttachmentDto {
    fn from_attachment(attachment: &StoredAttachment) -> Self {
        Self {
            id: attachment.id,
            message_id: attachment.message_id,
            part_index: attachment.part_index,
            filename: attachment.filename.clone(),
            mime_type: attachment.mime_type.clone(),
            size: attachment.size,
            content_id: attachment.content_id.clone(),
            is_inline: attachment.is_inline,
            local_path: attachment.local_path.clone(),
            state: attachment.state.as_str().to_string(),
        }
    }
}

/// 读信窗格要展示的一封邮件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageBodyDto {
    /// 邮件编号。
    pub message_id: i64,
    /// 纯文本正文。
    pub text_plain: Option<String>,
    /// 清洗后的 HTML 正文；只有用户放行本封时才带远程图片地址。
    pub html: Option<String>,
    /// 被拦下的远程图片数量。
    pub blocked_remote_images: usize,
    /// 附件清单。
    pub attachments: Vec<AttachmentDto>,
}

/// 取一封邮件的正文；本地没有就联网拉一次并落库。
///
/// `allow_remote_images` 是「本封放行」开关：默认关闭，只有用户点过一次才传 true；
/// 放行只影响本次返回的 HTML，库里保存的清洗结果不变。
#[tauri::command]
pub async fn get_message_body(
    state: tauri::State<'_, AppState>,
    message_id: i64,
    allow_remote_images: Option<bool>,
) -> Result<MessageBodyDto, CommandError> {
    if message_id <= 0 {
        return Err(CommandError::input("邮件编号不合法"));
    }
    let engine = state.engine().await;
    let view = engine
        .get_message_body(message_id, allow_remote_images.unwrap_or(false))
        .await?;
    Ok(MessageBodyDto {
        message_id: view.message_id,
        text_plain: view.text_plain,
        html: view.html,
        blocked_remote_images: view.blocked_remote_images,
        attachments: view
            .attachments
            .iter()
            .map(AttachmentDto::from_attachment)
            .collect(),
    })
}

/// 下载一个附件到本地，返回保存路径。
#[tauri::command]
pub async fn download_attachment(
    state: tauri::State<'_, AppState>,
    attachment_id: i64,
) -> Result<String, CommandError> {
    if attachment_id <= 0 {
        return Err(CommandError::input("附件编号不合法"));
    }
    let engine = state.engine().await;
    let path = engine.download_attachment(attachment_id).await?;
    Ok(path)
}

// ============================ 搜索与写信命令（Wave 5） ============================

/// 搜索默认每页条数。
const SEARCH_DEFAULT_LIMIT: i64 = 50;
/// 搜索每页条数上限。
const SEARCH_MAX_LIMIT: i64 = 200;

/// 搜索条件（前端传入）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchQueryDto {
    /// 原始查询串，例如 `发票 from:alice has:attachment`。
    pub raw: String,
    /// 只看某个账号；省略表示全部账号。
    pub account_id: Option<i64>,
    /// 跳过条数。
    pub offset: i64,
    /// 最多返回条数；省略用默认值。
    pub limit: Option<i64>,
    /// 是否联网补历史；false 只查本地库。
    pub deep: bool,
}

impl SearchQueryDto {
    /// 转成引擎查询；顺带把条数与偏移量夹在合法范围。
    fn to_query(&self) -> Result<SearchQuery, CommandError> {
        if self.raw.trim().is_empty() {
            return Err(CommandError::input("搜索关键词不能为空"));
        }
        if self.offset < 0 {
            return Err(CommandError::input("分页偏移量不能是负数"));
        }
        let limit = self.limit.unwrap_or(SEARCH_DEFAULT_LIMIT);
        if limit <= 0 {
            return Err(CommandError::input("每页条数要大于 0"));
        }
        if limit > SEARCH_MAX_LIMIT {
            return Err(CommandError::input(format!("每页最多 {SEARCH_MAX_LIMIT} 条")));
        }
        Ok(SearchQuery::new(
            self.raw.trim(),
            self.account_id,
            self.offset,
            limit,
        ))
    }
}

/// 高亮片段：命中处 `highlighted` 为 true，界面按普通文本渲染。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetSegmentDto {
    /// 片段文本。
    pub text: String,
    /// 是否是命中的关键词。
    pub highlighted: bool,
}

impl SnippetSegmentDto {
    fn from_segment(segment: &SnippetSegment) -> Self {
        Self {
            text: segment.text.clone(),
            highlighted: segment.highlighted,
        }
    }
}

/// 一条搜索结果：邮件 + 高亮片段。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHitDto {
    /// 命中的邮件。
    pub message: InboxMessageDto,
    /// 高亮片段。
    pub snippet: Vec<SnippetSegmentDto>,
}

impl SearchHitDto {
    fn from_hit(hit: &SearchHit) -> Self {
        Self {
            message: InboxMessageDto::from_message(&hit.message),
            snippet: hit.snippet.iter().map(SnippetSegmentDto::from_segment).collect(),
        }
    }
}

/// 一页搜索结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPageDto {
    /// 本页结果。
    pub items: Vec<SearchHitDto>,
    /// 符合条件的总条数。
    pub total: i64,
    /// 本页跳过的条数。
    pub offset: i64,
    /// 本页最大条数。
    pub limit: i64,
    /// 本次是否真的联网补过历史。
    pub deep_synced: bool,
    /// 补历史失败时的可读原因；成功或无需要时为 None。
    pub deep_error: Option<String>,
}

/// 搜索邮件：`deep` 为 true 时先联网补一批未同步的历史再搜。
#[tauri::command]
pub async fn search_messages(
    state: tauri::State<'_, AppState>,
    query: SearchQueryDto,
) -> Result<SearchPageDto, CommandError> {
    let deep = query.deep;
    let query = query.to_query()?;
    let engine = state.engine().await;
    if deep {
        let result = engine.search_messages_deep(&query).await?;
        Ok(SearchPageDto {
            items: result.page.items.iter().map(SearchHitDto::from_hit).collect(),
            total: result.page.total,
            offset: result.page.offset,
            limit: result.page.limit,
            deep_synced: result.deep_synced,
            deep_error: result.deep_error,
        })
    } else {
        let page = engine.search_messages(&query)?;
        Ok(SearchPageDto {
            items: page.items.iter().map(SearchHitDto::from_hit).collect(),
            total: page.total,
            offset: page.offset,
            limit: page.limit,
            deep_synced: false,
            deep_error: None,
        })
    }
}

// ============================ 写信与发件队列命令（Wave 5） ============================

/// 写信类型标识：new / reply / forward。
fn parse_outbox_kind(value: &str) -> Result<OutboxKind, CommandError> {
    OutboxKind::parse(value.trim()).ok_or_else(|| CommandError::input("写信类型只能是 new、reply 或 forward"))
}

/// 一位收件人（显示名可空）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantDto {
    /// 显示名。
    pub name: String,
    /// 邮箱地址。
    pub address: String,
}

/// 一个待发附件：本地路径 + 展示文件名。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeAttachmentDto {
    /// 本地文件路径。
    pub path: String,
    /// 展示文件名。
    pub filename: String,
}

/// 把收件人列表转成库里存的 JSON 数组。
fn participants_to_json(people: &[ParticipantDto]) -> String {
    let value: Vec<serde_json::Value> = people
        .iter()
        .map(|person| {
            serde_json::json!({
                "name": person.name,
                "address": person.address,
            })
        })
        .collect();
    serde_json::Value::Array(value).to_string()
}

/// 把库里存的 JSON 数组读回收件人列表；内容坏了就当空列表，不让界面崩。
fn participants_from_json(raw: &str) -> Vec<ParticipantDto> {
    serde_json::from_str::<Vec<mail_core::ComposeParticipant>>(raw)
        .unwrap_or_default()
        .into_iter()
        .map(|person| ParticipantDto {
            name: person.name,
            address: person.address,
        })
        .collect()
}

/// 附件清单转 JSON。
fn attachments_to_json(items: &[ComposeAttachmentDto]) -> String {
    let value: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            serde_json::json!({
                "path": item.path,
                "filename": item.filename,
            })
        })
        .collect();
    serde_json::Value::Array(value).to_string()
}

/// 附件清单读回；内容坏了就当空列表。
fn attachments_from_json(raw: &str) -> Vec<ComposeAttachmentDto> {
    serde_json::from_str::<Vec<mail_core::ComposeAttachment>>(raw)
        .unwrap_or_default()
        .into_iter()
        .map(|item| ComposeAttachmentDto {
            path: item.path,
            filename: item.filename,
        })
        .collect()
}

/// References 读回；内容坏了就当空列表。
fn references_from_json(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// References 转 JSON。
fn references_to_json(items: &[String]) -> String {
    serde_json::Value::Array(
        items
            .iter()
            .map(|item| serde_json::Value::String(item.clone()))
            .collect(),
    )
    .to_string()
}

/// 写信窗格的预填内容（新建 / 回复 / 转发都走同一个结构）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSeedDto {
    /// 写信类型。
    pub kind: String,
    /// 所属账号。
    pub account_id: i64,
    /// 收件人。
    pub to: Vec<ParticipantDto>,
    /// 抄送。
    pub cc: Vec<ParticipantDto>,
    /// 密送。
    pub bcc: Vec<ParticipantDto>,
    /// 主题。
    pub subject: String,
    /// HTML 正文。
    pub body_html: String,
    /// 纯文本正文。
    pub body_text: String,
    /// 回复的原始 Message-ID。
    pub in_reply_to: Option<String>,
    /// References 里的 Message-ID 列表。
    pub references: Vec<String>,
    /// 附件清单。
    pub attachments: Vec<ComposeAttachmentDto>,
}

impl DraftSeedDto {
    fn from_seed(seed: &mail_core::DraftSeed) -> Self {
        Self {
            kind: seed.kind.as_str().to_string(),
            account_id: seed.account_id,
            to: participants_from_json(&seed.to_json),
            cc: participants_from_json(&seed.cc_json),
            bcc: participants_from_json(&seed.bcc_json),
            subject: seed.subject.clone(),
            body_html: seed.body_html.clone(),
            body_text: seed.body_text.clone(),
            in_reply_to: seed.in_reply_to.clone(),
            references: references_from_json(&seed.references_json),
            attachments: attachments_from_json(&seed.attachments_json),
        }
    }
}

/// 组装一封回信 / 转发的预填内容；新建邮件由界面自己给空模板。
#[tauri::command]
pub async fn compose_draft(
    state: tauri::State<'_, AppState>,
    kind: String,
    source_message_id: i64,
) -> Result<DraftSeedDto, CommandError> {
    if source_message_id <= 0 {
        return Err(CommandError::input("原邮件编号不合法"));
    }
    let kind = parse_outbox_kind(&kind)?;
    let engine = state.engine().await;
    let seed = engine.compose_draft(kind, source_message_id)?;
    Ok(DraftSeedDto::from_seed(&seed))
}

/// 写信窗格提交的一封草稿；`id` 省略表示新建。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxDraftDto {
    /// 已有草稿编号；新建时省略。
    pub id: Option<i64>,
    /// 所属账号。
    pub account_id: i64,
    /// 写信类型。
    pub kind: String,
    /// 收件人。
    #[serde(default)]
    pub to: Vec<ParticipantDto>,
    /// 抄送。
    #[serde(default)]
    pub cc: Vec<ParticipantDto>,
    /// 密送。
    #[serde(default)]
    pub bcc: Vec<ParticipantDto>,
    /// 主题。
    #[serde(default)]
    pub subject: String,
    /// HTML 正文。
    #[serde(default)]
    pub body_html: String,
    /// 纯文本正文。
    #[serde(default)]
    pub body_text: String,
    /// 回复的原始 Message-ID。
    #[serde(default)]
    pub in_reply_to: Option<String>,
    /// References 里的 Message-ID 列表。
    #[serde(default)]
    pub references: Vec<String>,
    /// 附件清单。
    #[serde(default)]
    pub attachments: Vec<ComposeAttachmentDto>,
}

impl OutboxDraftDto {
    fn to_new_outbox(&self) -> Result<NewOutbox, CommandError> {
        if self.account_id <= 0 {
            return Err(CommandError::input("请先选择发信账号"));
        }
        let kind = parse_outbox_kind(&self.kind)?;
        Ok(NewOutbox {
            account_id: self.account_id,
            kind,
            to_json: participants_to_json(&self.to),
            cc_json: participants_to_json(&self.cc),
            bcc_json: participants_to_json(&self.bcc),
            subject: self.subject.clone(),
            body_html: self.body_html.clone(),
            body_text: self.body_text.clone(),
            in_reply_to: self.in_reply_to.clone(),
            references_json: references_to_json(&self.references),
            attachments_json: attachments_to_json(&self.attachments),
        })
    }
}

/// 发件队列里的一条记录（含账号展示信息）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxItemDto {
    /// 记录编号。
    pub id: i64,
    /// 所属账号。
    pub account_id: i64,
    /// 写信类型。
    pub kind: String,
    /// 收件人。
    pub to: Vec<ParticipantDto>,
    /// 抄送。
    pub cc: Vec<ParticipantDto>,
    /// 密送。
    pub bcc: Vec<ParticipantDto>,
    /// 主题。
    pub subject: String,
    /// HTML 正文。
    pub body_html: String,
    /// 纯文本正文。
    pub body_text: String,
    /// 回复的原始 Message-ID。
    pub in_reply_to: Option<String>,
    /// References 列表。
    pub references: Vec<String>,
    /// 附件清单。
    pub attachments: Vec<ComposeAttachmentDto>,
    /// 状态：draft / queued / sending / sent / failed。
    pub state: String,
    /// 已尝试发送次数。
    pub attempts: i64,
    /// 最近一次失败原因。
    pub last_error: Option<String>,
    /// 创建时间。
    pub created_at: String,
    /// 更新时间。
    pub updated_at: String,
    /// 发送成功时间。
    pub sent_at: Option<String>,
    /// 账号邮箱。
    pub account_email: String,
    /// 账号显示名。
    pub account_display_name: String,
}

impl OutboxItemDto {
    fn from_item(item: &mail_core::OutboxItem) -> Self {
        Self::from_parts(&item.outbox, &item.account_email, &item.account_display_name)
    }

    fn from_parts(outbox: &StoredOutbox, account_email: &str, account_display_name: &str) -> Self {
        Self {
            id: outbox.id,
            account_id: outbox.account_id,
            kind: outbox.kind.as_str().to_string(),
            to: participants_from_json(&outbox.to_json),
            cc: participants_from_json(&outbox.cc_json),
            bcc: participants_from_json(&outbox.bcc_json),
            subject: outbox.subject.clone(),
            body_html: outbox.body_html.clone(),
            body_text: outbox.body_text.clone(),
            in_reply_to: outbox.in_reply_to.clone(),
            references: references_from_json(&outbox.references_json),
            attachments: attachments_from_json(&outbox.attachments_json),
            state: outbox.state.as_str().to_string(),
            attempts: outbox.attempts,
            last_error: outbox.last_error.clone(),
            created_at: outbox.created_at.clone(),
            updated_at: outbox.updated_at.clone(),
            sent_at: outbox.sent_at.clone(),
            account_email: account_email.to_string(),
            account_display_name: account_display_name.to_string(),
        }
    }
}

/// 保存草稿：`id` 省略时新建，返回记录编号。
#[tauri::command]
pub async fn save_draft(
    state: tauri::State<'_, AppState>,
    draft: OutboxDraftDto,
) -> Result<i64, CommandError> {
    let id = draft.id;
    let payload = draft.to_new_outbox()?;
    let engine = state.engine().await;
    Ok(engine.save_draft(id, &payload)?)
}

/// 把草稿 / 失败件放进待发队列。
#[tauri::command]
pub async fn enqueue_outbox(state: tauri::State<'_, AppState>, id: i64) -> Result<bool, CommandError> {
    if id <= 0 {
        return Err(CommandError::input("记录编号不合法"));
    }
    let engine = state.engine().await;
    Ok(engine.enqueue_outbox(id)?)
}

/// 把失败件退回队列，尝试次数清零。
#[tauri::command]
pub async fn retry_outbox(state: tauri::State<'_, AppState>, id: i64) -> Result<bool, CommandError> {
    if id <= 0 {
        return Err(CommandError::input("记录编号不合法"));
    }
    let engine = state.engine().await;
    Ok(engine.retry_outbox(id)?)
}

/// 读发件队列；不传账号就列全部。
#[tauri::command]
pub async fn list_outbox(
    state: tauri::State<'_, AppState>,
    account_id: Option<i64>,
    limit: Option<usize>,
) -> Result<Vec<OutboxItemDto>, CommandError> {
    let limit = limit.unwrap_or(100);
    let engine = state.engine().await;
    let items = engine.list_outbox(account_id, limit)?;
    Ok(items.iter().map(OutboxItemDto::from_item).collect())
}

/// 读一条发件记录；不存在返回 None。
#[tauri::command]
pub async fn get_outbox(
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<Option<OutboxItemDto>, CommandError> {
    if id <= 0 {
        return Err(CommandError::input("记录编号不合法"));
    }
    let engine = state.engine().await;
    let Some(outbox) = engine.get_outbox(id)? else {
        return Ok(None);
    };
    let accounts = engine.list_accounts()?;
    let account = accounts.iter().find(|item| item.id.0 == outbox.account_id);
    Ok(Some(OutboxItemDto::from_parts(
        &outbox,
        account.map_or("", |item| item.email.as_str()),
        account.map_or("", |item| item.display_name.as_str()),
    )))
}

/// 删除草稿 / 失败件；已发送的记录不删。
#[tauri::command]
pub async fn delete_outbox(state: tauri::State<'_, AppState>, id: i64) -> Result<bool, CommandError> {
    if id <= 0 {
        return Err(CommandError::input("记录编号不合法"));
    }
    let engine = state.engine().await;
    Ok(engine.delete_outbox(id)?)
}

/// 联系人自动补全：按名字或邮箱片段搜。
#[tauri::command]
pub async fn search_contacts(
    state: tauri::State<'_, AppState>,
    account_id: i64,
    keyword: String,
    limit: Option<usize>,
) -> Result<Vec<ContactDto>, CommandError> {
    if account_id <= 0 {
        return Err(CommandError::input("请先选择发信账号"));
    }
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.unwrap_or(10);
    let engine = state.engine().await;
    let contacts = engine.search_contacts(account_id, keyword, limit)?;
    Ok(contacts.iter().map(ContactDto::from_contact).collect())
}

/// 联系人展示信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactDto {
    /// 联系人编号。
    pub id: i64,
    /// 所属账号；None 表示全局联系人。
    pub account_id: Option<i64>,
    /// 显示名。
    pub name: String,
    /// 邮箱地址。
    pub email: String,
    /// 最近一次使用时间。
    pub last_used_at: Option<String>,
}

impl ContactDto {
    fn from_contact(contact: &StoredContact) -> Self {
        Self {
            id: contact.id,
            account_id: contact.account_id,
            name: contact.name.clone(),
            email: contact.email.clone(),
            last_used_at: contact.last_used_at.clone(),
        }
    }
}

/// 一个账号的签名。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureDto {
    /// 所属账号。
    pub account_id: i64,
    /// 签名 HTML。
    pub html: String,
    /// 是否启用。
    pub enabled: bool,
    /// 更新时间。
    pub updated_at: String,
}

impl SignatureDto {
    fn from_signature(signature: &StoredSignature) -> Self {
        Self {
            account_id: signature.account_id,
            html: signature.html.clone(),
            enabled: signature.enabled,
            updated_at: signature.updated_at.clone(),
        }
    }
}

/// 读一个账号的签名；没设置过返回空签名。
#[tauri::command]
pub async fn get_signature(
    state: tauri::State<'_, AppState>,
    account_id: i64,
) -> Result<SignatureDto, CommandError> {
    if account_id <= 0 {
        return Err(CommandError::input("账号编号不合法"));
    }
    let engine = state.engine().await;
    Ok(SignatureDto::from_signature(&engine.get_signature(account_id)?))
}

/// 保存一个账号的签名。
#[tauri::command]
pub async fn save_signature(
    state: tauri::State<'_, AppState>,
    account_id: i64,
    html: String,
    enabled: bool,
) -> Result<SignatureDto, CommandError> {
    if account_id <= 0 {
        return Err(CommandError::input("账号编号不合法"));
    }
    let engine = state.engine().await;
    let signature = engine.save_signature(account_id, &html, enabled)?;
    Ok(SignatureDto::from_signature(&signature))
}

/// 一封成功投递的报告。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendReportDto {
    /// 服务器接收的收件人数量。
    pub accepted_recipients: usize,
}

/// 跑一轮发送队列的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendOutcomeDto {
    /// 本次尝试投递的封数（含失败）。
    pub attempted: usize,
    /// 投递成功的封数。
    pub sent: usize,
    /// 给用户看的失败说明。
    pub errors: Vec<String>,
    /// 成功投递的报告。
    pub reports: Vec<SendReportDto>,
}

/// 跑一轮发送队列；用户点了发送按钮才会调到这里。
#[tauri::command]
pub async fn send_outbox(state: tauri::State<'_, AppState>) -> Result<SendOutcomeDto, CommandError> {
    let engine = state.engine().await;
    let outcome = engine.send_outbox().await?;
    Ok(SendOutcomeDto {
        attempted: outcome.attempted,
        sent: outcome.sent,
        errors: outcome.errors.clone(),
        reports: outcome
            .reports
            .iter()
            .map(|report| SendReportDto {
                accepted_recipients: report.accepted_recipients,
            })
            .collect(),
    })
}
