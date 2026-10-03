//! 读信与附件表的读写（Wave 4）。
//!
//! 设计要点：
//! - `message_body` 缓存懒加载的正文；读到就复用，不再联网；
//! - `attachment` 保存附件元数据与本地保存状态，原件按需下载；
//! - 正文状态只用 `none / loading / ready / failed` 四个值，与迁移约束一致；
//! - 邮件正文是不可信内容，本模块只做无副作用的字符串存取，不解释、不执行。
//!
//! 约定：所有写入都在本模块内完成，其它 crate 只能经这些方法碰库。

use rusqlite::OptionalExtension;

use crate::connection::Store;
use crate::error::StoreError;

/// 正文缓存的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMessageBody {
    /// 所属邮件主键。
    pub message_id: i64,
    /// 纯文本正文；没有则为 None。
    pub text_plain: Option<String>,
    /// 清洗后的 HTML 正文；没有则为 None。
    pub html_sanitized: Option<String>,
    /// 上次落库时间（UTC）。
    pub fetched_at: String,
}

/// 正文加载状态；取值与迁移约束一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyState {
    /// 还没拉过。
    None,
    /// 正在拉。
    Loading,
    /// 已就绪。
    Ready,
    /// 拉取失败。
    Failed,
}

impl BodyState {
    /// 存库用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    /// 从存库字符串还原。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "loading" => Some(Self::Loading),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// 附件下载状态；取值与迁移约束一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentState {
    /// 还没下载。
    Pending,
    /// 正在下载。
    Downloading,
    /// 已下载到本地。
    Downloaded,
    /// 下载失败。
    Failed,
}

impl AttachmentState {
    /// 存库用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Downloading => "downloading",
            Self::Downloaded => "downloaded",
            Self::Failed => "failed",
        }
    }

    /// 从存库字符串还原。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "downloading" => Some(Self::Downloading),
            "downloaded" => Some(Self::Downloaded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// 准备写入的一条附件元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAttachment {
    /// MIME 分片下标。
    pub part_index: u32,
    /// 文件名（可能为空）。
    pub filename: String,
    /// MIME 类型（可能为空）。
    pub mime_type: String,
    /// 解码后的字节数。
    pub size: u64,
    /// Content-ID（内嵌图片引用用）。
    pub content_id: Option<String>,
    /// 是否内嵌展示。
    pub is_inline: bool,
}

/// 数据库里的一行附件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAttachment {
    /// 主键。
    pub id: i64,
    /// 所属邮件。
    pub message_id: i64,
    /// MIME 分片下标。
    pub part_index: u32,
    /// 文件名。
    pub filename: String,
    /// MIME 类型。
    pub mime_type: String,
    /// 字节数。
    pub size: u64,
    /// Content-ID。
    pub content_id: Option<String>,
    /// 是否内嵌。
    pub is_inline: bool,
    /// 本地保存路径；未下载时为 None。
    pub local_path: Option<String>,
    /// 下载状态。
    pub state: AttachmentState,
}

/// 一封邮件在服务器上的定位信息，拉取正文或附件时用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageLocation {
    /// 邮件主键。
    pub message_id: i64,
    /// 所属账号。
    pub account_id: i64,
    /// 所属文件夹。
    pub folder_id: i64,
    /// 服务器 UID。
    pub uid: u32,
    /// 文件夹在服务器上的完整路径。
    pub folder_path: String,
    /// 当前正文状态。
    pub body_state: BodyState,
}

const SELECT_BODY: &str = "SELECT message_id, text_plain, html_sanitized, fetched_at FROM message_body";
const SELECT_ATTACHMENT: &str = "SELECT id, message_id, part_index, filename, mime_type, size, \
    content_id, is_inline, local_path, state FROM attachment";

