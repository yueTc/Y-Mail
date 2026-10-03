//! SQLite 连接封装。
//!
//! 唯一写库者的落点：其它 crate 不持有 `rusqlite::Connection`，只能调用本模块暴露的接口。
//! Wave 0 只提供连接、健康检查与迁移入口；业务读写接口自 Wave 1 起逐步添加。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

use crate::error::StoreError;
use crate::migrations::{self, MigrationReport};

/// 遇到写锁时的等待上限；超时后返回可读错误而不是直接失败。
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 本地 SQLite 库的句柄。
pub struct Store {
    conn: Connection,
    path: PathBuf,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只暴露路径：连接内部状态是实现细节，不应出现在日志或 panic 信息里。
        f.debug_struct("Store")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// 打开（必要时创建）指定路径的数据库，并设置 WAL 等运行参数。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        if path.as_os_str().is_empty() {
            return Err(StoreError::InvalidPath("路径为空".to_string()));
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(&path)?;
        Self::prepare(conn, path)
    }

    /// 打开内存库（仅测试与临时验证使用，不落盘）。
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        Self::prepare(conn, PathBuf::from(":memory:"))
    }

    fn prepare(conn: Connection, path: PathBuf) -> Result<Self, StoreError> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // WAL：读写并发不互相阻塞；foreign_keys：约束真正生效。
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )?;
        Ok(Self { conn, path })
    }

    /// 数据库文件路径（内存库返回 `:memory:`）。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 当前日志模式，正常情况下应为 `wal`。
    pub fn journal_mode(&self) -> Result<String, StoreError> {
        let mode: String = self.conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        Ok(mode.to_lowercase())
    }

    /// 外键约束是否已打开。
    pub fn foreign_keys_enabled(&self) -> Result<bool, StoreError> {
        let flag: i64 = self.conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        Ok(flag != 0)
    }

    /// 已登记的迁移版本号（升序）。只读接口，供外壳与测试核对。
    pub fn applied_migration_versions(&self) -> Result<Vec<i64>, StoreError> {
        migrations::applied_versions(&self.conn)
    }

    /// 执行尚未应用的迁移；幂等，可重复调用。
    pub fn run_migrations(&mut self) -> Result<MigrationReport, StoreError> {
        migrations::migrate(&mut self.conn)
    }

    /// bundled SQLite 是否带 FTS5 全文检索（规格 R7 的前提）。
    pub fn fts5_available(&self) -> Result<bool, StoreError> {
        let sql = "CREATE VIRTUAL TABLE IF NOT EXISTS __fts5_probe USING fts5(payload);";
        match self.conn.execute_batch(sql) {
            Ok(()) => {
                self.conn.execute_batch("DROP TABLE IF EXISTS __fts5_probe;")?;
                Ok(true)
            }
            Err(err) => {
                tracing::debug!(error = %err, "FTS5 探测未通过");
                Ok(false)
            }
        }
    }

    /// 仅测试使用：拿到底层连接，用来构造损坏数据等边界场景。
    #[cfg(test)]
    pub(crate) fn raw_connection_for_test(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::Store;

    #[test]
    fn disk_db_uses_wal_and_foreign_keys() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let db = dir.path().join("em-master.db");
        let store = Store::open(&db).expect("打开数据库");
        assert_eq!(store.journal_mode().expect("读取日志模式"), "wal");
        assert!(store.foreign_keys_enabled().expect("读取外键开关"));
        assert!(db.exists(), "数据库文件应已创建");
    }

    #[test]
    fn bundled_sqlite_supports_fts5() {
        let store = Store::open_in_memory().expect("打开内存库");
        assert!(store.fts5_available().expect("探测 FTS5"));
    }

    #[test]
    fn empty_path_is_rejected() {
        let err = Store::open("").expect_err("空路径应失败");
        assert!(err.to_string().contains("路径"), "错误应可读：{err}");
    }
}
