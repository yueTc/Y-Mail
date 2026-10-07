//! 统一收件箱的只读查询（Wave 3）。
//!
//! 设计要点：
//! - 虚拟视图：直接跨账号 JOIN `message` / `folder` / `account` 查询，不复制邮件；
//! - 默认只看各账号的收件箱（`folder.kind = 'inbox'`），可用账号或具体文件夹收窄；
//! - 线程聚合按（账号, 线程键）折叠：不同账号的同名主题不合并，避免把无关邮件揉在一起；
//! - 线程键为空时用 Message-ID 兜底，再为空就用行号兜底，保证每封邮件都有键。
//!
//! 本模块以只读查询为主；唯一写入口是按主键切换邮件已读状态（`set_message_read`）。

use rusqlite::OptionalExtension;

use crate::connection::Store;
use crate::error::StoreError;

/// 统一收件箱的一行邮件（已带上账号色标等展示字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxMessage {
    /// `message` 表主键。
    pub id: i64,
    /// 所属账号。
    pub account_id: i64,
    /// 所属文件夹。
    pub folder_id: i64,
    /// 服务器 UID。
    pub uid: u32,
    /// 会话键（已含兜底值，可直接用于展开）。
    pub thread_key: String,
    /// 主题。
    pub subject: String,
    /// 发件人显示名。
    pub from_name: String,
    /// 发件人邮箱。
    pub from_addr: String,
    /// 日期（UTC ISO-8601）。
    pub date_utc: String,
    /// 邮件大小（字节）。
    pub size: u32,
    /// 是否含附件。
    pub has_attachments: bool,
    /// 是否已读。
    pub is_read: bool,
    /// 是否星标。
    pub is_flagged: bool,
    /// 摘要（Wave 3 多数为空，正文留给 Wave 4）。
    pub snippet: String,
    /// 所属账号邮箱。
    pub account_email: String,
    /// 所属账号显示名。
    pub account_display_name: String,
    /// 所属账号色标。
    pub account_color: String,
    /// 所属文件夹路径。
    pub folder_path: String,
}

/// 折叠后的一条会话线程。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxThread {
    /// 所属账号。
    pub account_id: i64,
    /// 会话键（已含兜底值）。
    pub thread_key: String,
    /// 线程里的邮件条数（收件箱范围内）。
    pub message_count: i64,
    /// 线程里的未读条数。
    pub unread_count: i64,
    /// 最新一封，作为列表展示行。
    pub latest: InboxMessage,
}

/// 一个账号的收件箱汇总。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountInboxSummary {
    /// 账号编号。
    pub account_id: i64,
    /// 邮箱地址。
    pub email: String,
    /// 显示名。
    pub display_name: String,
    /// 色标。
    pub color: String,
    /// 是否启用。
    pub enabled: bool,
    /// 收件箱邮件总数。
    pub message_count: i64,
    /// 收件箱未读数。
    pub unread_count: i64,
}

/// 收件箱查询条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxQuery {
    /// 只看某个账号；None 表示全部账号。
    pub account_id: Option<i64>,
    /// 只看某个文件夹；None 表示各账号的收件箱。
    pub folder_id: Option<i64>,
    /// 只看未读。
    pub unread_only: bool,
    /// 只看标了红旗的邮件（左侧「红旗邮件」入口用）。
    pub flagged_only: bool,
    /// 只看某一类文件夹（`inbox` / `draft` / `sent` 等）；None 表示按下面的默认范围。
    pub folder_kind: Option<String>,
    /// 跳过条数。
    pub offset: i64,
    /// 最多返回条数。
    pub limit: i64,
}

impl Default for InboxQuery {
    fn default() -> Self {
        Self {
            account_id: None,
            folder_id: None,
            unread_only: false,
            flagged_only: false,
            folder_kind: None,
            offset: 0,
            limit: 200,
        }
    }
}

/// 邮件列表用到的列（顺序与 `row_to_message` 的下标一一对应）。
pub(crate) const INBOX_COLUMNS: &str = "m.id, m.account_id, m.folder_id, m.uid, m.thread_key, \
    m.subject, m.from_name, m.from_addr, m.date_utc, m.size, \
    m.has_attachments, m.is_read, m.is_flagged, m.snippet, m.message_id_header, \
    a.email, a.display_name, a.color, f.full_path";

