//! 文件夹、邮件与同步任务的读写。
//!
//! Wave 2 起使用：`folder` 保存文件夹映射与同步断点，`message` 保存邮件元数据
//! （信封 + 标志，正文留给 Wave 4 懒加载），`sync_job` 供界面看进度。
//!
//! 约定：所有写入都在本模块内完成，其它 crate 只能经这些方法碰库。

use mail_domain::{FolderKind, SyncJobKind, SyncJobState};
use rusqlite::OptionalExtension;

use crate::connection::Store;
use crate::error::StoreError;

/// 数据库里的一行文件夹。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFolder {
    /// 主键。
    pub id: i64,
    /// 所属账号。
    pub account_id: i64,
    /// 服务器上的完整路径（如 `INBOX`、`其他文件夹/项目`）。
    pub full_path: String,
    /// 层级分隔符（多数服务器是 `/`）。
    pub delimiter: String,
    /// 归类结果。
    pub kind: FolderKind,
    /// 服务器给的 UIDVALIDITY；没同步过时为 None。
    pub uidvalidity: Option<u32>,
    /// 服务器给的下一封邮件 UID。
    pub uidnext: Option<u32>,
    /// 后台补齐的断点：本地已覆盖到的最小 UID。None 表示还没建立快照。
    pub synced_min_uid: Option<u32>,
    /// 上次同步完成时间（UTC）。
    pub last_sync_at: Option<String>,
    /// 服务器报告的未读数。
    pub unread_count: i64,
}

/// 准备写入的一条邮件元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    /// 所属账号。
    pub account_id: i64,
    /// 所属文件夹。
    pub folder_id: i64,
    /// 服务器 UID。
    pub uid: u32,
    /// Message-ID 头（可能为空）。
    pub message_id_header: String,
    /// 会话归并键。
    pub thread_key: String,
    /// 主题。
    pub subject: String,
    /// 发件人显示名。
    pub from_name: String,
    /// 发件人邮箱。
    pub from_addr: String,
    /// 收件人 JSON 数组。
    pub to_json: String,
    /// 抄送 JSON 数组。
    pub cc_json: String,
    /// 日期（UTC ISO-8601）。
    pub date_utc: String,
    /// 邮件大小（字节）。
    pub size: u32,
    /// 是否含附件（Wave 2 先用 BODYSTRUCTURE 缺失时的保守值 false）。
    pub has_attachments: bool,
    /// 是否已读（`\\Seen`）。
    pub is_read: bool,
    /// 是否星标（`\\Flagged`）。
    pub is_flagged: bool,
    /// 是否已回复（`\\Answered`）。
    pub is_answered: bool,
    /// 是否草稿（`\\Draft`）。
    pub is_draft: bool,
}

/// 数据库里的一行同步任务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSyncJob {
    /// 主键。
    pub id: i64,
    /// 所属账号。
    pub account_id: i64,
    /// 关联文件夹（账号级任务为 None）。
    pub folder_id: Option<i64>,
    /// 任务种类。
    pub kind: SyncJobKind,
    /// 状态。
    pub state: SyncJobState,
    /// 进度（已处理条数）。
    pub progress: i64,
    /// 失败原因（已脱敏）。
    pub error: Option<String>,
    /// 更新时间。
    pub updated_at: String,
}

const SELECT_FOLDER: &str = "SELECT id, account_id, full_path, delimiter, kind, uidvalidity, \
    uidnext, synced_min_uid, last_sync_at, unread_count FROM folder";

