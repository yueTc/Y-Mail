//! 引擎门面：初始化入口。
//!
//! 外壳（Tauri）与未来的外部接入都只跟这里打交道；数据库句柄被关在引擎内部，
//! 不出现在面向 UI 的接口里，保证「唯一写库者」这条不变量。
//!
//! Wave 1 起，引擎还持有凭据保险箱句柄；账号与代理的编排接口见
//! [`crate::accounts`] 与 [`crate::proxies`]。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use mail_domain::{ConnectionError, ValidationError};
use mail_store::{MigrationOutcome, Store, StoreError};

use crate::paths::SqlitePaths;
use crate::secrets::{ChunkedSecretStore, KeyringSecretStore, SecretStore, SecretStoreError};
use crate::settings_sync::SettingsSyncLive;
use crate::sync::{SyncConfig, SyncService};
use crate::sync_settings::GitHubLoginRegistry;

/// Windows 凭据管理器里，本应用使用的服务名。
pub const KEYRING_SERVICE: &str = "com.ymail.desktop";

/// 引擎初始化的结果摘要，供外壳显示与日志记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInit {
    /// 数据根目录。
    pub root_dir: String,
    /// 数据库文件路径。
    pub database_file: String,
    /// 附件下载目录；可在设置里单独指定，默认在数据根目录下的 `downloads`。
    pub attachment_dir: String,
    /// 本次新应用的迁移（按版本升序）。
    pub applied_migrations: Vec<MigrationOutcome>,
    /// 数据库结构版本。
    pub schema_version: i64,
    /// bundled SQLite 是否带 FTS5（规格 R7 的前提，Wave 0 先探明）。
    pub fts5_available: bool,
}

impl EngineInit {
    /// 本次新应用的迁移条数。
    pub fn applied_count(&self) -> usize {
        self.applied_migrations.len()
    }
}

