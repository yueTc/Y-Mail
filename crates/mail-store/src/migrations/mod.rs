//! 数据库迁移机制。
//!
//! 设计要点：
//! - 迁移清单写死在代码里（`MIGRATIONS`），按版本号升序，只允许在末尾追加；
//! - 每条迁移在**单独事务**里执行，执行成功后立刻往 `schema_migration` 写一条登记记录；
//! - 重复调用是幂等的：已登记且校验和一致的迁移直接跳过；
//! - 已登记的迁移若校验和对不上（SQL 被改过），或数据库版本高于本程序，都拒绝继续，避免把库改坏。

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tracing::{debug, info};

use crate::error::StoreError;

/// 一条数据库迁移。
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// 版本号，必须大于 0 且严格递增。
    pub version: i64,
    /// 名称，仅用于日志与排查。
    pub name: &'static str,
    /// 迁移 SQL（编译期从源文件读入）。
    pub sql: &'static str,
}

/// 全部迁移，按版本号升序。新增迁移只能追加在末尾，禁止改动历史条目。
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "0001_core_bootstrap",
    sql: include_str!("sql/0001_core_bootstrap.sql"),
}];

/// 单条迁移的执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationOutcome {
    /// 版本号。
    pub version: i64,
    /// 名称。
    pub name: String,
    /// 登记时间（数据库生成，UTC）。
    pub applied_at: String,
}

/// 一次迁移执行的整体结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// 本次新应用的迁移（按版本升序）。
    pub applied: Vec<MigrationOutcome>,
    /// 执行完成后的数据库结构版本。
    pub current_version: i64,
}

impl MigrationReport {
    /// 本次新应用的迁移条数。
    pub fn applied_count(&self) -> usize {
        self.applied.len()
    }
}

