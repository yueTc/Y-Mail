//! 存储层错误类型。
//!
//! 错误信息面向开发者与日志；面向用户的文案由上层（mail-core / 外壳）翻译，禁止把原始
//! SQLite 报错直接抛给用户。

use thiserror::Error;

/// mail-store 的统一错误类型。
#[derive(Debug, Error)]
pub enum StoreError {
    /// 底层 SQLite 报错。
    #[error("SQLite 报错：{0}")]
    Sqlite(#[from] rusqlite::Error),

    /// 文件系统报错（建目录、打开文件等）。
    #[error("文件读写报错：{0}")]
    Io(#[from] std::io::Error),

    /// 数据库路径不合法（例如为空）。
    #[error("数据库路径不合法：{0}")]
    InvalidPath(String),

    /// 迁移执行失败。
    #[error("迁移 v{version} 执行失败：{reason}")]
    Migration {
        /// 出问题的迁移版本号。
        version: i64,
        /// 可读的原因。
        reason: String,
    },

    /// 数据库里的结构版本比本程序支持的更新，拒绝降级运行。
    #[error("数据库结构版本为 v{found}，高于本程序支持的 v{supported}；请升级程序后再打开")]
    SchemaTooNew {
        /// 数据库中的版本号。
        found: i64,
        /// 本程序支持的最高版本号。
        supported: i64,
    },
}