fn row_to_folder(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredFolder> {
    let kind: String = row.get(4)?;
    let uidvalidity: Option<i64> = row.get(5)?;
    let uidnext: Option<i64> = row.get(6)?;
    let synced_min_uid: Option<i64> = row.get(7)?;
    Ok(StoredFolder {
        id: row.get(0)?,
        account_id: row.get(1)?,
        full_path: row.get(2)?,
        delimiter: row.get(3)?,
        kind: FolderKind::parse(&kind).unwrap_or(FolderKind::Custom),
        uidvalidity: uidvalidity.and_then(|value| u32::try_from(value).ok()),
        uidnext: uidnext.and_then(|value| u32::try_from(value).ok()),
        synced_min_uid: synced_min_uid.and_then(|value| u32::try_from(value).ok()),
        last_sync_at: row.get(8)?,
        unread_count: row.get(9)?,
    })
}

const SELECT_SYNC_JOB: &str = "SELECT id, account_id, folder_id, kind, state, progress, error, \
    updated_at FROM sync_job";

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredSyncJob> {
    let kind: String = row.get(3)?;
    let state: String = row.get(4)?;
    Ok(StoredSyncJob {
        id: row.get(0)?,
        account_id: row.get(1)?,
        folder_id: row.get(2)?,
        kind: SyncJobKind::parse(&kind).unwrap_or(SyncJobKind::Incremental),
        state: SyncJobState::parse(&state).unwrap_or(SyncJobState::Queued),
        progress: row.get(5)?,
        error: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

impl Store {
    /// 新建或更新一个文件夹，返回主键。
    pub fn upsert_folder(
        &self,
        account_id: i64,
        full_path: &str,
        delimiter: &str,
        kind: FolderKind,
    ) -> Result<i64, StoreError> {
        self.conn().execute(
            "INSERT INTO folder (account_id, full_path, delimiter, kind)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, full_path) DO UPDATE SET
                 delimiter = excluded.delimiter,
                 kind = excluded.kind,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            rusqlite::params![account_id, full_path, delimiter, kind.as_str()],
        )?;
        let id: i64 = self.conn().query_row(
            "SELECT id FROM folder WHERE account_id = ?1 AND full_path = ?2",
            rusqlite::params![account_id, full_path],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    /// 按账号与路径取文件夹。
    pub fn get_folder(&self, account_id: i64, full_path: &str) -> Result<Option<StoredFolder>, StoreError> {
        let sql = format!("{SELECT_FOLDER} WHERE account_id = ?1 AND full_path = ?2");
        self.conn()
            .query_row(&sql, rusqlite::params![account_id, full_path], row_to_folder)
            .optional()
            .map_err(StoreError::from)
    }

    /// 按主键读一行文件夹（同步线程只拿得到编号时用）。
    pub fn get_folder_by_id(&self, folder_id: i64) -> Result<Option<StoredFolder>, StoreError> {
        let sql = format!("{SELECT_FOLDER} WHERE id = ?1");
        self.conn()
            .query_row(&sql, [folder_id], row_to_folder)
            .optional()
            .map_err(StoreError::from)
    }
    /// 列出某账号的全部文件夹（收件箱排最前，其余按路径）。
    pub fn list_folders(&self, account_id: i64) -> Result<Vec<StoredFolder>, StoreError> {
        let sql = format!(
            "{SELECT_FOLDER} WHERE account_id = ?1 \
             ORDER BY CASE kind WHEN 'inbox' THEN 0 ELSE 1 END, full_path COLLATE NOCASE"
        );
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query([account_id])?;
        let mut folders = Vec::new();
        while let Some(row) = rows.next()? {
            folders.push(row_to_folder(row)?);
        }
        Ok(folders)
    }

    /// 记录 SELECT 的结果：UIDVALIDITY、UIDNEXT 与未读数。
    pub fn update_folder_select(
        &self,
        folder_id: i64,
        uidvalidity: Option<u32>,
        uidnext: Option<u32>,
        unread_count: i64,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE folder SET uidvalidity = ?2, uidnext = ?3, unread_count = ?4,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![
                folder_id,
                uidvalidity.map(i64::from),
                uidnext.map(i64::from),
                unread_count,
            ],
        )?;
        Ok(())
    }

    /// 更新后台补齐断点。
    pub fn set_folder_synced_min_uid(&self, folder_id: i64, uid: Option<u32>) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE folder SET synced_min_uid = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![folder_id, uid.map(i64::from)],
        )?;
        Ok(())
    }

    /// 标记文件夹刚同步过。
    pub fn touch_folder_synced_at(&self, folder_id: i64) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE folder SET last_sync_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            [folder_id],
        )?;
        Ok(())
    }

    /// 批量写入邮件元数据；重复的 (账号, 文件夹, UID) 跳过。返回真正写入的条数。
    pub fn insert_messages(&self, messages: &[NewMessage]) -> Result<usize, StoreError> {
        if messages.is_empty() {
            return Ok(0);
        }
        let tx = self.conn().unchecked_transaction()?;
        let mut inserted = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO message (
                    account_id, folder_id, uid, message_id_header, thread_key, subject,
                    from_name, from_addr, to_json, cc_json, date_utc, size,
                    has_attachments, is_read, is_flagged, is_answered, is_draft
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            )?;
            for message in messages {
                inserted += stmt.execute(rusqlite::params![
                    message.account_id,
                    message.folder_id,
                    i64::from(message.uid),
                    message.message_id_header,
                    message.thread_key,
                    message.subject,
                    message.from_name,
                    message.from_addr,
                    message.to_json,
                    message.cc_json,
                    message.date_utc,
                    i64::from(message.size),
                    i64::from(message.has_attachments),
                    i64::from(message.is_read),
                    i64::from(message.is_flagged),
                    i64::from(message.is_answered),
                    i64::from(message.is_draft),
                ])?;
            }
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// 刷新一封邮件的标志（远端 → 本地）。
    pub fn update_message_flags(
        &self,
        folder_id: i64,
        uid: u32,
        is_read: bool,
        is_flagged: bool,
        is_answered: bool,
        is_draft: bool,
    ) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "UPDATE message SET is_read = ?3, is_flagged = ?4, is_answered = ?5, is_draft = ?6,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE folder_id = ?1 AND uid = ?2",
            rusqlite::params![
                folder_id,
                i64::from(uid),
                i64::from(is_read),
                i64::from(is_flagged),
                i64::from(is_answered),
                i64::from(is_draft),
            ],
        )?;
        Ok(changed > 0)
    }

    /// 文件夹里最大的 UID；空文件夹返回 None。
    pub fn max_message_uid(&self, folder_id: i64) -> Result<Option<u32>, StoreError> {
        let value: Option<i64> = self.conn().query_row(
            "SELECT MAX(uid) FROM message WHERE folder_id = ?1",
            [folder_id],
            |row| row.get(0),
        )?;
        Ok(value.and_then(|v| u32::try_from(v).ok()))
    }

    /// 文件夹里最小的 UID；空文件夹返回 None。
    pub fn min_message_uid(&self, folder_id: i64) -> Result<Option<u32>, StoreError> {
        let value: Option<i64> = self.conn().query_row(
            "SELECT MIN(uid) FROM message WHERE folder_id = ?1",
            [folder_id],
            |row| row.get(0),
        )?;
        Ok(value.and_then(|v| u32::try_from(v).ok()))
    }

    /// 清空一个文件夹的全部邮件（UIDVALIDITY 变化时重建用）。
    pub fn delete_folder_messages(&self, folder_id: i64) -> Result<usize, StoreError> {
        let removed = self
            .conn()
            .execute("DELETE FROM message WHERE folder_id = ?1", [folder_id])?;
        Ok(removed)
    }

    /// 文件夹里的邮件条数。
    pub fn count_folder_messages(&self, folder_id: i64) -> Result<i64, StoreError> {
        let count: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM message WHERE folder_id = ?1",
            [folder_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// 账号下的邮件总条数。
    pub fn count_account_messages(&self, account_id: i64) -> Result<i64, StoreError> {
        let count: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM message WHERE account_id = ?1",
            [account_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// 账号下邮件总大小（字节）。
    pub fn account_message_bytes(&self, account_id: i64) -> Result<i64, StoreError> {
        let total: Option<i64> = self.conn().query_row(
            "SELECT SUM(size) FROM message WHERE account_id = ?1",
            [account_id],
            |row| row.get(0),
        )?;
        Ok(total.unwrap_or(0))
    }

    /// 新建一条同步任务，返回主键。
    pub fn create_sync_job(
        &self,
        account_id: i64,
        folder_id: Option<i64>,
        kind: SyncJobKind,
    ) -> Result<i64, StoreError> {
        self.conn().execute(
            "INSERT INTO sync_job (account_id, folder_id, kind, state, progress)
             VALUES (?1, ?2, ?3, 'queued', 0)",
            rusqlite::params![account_id, folder_id, kind.as_str()],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// 更新同步任务的状态、进度与失败原因。
    pub fn update_sync_job(
        &self,
        job_id: i64,
        state: SyncJobState,
        progress: i64,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE sync_job SET state = ?2, progress = ?3, error = ?4,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![job_id, state.as_str(), progress, error],
        )?;
        Ok(())
    }

    /// 最近若干条同步任务（按更新时间倒序）。
    pub fn list_sync_jobs(&self, account_id: i64, limit: usize) -> Result<Vec<StoredSyncJob>, StoreError> {
        let sql =
            format!("{SELECT_SYNC_JOB} WHERE account_id = ?1 ORDER BY updated_at DESC, id DESC LIMIT ?2");
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params![account_id, limit as i64])?;
        let mut jobs = Vec::new();
        while let Some(row) = rows.next()? {
            jobs.push(row_to_job(row)?);
        }
        Ok(jobs)
    }

    /// 读一条设置项。
    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        self.conn()
            .query_row("SELECT value FROM setting WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(StoreError::from)
    }

    /// 写一条设置项。
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use mail_domain::{FolderKind, SyncJobKind, SyncJobState};

    use super::NewMessage;
    use crate::Store;

    fn migrated() -> Store {
        let mut store = Store::open_in_memory().expect("打开内存库");
        store.run_migrations().expect("迁移");
        store
    }

    fn account(store: &Store) -> i64 {
        let draft = mail_domain::AccountDraft {
            display_name: "测试".to_string(),
            email: "t@example.com".to_string(),
            auth_type: mail_domain::AuthType::Password,
            username: "t@example.com".to_string(),
            imap: mail_domain::ServerConfig {
                host: "imap.example.com".to_string(),
                port: 993,
                security: mail_domain::Security::Tls,
            },
            smtp: mail_domain::ServerConfig {
                host: "smtp.example.com".to_string(),
                port: 465,
                security: mail_domain::Security::Tls,
            },
            proxy: mail_domain::AccountProxyMode::InheritGlobal,
            color: String::new(),
            enabled: true,
        };
        store.insert_account(&draft, None).expect("插入账号").0
    }

    fn message(account_id: i64, folder_id: i64, uid: u32) -> NewMessage {
        NewMessage {
            account_id,
            folder_id,
            uid,
            message_id_header: format!("<m{uid}@x>"),
            thread_key: format!("主题 {uid}"),
            subject: format!("主题 {uid}"),
            from_name: "张三".to_string(),
            from_addr: "z@example.com".to_string(),
            to_json: "[]".to_string(),
            cc_json: "[]".to_string(),
            date_utc: "2026-10-04T00:00:00Z".to_string(),
            size: 1000,
            has_attachments: false,
            is_read: false,
            is_flagged: false,
            is_answered: false,
            is_draft: false,
        }
    }

    #[test]
    fn 文件夹可增改查且路径唯一() {
        let store = migrated();
        let account_id = account(&store);
        let id = store
            .upsert_folder(account_id, "INBOX", "/", FolderKind::Inbox)
            .expect("插入文件夹");
        let again = store
            .upsert_folder(account_id, "INBOX", "/", FolderKind::Inbox)
            .expect("重复插入应复用");
        assert_eq!(id, again, "同一路径应命中同一行");

        store
            .upsert_folder(account_id, "Sent", "/", FolderKind::Sent)
            .expect("插入已发送");
        let folders = store.list_folders(account_id).expect("列文件夹");
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].full_path, "INBOX", "收件箱排最前");
        assert_eq!(folders[0].kind, FolderKind::Inbox);
    }

    #[test]
    fn 记录选择结果与断点() {
        let store = migrated();
        let account_id = account(&store);
        let id = store
            .upsert_folder(account_id, "INBOX", "/", FolderKind::Inbox)
            .expect("插入");
        store
            .update_folder_select(id, Some(42), Some(1000), 3)
            .expect("记录选择");
        store.set_folder_synced_min_uid(id, Some(500)).expect("断点");
        store.touch_folder_synced_at(id).expect("时间");

        let folder = store
            .get_folder(account_id, "INBOX")
            .expect("查询")
            .expect("存在");
        assert_eq!(folder.uidvalidity, Some(42));
        assert_eq!(folder.uidnext, Some(1000));
        assert_eq!(folder.synced_min_uid, Some(500));
        assert_eq!(folder.unread_count, 3);
        assert!(folder.last_sync_at.is_some());
    }

    #[test]
    fn 邮件批量写入去重且能读回极值() {
        let store = migrated();
        let account_id = account(&store);
        let folder_id = store
            .upsert_folder(account_id, "INBOX", "/", FolderKind::Inbox)
            .expect("插入文件夹");

        let batch = vec![
            message(account_id, folder_id, 10),
            message(account_id, folder_id, 11),
            message(account_id, folder_id, 12),
        ];
        assert_eq!(store.insert_messages(&batch).expect("写入"), 3);
        // 重复写同一 UID 应被忽略。
        assert_eq!(store.insert_messages(&batch[..1]).expect("重复写入"), 0);
        assert_eq!(store.count_folder_messages(folder_id).expect("计数"), 3);
        assert_eq!(store.max_message_uid(folder_id).expect("最大"), Some(12));
        assert_eq!(store.min_message_uid(folder_id).expect("最小"), Some(10));
        assert_eq!(store.count_account_messages(account_id).expect("总数"), 3);
        assert_eq!(store.account_message_bytes(account_id).expect("字节"), 3000);

        assert!(store
            .update_message_flags(folder_id, 11, true, true, false, false)
            .expect("刷新标志"));
        assert!(store.delete_folder_messages(folder_id).expect("清空") >= 3);
        assert_eq!(store.max_message_uid(folder_id).expect("清空后"), None);
    }

    #[test]
    fn 同步任务与设置项可读写() {
        let store = migrated();
        let account_id = account(&store);
        let folder_id = store
            .upsert_folder(account_id, "INBOX", "/", FolderKind::Inbox)
            .expect("插入文件夹");

        let job = store
            .create_sync_job(account_id, Some(folder_id), SyncJobKind::Initial)
            .expect("建任务");
        store
            .update_sync_job(job, SyncJobState::Failed, 120, Some("网络超时"))
            .expect("更新任务");
        let jobs = store.list_sync_jobs(account_id, 10).expect("列任务");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, SyncJobKind::Initial);
        assert_eq!(jobs[0].state, SyncJobState::Failed);
        assert_eq!(jobs[0].progress, 120);
        assert_eq!(jobs[0].error.as_deref(), Some("网络超时"));

        assert_eq!(store.get_setting("sync.enabled").expect("读设置"), None);
        store.set_setting("sync.enabled", "true").expect("写设置");
        assert_eq!(
            store.get_setting("sync.enabled").expect("读设置").as_deref(),
            Some("true")
        );
        store.set_setting("sync.enabled", "false").expect("覆盖设置");
        assert_eq!(
            store.get_setting("sync.enabled").expect("读设置").as_deref(),
            Some("false")
        );
    }
}
