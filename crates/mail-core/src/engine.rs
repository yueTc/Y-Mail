//! 引擎门面：初始化入口。
//!
//! 外壳（Tauri）与未来的外部接入都只跟这里打交道；数据库句柄被关在引擎内部，
//! 不出现在面向 UI 的接口里，保证「唯一写库者」这条不变量。
//!
//! Wave 1 起，引擎还持有凭据保险箱句柄；账号与代理的编排接口见
//! [`crate::accounts`] 与 [`crate::proxies`]。

use std::fs;
use std::path::Path;
use std::sync::Arc;

use mail_domain::{ConnectionError, ValidationError};
use mail_store::{MigrationOutcome, Store, StoreError};

use crate::paths::SqlitePaths;
use crate::secrets::{KeyringSecretStore, SecretStore, SecretStoreError};

/// Windows 凭据管理器里，本应用使用的服务名。
pub const KEYRING_SERVICE: &str = "com.emmaster.desktop";

/// 引擎初始化的结果摘要，供外壳显示与日志记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInit {
    /// 数据根目录。
    pub root_dir: String,
    /// 数据库文件路径。
    pub database_file: String,
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
}

/// 引擎门面。
///
/// 内部持有存储句柄与凭据保险箱；账号、代理、自检都在这里编排。
pub struct MailEngine {
    /// 存储句柄套一层互斥锁：`rusqlite::Connection` 能跨线程移动但不能被多线程共享，
    /// 而门面要能被 Tauri 的应用状态共享、也允许界面在多个命令间并发调用。
    /// 锁只包住同步的库操作，绝不跨 `.await` 持有。
    store: std::sync::Mutex<Store>,
    init: EngineInit,
    secrets: Arc<dyn SecretStore>,
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
        Self::initialize_with_secrets(data_dir, Arc::new(KeyringSecretStore::new(KEYRING_SERVICE)))
    }

    /// 同 [`MailEngine::initialize`]，但由调用方指定凭据保险箱实现。
    pub fn initialize_with_secrets(
        data_dir: impl AsRef<Path>,
        secrets: Arc<dyn SecretStore>,
    ) -> Result<Self, EngineError> {
        let data_dir = data_dir.as_ref();
        if data_dir.as_os_str().is_empty() {
            return Err(EngineError::InvalidDataDir("路径为空".to_string()));
        }
        let paths = SqlitePaths::from_root(data_dir);
        fs::create_dir_all(paths.root())?;
        fs::create_dir_all(&paths.log_dir)?;

        let mut store = Store::open(&paths.database_file)?;
        let report = store.run_migrations()?;
        let fts5_available = store.fts5_available()?;

        let init = EngineInit {
            root_dir: paths.root().to_string_lossy().to_string(),
            database_file: paths.database_file.to_string_lossy().to_string(),
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

        Ok(Self {
            store: std::sync::Mutex::new(store),
            init,
            secrets,
        })
    }

    /// 初始化摘要（可克隆，供外壳展示或记录日志）。
    pub fn init_summary(&self) -> EngineInit {
        self.init.clone()
    }

    /// 数据库文件路径。
    pub fn database_file(&self) -> &str {
        &self.init.database_file
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
