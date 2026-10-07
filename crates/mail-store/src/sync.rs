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
    /// 展示用完整路径（已把 IMAP 文件夹编码解成正常文字）。
    pub full_path: String,
    /// 服务器上的原始完整路径；老数据尚未对齐时为 None。
    pub server_path: Option<String>,
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

/// 准备对齐到本地的一条服务器文件夹。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFolder {
    /// 展示用完整路径。
    pub full_path: String,
    /// 服务器原始完整路径。
    pub server_path: String,
    /// 层级分隔符。
    pub delimiter: String,
    /// 归类结果。
    pub kind: FolderKind,
}

/// 一条还没得到服务器确认的红旗变更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingFlag {
    /// 邮件主键。
    pub message_id: i64,
    /// 所属账号。
    pub account_id: i64,
    /// 所属文件夹主键。
    pub folder_id: i64,
    /// 服务器上的原始文件夹名（回写时要 SELECT 这个）。
    pub server_path: String,
    /// 服务器上的邮件 UID。
    pub uid: u32,
    /// 本地期望的目标状态：true 标红，false 取消。
    pub flagged: bool,
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

const SELECT_FOLDER: &str = "SELECT id, account_id, full_path, server_path, delimiter, kind, \
    uidvalidity, uidnext, synced_min_uid, last_sync_at, unread_count FROM folder";