/// 作用范围过滤：?1 账号编号，?2 文件夹编号，?3 文件夹类型，?5 是否只看红旗。
///
/// 优先级：指定了文件夹就只看它；否则指定了类型就只看这一类（「所有草稿」「所有已发送」）；
/// 否则红旗视图（?5 = 1）看这个账号的全部文件夹，剩下的默认只看收件箱。
const SCOPE_FILTER: &str = "((?1 IS NULL OR m.account_id = ?1) \
    AND ((?2 IS NOT NULL AND m.folder_id = ?2) \
        OR (?2 IS NULL AND ((?3 IS NOT NULL AND f.kind = ?3) \
            OR (?3 IS NULL AND (?5 = 1 OR f.kind = 'inbox'))))))";

/// 线程聚合的基础查询（含有效线程键 tkey）。
const THREAD_BASE: &str = "SELECT m.id, m.account_id, m.folder_id, m.uid, m.thread_key, \
    m.subject, m.from_name, m.from_addr, m.date_utc, m.size, \
    m.has_attachments, m.is_read, m.is_flagged, m.snippet, m.message_id_header, \
    a.email, a.display_name, a.color, f.full_path, \
    COALESCE(NULLIF(m.thread_key, ''), NULLIF(m.message_id_header, ''), '#' || m.id) AS tkey \
    FROM message m \
    JOIN folder f ON f.id = m.folder_id \
    JOIN account a ON a.id = m.account_id";

/// 线程聚合的排序与计数（窗口函数，SQLite 3.25 起可用）。
const THREAD_RANKED: &str = "SELECT base.*, \
    ROW_NUMBER() OVER (PARTITION BY base.account_id, base.tkey \
        ORDER BY base.date_utc DESC, base.id DESC) AS rn, \
    COUNT(*) OVER (PARTITION BY base.account_id, base.tkey) AS message_count, \
    SUM(CASE WHEN base.is_read = 0 THEN 1 ELSE 0 END) OVER (PARTITION BY base.account_id, base.tkey) AS unread_count \
    FROM base";

/// 线程模式的行过滤：每个线程只留最新一封；未读过滤看线程整体。
const THREAD_ROW_FILTER: &str = "rn = 1 AND (?4 = 0 OR unread_count > 0)";

/// 线程键兜底规则：空串 / NULL 时用 Message-ID，再为空用 `#行号`。
///
/// 与 SQL 里的
/// `COALESCE(NULLIF(m.thread_key, ''), NULLIF(m.message_id_header, ''), '#' || m.id)`
/// 保持一致。
pub(crate) fn effective_thread_key(
    thread_key: Option<&str>,
    message_id_header: Option<&str>,
    id: i64,
) -> String {
    for value in [thread_key, message_id_header].into_iter().flatten() {
        if !value.trim().is_empty() {
            return value.to_string();
        }
    }
    format!("#{id}")
}

/// 把查询结果里的一行读成 [`InboxMessage`]。
pub(crate) fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<InboxMessage> {
    let id: i64 = row.get(0)?;
    let raw_thread_key: Option<String> = row.get(4)?;
    let message_id_header: Option<String> = row.get(14)?;
    let uid: i64 = row.get(3)?;
    let size: i64 = row.get(9)?;
    Ok(InboxMessage {
        id,
        account_id: row.get(1)?,
        folder_id: row.get(2)?,
        uid: u32::try_from(uid).unwrap_or(0),
        thread_key: effective_thread_key(raw_thread_key.as_deref(), message_id_header.as_deref(), id),
        subject: row.get(5)?,
        from_name: row.get(6)?,
        from_addr: row.get(7)?,
        date_utc: row.get(8)?,
        size: u32::try_from(size).unwrap_or(0),
        has_attachments: row.get::<_, i64>(10)? != 0,
        is_read: row.get::<_, i64>(11)? != 0,
        is_flagged: row.get::<_, i64>(12)? != 0,
        snippet: row.get(13)?,
        account_email: row.get(15)?,
        account_display_name: row.get(16)?,
        account_color: row.get(17)?,
        folder_path: row.get(18)?,
    })
}

/// 把窗口查询结果读成 [`InboxThread`]；tkey 用 SQL 算出的值，保证与展开查询一致。
fn row_to_thread(row: &rusqlite::Row<'_>) -> rusqlite::Result<InboxThread> {
    let mut latest = row_to_message(row)?;
    let account_id: i64 = row.get(1)?;
    let thread_key: String = row.get(19)?;
    latest.thread_key = thread_key.clone();
    Ok(InboxThread {
        account_id,
        thread_key,
        message_count: row.get(21)?,
        unread_count: row.get(22)?,
        latest,
    })
}

