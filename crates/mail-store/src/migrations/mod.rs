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
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "0001_core_bootstrap",
        sql: include_str!("sql/0001_core_bootstrap.sql"),
    },
    Migration {
        version: 2,
        name: "0002_accounts_and_proxies",
        sql: include_str!("sql/0002_accounts_and_proxies.sql"),
    },
    Migration {
        version: 3,
        name: "0003_email_case_insensitive_unique",
        sql: include_str!("sql/0003_email_case_insensitive_unique.sql"),
    },
    Migration {
        version: 4,
        name: "0004_folders_messages_sync",
        sql: include_str!("sql/0004_folders_messages_sync.sql"),
    },
    Migration {
        version: 5,
        name: "0005_reading",
        sql: include_str!("sql/0005_reading.sql"),
    },
    Migration {
        version: 6,
        name: "0006_search_compose",
        sql: include_str!("sql/0006_search_compose.sql"),
    },
    Migration {
        version: 7,
        name: "0007_oauth_accounts",
        sql: include_str!("sql/0007_oauth_accounts.sql"),
    },
    Migration {
        version: 8,
        name: "0008_ai",
        sql: include_str!("sql/0008_ai.sql"),
    },
    Migration {
        version: 9,
        name: "0009_mcp",
        sql: include_str!("sql/0009_mcp.sql"),
    },
    Migration {
        version: 10,
        name: "0010_folder_server_path_flags",
        sql: include_str!("sql/0010_folder_server_path_flags.sql"),
    },
    Migration {
        version: 11,
        name: "0011_contact_address_book",
        sql: include_str!("sql/0011_contact_address_book.sql"),
    },
    Migration {
        version: 12,
        name: "0012_ai_notification_verify",
        sql: include_str!("sql/0012_ai_notification_verify.sql"),
    },
    Migration {
        version: 13,
        name: "0013_mail_label_classification",
        sql: include_str!("sql/0013_mail_label_classification.sql"),
    },
    Migration {
        version: 14,
        name: "0014_folder_attributes",
        sql: include_str!("sql/0014_folder_attributes.sql"),
    },
    Migration {
        version: 15,
        name: "0015_folder_child_create_blocked",
        sql: include_str!("sql/0015_folder_child_create_blocked.sql"),
    },
    Migration {
        version: 16,
        name: "0016_mail_label_groups",
        sql: include_str!("sql/0016_mail_label_groups.sql"),
    },
    Migration {
        version: 17,
        name: "0017_ai_provider_proxy",
        sql: include_str!("sql/0017_ai_provider_proxy.sql"),
    },
    Migration {
        version: 18,
        name: "0018_decision_provider_kind",
        sql: include_str!("sql/0018_decision_provider_kind.sql"),
    },
];

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
        assert_eq!(first.applied_count(), 18, "首次应应用 18 条迁移");
        assert_eq!(first.current_version, 18);
        assert_eq!(first.applied[0].name, "0001_core_bootstrap");
        assert!(!first.applied[0].applied_at.is_empty(), "登记时间不应为空");

        let second = store.run_migrations().expect("二次迁移");
        assert_eq!(second.applied_count(), 0, "二次执行不应重复应用");

        assert_eq!(
            store.applied_migration_versions().expect("读取版本"),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18]
        );
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
    fn 从二号库升级到三号库后邮箱索引不区分大小写() {
        let mut store = Store::open_in_memory().expect("打开内存库");
        {
            let conn = store.raw_connection_for_test();
            conn.execute_batch(super::MIGRATION_TABLE_SQL).expect("建登记表");
            for migration in &MIGRATIONS[..2] {
                conn.execute_batch(migration.sql).expect("应用旧迁移");
                conn.execute(
                    "INSERT INTO schema_migration (version, name, checksum) VALUES (?1, ?2, ?3)",
                    rusqlite::params![migration.version, migration.name, checksum(migration.sql)],
                )
                .expect("登记旧迁移");
            }
            conn.execute(
                "INSERT INTO account (display_name, email, auth_type, username, \
                 imap_host, imap_port, imap_security, smtp_host, smtp_port, smtp_security) \
                 VALUES ('旧账号', 'Someone@example.com', 'password', 'Someone@example.com', \
                 'imap.example.com', 993, 'tls', 'smtp.example.com', 465, 'tls')",
                [],
            )
            .expect("插入旧账号");
        }

        store.run_migrations().expect("升级到三号库");
        assert_eq!(
            store.applied_migration_versions().expect("读取版本"),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18]
        );

        // 升级到七号库后，旧账号的授权字段应为空（NULL / 空串），不影响既有数据。
        let (provider, client_id): (Option<String>, String) = store
            .raw_connection_for_test()
            .query_row(
                "SELECT oauth_provider, oauth_client_id FROM account LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("读取授权字段");
        assert_eq!(provider, None);
        assert!(client_id.is_empty());

        let duplicate = store.raw_connection_for_test().execute(
            "INSERT INTO account (display_name, email, auth_type, username, \
             imap_host, imap_port, imap_security, smtp_host, smtp_port, smtp_security) \
             VALUES ('重复账号', 'SOMEONE@example.com', 'password', 'SOMEONE@example.com', \
             'imap.example.com', 993, 'tls', 'smtp.example.com', 465, 'tls')",
            [],
        );
        assert!(duplicate.is_err(), "升级后大小写不同的同一邮箱应被唯一索引拒绝");
    }

    #[test]
    fn 从十一号库升级后通知智能识别功能映射可用() {
        let mut store = Store::open_in_memory().expect("打开内存库");
        {
            let conn = store.raw_connection_for_test();
            conn.execute_batch(super::MIGRATION_TABLE_SQL).expect("建登记表");
            for migration in &MIGRATIONS[..11] {
                conn.execute_batch(migration.sql).expect("应用旧迁移");
                conn.execute(
                    "INSERT INTO schema_migration (version, name, checksum) VALUES (?1, ?2, ?3)",
                    rusqlite::params![migration.version, migration.name, checksum(migration.sql)],
                )
                .expect("登记旧迁移");
            }
            conn.execute(
                "INSERT INTO ai_provider (label, kind, base_url) VALUES ('本机', 'ollama', 'http://127.0.0.1:11434/v1')",
                [],
            )
            .expect("插入站点");
            conn.execute(
                "INSERT INTO ai_model_map (function, provider_id, model) VALUES ('translate', 1, 'qwen')",
                [],
            )
            .expect("插入旧映射");
        }

        store.run_migrations().expect("升级到十二号库");
        assert_eq!(
            store.applied_migration_versions().expect("读取版本"),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18]
        );

        // 旧 translate 映射要保留；新增 notification_verify 要能写；非法值仍被拒。
        let conn = store.raw_connection_for_test();
        let kept: String = conn
            .query_row(
                "SELECT model FROM ai_model_map WHERE function = 'translate'",
                [],
                |row| row.get(0),
            )
            .expect("旧映射应保留");
        assert_eq!(kept, "qwen");

        conn.execute(
            "INSERT INTO ai_model_map (function, provider_id, model) VALUES ('notification_verify', 1, 'deepseek')",
            [],
        )
        .expect("升级后应能写通知智能识别映射");

        let bad = conn.execute(
            "INSERT INTO ai_model_map (function, provider_id, model) VALUES ('nope', 1, 'x')",
            [],
        );
        assert!(bad.is_err(), "非法功能值仍应被 CHECK 拒绝");
    }

    #[test]
    fn migration_0013_creates_mail_labels() {
        let dir = tempfile::tempdir().expect("临时目录");
        let mut store = Store::open(dir.path().join("mail.db")).expect("打开数据库");
        store.run_migrations().expect("跑迁移");
        let count: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM mail_label WHERE group_id = (SELECT id FROM mail_label_group WHERE name = '内容分类')", [], |row| row.get(0))
            .expect("数标签");
        assert_eq!(count, 8);
        let fallback_color: String = store
            .conn()
            .query_row("SELECT color FROM mail_label WHERE is_fallback = 1", [], |row| {
                row.get(0)
            })
            .expect("读兜底标签颜色");
        assert_eq!(fallback_color, "gray");
        let distinct_colors: i64 = store
            .conn()
            .query_row("SELECT COUNT(DISTINCT color) FROM mail_label WHERE group_id = (SELECT id FROM mail_label_group WHERE name = '内容分类')", [], |row| {
                row.get(0)
            })
            .expect("数标签颜色");
        assert_eq!(distinct_colors, 8);
        let kind: String = store
            .conn()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'ai_provider'",
                [],
                |row| row.get(0),
            )
            .expect("读建表 SQL");
        assert!(kind.contains("typesafe"), "站点约束要包含 typesafe：{kind}");
        let function: String = store
            .conn()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'ai_model_map'",
                [],
                |row| row.get(0),
            )
            .expect("读功能映射 SQL");
        assert!(
            function.contains("classify"),
            "功能约束要包含 classify：{function}"
        );
    }

    #[test]
    fn 从十二号库升级后分类站点映射与标签可用() {
        let mut store = Store::open_in_memory().expect("打开内存库");
        {
            let conn = store.raw_connection_for_test();
            conn.execute_batch(super::MIGRATION_TABLE_SQL).expect("建登记表");
            for migration in &MIGRATIONS[..12] {
                conn.execute_batch(migration.sql).expect("应用旧迁移");
                conn.execute(
                    "INSERT INTO schema_migration (version, name, checksum) VALUES (?1, ?2, ?3)",
                    rusqlite::params![migration.version, migration.name, checksum(migration.sql)],
                )
                .expect("登记旧迁移");
            }
            conn.execute(
                "INSERT INTO ai_provider (label, kind, base_url, default_model, models_json) \
                 VALUES ('本机', 'ollama', 'http://127.0.0.1:11434/v1', 'qwen', '[\"qwen\",\"llama\"]')",
                [],
            )
            .expect("插入旧站点");
            conn.execute(
                "INSERT INTO ai_model_map (function, provider_id, model) VALUES ('translate', 1, 'qwen')",
                [],
            )
            .expect("插入旧映射");
        }

        store.run_migrations().expect("升级到十三号库");
        assert_eq!(
            store.applied_migration_versions().expect("读取版本"),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18]
        );

        let conn = store.raw_connection_for_test();
        // 旧站点和旧映射必须原样保留。
        let (label, models): (String, String) = conn
            .query_row(
                "SELECT label, models_json FROM ai_provider WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("旧站点应保留");
        assert_eq!(label, "本机");
        assert_eq!(models, "[\"qwen\",\"llama\"]");
        let kept: String = conn
            .query_row(
                "SELECT model FROM ai_model_map WHERE function = 'translate'",
                [],
                |row| row.get(0),
            )
            .expect("旧映射应保留");
        assert_eq!(kept, "qwen");

        // 升级后新增的 typesafe 站点与 classify 映射要能写；非法值仍被拒。
        conn.execute(
            "INSERT INTO ai_provider (label, kind, base_url) VALUES ('jev', 'typesafe', 'https://api.typesafe.ai/v1')",
            [],
        )
        .expect("升级后应能写 typesafe 站点");
        let typesafe_id: i64 = conn
            .query_row("SELECT id FROM ai_provider WHERE kind = 'typesafe'", [], |row| {
                row.get(0)
            })
            .expect("读 typesafe 站点");
        conn.execute(
            "INSERT INTO ai_model_map (function, provider_id, model) VALUES ('classify', ?1, 'jev-latest')",
            rusqlite::params![typesafe_id],
        )
        .expect("升级后应能写 classify 映射");
        let bad_kind = conn.execute(
            "INSERT INTO ai_provider (label, kind, base_url) VALUES ('坏', 'nope', 'https://x')",
            [],
        );
        assert!(bad_kind.is_err(), "非法站点类型仍应被 CHECK 拒绝");
        let bad_function = conn.execute(
            "INSERT INTO ai_model_map (function, provider_id, model) VALUES ('nope', 1, 'x')",
            [],
        );
        assert!(bad_function.is_err(), "非法功能值仍应被 CHECK 拒绝");

        // 标签表升级后应有十条标签（八条老标签 + 两条是否题标签），且兜底组内唯一。
        let labels: i64 = conn
            .query_row("SELECT COUNT(*) FROM mail_label", [], |row| row.get(0))
            .expect("数标签");
        assert_eq!(labels, 10, "0016 后应有十条标签");
        let content_group_id: i64 = conn
            .query_row(
                "SELECT id FROM mail_label_group WHERE name = '内容分类'",
                [],
                |row| row.get(0),
            )
            .expect("读内容分类组");
        let second_fallback = conn.execute(
            "INSERT INTO mail_label (group_id, name, color, sort_order, is_fallback) \
             VALUES (?1, '假兜底', 'gray', 2000, 1)",
            rusqlite::params![content_group_id],
        );
        assert!(second_fallback.is_err(), "同一组内兜底标签只能有一条");
    }

    #[test]
    fn migration_0016_creates_label_groups() {
        let dir = tempfile::tempdir().expect("临时目录");
        let mut store = Store::open(dir.path().join("mail.db")).expect("打开数据库");
        store.run_migrations().expect("跑迁移");

        let conn = store.conn();
        let mut stmt = conn
            .prepare(
                "SELECT name, kind, enabled, confidence_threshold, score_min, score_max, sort_order \
                 FROM mail_label_group ORDER BY sort_order ASC",
            )
            .expect("准备查询组");
        type RawLabelGroup = (String, String, i64, f64, Option<i64>, Option<i64>, i64);
        let groups: Vec<RawLabelGroup> = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .expect("查询组")
            .collect::<Result<_, _>>()
            .expect("收集组");

        assert_eq!(groups.len(), 3, "应预置三个默认组");
        assert_eq!(
            groups
                .iter()
                .map(|g| (g.0.as_str(), g.1.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("内容分类", "choice"),
                ("需要我回复", "boolean"),
                ("紧急程度", "score"),
            ]
        );
        assert!(groups.iter().all(|g| g.2 == 0), "三个默认组应默认关闭");
        assert_eq!(groups[0].3, 0.75);
        assert_eq!(groups[1].3, 0.5);
        assert_eq!(groups[2].3, 0.75);
        assert_eq!((groups[0].4, groups[0].5), (None, None));
        assert_eq!((groups[2].4, groups[2].5), (Some(1), Some(5)));
        assert_eq!(groups.iter().map(|g| g.6).collect::<Vec<_>>(), vec![10, 20, 30]);
    }

    #[test]
    fn migration_0016_keeps_old_labels_in_content_group() {
        let dir = tempfile::tempdir().expect("临时目录");
        let mut store = Store::open(dir.path().join("mail.db")).expect("打开数据库");
        store.run_migrations().expect("跑迁移");

        let conn = store.conn();
        let content_id: i64 = conn
            .query_row(
                "SELECT id FROM mail_label_group WHERE name = '内容分类'",
                [],
                |row| row.get(0),
            )
            .expect("读内容分类组");

        for (name, expected_sort) in [
            ("客户", 10_i64),
            ("账单", 20),
            ("通知", 30),
            ("推广", 40),
            ("招聘", 50),
            ("同事", 60),
            ("家人", 70),
            ("其他", 1000),
        ] {
            let (group_id, sort_order): (i64, i64) = conn
                .query_row(
                    "SELECT group_id, sort_order FROM mail_label WHERE name = ?1",
                    rusqlite::params![name],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("读老标签");
            assert_eq!(group_id, content_id, "老标签 {name} 应在内容分类组");
            assert_eq!(sort_order, expected_sort, "老标签 {name} 排序应不变");
        }

        let (fallback_name, fallback_color): (String, String) = conn
            .query_row(
                "SELECT name, color FROM mail_label WHERE group_id = ?1 AND is_fallback = 1",
                rusqlite::params![content_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("读兜底标签");
        assert_eq!(fallback_name, "其他");
        assert_eq!(fallback_color, "gray");

        let fallback_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mail_label WHERE group_id = ?1 AND is_fallback = 1",
                rusqlite::params![content_id],
                |row| row.get(0),
            )
            .expect("数组内兜底");
        assert_eq!(fallback_count, 1, "每组内最多一个兜底");

        let boolean_labels: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mail_label l JOIN mail_label_group g ON g.id = l.group_id \
                 WHERE g.name = '需要我回复'",
                [],
                |row| row.get(0),
            )
            .expect("数是否题标签");
        assert_eq!(boolean_labels, 2, "是否题组应预置两条标签");

        let message_label_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'message_label'",
                [],
                |row| row.get(0),
            )
            .expect("读结果表 SQL");
        assert!(
            message_label_sql.contains("PRIMARY KEY (message_id, group_id)"),
            "结果表主键应是（邮件, 组）：{message_label_sql}"
        );
    }
    #[test]
    fn 从十七号库升级后可写决策模型站点() {
        let mut store = Store::open_in_memory().expect("打开内存库");
        {
            let conn = store.raw_connection_for_test();
            conn.execute_batch(super::MIGRATION_TABLE_SQL).expect("建登记表");
            for migration in &MIGRATIONS[..17] {
                conn.execute_batch(migration.sql).expect("应用旧迁移");
                conn.execute(
                    "INSERT INTO schema_migration (version, name, checksum) VALUES (?1, ?2, ?3)",
                    rusqlite::params![migration.version, migration.name, checksum(migration.sql)],
                )
                .expect("登记旧迁移");
            }
            conn.execute(
                "INSERT INTO ai_provider (label, kind, base_url, default_model) \
                 VALUES ('旧 JEV', 'typesafe', 'https://api.typesafe.ai/v1', 'jev-latest')",
                [],
            )
            .expect("插入旧决策站点");
            conn.execute_batch(
                "INSERT INTO proxy (id, label, kind, host, port) VALUES (18, '测试代理', 'http', '127.0.0.1', 8080);
                 UPDATE ai_provider SET models_json = '[\"jev-latest\"]', thinking_level = 'high',
                     enabled = 0, api_key_ref = 'ai/test-reference-only', proxy_mode = 'custom', proxy_id = 18,
                     created_at = '2026-10-19T00:00:00Z', updated_at = '2026-10-20T00:00:00Z';
                 INSERT INTO ai_model_map (function, provider_id, model, thinking_level, updated_at)
                     VALUES ('classify', 1, 'jev-latest', 'low', '2026-10-20T00:00:00Z');",
            )
            .expect("准备旧站点字段与映射");
        }

        let old_providers = store.list_ai_providers().expect("读取旧站点快照");
        let old_maps = store.list_ai_model_maps().expect("读取旧映射快照");
        store.run_migrations().expect("升级到十八号库");
        assert_eq!(store.list_ai_providers().expect("读取升级后站点"), old_providers);
        assert_eq!(store.list_ai_model_maps().expect("读取升级后映射"), old_maps);
        assert_eq!(
            store.applied_migration_versions().expect("读取版本"),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18]
        );

        let conn = store.raw_connection_for_test();
        let old_kind: String = conn
            .query_row("SELECT kind FROM ai_provider WHERE label = '旧 JEV'", [], |row| {
                row.get(0)
            })
            .expect("旧站点应保留");
        assert_eq!(old_kind, "typesafe");
        conn.execute(
            "INSERT INTO ai_provider (label, kind, base_url, default_model) \
             VALUES ('StartLux', 'decision_api', 'https://api.bamboonode.dev/v1/decisions', \
                     'startlux-decision-27b-autothink')",
            [],
        )
        .expect("升级后应能写 decision_api 站点");
        assert!(conn
            .execute(
                "INSERT INTO ai_provider (label, kind, base_url) VALUES ('坏', 'nope', 'https://x')",
                [],
            )
            .is_err());
    }

    #[test]
    fn 迁移清单与校验和的基本性质() {
        assert_eq!(MIGRATIONS.len(), 18);
        assert_eq!(supported_version(), 18);
        assert_eq!(checksum("abc"), checksum("abc"));
        assert_ne!(checksum("abc"), checksum("abd"));
    }
}
