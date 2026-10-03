//! 引擎门面：初始化入口。
//!
//! 外壳（Tauri）与未来的外部接入都只跟这里打交道；数据库句柄被关在引擎内部，
//! 不出现在面向 UI 的接口里，保证「唯一写库者」这条不变量。

use std::fs;
use std::path::Path;

use mail_store::{MigrationOutcome, Store, StoreError};

use crate::paths::SqlitePaths;

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

/// 引擎初始化错误。
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
}

/// 引擎门面。
///
/// 当前只持有存储句柄；Wave 1 起会在内部装配账号、代理、同步等子系统，
/// 但这些细节不会越过本类型的公开接口。
pub struct MailEngine {
    store: Store,
    init: EngineInit,
}

impl std::fmt::Debug for MailEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只暴露初始化摘要；存储句柄不进 Debug 输出。
        f.debug_struct("MailEngine")
            .field("init", &self.init)
            .finish_non_exhaustive()
    }
}

impl MailEngine {
    /// 在指定数据目录上初始化引擎：建目录 → 打开数据库 → 执行迁移。
    ///
    /// 这是 Wave 0 的退出验证入口；调用方传入的应是应用自己的数据目录。
    pub fn initialize(data_dir: impl AsRef<Path>) -> Result<Self, EngineError> {
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

        Ok(Self { store, init })
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

    /// 只读存储访问。Wave 1 起业务接口也从这里暴露，但不暴露 `Connection` 本身。
    pub fn store(&self) -> &Store {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use super::MailEngine;

    #[test]
    fn initialize_creates_database_and_records_migration() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let engine = MailEngine::initialize(dir.path()).expect("初始化引擎");
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
            let first = MailEngine::initialize(dir.path()).expect("首次初始化");
            assert!(first.init_summary().applied_count() >= 1);
        }
        let second = MailEngine::initialize(dir.path()).expect("二次初始化");
        assert_eq!(
            second.init_summary().applied_count(),
            0,
            "二次初始化不应重复应用迁移"
        );
    }

    #[test]
    fn empty_data_dir_is_rejected() {
        let err = MailEngine::initialize("").expect_err("空目录应失败");
        assert!(err.to_string().contains("数据目录"), "错误应可读：{err}");
    }
}