impl Store {
    /// 统一收件箱：平铺分页查询（每封邮件一行，时间倒序）。
    pub fn list_inbox_messages(&self, query: &InboxQuery) -> Result<Vec<InboxMessage>, StoreError> {
        let sql = format!(
            "SELECT {INBOX_COLUMNS} FROM message m \
             JOIN folder f ON f.id = m.folder_id \
             JOIN account a ON a.id = m.account_id \
             WHERE {SCOPE_FILTER} AND (?4 = 0 OR m.is_read = 0) \
             AND (?5 = 0 OR m.is_flagged = 1) \
             ORDER BY m.date_utc DESC, m.id DESC LIMIT ?6 OFFSET ?7"
        );
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params![
            query.account_id,
            query.folder_id,
            query.folder_kind,
            i64::from(query.unread_only),
            i64::from(query.flagged_only),
            query.limit,
            query.offset,
        ])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push(row_to_message(row)?);
        }
        Ok(items)
    }

    /// 平铺模式下的总条数（配合分页使用）。
    pub fn count_inbox_messages(&self, query: &InboxQuery) -> Result<i64, StoreError> {
        let sql = format!(
            "SELECT COUNT(*) FROM message m \
             JOIN folder f ON f.id = m.folder_id \
             WHERE {SCOPE_FILTER} AND (?4 = 0 OR m.is_read = 0) \
             AND (?5 = 0 OR m.is_flagged = 1)"
        );
        let count: i64 = self.conn().query_row(
            &sql,
            rusqlite::params![
                query.account_id,
                query.folder_id,
                query.folder_kind,
                i64::from(query.unread_only),
                i64::from(query.flagged_only)
            ],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// 会话线程聚合分页查询（每个账号的同名主题折叠成一行，取最新一封）。
    pub fn list_inbox_threads(&self, query: &InboxQuery) -> Result<Vec<InboxThread>, StoreError> {
        let sql = format!(
            "WITH base AS ({THREAD_BASE} WHERE {SCOPE_FILTER} AND (?5 = 0 OR m.is_flagged = 1)), \
             ranked AS ({THREAD_RANKED}) \
             SELECT * FROM ranked WHERE {THREAD_ROW_FILTER} \
             ORDER BY date_utc DESC, id DESC LIMIT ?6 OFFSET ?7"
        );
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params![
            query.account_id,
            query.folder_id,
            query.folder_kind,
            i64::from(query.unread_only),
            i64::from(query.flagged_only),
            query.limit,
            query.offset,
        ])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push(row_to_thread(row)?);
        }
        Ok(items)
    }

    /// 线程模式下的总行数（配合分页使用）。
    pub fn count_inbox_threads(&self, query: &InboxQuery) -> Result<i64, StoreError> {
        let sql = format!(
            "WITH base AS ({THREAD_BASE} WHERE {SCOPE_FILTER} AND (?5 = 0 OR m.is_flagged = 1)), \
             ranked AS ({THREAD_RANKED}) \
             SELECT COUNT(*) FROM ranked WHERE {THREAD_ROW_FILTER}"
        );
        let count: i64 = self.conn().query_row(
            &sql,
            rusqlite::params![
                query.account_id,
                query.folder_id,
                query.folder_kind,
                i64::from(query.unread_only),
                i64::from(query.flagged_only)
            ],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// 按主键取一封邮件（带上账号与文件夹展示字段）；不存在返回空值。
    ///
    /// 供线程摘要、翻译等 AI 编排读取主题、发件人与正文时使用；
    /// 不在本方法里触碰正文缓存，正文由读信模块单独读取。
    pub fn get_inbox_message(&self, message_id: i64) -> Result<Option<InboxMessage>, StoreError> {
        let sql = format!(
            "SELECT {INBOX_COLUMNS} FROM message m JOIN folder f ON f.id = m.folder_id JOIN account a ON a.id = m.account_id WHERE m.id = ?1"
        );
        self.conn()
            .query_row(&sql, [message_id], row_to_message)
            .optional()
            .map_err(StoreError::from)
    }

    /// 按主键切换一封邮件的已读状态，并同步维护所在文件夹的未读数。
    ///
    /// 返回值表示状态是否真的发生变化：邮件不存在或已经是目标状态时返回 `false`。
    /// 这样调用方只有在 `true` 时才需要发起后续同步，重复调用不会重复扣减。
    pub fn set_message_read(&self, message_id: i64, read: bool) -> Result<bool, StoreError> {
        let tx = self.conn().unchecked_transaction()?;
        let current: Option<(i64, i64)> = tx
            .query_row(
                "SELECT folder_id, is_read FROM message WHERE id = ?1",
                [message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((folder_id, current_read)) = current else {
            return Ok(false);
        };
        let target = i64::from(read);
        if current_read == target {
            return Ok(false);
        }

        tx.execute(
            "UPDATE message SET is_read = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![message_id, target],
        )?;

        // 未读数不能减到负数；反向标未读时补回 1。
        let delta = if read { -1 } else { 1 };
        tx.execute(
            "UPDATE folder SET unread_count = MAX(0, unread_count + ?2),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![folder_id, delta],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// 展开一条会话：取该账号该线程在收件箱范围内的全部邮件（新的在前）。
    pub fn list_thread_messages(
        &self,
        account_id: i64,
        thread_key: &str,
        limit: i64,
    ) -> Result<Vec<InboxMessage>, StoreError> {
        let sql = format!(
            "SELECT {INBOX_COLUMNS} FROM message m \
             JOIN folder f ON f.id = m.folder_id \
             JOIN account a ON a.id = m.account_id \
             WHERE m.account_id = ?1 AND f.kind = 'inbox' \
               AND COALESCE(NULLIF(m.thread_key, ''), NULLIF(m.message_id_header, ''), '#' || m.id) = ?2 \
             ORDER BY m.date_utc DESC, m.id DESC LIMIT ?3"
        );
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params![account_id, thread_key, limit])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push(row_to_message(row)?);
        }
        Ok(items)
    }

    /// 每个账号的收件箱汇总（邮件总数与未读数）。
    pub fn account_inbox_summary(&self) -> Result<Vec<AccountInboxSummary>, StoreError> {
        let sql = "SELECT a.id, a.email, a.display_name, a.color, a.enabled, \
            COUNT(m.id), COALESCE(SUM(CASE WHEN m.is_read = 0 THEN 1 ELSE 0 END), 0) \
            FROM account a \
            LEFT JOIN folder f ON f.account_id = a.id AND f.kind = 'inbox' \
            LEFT JOIN message m ON m.folder_id = f.id \
            GROUP BY a.id ORDER BY a.id";
        let mut stmt = self.conn().prepare(sql)?;
        let mut rows = stmt.query([])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push(AccountInboxSummary {
                account_id: row.get(0)?,
                email: row.get(1)?,
                display_name: row.get(2)?,
                color: row.get(3)?,
                enabled: row.get::<_, i64>(4)? != 0,
                message_count: row.get(5)?,
                unread_count: row.get(6)?,
            });
        }
        Ok(items)
    }
}

/// 一个账号下的文件夹（带本地邮件条数），供左侧文件夹树使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxFolder {
    /// 所属账号。
    pub account_id: i64,
    /// 文件夹编号。
    pub folder_id: i64,
    /// 服务器上的完整路径。
    pub full_path: String,
    /// 归类结果（inbox / sent / draft / trash / junk / custom）。
    pub kind: String,
    /// 本地邮件条数。
    pub message_count: i64,
    /// 本地未读条数。
    pub unread_count: i64,
}

impl Store {
    /// 列出全部账号的文件夹及各文件夹的本地邮件条数（收件箱排最前）。
    pub fn list_inbox_folders(&self) -> Result<Vec<InboxFolder>, StoreError> {
        let sql = "SELECT f.account_id, f.id, f.full_path, f.kind, \
            COUNT(m.id), COALESCE(SUM(CASE WHEN m.is_read = 0 THEN 1 ELSE 0 END), 0) \
            FROM folder f \
            LEFT JOIN message m ON m.folder_id = f.id \
            GROUP BY f.id \
            ORDER BY f.account_id, \
                CASE f.kind WHEN 'inbox' THEN 0 WHEN 'sent' THEN 1 WHEN 'draft' THEN 2 \
                    WHEN 'junk' THEN 3 WHEN 'trash' THEN 4 ELSE 5 END, \
                f.full_path";
        let mut stmt = self.conn().prepare(sql)?;
        let mut rows = stmt.query([])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push(InboxFolder {
                account_id: row.get(0)?,
                folder_id: row.get(1)?,
                full_path: row.get(2)?,
                kind: row.get(3)?,
                message_count: row.get(4)?,
                unread_count: row.get(5)?,
            });
        }
        Ok(items)
    }
}
#[cfg(test)]
mod tests {
    use mail_domain::FolderKind;

