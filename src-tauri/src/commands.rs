//! 前端可调用的命令。
//!
//! Wave 0 只有一个只读命令 `db_status`：让界面确认「数据库已就绪、迁移记录已写入」。
//! 这里不返回数据库句柄，也不暴露任何写库能力。

use mail_core::EngineInit;
use serde::Serialize;

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
    pub fn from_init(init: &EngineInit, applied_versions: Vec<i64>, log_dir: String) -> Self {
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
pub fn db_status(state: tauri::State<'_, crate::state::AppState>) -> Result<DbStatus, String> {
    state.db_status()
}