fn row_to_folder(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredFolder> {
    let kind: String = row.get(5)?;
    let uidvalidity: Option<i64> = row.get(6)?;
    let uidnext: Option<i64> = row.get(7)?;
    let synced_min_uid: Option<i64> = row.get(8)?;
    Ok(StoredFolder {
        id: row.get(0)?,
        account_id: row.get(1)?,
        full_path: row.get(2)?,
        server_path: row.get(3)?,
        delimiter: row.get(4)?,
        kind: FolderKind::parse(&kind).unwrap_or(FolderKind::Custom),
        uidvalidity: uidvalidity.and_then(|value| u32::try_from(value).ok()),
        uidnext: uidnext.and_then(|value| u32::try_from(value).ok()),
        synced_min_uid: synced_min_uid.and_then(|value| u32::try_from(value).ok()),
        last_sync_at: row.get(9)?,
        unread_count: row.get(10)?,
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

/// 把同一账号下的重复文件夹并进目标行，并把邮件一起搬过去。
fn merge_folder_rows(
    tx: &rusqlite::Transaction<'_>,
    target_id: i64,
    source_id: i64,
) -> Result<(), StoreError> {
    if target_id == source_id {
        return Ok(());
    }
    // 两边 UID 相同的是同一封服务器邮件，先删旧行里的副本，避免唯一索引冲突。
    tx.execute(
        "DELETE FROM message WHERE folder_id = ?1 AND uid IN \
         (SELECT uid FROM message WHERE folder_id = ?2)",
        rusqlite::params![source_id, target_id],
    )?;
    tx.execute(
        "UPDATE message SET folder_id = ?2 WHERE folder_id = ?1",
        rusqlite::params![source_id, target_id],
    )?;
    tx.execute("DELETE FROM folder WHERE id = ?1", [source_id])?;
    Ok(())
}

/// 在事务里按“服务器原始名优先、旧乱码名兜底”的规则写入或更新一行文件夹。
fn upsert_folder_tx(
    tx: &rusqlite::Transaction<'_>,
    account_id: i64,
    full_path: &str,
    server_path: &str,
    delimiter: &str,
    kind: FolderKind,
) -> Result<i64, StoreError> {
    let target: Option<i64> = tx
        .query_row(
            "SELECT id FROM folder WHERE account_id = ?1 AND server_path = ?2",
            rusqlite::params![account_id, server_path],
            |row| row.get(0),
        )
        .optional()?;
    let legacy: Option<i64> = tx
        .query_row(
            "SELECT id FROM folder WHERE account_id = ?1 \
             AND (server_path IS NULL OR server_path = '') AND full_path = ?2 \
             ORDER BY id LIMIT 1",
            rusqlite::params![account_id, server_path],
            |row| row.get(0),
        )
        .optional()?;
    // displayed：认领展示名相同的老行。server_path 等于自身 full_path 的是历史脏行
    // （早期版本把展示名误当成服务器名写入），一并合并，避免留下重复文件夹。
    let displayed: Option<i64> = tx
        .query_row(
            "SELECT id FROM folder WHERE account_id = ?1 AND full_path = ?2 \
             AND (server_path IS NULL OR server_path = '' OR server_path = full_path) \
             ORDER BY id LIMIT 1",
            rusqlite::params![account_id, full_path],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(target_id) = target {
        for source_id in [legacy, displayed].into_iter().flatten() {
            merge_folder_rows(tx, target_id, source_id)?;
        }
        tx.execute(
            "UPDATE folder SET full_path = ?2, delimiter = ?3, kind = ?4, \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ?1",
            rusqlite::params![target_id, full_path, delimiter, kind.as_str()],
        )?;
        return Ok(target_id);
    }

    if let Some(legacy_id) = legacy {
        if let Some(displayed_id) = displayed {
            merge_folder_rows(tx, legacy_id, displayed_id)?;
        }
        tx.execute(
            "UPDATE folder SET full_path = ?2, server_path = ?3, delimiter = ?4, kind = ?5, \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ?1",
            rusqlite::params![legacy_id, full_path, server_path, delimiter, kind.as_str()],
        )?;
        return Ok(legacy_id);
    }

    if let Some(displayed_id) = displayed {
        tx.execute(
            "UPDATE folder SET server_path = ?2, delimiter = ?3, kind = ?4, \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ?1",
            rusqlite::params![displayed_id, server_path, delimiter, kind.as_str()],
        )?;
        return Ok(displayed_id);
    }

    tx.execute(
        "INSERT INTO folder (account_id, full_path, server_path, delimiter, kind) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![account_id, full_path, server_path, delimiter, kind.as_str()],
    )?;
    Ok(tx.last_insert_rowid())
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
        self.upsert_folder_mapped(account_id, full_path, full_path, delimiter, kind)
    }

    /// 按显示名 + 服务器原始名写入一个文件夹，返回主键。
    pub fn upsert_folder_mapped(
        &self,
        account_id: i64,
        full_path: &str,
        server_path: &str,
        delimiter: &str,
        kind: FolderKind,
    ) -> Result<i64, StoreError> {
        let tx = self.conn().unchecked_transaction()?;
        let id = upsert_folder_tx(&tx, account_id, full_path, server_path, delimiter, kind)?;
        tx.commit()?;
        Ok(id)
    }

    /// 用服务器刚返回的文件夹列表对齐本地：认领旧乱码行、合并重复行、清掉空的乱码残留。
    pub fn align_folders(&self, account_id: i64, folders: &[NewFolder]) -> Result<(), StoreError> {
        let tx = self.conn().unchecked_transaction()?;
        let mut kept_ids = Vec::with_capacity(folders.len());
        for folder in folders {
            let id = upsert_folder_tx(
                &tx,
                account_id,
                &folder.full_path,
                &folder.server_path,
                &folder.delimiter,
                folder.kind,
            )?;
            kept_ids.push(id);
        }

        let leftovers: Vec<(i64, String, Option<String>)> = {
            let mut stmt =
                tx.prepare("SELECT id, full_path, server_path FROM folder WHERE account_id = ?1")?;
            let rows = stmt.query_map([account_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            let mut items = Vec::new();
            for row in rows {
                items.push(row?);
            }
            items
        };

        for (id, full_path, server_path) in leftovers {
            if kept_ids.contains(&id) {
                continue;
            }
            let is_garbled_legacy =
                server_path.as_deref().unwrap_or("").is_empty() && full_path.contains('&');
            if !is_garbled_legacy {
                continue;
            }
            let message_count: i64 =
                tx.query_row("SELECT COUNT(*) FROM message WHERE folder_id = ?1", [id], |row| {
                    row.get(0)
                })?;
            if message_count == 0 {
                tx.execute("DELETE FROM folder WHERE id = ?1", [id])?;
            } else {
                // 主规格没给“服务器已删文件夹”的落盘规则；这里宁可保留，也不静默丢邮件。
                tracing::warn!(
                    account = account_id,
                    folder = %full_path,
                    "乱码残留文件夹里还有邮件，先保留避免数据丢失"
                );
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// 按账号与路径取文件夹。
    pub fn get_folder(&self, account_id: i64, full_path: &str) -> Result<Option<StoredFolder>, StoreError> {
        let sql = format!("{SELECT_FOLDER} WHERE account_id = ?1 AND full_path = ?2");
        self.conn()
            .query_row(&sql, rusqlite::params![account_id, full_path], row_to_folder)
            .optional()
            .map_err(StoreError::from)
    }

    /// 按账号与服务器原始名取文件夹。
    pub fn get_folder_by_server_path(
        &self,
        account_id: i64,
        server_path: &str,
    ) -> Result<Option<StoredFolder>, StoreError> {
        let sql = format!("{SELECT_FOLDER} WHERE account_id = ?1 AND server_path = ?2");
        self.conn()
            .query_row(&sql, rusqlite::params![account_id, server_path], row_to_folder)
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
        // 已存在的邮件不重写整行，只刷新「有附件」这一项：老库第一次跑新的
        // BODYSTRUCTURE 抓取后，附件标记才能补上，也不碰本地红旗等状态。
        {
            let mut refresh = tx.prepare(
                "UPDATE message SET has_attachments = ?3,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE folder_id = ?1 AND uid = ?2 AND has_attachments <> ?3",
            )?;
            for message in messages {
                refresh.execute(rusqlite::params![
                    message.folder_id,
                    i64::from(message.uid),
                    i64::from(message.has_attachments),
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
            "UPDATE message SET is_read = ?3,
                 is_flagged = CASE WHEN flag_pending = 1 THEN is_flagged ELSE ?4 END,
                 is_answered = ?5, is_draft = ?6,
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

    /// 本地切换一封邮件的红旗；状态真的变了才记「待同步」。
    pub fn set_message_flagged(&self, message_id: i64, flagged: bool) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "UPDATE message SET is_flagged = ?2, flag_pending = 1,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND is_flagged <> ?2",
            rusqlite::params![message_id, i64::from(flagged)],
        )?;
        Ok(changed > 0)
    }

    /// 列出某账号所有还没得到服务器确认的红旗变更。
    pub fn list_pending_flags(&self, account_id: i64) -> Result<Vec<PendingFlag>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT m.id, m.account_id, m.folder_id,
                    COALESCE(NULLIF(f.server_path, ''), f.full_path), m.uid, m.is_flagged
             FROM message m JOIN folder f ON f.id = m.folder_id
             WHERE m.account_id = ?1 AND m.flag_pending = 1
             ORDER BY m.id",
        )?;
        let rows = stmt.query_map([account_id], |row| {
            let uid: i64 = row.get(4)?;
            let flagged: i64 = row.get(5)?;
            Ok(PendingFlag {
                message_id: row.get(0)?,
                account_id: row.get(1)?,
                folder_id: row.get(2)?,
                server_path: row.get(3)?,
                uid: u32::try_from(uid).unwrap_or(0),
                flagged: flagged != 0,
            })
        })?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    /// 服务器回写成功后清掉「待同步」标记。
    pub fn clear_message_flag_pending(&self, message_id: i64) -> Result<(), StoreError> {
        self.conn()
            .execute("UPDATE message SET flag_pending = 0 WHERE id = ?1", [message_id])?;
        Ok(())
    }

    /// 一封邮件属于哪个账号；邮件不存在返回 None。
    pub fn message_account_id(&self, message_id: i64) -> Result<Option<i64>, StoreError> {
        self.conn()
            .query_row(
                "SELECT account_id FROM message WHERE id = ?1",
                [message_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    /// 这封邮件是否还有待同步的红旗变更；邮件不存在返回 false。
    pub fn is_flag_pending(&self, message_id: i64) -> Result<bool, StoreError> {
        let value: Option<i64> = self
            .conn()
            .query_row(
                "SELECT flag_pending FROM message WHERE id = ?1",
                [message_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.unwrap_or(0) != 0)
    }

    /// 文件夹里最大的 UID；空文件夹返回 None。
    /// 列出某文件夹里已入库的服务器 UID（升序去零）；附件标记一次性补齐用。
    pub fn list_message_uids(&self, folder_id: i64) -> Result<Vec<u32>, StoreError> {
        let mut stmt = self
            .conn()
            .prepare("SELECT uid FROM message WHERE folder_id = ?1 ORDER BY uid ASC")?;
        let rows = stmt.query_map([folder_id], |row| row.get::<_, i64>(0))?;
        let mut uids = Vec::new();
        for row in rows {
            let uid = row?;
            if let Ok(uid) = u32::try_from(uid) {
                if uid > 0 {
                    uids.push(uid);
                }
            }
        }
        Ok(uids)
    }

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

    use super::{NewFolder, NewMessage};
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
            oauth_provider: None,
            oauth_client_id: String::new(),
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
    fn 对齐文件夹会认领旧乱码行并补上服务器原名() {
        let store = migrated();
        let account_id = account(&store);
        store
            .raw_connection_for_test()
            .execute(
                "INSERT INTO folder (account_id, full_path, delimiter, kind) \
                 VALUES (?1, '&Xn9USpCuTvY-', '/', 'custom')",
                [account_id],
            )
            .expect("插入旧乱码行");
        let legacy_id: i64 = store
            .raw_connection_for_test()
            .query_row(
                "SELECT id FROM folder WHERE account_id = ?1",
                [account_id],
                |row| row.get(0),
            )
            .expect("取旧行编号");
        store
            .insert_messages(&[message(account_id, legacy_id, 7)])
            .expect("给旧行写一封邮件");

        store
            .align_folders(
                account_id,
                &[NewFolder {
                    full_path: "广告邮件".to_string(),
                    server_path: "&Xn9USpCuTvY-".to_string(),
                    delimiter: "/".to_string(),
                    kind: FolderKind::Custom,
                }],
            )
            .expect("对齐文件夹");

        let folders = store.list_folders(account_id).expect("列文件夹");
        assert_eq!(folders.len(), 1, "旧乱码行应被认领，而不是新增一行");
        assert_eq!(folders[0].id, legacy_id);
        assert_eq!(folders[0].full_path, "广告邮件");
        assert_eq!(folders[0].server_path.as_deref(), Some("&Xn9USpCuTvY-"));
        assert_eq!(
            store.count_folder_messages(legacy_id).expect("邮件仍在"),
            1,
            "认领旧行不能丢邮件"
        );
        let by_server = store
            .get_folder_by_server_path(account_id, "&Xn9USpCuTvY-")
            .expect("按服务器名查")
            .expect("应能查到");
        assert_eq!(by_server.id, legacy_id);
    }

    #[test]
    fn 对齐文件夹会把旧乱码重复行并进服务器名那一行() {
        let store = migrated();
        let account_id = account(&store);
        let mapped_id = store
            .upsert_folder_mapped(account_id, "广告邮件", "&Xn9USpCuTvY-", "/", FolderKind::Custom)
            .expect("服务器名那行");
        store
            .raw_connection_for_test()
            .execute(
                "INSERT INTO folder (account_id, full_path, delimiter, kind) \
                 VALUES (?1, '&Xn9USpCuTvY-', '/', 'custom')",
                [account_id],
            )
            .expect("旧乱码重复行");
        let legacy_id: i64 = store
            .raw_connection_for_test()
            .query_row(
                "SELECT id FROM folder WHERE account_id = ?1 AND full_path = '&Xn9USpCuTvY-'",
                [account_id],
                |row| row.get(0),
            )
            .expect("取旧行编号");
        store
            .insert_messages(&[message(account_id, legacy_id, 7)])
            .expect("给旧行写一封邮件");

        store
            .align_folders(
                account_id,
                &[NewFolder {
                    full_path: "广告邮件".to_string(),
                    server_path: "&Xn9USpCuTvY-".to_string(),
                    delimiter: "/".to_string(),
                    kind: FolderKind::Custom,
                }],
            )
            .expect("对齐文件夹");

        let folders = store.list_folders(account_id).expect("列文件夹");
        assert_eq!(folders.len(), 1, "重复行应被并掉");
        assert_eq!(folders[0].id, mapped_id, "保留服务器名那一行");
        assert_eq!(store.count_folder_messages(mapped_id).expect("邮件已搬过来"), 1);
    }

    #[test]
    fn 对齐文件夹会把服务器名写成展示名的脏行并进乱码行() {
        let store = migrated();
        let account_id = account(&store);
        store
            .raw_connection_for_test()
            .execute(
                "INSERT INTO folder (account_id, full_path, delimiter, kind) \
                 VALUES (?1, '&g0l6P3ux-', '/', 'custom')",
                [account_id],
            )
            .expect("insert legacy garbled row");
        let legacy_id: i64 = store
            .raw_connection_for_test()
            .query_row(
                "SELECT id FROM folder WHERE account_id = ?1 AND full_path = '&g0l6P3ux-'",
                [account_id],
                |row| row.get(0),
            )
            .expect("legacy id");
        store
            .raw_connection_for_test()
            .execute(
                "INSERT INTO folder (account_id, full_path, server_path, delimiter, kind) \
                 VALUES (?1, 'Drafts', 'Drafts', '/', 'draft')",
                [account_id],
            )
            .expect("insert dirty row");
        let dirty_id: i64 = store
            .raw_connection_for_test()
            .query_row(
                "SELECT id FROM folder WHERE account_id = ?1 AND full_path = 'Drafts'",
                [account_id],
                |row| row.get(0),
            )
            .expect("dirty id");
        let legacy_batch: Vec<NewMessage> = (1..=6).map(|uid| message(account_id, legacy_id, uid)).collect();
        store.insert_messages(&legacy_batch).expect("legacy messages");
        let dirty_batch: Vec<NewMessage> = (1..=6).map(|uid| message(account_id, dirty_id, uid)).collect();
        store.insert_messages(&dirty_batch).expect("dirty messages");

        store
            .align_folders(
                account_id,
                &[NewFolder {
                    full_path: "Drafts".to_string(),
                    server_path: "&g0l6P3ux-".to_string(),
                    delimiter: "/".to_string(),
                    kind: FolderKind::Draft,
                }],
            )
            .expect("align");

        let folders = store.list_folders(account_id).expect("list");
        assert_eq!(folders.len(), 1, "dirty duplicate row should be merged away");
        assert_eq!(folders[0].id, legacy_id, "keep claimed legacy row");
        assert_eq!(folders[0].full_path, "Drafts");
        assert_eq!(folders[0].server_path.as_deref(), Some("&g0l6P3ux-"));
        assert_eq!(
            store.count_folder_messages(legacy_id).expect("messages"),
            6,
            "same-uid duplicates must be deduped"
        );
    }
    #[test]
    fn reinserting_a_known_uid_only_refreshes_the_attachment_flag() {
        let store = migrated();
        let account_id = account(&store);
        let folder_id = store
            .upsert_folder(account_id, "INBOX", "/", FolderKind::Inbox)
            .expect("folder");
        let first = message(account_id, folder_id, 10);
        assert_eq!(store.insert_messages(&[first]).expect("insert"), 1);

        let mut again = message(account_id, folder_id, 10);
        again.has_attachments = true;
        assert_eq!(store.insert_messages(&[again]).expect("reinsert"), 0);

        let flag: i64 = store
            .raw_connection_for_test()
            .query_row(
                "SELECT has_attachments FROM message WHERE folder_id = ?1 AND uid = 10",
                [folder_id],
                |row| row.get(0),
            )
            .expect("read");
        assert_eq!(flag, 1, "attachment flag should be refreshed on reinsert");
        assert_eq!(store.count_folder_messages(folder_id).expect("count"), 1);
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
    fn 红旗本地切换会记待同步且不被远端刷新覆盖() {
        let store = migrated();
        let account_id = account(&store);
        let folder_id = store
            .upsert_folder(account_id, "INBOX", "/", FolderKind::Inbox)
            .expect("插入文件夹");
        store
            .insert_messages(&[message(account_id, folder_id, 10)])
            .expect("写邮件");
        let message_id: i64 = store
            .raw_connection_for_test()
            .query_row(
                "SELECT id FROM message WHERE folder_id = ?1 AND uid = 10",
                [folder_id],
                |row| row.get(0),
            )
            .expect("取邮件编号");

        assert!(store.set_message_flagged(message_id, true).expect("标红"));
        assert!(
            !store.set_message_flagged(message_id, true).expect("同值再点"),
            "状态没变不该重复记待同步"
        );

        let pending = store.list_pending_flags(account_id).expect("列待同步");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].uid, 10);
        assert_eq!(pending[0].server_path, "INBOX");
        assert!(pending[0].flagged);

        // 服务器还是旧的「没红旗」，回来时不能覆盖本地待同步状态。
        store
            .update_message_flags(folder_id, 10, false, false, false, false)
            .expect("刷新远端标志");
        let is_flagged: i64 = store
            .raw_connection_for_test()
            .query_row(
                "SELECT is_flagged FROM message WHERE id = ?1",
                [message_id],
                |row| row.get(0),
            )
            .expect("读状态");
        assert_eq!(is_flagged, 1, "待同步的红旗不能被远端覆盖");

        // 回写成功后清掉待同步，再以服务器为准。
        store.clear_message_flag_pending(message_id).expect("清待同步");
        assert!(store.list_pending_flags(account_id).expect("列待同步").is_empty());
        store
            .update_message_flags(folder_id, 10, false, false, false, false)
            .expect("再刷新远端标志");
        let is_flagged: i64 = store
            .raw_connection_for_test()
            .query_row(
                "SELECT is_flagged FROM message WHERE id = ?1",
                [message_id],
                |row| row.get(0),
            )
            .expect("读状态");
        assert_eq!(is_flagged, 0, "没有待同步时以服务器为准");
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