    use super::InboxQuery;
    use crate::sync::NewMessage;
    use crate::Store;

    fn migrated() -> Store {
        let mut store = Store::open_in_memory().expect("打开内存库");
        store.run_migrations().expect("迁移");
        store
    }

    fn account(store: &Store, email: &str, color: &str) -> i64 {
        let draft = mail_domain::AccountDraft {
            display_name: email.to_string(),
            email: email.to_string(),
            auth_type: mail_domain::AuthType::Password,
            username: email.to_string(),
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
            color: color.to_string(),
            enabled: true,
            oauth_provider: None,
            oauth_client_id: String::new(),
        };
        store.insert_account(&draft, None).expect("插入账号").0
    }

    fn inbox(store: &Store, account_id: i64) -> i64 {
        store
            .upsert_folder(account_id, "INBOX", "/", FolderKind::Inbox)
            .expect("插入收件箱")
    }

    fn message(
        account_id: i64,
        folder_id: i64,
        uid: u32,
        thread_key: &str,
        message_id_header: &str,
        date_utc: &str,
        is_read: bool,
    ) -> NewMessage {
        NewMessage {
            account_id,
            folder_id,
            uid,
            message_id_header: message_id_header.to_string(),
            thread_key: thread_key.to_string(),
            subject: format!("主题 {thread_key}"),
            from_name: "张三".to_string(),
            from_addr: "z@example.com".to_string(),
            to_json: "[]".to_string(),
            cc_json: "[]".to_string(),
            date_utc: date_utc.to_string(),
            size: 1000,
            has_attachments: false,
            is_read,
            is_flagged: false,
            is_answered: false,
            is_draft: false,
        }
    }

