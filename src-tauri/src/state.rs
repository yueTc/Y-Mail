//! 应用状态。
//!
//! 两个关键约束：
//! 1. `rusqlite::Connection` 可以跨线程移动（`Send`），但不能被多个线程共享（不是 `Sync`）。
//!    Tauri 的应用状态要求 `Send + Sync`，所以引擎句柄不能直接放进来，必须套 `Mutex`。
//! 2. 命令处理函数只做只读查询，因此每次调用短暂加锁即可，不会形成长阻塞。
//!
//! 状态里同时保存不可变的初始化摘要，避免每次查询都加锁。

use std::sync::Mutex;

use mail_core::{EngineInit, MailEngine};

use crate::commands::DbStatus;
use crate::logging::Logging;

/// 外壳状态：引擎句柄（加锁）+ 只读摘要 + 日志句柄。
pub struct AppState {
    engine: Mutex<MailEngine>,
    init: EngineInit,
    logging: Logging,
}

impl AppState {
    /// 构造状态。
    pub fn new(engine: MailEngine, logging: Logging) -> Self {
        let init = engine.init_summary();
        Self {
            engine: Mutex::new(engine),
            init,
            logging,
        }
    }

    /// 数据库状态快照。
    ///
    /// 迁移版本号从数据库实时读取，用来证明迁移登记记录确实落盘。
    pub fn db_status(&self) -> Result<DbStatus, String> {
        let engine = self
            .engine
            .lock()
            .map_err(|_| "引擎状态锁已损坏，无法读取数据库状态".to_string())?;
        let applied_versions = engine
            .store()
            .applied_migration_versions()
            .map_err(|err| format!("读取迁移登记记录失败：{err}"))?;

        Ok(DbStatus::from_init(
            &self.init,
            applied_versions,
            self.logging.dir().to_string_lossy().to_string(),
        ))
    }
}
