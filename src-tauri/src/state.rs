//! 应用状态。
//!
//! 三个关键约束：
//! 1. `rusqlite::Connection` 可以跨线程移动（`Send`），但不能被多个线程共享（不是 `Sync`）。
//!    Tauri 的应用状态要求 `Send + Sync`，所以引擎句柄必须套一层锁。
//! 2. 连接自检最长可跑十几秒，不能阻塞界面线程，所以锁用 `tokio::sync::Mutex`：
//!    命令是 `async` 的，自检期间会让出执行权，界面照常响应。
//! 3. 状态里同时保存不可变的初始化摘要，避免每次查询都加锁。

use tokio::sync::{Mutex, MutexGuard};

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

    /// 取得引擎句柄；调用方持有返回值期间独占引擎（自检、写库都经它）。
    pub(crate) async fn engine(&self) -> MutexGuard<'_, MailEngine> {
        self.engine.lock().await
    }

    /// 数据库状态快照。
    ///
    /// 迁移版本号从数据库实时读取，用来证明迁移登记记录确实落盘。
    pub async fn db_status(&self) -> Result<DbStatus, String> {
        let engine = self.engine.lock().await;
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