/// 引擎统一错误。
///
/// 面向用户的文案必须可读；底层英文报错只出现在开发者看得到的日志里。
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// 存储层错误。
    #[error(transparent)]
    Store(#[from] StoreError),

    /// 文件系统错误（建目录等）。
    #[error("初始化目录失败：{0}")]
    Io(#[from] std::io::Error),

    /// 数据目录不合法。
    #[error("数据目录不合法：{0}")]
    InvalidDataDir(String),

    /// 表单校验没通过（一次列出全部问题）。
    #[error("输入有误：{0}")]
    Validation(#[from] ValidationError),

    /// 连接自检失败（分类与描述都来自协议层）。
    #[error(transparent)]
    Connection(#[from] ConnectionError),

    /// 凭据保险箱操作失败。
    #[error("凭据保存失败：{0}")]
    Secrets(#[from] SecretStoreError),

    /// 请求本身不成立（缺少前置条件等）。
    #[error("{0}")]
    BadRequest(String),

    /// 同步任务相关的失败（启动、停止等）。
    #[error("同步失败：{0}")]
    Sync(String),

    /// 设置同步底座的错误（文案已在底座里脱敏，最多带状态码或格式名）。
    #[error(transparent)]
    SettingsSync(#[from] mail_sync::SyncError),

    /// 邮件不存在或已被删除。
    #[error("邮件不存在或已被删除（编号 {0}）")]
    MessageNotFound(i64),

    /// 附件不存在或已被删除。
    #[error("附件不存在或已被删除（编号 {0}）")]
    AttachmentNotFound(i64),

    /// 账号不存在或已被删除。
    #[error("账号不存在或已被删除（编号 {0}）")]
    AccountNotFound(i64),

    /// 代理不存在或已被删除。
    #[error("代理不存在或已被删除（编号 {0}）")]
    ProxyNotFound(i64),

    /// 邮箱地址与已有账号重复。
    #[error("邮箱地址已存在：{0}")]
    EmailTaken(String),

    /// 系统代理是自动配置脚本（PAC），当前版本还不支持。
    #[error(
        "系统代理使用的是自动配置脚本（{0}），当前版本还不支持；请在代理设置里改用「自定义代理」或「直连」"
    )]
    SystemProxyAutoConfig(String),

    /// OAuth2 授权已失效，必须用户重新授权。
    #[error("授权已失效，请到账号设置里重新授权")]
    OAuthReauthRequired,

    /// 找不到这次授权（已过期、已取消，或重启后丢失）。
    #[error("这次授权已过期或已取消，请重新发起授权")]
    AuthorizationNotFound,

    /// AI 站点不存在。
    #[error("AI 站点不存在（编号 {0}）")]
    AiProviderNotFound(i64),

    /// AI 与翻译默认关闭，没有启用的站点。
    #[error("AI 功能还没开启：请到设置里添加并启用一个 AI 站点")]
    AiDisabled,

    /// 外发前必须用户点头。
    #[error("按安全约定，这次外发需要你先确认目标（域名 / 模型 / 是否本地）")]
    AiAuthorizationRequired,

    /// AI 外发授权已过期、已用过，或与当前内容不匹配。
    #[error("这次 AI 外发授权已失效，请重新确认目标后再试")]
    AiAuthorizationInvalid,

    /// AI 调用失败（已脱敏）。
    #[error(transparent)]
    Ai(#[from] mail_ai::AiError),
}

/// 引擎门面。
///
/// 内部持有存储句柄与凭据保险箱；账号、代理、自检都在这里编排。
pub struct MailEngine {
    /// 存储句柄套一层互斥锁：`rusqlite::Connection` 能跨线程移动但不能被多线程共享，
    /// 而门面要能被 Tauri 的应用状态共享、也允许界面在多个命令间并发调用。
    /// 锁只包住同步的库操作，绝不跨 `.await` 持有。
    pub(crate) store: Arc<std::sync::Mutex<Store>>,
    init: EngineInit,
    secrets: Arc<dyn SecretStore>,
    pub(crate) sync: Arc<SyncService>,
    /// 还没收口的 OAuth2 授权：键是本次授权的校验串。
    ///
    /// 回环端口要一直挂着等浏览器回调，所以先存在引擎里，等界面回来收口。
    pub(crate) oauth_pending: std::sync::Mutex<HashMap<String, crate::oauth::PendingAuthorization>>,
    /// 已确认但还没真正外发的 AI 调用；键是一次性令牌。
    pub(crate) ai_authorizations: std::sync::Mutex<HashMap<String, crate::ai::PendingAiAuthorization>>,
    /// 读信时是否默认拦截远程图片；外壳改设置时同步更新。默认拦（true）。
    pub(crate) block_remote_images: Arc<AtomicBool>,
    /// 设置同步的「待同步」标记与唤醒（规格 3.6 / 3.9）。
    pub(crate) settings_sync_live: Arc<SettingsSyncLive>,
    /// 还没确认的 GitHub 设备码登录；设备码只在本进程内存里，退出即清。
    pub(crate) github_login_registry: Arc<GitHubLoginRegistry>,
}

impl std::fmt::Debug for MailEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只暴露初始化摘要；存储句柄与保险箱不进 Debug 输出。
        f.debug_struct("MailEngine")
            .field("init", &self.init)
            .finish_non_exhaustive()
    }
}

impl MailEngine {
    /// 在指定数据目录上初始化引擎：建目录 → 打开数据库 → 执行迁移。
    ///
    /// 凭据走系统凭据管理器（Windows 凭据管理器）。测试要注入内存保险箱时，
    /// 用 [`MailEngine::initialize_with_secrets`]。
    pub fn initialize(data_dir: impl AsRef<Path>) -> Result<Self, EngineError> {
        Self::initialize_with_secrets(data_dir, Self::default_secrets())
    }

    /// 默认保险箱：系统凭据管理器，外面再套一层「超长凭据自动分片」。
    ///
    /// 分片这层是必需的：微软 OAuth 的令牌包远超单条系统凭据上限，
    /// 不拆片就会被系统直接拒写（真实报错：Attribute 'password encoded as UTF-16'
    /// is longer than platform limit of 2560 chars）。
    fn default_secrets() -> Arc<dyn SecretStore> {
        Arc::new(ChunkedSecretStore::new(KeyringSecretStore::new(KEYRING_SERVICE)))
    }

    /// 同 [`MailEngine::initialize`]，但允许单独指定附件下载目录。
    ///
    /// `attachment_dir` 传 `None` 表示沿用默认的「数据根目录 / downloads」。
    pub fn initialize_with_attachment_dir(
        data_dir: impl AsRef<Path>,
        attachment_dir: Option<PathBuf>,
    ) -> Result<Self, EngineError> {
        Self::initialize_full(data_dir, attachment_dir, Self::default_secrets())
    }

    /// 同 [`MailEngine::initialize`]，但由调用方指定凭据保险箱实现。
    pub fn initialize_with_secrets(
        data_dir: impl AsRef<Path>,
        secrets: Arc<dyn SecretStore>,
    ) -> Result<Self, EngineError> {
        Self::initialize_full(data_dir, None, secrets)
    }

    /// 完整初始化入口：数据目录 + 可选附件目录 + 凭据保险箱。
    pub fn initialize_full(
        data_dir: impl AsRef<Path>,
        attachment_dir: Option<PathBuf>,
        secrets: Arc<dyn SecretStore>,
    ) -> Result<Self, EngineError> {
        let data_dir = data_dir.as_ref();
        if data_dir.as_os_str().is_empty() {
            return Err(EngineError::InvalidDataDir("路径为空".to_string()));
        }
        let paths = SqlitePaths::from_root_with_attachment(data_dir, attachment_dir);
        fs::create_dir_all(paths.root())?;
        fs::create_dir_all(&paths.log_dir)?;
        fs::create_dir_all(&paths.attachment_dir)?;

        let mut store = Store::open(&paths.database_file)?;
        let report = store.run_migrations()?;
        let fts5_available = store.fts5_available()?;

        let init = EngineInit {
            root_dir: paths.root().to_string_lossy().to_string(),
            database_file: paths.database_file.to_string_lossy().to_string(),
            attachment_dir: paths.attachment_dir.to_string_lossy().to_string(),
            applied_migrations: report.applied,
            schema_version: report.current_version,
            fts5_available,
        };

        tracing::info!(
            database_file = %init.database_file,
            schema_version = init.schema_version,
            applied = init.applied_count(),
            fts5 = init.fts5_available,
            "引擎初始化完成"
        );

        let store = Arc::new(std::sync::Mutex::new(store));
        let sync = Arc::new(SyncService::new(
            store.clone(),
            secrets.clone(),
            SyncConfig::default(),
        ));

        Ok(Self {
            store,
            init,
            secrets,
            sync,
            oauth_pending: std::sync::Mutex::new(HashMap::new()),
            ai_authorizations: std::sync::Mutex::new(HashMap::new()),
            block_remote_images: Arc::new(AtomicBool::new(true)),
            settings_sync_live: Arc::new(SettingsSyncLive::new()),
            github_login_registry: Arc::new(GitHubLoginRegistry::new()),
        })
    }

    /// 初始化摘要（可克隆，供外壳展示或记录日志）。
    pub fn init_summary(&self) -> EngineInit {
        self.init.clone()
    }

    /// 远程图片默认拦截开关的共享句柄；外壳拿它跟引擎保持同一个值。
    pub fn block_remote_images_handle(&self) -> Arc<AtomicBool> {
        self.block_remote_images.clone()
    }

    /// 更新「默认拦截远程图片」；保存设置后由外壳调用，立刻生效。
    pub fn set_block_remote_images(&self, block: bool) {
        self.block_remote_images.store(block, Ordering::Relaxed);
    }

    /// 数据库文件路径。
    pub fn database_file(&self) -> &str {
        &self.init.database_file
    }
    /// 数据根目录；本机缓存文件（如头像）按它定位。
    pub fn root_dir(&self) -> &str {
        &self.init.root_dir
    }

    /// 结构版本。
    pub fn schema_version(&self) -> i64 {
        self.init.schema_version
    }

    /// 存储访问。业务接口从这里暴露，但不暴露 `Connection` 本身。
    ///
    /// 返回的锁守卫只应在同步代码里短暂持有；锁中毒时取回内部值继续用，
    /// 因为这里的写操作都是单条 SQL 或事务，失败已由 `StoreError` 表达。
    pub fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 凭据保险箱的只读引用（内部编排用）。
    pub(crate) fn secrets(&self) -> &dyn SecretStore {
        self.secrets.as_ref()
    }

    /// 存储句柄的一份共享引用；设置同步后台任务要拿它自己决定出网路由。
    pub(crate) fn store_handle(&self) -> Arc<std::sync::Mutex<Store>> {
        self.store.clone()
    }

    /// 保险箱句柄的一份共享引用；设置同步后端不持有引擎，避免把网络等待绑在界面锁上。
    pub(crate) fn secrets_handle(&self) -> Arc<dyn SecretStore> {
        self.secrets.clone()
    }

    /// 同步服务的共享句柄。
    ///
    /// 外壳停止同步时要先取到它、马上放掉引擎锁，再 await 线程退出；
    /// 否则一次最长 4 分钟的 IDLE 收尾会把收件箱、读信等命令全部堵住。
    pub fn sync_handle(&self) -> Arc<SyncService> {
        self.sync.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::secrets::MemorySecretStore;

    use super::MailEngine;

    fn engine(dir: &std::path::Path) -> MailEngine {
        MailEngine::initialize_with_secrets(dir, Arc::new(MemorySecretStore::new())).expect("初始化引擎")
    }

    #[test]
    fn initialize_creates_database_and_records_migration() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let engine = engine(dir.path());
        let summary = engine.init_summary();

        assert!(summary.applied_count() >= 1, "应至少应用 1 条迁移");
        assert!(summary.schema_version >= 1);
        assert!(std::path::Path::new(&summary.database_file).exists());
        assert!(summary.fts5_available, "FTS5 应可用");
    }

    #[test]
    fn initialize_is_idempotent() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        {
            let first = engine(dir.path());
            assert!(first.init_summary().applied_count() >= 1);
        }
        let second = engine(dir.path());
        assert_eq!(
            second.init_summary().applied_count(),
            0,
            "二次初始化不应重复应用迁移"
        );
    }

    #[test]
    fn initialize_with_secrets_keeps_injected_store() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("初始化引擎");
        engine
            .secrets()
            .set("k", &mail_domain::Secret::new("v"))
            .expect("写入保险箱");
        assert!(secrets.contains("k"), "应使用注入的保险箱实现");
    }

    #[test]
    fn empty_data_dir_is_rejected() {
        let err = MailEngine::initialize("").expect_err("空目录应失败");
        assert!(err.to_string().contains("数据目录"), "错误应可读：{err}");
    }
}
