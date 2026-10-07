//! SQLite 连接封装。
//!
//! 唯一写库者的落点：其它 crate 不持有 `rusqlite::Connection`，只能调用本模块暴露的接口。
//! Wave 0 只提供连接、健康检查与迁移入口；业务读写接口自 Wave 1 起逐步添加。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode};

use crate::error::StoreError;
use crate::migrations::{self, MigrationReport};

/// 遇到写锁时的等待上限；超时后返回可读错误而不是直接失败。
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 数据库首次打开失败后最多重试的次数。
///
/// 开发热重启、杀毒软件扫描或旧进程刚退出时，SQLite 可能短暂看到不完整的
/// WAL 现场。多等一会儿通常就能恢复，不需要把主库当成永久损坏。
const OPEN_RETRIES: usize = 3;

/// 每次重试前等待的时长；递增等待，给文件句柄释放和 WAL 恢复留时间。
const OPEN_RETRY_DELAYS: [Duration; OPEN_RETRIES] = [
    Duration::from_millis(250),
    Duration::from_millis(750),
    Duration::from_millis(1500),
];

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
    ///
    /// 开发热重启、杀毒软件扫描或旧进程刚退出时，SQLite 可能短暂看到不完整的
    /// WAL 现场。遇到「数据库映像损坏」或文件暂时打不开时，按递增间隔重试几次。
    /// **绝不动数据库的任何文件**：不删、不改名、不挪走 `-wal` / `-shm`。
    /// SQLite 官方文档明确警告：崩溃后移动或改名热日志文件会让自动恢复失败，
    /// 数据库可能真的损坏；重试解决不了的问题，也不该用挪文件来「修」。
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

        match Self::open_at(&path) {
            Ok(store) => Ok(store),
            Err(err) if should_retry_open(&err) => {
                tracing::warn!(
                    database = %path.display(),
                    error = %err,
                    "数据库首次打开失败，按间隔重试（不移动任何文件）"
                );
                Self::open_with_retry(&path, err)
            }
            Err(err) => Err(err),
        }
    }

    /// 首次打开失败后的恢复尝试。
    ///
    /// 只做「等一会儿再试」，绝不动数据库的任何文件。
    ///
    /// 这里曾经会把 `-wal` / `-shm` 挪进 `backups/`，那是错的：SQLite 官方文档
    /// 明确写着，崩溃后移动或改名热日志文件会让自动恢复失败，数据库可能真的损坏。
    /// 本机实测两个进程同时打开同一个 WAL 库完全正常（第二个进程照样能读，
    /// `integrity_check` 仍是 ok），说明「被占用」并不是 malformed 的成因；
    /// 隔离临时文件解决不了问题，反而会在别的连接正用着这些文件时制造真正的不一致。
    /// 重试用尽后把最后一个可读错误返回给上层，绝不在这里静默重建空库。
    fn open_with_retry(path: &Path, first_err: StoreError) -> Result<Self, StoreError> {
        let mut last_err = first_err;
        for (attempt, delay) in OPEN_RETRY_DELAYS.iter().enumerate() {
            let attempt = attempt + 1;
            let delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
            tracing::warn!(
                database = %path.display(),
                attempt,
                delay_ms,
                "数据库打开失败，等待后重试（不移动任何文件）"
            );
            std::thread::sleep(*delay);
            match Self::open_at(path) {
                Ok(store) => {
                    tracing::info!(
                        database = %path.display(),
                        attempt,
                        "数据库重试打开成功"
                    );
                    return Ok(store);
                }
                Err(err) if should_retry_open(&err) => {
                    last_err = err;
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_err)
    }
    /// 打开内存库（仅测试与临时验证使用，不落盘）。
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        Self::prepare(conn, PathBuf::from(":memory:"))
    }

    /// 单次打开尝试：建连接并设置运行参数。是否重试由外层决定。
    fn open_at(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        Self::prepare(conn, path.to_path_buf())
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

    /// 做一次 SQLite 官方完整性检查；迁移切换前用来确认复制出来的库能安全打开。
    pub fn integrity_check(&self) -> Result<(), StoreError> {
        let result: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if result.eq_ignore_ascii_case("ok") {
            Ok(())
        } else {
            Err(StoreError::Integrity(result))
        }
    }

    /// 用 SQLite 自己的备份写入能力生成一致性快照。
    ///
    /// 不直接复制正在使用的 `ymail.db` / `-wal` / `-shm`，避免把半写入状态
    /// 当成新库；`VACUUM INTO` 会通过当前连接读出一个完整快照。
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<(), StoreError> {
        let destination = destination.as_ref();
        if destination.exists() {
            return Err(StoreError::InvalidPath("备份目标已存在，拒绝覆盖".to_string()));
        }
        if let Some(parent) = destination.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let destination_text = destination.to_string_lossy().to_string();
        self.conn.execute("VACUUM INTO ?1", [destination_text])?;
        Ok(())
    }

    /// bundled SQLite 是否带 FTS5 全文检索（规格 R7 的前提）。
    ///
    /// 只在一次性内存库上探测，绝不在用户的库里建表删表。
    /// 早先的实现每次启动都会往真实库写一张 `__fts5_probe` 虚拟表再删掉；
    /// 一旦进程恰好在「表已建好、还没删」之间被杀，库里会残留半张 FTS5 表，
    /// 下次启动删它时就可能报「database disk image is malformed」。
    pub fn fts5_available(&self) -> Result<bool, StoreError> {
        let probe = Connection::open_in_memory()?;
        match probe.execute_batch("CREATE VIRTUAL TABLE __fts5_probe USING fts5(payload);") {
            Ok(()) => Ok(true),
            Err(err) => {
                tracing::debug!(error = %err, "FTS5 探测未通过");
                Ok(false)
            }
        }
    }

    /// crate 内部使用：账号、代理等存储模块通过它拿到底层连接。
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// 仅测试使用：拿到底层连接，用来构造损坏数据等边界场景。
    #[cfg(test)]
    pub(crate) fn raw_connection_for_test(&self) -> &Connection {
        &self.conn
    }
}

/// 判断错误是否值得做延迟重试。
///
/// `DatabaseCorrupt` / `NotADatabase` 代表 SQLite 看到损坏现场；
/// `CannotOpen` / `DatabaseBusy` / `DatabaseLocked` 常见于旧进程、杀软扫描
/// 或热重启造成的短暂文件占用。主库真坏时重试会用尽并返回最后一个错误，
/// 不会静默重建空库。
fn should_retry_open(err: &StoreError) -> bool {
    matches!(
        err,
        StoreError::Sqlite(rusqlite::Error::SqliteFailure(inner, _))
            if matches!(
                inner.code,
                ErrorCode::DatabaseCorrupt
                    | ErrorCode::NotADatabase
                    | ErrorCode::CannotOpen
                    | ErrorCode::DatabaseBusy
                    | ErrorCode::DatabaseLocked
            )
    )
}

#[cfg(test)]
mod tests {
    use super::{should_retry_open, Store};
    use crate::error::StoreError;
    use rusqlite::ffi;

    #[test]
    fn disk_db_uses_wal_and_foreign_keys() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let db = dir.path().join("ymail.db");
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

    #[test]
    fn backup_to_creates_a_consistent_copy() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let db = dir.path().join("ymail.db");
        let backup = dir.path().join("backup").join("ymail.db");
        let store = Store::open(&db).expect("打开数据库");
        store
            .raw_connection_for_test()
            .execute_batch("CREATE TABLE probe (value INTEGER); INSERT INTO probe VALUES (42);")
            .expect("写入测试数据");

        store.backup_to(&backup).expect("生成备份");
        let copied = Store::open(&backup).expect("打开备份");
        let value: i64 = copied
            .raw_connection_for_test()
            .query_row("SELECT value FROM probe", [], |row| row.get(0))
            .expect("读取备份数据");
        assert_eq!(value, 42);
    }

    #[test]
    fn integrity_check_reports_ok_for_a_valid_database() {
        let store = Store::open_in_memory().expect("打开内存库");
        store.integrity_check().expect("完整性检查通过");
    }

    #[test]
    fn probing_fts5_must_not_write_to_the_real_db() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let db = dir.path().join("ymail.db");
        let store = Store::open(&db).expect("打开数据库");

        assert!(store.fts5_available().expect("探测 FTS5"));

        // 回归护栏：探测不许在用户库里留下任何对象。
        // 旧实现每次启动都建/删 `__fts5_probe`，进程卡在中间就会留下半张表。
        let conn = store.raw_connection_for_test();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master ORDER BY name")
            .expect("查询结构");
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .expect("读取结构")
            .collect::<Result<_, _>>()
            .expect("收集结构");
        assert!(names.is_empty(), "全新的库不该被探测写入任何对象：{names:?}");
    }

    #[test]
    fn open_failures_that_can_be_transient_are_retried() {
        for code in [
            ffi::ErrorCode::DatabaseCorrupt,
            ffi::ErrorCode::NotADatabase,
            ffi::ErrorCode::CannotOpen,
            ffi::ErrorCode::DatabaseBusy,
            ffi::ErrorCode::DatabaseLocked,
        ] {
            let err = StoreError::Sqlite(rusqlite::Error::SqliteFailure(
                ffi::Error {
                    code,
                    extended_code: 0,
                },
                None,
            ));
            assert!(should_retry_open(&err), "{code:?} 应进入重试");
        }

        let err = StoreError::Sqlite(rusqlite::Error::SqliteFailure(
            ffi::Error {
                code: ffi::ErrorCode::ReadOnly,
                extended_code: 0,
            },
            None,
        ));
        assert!(!should_retry_open(&err), "只读错误不该当损坏重试");
    }
}