/// 迁移登记表；由迁移器自己在最前面建立，先于任何业务迁移。
const MIGRATION_TABLE_SQL: &str = "
CREATE TABLE IF NOT EXISTS schema_migration (
    version    INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    checksum   TEXT NOT NULL,
    applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
";

/// 已登记的迁移行（只保留核对所需字段）。
#[derive(Debug, Clone)]
struct AppliedRow {
    version: i64,
    name: String,
    checksum: String,
}

/// 计算迁移 SQL 的校验和（SHA-256 十六进制小写）。
fn checksum(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 本程序支持的最高结构版本。
fn supported_version() -> i64 {
    MIGRATIONS.last().map(|m| m.version).unwrap_or(0)
}

/// 校验迁移清单本身是否合法（版本唯一、严格递增、名称非空）。
fn validate_plan(migrations: &[Migration]) -> Result<(), StoreError> {
    let mut last = 0_i64;
    for migration in migrations {
        if migration.version <= last {
            return Err(StoreError::Migration {
                version: migration.version,
                reason: format!(
                    "迁移清单版本号必须严格递增，当前版本 {} 未大于前一条 {}",
                    migration.version, last
                ),
            });
        }
        if migration.name.trim().is_empty() {
            return Err(StoreError::Migration {
                version: migration.version,
                reason: "迁移名称为空".to_string(),
            });
        }
        last = migration.version;
    }
    Ok(())
}

fn ensure_registry(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(MIGRATION_TABLE_SQL)?;
    Ok(())
}

fn load_applied(conn: &Connection) -> Result<Vec<AppliedRow>, StoreError> {
    let mut stmt =
        conn.prepare("SELECT version, name, checksum FROM schema_migration ORDER BY version ASC")?;
    let rows = stmt.query_map([], |row| {
        Ok(AppliedRow {
            version: row.get(0)?,
            name: row.get(1)?,
            checksum: row.get(2)?,
        })
    })?;
    let mut applied = Vec::new();
    for row in rows {
        applied.push(row?);
    }
    Ok(applied)
}

/// 已登记的迁移版本号（升序）。只读接口，供外壳与测试核对。
pub fn applied_versions(conn: &Connection) -> Result<Vec<i64>, StoreError> {
    ensure_registry(conn)?;
    let mut stmt = conn.prepare("SELECT version FROM schema_migration ORDER BY version ASC")?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    let mut versions = Vec::new();
    for row in rows {
        versions.push(row?);
    }
    Ok(versions)
}

/// 执行所有尚未应用的迁移；幂等，可安全重复调用。
pub fn migrate(conn: &mut Connection) -> Result<MigrationReport, StoreError> {
    validate_plan(MIGRATIONS)?;
    ensure_registry(conn)?;

    let already = load_applied(conn)?;
    let supported_max = supported_version();

    // 1) 核对已登记的迁移：不能比程序新，必须能找到，内容必须没被改过。
    for row in &already {
        if row.version > supported_max {
            return Err(StoreError::SchemaTooNew {
                found: row.version,
                supported: supported_max,
            });
        }
        let Some(known) = MIGRATIONS.iter().find(|m| m.version == row.version) else {
            return Err(StoreError::Migration {
                version: row.version,
                reason: format!("数据库中登记的迁移 “{}” 在本程序里不存在", row.name),
            });
        };
        if known.name != row.name {
            return Err(StoreError::Migration {
                version: row.version,
                reason: format!("迁移名称不一致：数据库为 “{}”，程序为 “{}”", row.name, known.name),
            });
        }
        if checksum(known.sql) != row.checksum {
            return Err(StoreError::Migration {
                version: row.version,
                reason: "迁移 SQL 与登记时的校验和不一致（历史迁移被改动过）".to_string(),
            });
        }
    }

    // 2) 依次应用新迁移。
    let pending: Vec<&Migration> = MIGRATIONS
        .iter()
        .filter(|m| !already.iter().any(|row| row.version == m.version))
        .collect();

    let mut applied_now = Vec::new();
    for migration in pending {
        let tx = conn.transaction()?;
        tx.execute_batch(migration.sql)
            .map_err(|err| StoreError::Migration {
                version: migration.version,
                reason: err.to_string(),
            })?;
        tx.execute(
            "INSERT INTO schema_migration (version, name, checksum) VALUES (?1, ?2, ?3)",
            rusqlite::params![migration.version, migration.name, checksum(migration.sql)],
        )?;
        let applied_at: String = tx.query_row(
            "SELECT applied_at FROM schema_migration WHERE version = ?1",
            [migration.version],
            |row| row.get(0),
        )?;
        tx.commit()?;

        info!(
            version = migration.version,
            name = migration.name,
            "数据库迁移已应用"
        );
        applied_now.push(MigrationOutcome {
            version: migration.version,
            name: migration.name.to_string(),
            applied_at,
        });
    }

    if applied_now.is_empty() {
        debug!(version = supported_max, "数据库结构已是最新，无需迁移");
    }

    Ok(MigrationReport {
        current_version: supported_max,
        applied: applied_now,
    })
}

#[cfg(test)]
mod tests {
    use super::{checksum, supported_version, MIGRATIONS};
    use crate::Store;

    #[test]
    fn 首次迁移应写入登记记录且重复执行幂等() {
        let mut store = Store::open_in_memory().expect("打开内存库");

        let first = store.run_migrations().expect("首次迁移");
        assert_eq!(first.applied_count(), 1, "首次应应用 1 条迁移");
        assert_eq!(first.current_version, 1);
        assert_eq!(first.applied[0].name, "0001_core_bootstrap");
        assert!(!first.applied[0].applied_at.is_empty(), "登记时间不应为空");

        let second = store.run_migrations().expect("二次迁移");
        assert_eq!(second.applied_count(), 0, "二次执行不应重复应用");

        assert_eq!(store.applied_migration_versions().expect("读取版本"), vec![1]);
    }

    #[test]
    fn 校验和被篡改时应拒绝继续() {
        let mut store = Store::open_in_memory().expect("打开内存库");
        store.run_migrations().expect("首次迁移");

        store
            .raw_connection_for_test()
            .execute(
                "UPDATE schema_migration SET checksum = 'tampered' WHERE version = 1",
                [],
            )
            .expect("篡改校验和");

        let err = store.run_migrations().expect_err("应拒绝被篡改的迁移");
        assert!(
            err.to_string().contains("校验和"),
            "错误应指向校验和不一致：{err}"
        );
    }

    #[test]
    fn 数据库版本高于程序支持时应拒绝降级() {
        let mut store = Store::open_in_memory().expect("打开内存库");
        store.run_migrations().expect("首次迁移");
        store
            .raw_connection_for_test()
            .execute(
                "INSERT INTO schema_migration (version, name, checksum) VALUES (999, 'future', 'x')",
                [],
            )
            .expect("插入未来版本");

        let err = store.run_migrations().expect_err("应拒绝降级");
        assert!(err.to_string().contains("999"), "错误应包含版本号：{err}");
    }

    #[test]
    fn 迁移清单与校验和的基本性质() {
        assert_eq!(MIGRATIONS.len(), 1);
        assert_eq!(supported_version(), 1);
        assert_eq!(checksum("abc"), checksum("abc"));
        assert_ne!(checksum("abc"), checksum("abd"));
    }
}