fn row_to_body(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMessageBody> {
    Ok(StoredMessageBody {
        message_id: row.get(0)?,
        text_plain: row.get(1)?,
        html_sanitized: row.get(2)?,
        fetched_at: row.get(3)?,
    })
}

fn row_to_attachment(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredAttachment> {
    let part_index: i64 = row.get(2)?;
    let size: i64 = row.get(5)?;
    let is_inline: i64 = row.get(7)?;
    let state: String = row.get(9)?;
    Ok(StoredAttachment {
        id: row.get(0)?,
        message_id: row.get(1)?,
        part_index: u32::try_from(part_index).unwrap_or(0),
        filename: row.get(3)?,
        mime_type: row.get(4)?,
        size: u64::try_from(size).unwrap_or(0),
        content_id: row.get(6)?,
        is_inline: is_inline != 0,
        local_path: row.get(8)?,
        state: AttachmentState::parse(&state).unwrap_or(AttachmentState::Pending),
    })
}

impl Store {
    /// 读取一封邮件的正文缓存；没有缓存返回 None。
    pub fn get_message_body(&self, message_id: i64) -> Result<Option<StoredMessageBody>, StoreError> {
        let sql = format!("{SELECT_BODY} WHERE message_id = ?1");
        let body = self
            .conn()
            .query_row(&sql, rusqlite::params![message_id], row_to_body)
            .optional()?;
        Ok(body)
    }

    /// 写入或更新一封邮件的正文缓存，并把正文状态标为就绪。
    pub fn save_message_body(
        &self,
        message_id: i64,
        text_plain: Option<&str>,
        html_sanitized: Option<&str>,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO message_body (message_id, text_plain, html_sanitized, fetched_at)
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(message_id) DO UPDATE SET
                 text_plain = excluded.text_plain,
                 html_sanitized = excluded.html_sanitized,
                 fetched_at = excluded.fetched_at",
            rusqlite::params![message_id, text_plain, html_sanitized],
        )?;
        self.set_message_body_state(message_id, BodyState::Ready)?;
        Ok(())
    }

    /// 更新邮件的正文状态（只允许四个固定取值）。
    pub fn set_message_body_state(&self, message_id: i64, state: BodyState) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE message SET body_state = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![message_id, state.as_str()],
        )?;
        Ok(())
    }

    /// 更新邮件的「是否含附件」标记。
    pub fn set_message_has_attachments(
        &self,
        message_id: i64,
        has_attachments: bool,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE message SET has_attachments = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![message_id, i64::from(has_attachments)],
        )?;
        Ok(())
    }

    /// 用最新解析出的附件清单替换一封邮件的附件记录；返回写入条数。
    ///
    /// 会清掉旧记录再插入，避免重复解析后留下重复附件。已下载的本地文件随之失去记录，
    /// 需要重新下载（当前版本可接受）。
    pub fn replace_attachments(
        &self,
        message_id: i64,
        attachments: &[NewAttachment],
    ) -> Result<usize, StoreError> {
        let tx = self.conn().unchecked_transaction()?;
        tx.execute(
            "DELETE FROM attachment WHERE message_id = ?1",
            rusqlite::params![message_id],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO attachment
                     (message_id, part_index, filename, mime_type, size, content_id, is_inline)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for item in attachments {
                stmt.execute(rusqlite::params![
                    message_id,
                    i64::from(item.part_index),
                    item.filename,
                    item.mime_type,
                    i64::try_from(item.size).unwrap_or(i64::MAX),
                    item.content_id,
                    i64::from(item.is_inline),
                ])?;
            }
        }
        tx.commit()?;
        Ok(attachments.len())
    }

    /// 列出一封邮件的附件，按分片下标升序。
    pub fn list_attachments(&self, message_id: i64) -> Result<Vec<StoredAttachment>, StoreError> {
        let sql = format!("{SELECT_ATTACHMENT} WHERE message_id = ?1 ORDER BY part_index ASC");
        let mut stmt = self.conn().prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![message_id], row_to_attachment)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 读一个附件。
    pub fn get_attachment(&self, attachment_id: i64) -> Result<Option<StoredAttachment>, StoreError> {
        let sql = format!("{SELECT_ATTACHMENT} WHERE id = ?1");
        let attachment = self
            .conn()
            .query_row(&sql, rusqlite::params![attachment_id], row_to_attachment)
            .optional()?;
        Ok(attachment)
    }

    /// 更新附件的下载状态（可只改状态，不改路径）。
    pub fn set_attachment_state(&self, attachment_id: i64, state: AttachmentState) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE attachment SET state = ?2 WHERE id = ?1",
            rusqlite::params![attachment_id, state.as_str()],
        )?;
        Ok(())
    }

    /// 记录附件下载结果：本地路径 + 状态。
    pub fn set_attachment_downloaded(&self, attachment_id: i64, local_path: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE attachment SET local_path = ?2, state = ?3 WHERE id = ?1",
            rusqlite::params![attachment_id, local_path, AttachmentState::Downloaded.as_str()],
        )?;
        Ok(())
    }

    /// 读取一封邮件在服务器上的定位信息（账号、文件夹、UID、正文状态）。
    pub fn message_location(&self, message_id: i64) -> Result<Option<MessageLocation>, StoreError> {
        let state: Option<String> = self
            .conn()
            .query_row(
                "SELECT body_state FROM message WHERE id = ?1",
                rusqlite::params![message_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(state) = state else {
            return Ok(None);
        };
        let row = self
            .conn()
            .query_row(
                "SELECT m.id, m.account_id, m.folder_id, m.uid, f.full_path
                 FROM message m JOIN folder f ON f.id = m.folder_id
                 WHERE m.id = ?1",
                rusqlite::params![message_id],
                |row| {
                    let uid: i64 = row.get(3)?;
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        u32::try_from(uid).unwrap_or(0),
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, account_id, folder_id, uid, folder_path)) = row else {
            return Ok(None);
        };
        Ok(Some(MessageLocation {
            message_id: id,
            account_id,
            folder_id,
            uid,
            folder_path,
            body_state: BodyState::parse(&state).unwrap_or(BodyState::None),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{AttachmentState, BodyState, NewAttachment};
    use crate::Store;

    /// 打开内存库并跑完全部迁移。
    fn store() -> Store {
        let mut store = Store::open_in_memory().expect("打开内存库");
        store.run_migrations().expect("跑迁移");
        store
    }

    /// 直接造一条账号 / 文件夹 / 邮件，返回邮件主键。
    fn seed(store: &Store) -> i64 {
        let conn = store.raw_connection_for_test();
        conn.execute(
            "INSERT INTO account (display_name, email, auth_type, username, \
             imap_host, imap_port, imap_security, smtp_host, smtp_port, smtp_security) \
             VALUES ('测试账号', 'a@example.com', 'password', 'a@example.com', \
             'imap.example.com', 993, 'tls', 'smtp.example.com', 465, 'tls')",
            [],
        )
        .expect("插入账号");
        let account_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO folder (account_id, full_path, delimiter, kind) VALUES (?1, 'INBOX', '/', 'inbox')",
            rusqlite::params![account_id],
        )
        .expect("插入文件夹");
        let folder_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO message (account_id, folder_id, uid, subject, from_addr, date_utc, size) \
             VALUES (?1, ?2, 7, '主题', 'z@example.com', '2026-10-04T00:00:00Z', 10)",
            rusqlite::params![account_id, folder_id],
        )
        .expect("插入邮件");
        conn.last_insert_rowid()
    }

    #[test]
    fn 正文能存取并按状态标记就绪() {
        let store = store();
        let message_id = seed(&store);
        assert!(store.get_message_body(message_id).expect("读正文").is_none());

        store
            .save_message_body(message_id, Some("纯文本"), Some("<p>纯文本</p>"))
            .expect("写正文");
        let body = store
            .get_message_body(message_id)
            .expect("读正文")
            .expect("应有正文");
        assert_eq!(body.text_plain.as_deref(), Some("纯文本"));
        assert_eq!(body.html_sanitized.as_deref(), Some("<p>纯文本</p>"));
        assert!(!body.fetched_at.is_empty());
        assert_eq!(
            store
                .message_location(message_id)
                .expect("定位")
                .expect("应有")
                .body_state,
            BodyState::Ready
        );
        assert_eq!(
            store
                .message_location(message_id)
                .expect("定位")
                .expect("应有")
                .folder_path,
            "INBOX"
        );
    }

    #[test]
    fn 附件清单可替换且不重复() {
        let store = store();
        let message_id = seed(&store);
        let items = vec![
            NewAttachment {
                part_index: 1,
                filename: "报告.pdf".to_string(),
                mime_type: "application/pdf".to_string(),
                size: 5,
                content_id: None,
                is_inline: false,
            },
            NewAttachment {
                part_index: 2,
                filename: "图.png".to_string(),
                mime_type: "image/png".to_string(),
                size: 5,
                content_id: Some("img-1@example.com".to_string()),
                is_inline: true,
            },
        ];
        assert_eq!(store.replace_attachments(message_id, &items).expect("写附件"), 2);
        store.replace_attachments(message_id, &items).expect("重复写附件");

        let listed = store.list_attachments(message_id).expect("列附件");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].filename, "报告.pdf");
        assert!(listed[1].is_inline);
        assert_eq!(listed[1].state, AttachmentState::Pending);

        store
            .set_attachment_downloaded(listed[1].id, "D:/tmp/图.png")
            .expect("标记下载");
        let after = store.get_attachment(listed[1].id).expect("读附件").expect("应有");
        assert_eq!(after.state, AttachmentState::Downloaded);
        assert_eq!(after.local_path.as_deref(), Some("D:/tmp/图.png"));

        store
            .set_attachment_state(listed[0].id, AttachmentState::Failed)
            .expect("标失败");
        assert_eq!(
            store
                .get_attachment(listed[0].id)
                .expect("读附件")
                .expect("应有")
                .state,
            AttachmentState::Failed
        );
    }

    #[test]
    fn 正文状态与含附件标记能更新() {
        let store = store();
        let message_id = seed(&store);
        store
            .set_message_body_state(message_id, BodyState::Failed)
            .expect("标失败");
        store
            .set_message_has_attachments(message_id, true)
            .expect("标附件");

        let flag: i64 = store
            .raw_connection_for_test()
            .query_row(
                "SELECT has_attachments FROM message WHERE id = ?1",
                rusqlite::params![message_id],
                |row| row.get(0),
            )
            .expect("读附件标记");
        assert_eq!(flag, 1);
        assert_eq!(
            store
                .message_location(message_id)
                .expect("定位")
                .expect("应有")
                .body_state,
            BodyState::Failed
        );
    }

    #[test]
    fn 邮件不存在时定位为空() {
        let store = store();
        assert!(store.message_location(999).expect("定位").is_none());
        assert!(store.get_attachment(999).expect("读附件").is_none());
    }
}