    #[test]
    fn 平铺查询按时间倒序且支持过滤分页() {
        let store = migrated();
        let a = account(&store, "a@example.com", "#111111");
        let b = account(&store, "b@example.com", "#222222");
        let fa = inbox(&store, a);
        let fb = inbox(&store, b);

        store
            .insert_messages(&[
                message(a, fa, 1, "t1", "<a1@x>", "2026-10-04T10:00:00Z", true),
                message(a, fa, 2, "t2", "<a2@x>", "2026-10-04T12:00:00Z", false),
                message(a, fa, 3, "t1", "<a3@x>", "2026-10-04T11:00:00Z", false),
                message(b, fb, 1, "t1", "<b1@x>", "2026-10-04T13:00:00Z", false),
            ])
            .expect("插入邮件");

        let all = store
            .list_inbox_messages(&InboxQuery::default())
            .expect("查询全部");
        assert_eq!(all.len(), 4);
        // 时间倒序：B 的 13 点最前，其次 A 的 12 点。
        assert_eq!(all[0].account_id, b);
        assert_eq!(all[0].date_utc, "2026-10-04T13:00:00Z");
        assert_eq!(all[1].date_utc, "2026-10-04T12:00:00Z");
        assert_eq!(all[3].date_utc, "2026-10-04T10:00:00Z");
        // 色标带上来了。
        assert_eq!(all[0].account_color, "#222222");

        let only_a = InboxQuery {
            account_id: Some(a),
            ..InboxQuery::default()
        };
        assert_eq!(store.count_inbox_messages(&only_a).expect("计数"), 3);
        assert_eq!(store.list_inbox_messages(&only_a).expect("查询").len(), 3);

        let unread = InboxQuery {
            account_id: Some(a),
            unread_only: true,
            ..InboxQuery::default()
        };
        assert_eq!(store.count_inbox_messages(&unread).expect("计数"), 2);
        let unread_rows = store.list_inbox_messages(&unread).expect("查询");
        assert!(unread_rows.iter().all(|m| !m.is_read));

        let page = InboxQuery {
            offset: 1,
            limit: 2,
            ..InboxQuery::default()
        };
        let rows = store.list_inbox_messages(&page).expect("分页");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].date_utc, "2026-10-04T12:00:00Z");
        assert_eq!(rows[1].date_utc, "2026-10-04T11:00:00Z");
    }

    #[test]
    fn 只看标红只返回红旗邮件并跨文件夹计入总数() {
        let store = migrated();
        let a = account(&store, "a@example.com", "#111111");
        let fa = inbox(&store, a);
        let fs = store
            .upsert_folder(a, "已发送", "/", FolderKind::Sent)
            .expect("插入已发送");

        store
            .insert_messages(&[
                message(a, fa, 1, "t1", "<a1@x>", "2026-10-04T10:00:00Z", true),
                message(a, fa, 2, "t2", "<a2@x>", "2026-10-04T12:00:00Z", false),
                message(a, fa, 3, "t3", "<a3@x>", "2026-10-04T11:00:00Z", false),
                message(a, fs, 1, "t4", "<s1@x>", "2026-10-04T09:00:00Z", true),
            ])
            .expect("插入邮件");

        // 默认范围只看收件箱：已发送那封不在里面。
        let inbox_ids: Vec<i64> = store
            .list_inbox_messages(&InboxQuery::default())
            .expect("查询")
            .iter()
            .map(|item| item.id)
            .collect();
        assert_eq!(inbox_ids.len(), 3, "默认只看收件箱");
        for id in &inbox_ids {
            assert!(store.set_message_flagged(*id, true).expect("标红"));
        }
        // 只标红其中两封，留一封没标的当反例。
        let unflagged_id = store
            .set_message_flagged(*inbox_ids.last().expect("有邮件"), false)
            .expect("取消标红");
        assert!(unflagged_id, "取消标红要真的改到");
        let sent_id = store
            .list_inbox_messages(&InboxQuery {
                account_id: Some(a),
                folder_id: Some(fs),
                ..InboxQuery::default()
            })
            .expect("查已发送")
            .first()
            .expect("已发送有一封")
            .id;
        assert!(store.set_message_flagged(sent_id, true).expect("标红"));

        let flagged = InboxQuery {
            account_id: Some(a),
            flagged_only: true,
            ..InboxQuery::default()
        };
        assert_eq!(store.count_inbox_messages(&flagged).expect("计数"), 3);
        let rows = store.list_inbox_messages(&flagged).expect("查询");
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|item| item.is_flagged));
        // 已发送里的红旗邮也要算进来：红旗视图不按文件夹收窄。
        assert!(rows.iter().any(|item| item.id == sent_id));

        // 线程模式同样按红旗收窄，而且跨文件夹。
        assert_eq!(store.count_inbox_threads(&flagged).expect("线程计数"), 3);
        assert_eq!(store.list_inbox_threads(&flagged).expect("线程查询").len(), 3);
    }

    #[test]
    fn 按文件夹类型收窄能跨账号看草稿和已发送() {
        let store = migrated();
        let a = account(&store, "a@example.com", "#111111");
        let b = account(&store, "b@example.com", "#222222");
        let da = store
            .upsert_folder(a, "草稿箱", "/", FolderKind::Draft)
            .expect("A 草稿箱");
        let db = store
            .upsert_folder(b, "草稿箱", "/", FolderKind::Draft)
            .expect("B 草稿箱");
        let sa = store
            .upsert_folder(a, "已发送", "/", FolderKind::Sent)
            .expect("A 已发送");
        let fa = inbox(&store, a);

        store
            .insert_messages(&[
                message(a, da, 1, "t1", "<d1@x>", "2026-10-04T10:00:00Z", true),
                message(b, db, 1, "t2", "<d2@x>", "2026-10-04T11:00:00Z", true),
                message(a, sa, 1, "t3", "<s1@x>", "2026-10-04T12:00:00Z", true),
                message(a, fa, 1, "t4", "<i1@x>", "2026-10-04T13:00:00Z", true),
            ])
            .expect("插入邮件");

        // 默认范围没变：不带条件还是只看收件箱。
        assert_eq!(
            store
                .count_inbox_messages(&InboxQuery::default())
                .expect("默认计数"),
            1
        );

        let drafts = InboxQuery {
            folder_kind: Some("draft".to_string()),
            ..InboxQuery::default()
        };
        assert_eq!(store.count_inbox_messages(&drafts).expect("草稿计数"), 2);
        let draft_rows = store.list_inbox_messages(&drafts).expect("草稿查询");
        assert_eq!(draft_rows.len(), 2);
        assert!(draft_rows.iter().all(|item| item.folder_path.contains("草稿箱")));

        let sent = InboxQuery {
            folder_kind: Some("sent".to_string()),
            ..InboxQuery::default()
        };
        assert_eq!(store.count_inbox_messages(&sent).expect("已发送计数"), 1);
        assert_eq!(store.list_inbox_messages(&sent).expect("已发送查询").len(), 1);

        // 线程模式也认这个类型范围。
        assert_eq!(store.count_inbox_threads(&drafts).expect("草稿线程计数"), 2);
        assert_eq!(store.list_inbox_threads(&drafts).expect("草稿线程").len(), 2);
    }

    #[test]
    fn 按主键取一封邮件会带上账号展示字段() {
        let store = migrated();
        let a = account(&store, "a@example.com", "#123456");
        let fa = inbox(&store, a);
        store
            .insert_messages(&[message(a, fa, 7, "t1", "<a7@x>", "2026-10-04T10:00:00Z", false)])
            .expect("插入邮件");
        let id = store
            .list_inbox_messages(&InboxQuery::default())
            .expect("查询")
            .first()
            .expect("存在")
            .id;

        let found = store.get_inbox_message(id).expect("按主键查").expect("存在");
        assert_eq!(found.id, id);
        assert_eq!(found.account_id, a);
        assert_eq!(found.account_email, "a@example.com");
        assert_eq!(found.account_color, "#123456");
        assert_eq!(found.thread_key, "t1");
        assert!(store.get_inbox_message(999_999).expect("查不存在").is_none());
    }

    #[test]
    fn 会话聚合折叠同账号线程且不跨账号合并() {
        let store = migrated();
        let a = account(&store, "a@example.com", "#111111");
        let b = account(&store, "b@example.com", "#222222");
        let fa = inbox(&store, a);
        let fb = inbox(&store, b);

        store
            .insert_messages(&[
                message(a, fa, 1, "t1", "<a1@x>", "2026-10-04T10:00:00Z", true),
                message(a, fa, 2, "t2", "<a2@x>", "2026-10-04T12:00:00Z", false),
                message(a, fa, 3, "t1", "<a3@x>", "2026-10-04T11:00:00Z", false),
                message(b, fb, 1, "t1", "<b1@x>", "2026-10-04T13:00:00Z", false),
            ])
            .expect("插入邮件");

        let threads = store.list_inbox_threads(&InboxQuery::default()).expect("聚合");
        assert_eq!(threads.len(), 3, "两个账号各成线程，不合并同名主题");
        assert_eq!(
            store.count_inbox_threads(&InboxQuery::default()).expect("计数"),
            3
        );

        let a_t1 = threads
            .iter()
            .find(|t| t.account_id == a && t.thread_key == "t1")
            .expect("A 的 t1");
        assert_eq!(a_t1.message_count, 2);
        assert_eq!(a_t1.unread_count, 1);
        assert_eq!(a_t1.latest.date_utc, "2026-10-04T11:00:00Z");
        assert_eq!(a_t1.latest.uid, 3);

        // 排序按最新日期：B 的 13 点 > A 的 t2（12 点）> A 的 t1（11 点）。
        assert_eq!(threads[0].account_id, b);
        assert_eq!(threads[0].latest.date_utc, "2026-10-04T13:00:00Z");
        assert_eq!(threads[1].thread_key, "t2");
        assert_eq!(threads[2].thread_key, "t1");
    }

    #[test]
    fn 展开线程取该账号全部邮件且时间倒序() {
        let store = migrated();
        let a = account(&store, "a@example.com", "#111111");
        let fa = inbox(&store, a);
        store
            .insert_messages(&[
                message(a, fa, 1, "t1", "<a1@x>", "2026-10-04T10:00:00Z", true),
                message(a, fa, 3, "t1", "<a3@x>", "2026-10-04T11:00:00Z", false),
                message(a, fa, 2, "t2", "<a2@x>", "2026-10-04T12:00:00Z", false),
            ])
            .expect("插入邮件");
        let rows = store.list_thread_messages(a, "t1", 50).expect("展开");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].uid, 3);
        assert_eq!(rows[1].uid, 1);
        assert!(rows.iter().all(|m| m.thread_key == "t1"));
    }

    #[test]
    fn 账号未读汇总等于各账号之和() {
        let store = migrated();
        let a = account(&store, "a@example.com", "#111111");
        let b = account(&store, "b@example.com", "#222222");
        let fa = inbox(&store, a);
        let fb = inbox(&store, b);
        store
            .insert_messages(&[
                message(a, fa, 1, "t1", "<a1@x>", "2026-10-04T10:00:00Z", true),
                message(a, fa, 2, "t2", "<a2@x>", "2026-10-04T12:00:00Z", false),
                message(b, fb, 1, "t1", "<b1@x>", "2026-10-04T13:00:00Z", false),
            ])
            .expect("插入邮件");
        let summary = store.account_inbox_summary().expect("汇总");
        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0].account_id, a);
        assert_eq!(summary[0].message_count, 2);
        assert_eq!(summary[0].unread_count, 1);
        assert_eq!(summary[1].message_count, 1);
        assert_eq!(summary[1].unread_count, 1);
        let total_unread: i64 = summary.iter().map(|s| s.unread_count).sum();
        assert_eq!(total_unread, 2);
        assert_eq!(summary[0].color, "#111111");
    }

    #[test]
    fn 线程键兜底先用message_id再用行号() {
        // 纯函数部分。
        assert_eq!(super::effective_thread_key(Some("t"), Some("h"), 9), "t");
        assert_eq!(super::effective_thread_key(Some(""), Some("h"), 9), "h");
        assert_eq!(super::effective_thread_key(None, Some("h"), 9), "h");
        assert_eq!(super::effective_thread_key(Some(""), Some(""), 9), "#9");
        assert_eq!(super::effective_thread_key(None, None, 9), "#9");

        // SQL 路径部分。
        let store = migrated();
        let a = account(&store, "a@example.com", "#111111");
        let fa = inbox(&store, a);
        store
            .insert_messages(&[
                message(a, fa, 1, "", "<only@x>", "2026-10-04T10:00:00Z", false),
                message(a, fa, 2, "", "", "2026-10-04T11:00:00Z", false),
            ])
            .expect("插入邮件");
        let rows = store.list_inbox_messages(&InboxQuery::default()).expect("查询");
        let row1 = rows.iter().find(|m| m.uid == 1).expect("uid1");
        assert_eq!(row1.thread_key, "<only@x>");
        let row2 = rows.iter().find(|m| m.uid == 2).expect("uid2");
        assert!(
            row2.thread_key.starts_with('#'),
            "应为行号兜底：{}",
            row2.thread_key
        );
        assert_eq!(row2.thread_key, format!("#{}", row2.id));
    }

    #[test]
    fn 标已读后邮件与文件夹未读数同步变化() {
        let store = migrated();
        let a = account(&store, "a@example.com", "#111111");
        let fa = inbox(&store, a);
        store.update_folder_select(fa, None, None, 3).expect("设置未读数");
        store
            .insert_messages(&[message(a, fa, 1, "t1", "<a1@x>", "2026-10-04T10:00:00Z", false)])
            .expect("插入邮件");
        let id = store.list_inbox_messages(&InboxQuery::default()).expect("查询")[0].id;

        assert!(store.set_message_read(id, true).expect("标已读"));

        let m = store.get_inbox_message(id).expect("查询邮件").expect("邮件存在");
        assert!(m.is_read, "邮件应标记为已读");
        let folder = store
            .get_folder_by_id(fa)
            .expect("查询文件夹")
            .expect("文件夹存在");
        assert_eq!(folder.unread_count, 2, "未读数应减一");
        let summary = store.account_inbox_summary().expect("汇总");
        assert_eq!(summary[0].unread_count, 0, "账号未读数按邮件实时算出");
    }

    #[test]
    fn 重复标已读不会重复扣减未读数() {
        let store = migrated();
        let a = account(&store, "a@example.com", "#111111");
        let fa = inbox(&store, a);
        store.update_folder_select(fa, None, None, 2).expect("设置未读数");
        store
            .insert_messages(&[message(a, fa, 1, "t1", "<a1@x>", "2026-10-04T10:00:00Z", false)])
            .expect("插入邮件");
        let id = store.list_inbox_messages(&InboxQuery::default()).expect("查询")[0].id;

        assert!(store.set_message_read(id, true).expect("首次标已读"));
        assert!(!store.set_message_read(id, true).expect("重复标已读"));
        let folder = store
            .get_folder_by_id(fa)
            .expect("查询文件夹")
            .expect("文件夹存在");
        assert_eq!(folder.unread_count, 1, "只应扣一次");
    }

    #[test]
    fn 反向标未读会把未读数加回来() {
        let store = migrated();
        let a = account(&store, "a@example.com", "#111111");
        let fa = inbox(&store, a);
        store
            .insert_messages(&[message(a, fa, 1, "t1", "<a1@x>", "2026-10-04T10:00:00Z", true)])
            .expect("插入邮件");
        let id = store.list_inbox_messages(&InboxQuery::default()).expect("查询")[0].id;

        assert!(store.set_message_read(id, false).expect("标未读"));

        let m = store.get_inbox_message(id).expect("查询邮件").expect("邮件存在");
        assert!(!m.is_read, "邮件应标记为未读");
        let folder = store
            .get_folder_by_id(fa)
            .expect("查询文件夹")
            .expect("文件夹存在");
        assert_eq!(folder.unread_count, 1, "未读数应加一");
        let summary = store.account_inbox_summary().expect("汇总");
        assert_eq!(summary[0].unread_count, 1);
    }

    #[test]
    fn 未读数不会被减到负数() {
        let store = migrated();
        let a = account(&store, "a@example.com", "#111111");
        let fa = inbox(&store, a);
        store
            .insert_messages(&[message(a, fa, 1, "t1", "<a1@x>", "2026-10-04T10:00:00Z", false)])
            .expect("插入邮件");
        let id = store.list_inbox_messages(&InboxQuery::default()).expect("查询")[0].id;

        assert!(store.set_message_read(id, true).expect("标已读"));
        let folder = store
            .get_folder_by_id(fa)
            .expect("查询文件夹")
            .expect("文件夹存在");
        assert_eq!(folder.unread_count, 0, "未读数不应为负");
    }
}
