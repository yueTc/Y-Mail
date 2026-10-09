//! 写信相关的本地数据：发件队列、地址簿与签名（Wave 5）。
//!
//! 状态机（outbox.state）：
//!   draft   草稿，用户还在改
//!   queued  待发送
//!   sending 已认领，正在发送
//!   sent    已成功投递
//!   failed  永久失败或重试用尽
//!
//! 关键约束：认领待发邮件用一条带条件的 UPDATE（`state='queued'` → `'sending'`），
//! 影响行数为 1 才算拿到，从根上防重复发送；进程重启时残留的 sending 一律标失败，
//! 让用户人工确认是否重发，避免「其实已发出」被自动重发。

use rusqlite::OptionalExtension;

use crate::connection::Store;
use crate::error::StoreError;

/// 发件队列里一封邮件的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxState {
    /// 草稿。
    Draft,
    /// 待发送。
    Queued,
    /// 正在发送（已被某个发送任务认领）。
    Sending,
    /// 已成功投递。
    Sent,
    /// 发送失败（永久失败或重试次数用尽）。
    Failed,
}

impl OutboxState {
    /// 入库用的稳定小写标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Queued => "queued",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Failed => "failed",
        }
    }

    /// 从库里的文本解析。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "queued" => Some(Self::Queued),
            "sending" => Some(Self::Sending),
            "sent" => Some(Self::Sent),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// 写信类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxKind {
    /// 新邮件。
    New,
    /// 回复。
    Reply,
    /// 转发。
    Forward,
}

impl OutboxKind {
    /// 入库用的稳定小写标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Reply => "reply",
            Self::Forward => "forward",
        }
    }

    /// 从库里的文本解析；认不出来按新邮件处理。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "new" => Some(Self::New),
            "reply" => Some(Self::Reply),
            "forward" => Some(Self::Forward),
            _ => None,
        }
    }
}

/// 新建草稿时提交的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOutbox {
    /// 所属账号。
    pub account_id: i64,
    /// 写信类型。
    pub kind: OutboxKind,
    /// 收件人 JSON 数组（`[{"name":"","address":""}]`）。
    pub to_json: String,
    /// 抄送 JSON 数组。
    pub cc_json: String,
    /// 密送 JSON 数组。
    pub bcc_json: String,
    /// 主题。
    pub subject: String,
    /// HTML 正文。
    pub body_html: String,
    /// 纯文本正文。
    pub body_text: String,
    /// 回复的原始 Message-ID（可为空）。
    pub in_reply_to: Option<String>,
    /// References 头的 Message-ID 列表 JSON。
    pub references_json: String,
    /// 附件清单 JSON（本地路径 + 文件名）。
    pub attachments_json: String,
}

/// 库里读回的一封待发 / 已发邮件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredOutbox {
    /// 主键。
    pub id: i64,
    /// 所属账号。
    pub account_id: i64,
    /// 写信类型。
    pub kind: OutboxKind,
    /// 收件人 JSON。
    pub to_json: String,
    /// 抄送 JSON。
    pub cc_json: String,
    /// 密送 JSON。
    pub bcc_json: String,
    /// 主题。
    pub subject: String,
    /// HTML 正文。
    pub body_html: String,
    /// 纯文本正文。
    pub body_text: String,
    /// 回复的 Message-ID。
    pub in_reply_to: Option<String>,
    /// References JSON。
    pub references_json: String,
    /// 附件 JSON。
    pub attachments_json: String,
    /// 状态。
    pub state: OutboxState,
    /// 已尝试发送次数。
    pub attempts: i64,
    /// 最近一次失败原因（已脱敏）。
    pub last_error: Option<String>,
    /// 创建时间。
    pub created_at: String,
    /// 更新时间。
    pub updated_at: String,
    /// 发送成功时间。
    pub sent_at: Option<String>,
}

/// 一个账号的签名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSignature {
    /// 所属账号。
    pub account_id: i64,
    /// 签名 HTML。
    pub html: String,
    /// 是否启用。
    pub enabled: bool,
    /// 更新时间。
    pub updated_at: String,
}

/// 写回信 / 转发时要从原邮件取出的一组字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeSource {
    /// 原邮件主键。
    pub message_id: i64,
    /// 原邮件所属账号。
    pub account_id: i64,
    /// 原主题。
    pub subject: String,
    /// 原发件人显示名。
    pub from_name: String,
    /// 原发件人邮箱。
    pub from_addr: String,
    /// 原收件人 JSON 数组。
    pub to_json: String,
    /// 原抄送 JSON 数组。
    pub cc_json: String,
    /// 原 Message-ID 头（含尖括号）。
    pub message_id_header: Option<String>,
    /// 原邮件日期（UTC ISO-8601）。
    pub date_utc: String,
}
const OUTBOX_COLUMNS: &str = "id, account_id, kind, to_json, cc_json, bcc_json, subject, \
    body_html, body_text, in_reply_to, references_json, attachments_json, state, attempts, \
    last_error, created_at, updated_at, sent_at";

fn row_to_outbox(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredOutbox> {
    let state_text: String = row.get(12)?;
    Ok(StoredOutbox {
        id: row.get(0)?,
        account_id: row.get(1)?,
        kind: row
            .get::<_, String>(2)
            .ok()
            .and_then(|value| OutboxKind::parse(&value))
            .unwrap_or(OutboxKind::New),
        to_json: row.get(3)?,
        cc_json: row.get(4)?,
        bcc_json: row.get(5)?,
        subject: row.get(6)?,
        body_html: row.get(7)?,
        body_text: row.get(8)?,
        in_reply_to: row.get(9)?,
        references_json: row.get(10)?,
        attachments_json: row.get(11)?,
        state: OutboxState::parse(&state_text).unwrap_or(OutboxState::Draft),
        attempts: row.get(13)?,
        last_error: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
        sent_at: row.get(17)?,
    })
}

impl Store {
    /// 新建一封草稿或待发邮件，返回主键。
    pub fn create_outbox(&self, draft: &NewOutbox, state: OutboxState) -> Result<i64, StoreError> {
        self.conn().execute(
            "INSERT INTO outbox
                 (account_id, kind, to_json, cc_json, bcc_json, subject, body_html, body_text,
                  in_reply_to, references_json, attachments_json, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                draft.account_id,
                draft.kind.as_str(),
                draft.to_json,
                draft.cc_json,
                draft.bcc_json,
                draft.subject,
                draft.body_html,
                draft.body_text,
                draft.in_reply_to,
                draft.references_json,
                draft.attachments_json,
                state.as_str(),
            ],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// 读一封发件记录。
    pub fn get_outbox(&self, id: i64) -> Result<Option<StoredOutbox>, StoreError> {
        let sql = format!("SELECT {OUTBOX_COLUMNS} FROM outbox WHERE id = ?1");
        Ok(self
            .conn()
            .query_row(&sql, rusqlite::params![id], row_to_outbox)
            .optional()?)
    }

    /// 取一封原邮件用于回信 / 转发的预填字段。
    pub fn get_compose_source(&self, message_id: i64) -> Result<Option<ComposeSource>, StoreError> {
        self.conn()
            .query_row(
                "SELECT m.id, m.account_id, m.subject, m.from_name, m.from_addr, \
                        m.to_json, m.cc_json, m.message_id_header, m.date_utc \
                 FROM message m WHERE m.id = ?1",
                rusqlite::params![message_id],
                |row| {
                    Ok(ComposeSource {
                        message_id: row.get(0)?,
                        account_id: row.get(1)?,
                        subject: row.get(2)?,
                        from_name: row.get(3)?,
                        from_addr: row.get(4)?,
                        to_json: row.get(5)?,
                        cc_json: row.get(6)?,
                        message_id_header: row.get(7)?,
                        date_utc: row.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }
    /// 列出一个账号的发件记录（时间倒序）；账号为空时列全部。
    pub fn list_outbox(
        &self,
        account_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<StoredOutbox>, StoreError> {
        let sql = format!(
            "SELECT {OUTBOX_COLUMNS} FROM outbox \
             WHERE (?1 IS NULL OR account_id = ?1) \
             ORDER BY updated_at DESC, id DESC LIMIT ?2"
        );
        let mut stmt = self.conn().prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![account_id, limit as i64], row_to_outbox)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 草稿保存：覆盖内容；只允许改 draft / failed 的记录，已发送的不再改。
    pub fn update_outbox_draft(&self, id: i64, draft: &NewOutbox) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "UPDATE outbox SET kind = ?2, to_json = ?3, cc_json = ?4, bcc_json = ?5,
                 subject = ?6, body_html = ?7, body_text = ?8, in_reply_to = ?9,
                 references_json = ?10, attachments_json = ?11,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND state IN ('draft', 'failed')",
            rusqlite::params![
                id,
                draft.kind.as_str(),
                draft.to_json,
                draft.cc_json,
                draft.bcc_json,
                draft.subject,
                draft.body_html,
                draft.body_text,
                draft.in_reply_to,
                draft.references_json,
                draft.attachments_json,
            ],
        )?;
        Ok(changed == 1)
    }

    /// 入队：草稿 / 失败件变成待发送。
    pub fn enqueue_outbox(&self, id: i64) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "UPDATE outbox SET state = 'queued', attempts = 0, last_error = NULL,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND state IN ('draft', 'failed')",
            rusqlite::params![id],
        )?;
        Ok(changed == 1)
    }

    /// 原子认领一封待发邮件：只有把 queued 改成 sending 成功的调用方才能发送。
    pub fn claim_outbox(&self, id: i64) -> Result<Option<StoredOutbox>, StoreError> {
        let changed = self.conn().execute(
            "UPDATE outbox SET state = 'sending', attempts = attempts + 1,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND state = 'queued'",
            rusqlite::params![id],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.get_outbox(id)
    }

    /// 认领队列里最早的一封待发邮件（发送工作线程用）。
    pub fn claim_next_outbox(&self) -> Result<Option<StoredOutbox>, StoreError> {
        let id: Option<i64> = self
            .conn()
            .query_row(
                "SELECT id FROM outbox WHERE state = 'queued' \
                 ORDER BY updated_at ASC, id ASC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.claim_outbox(id),
            None => Ok(None),
        }
    }

    /// 标记发送成功。
    pub fn mark_outbox_sent(&self, id: i64) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE outbox SET state = 'sent', last_error = NULL,
                 sent_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(())
    }

    /// 标记发送失败；`retryable` 且次数没用完时退回待发送队列。
    pub fn mark_outbox_failed(
        &self,
        id: i64,
        error: &str,
        retryable: bool,
    ) -> Result<OutboxState, StoreError> {
        let attempts: i64 = self
            .conn()
            .query_row(
                "SELECT attempts FROM outbox WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        // 临时错误最多尝试 3 次（1 次首发 + 2 次重试），与设计口径一致。
        let state = if retryable && attempts < 3 {
            OutboxState::Queued
        } else {
            OutboxState::Failed
        };
        self.conn().execute(
            "UPDATE outbox SET state = ?2, last_error = ?3,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![id, state.as_str(), error],
        )?;
        Ok(state)
    }

    /// 用户点重试：失败件回到队列，尝试次数清零。
    pub fn retry_outbox(&self, id: i64) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "UPDATE outbox SET state = 'queued', attempts = 0, last_error = NULL,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND state = 'failed'",
            rusqlite::params![id],
        )?;
        Ok(changed == 1)
    }

    /// 删除一封草稿 / 失败件（已发送的不再删，避免历史丢档）。
    pub fn delete_outbox(&self, id: i64) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "DELETE FROM outbox WHERE id = ?1 AND state IN ('draft', 'failed')",
            rusqlite::params![id],
        )?;
        Ok(changed == 1)
    }

    /// 启动时收尾：把上次进程留下的 sending 全部标失败，等用户人工确认再重发。
    pub fn recover_stuck_sending(&self) -> Result<usize, StoreError> {
        let changed = self.conn().execute(
            "UPDATE outbox SET state = 'failed',
                 last_error = '上次发送中断，状态未知，请人工确认后再重试',
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE state = 'sending'",
            [],
        )?;
        Ok(changed)
    }

    /// 等待发送的任务条数（queued + sending），供界面显示。
    pub fn count_pending_outbox(&self) -> Result<i64, StoreError> {
        let count: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM outbox WHERE state IN ('queued', 'sending')",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// 读一个账号的签名；没有则返回默认的「未启用空签名」。
    pub fn get_signature(&self, account_id: i64) -> Result<StoredSignature, StoreError> {
        let row = self
            .conn()
            .query_row(
                "SELECT account_id, html, enabled, updated_at FROM signature WHERE account_id = ?1",
                rusqlite::params![account_id],
                |row| {
                    Ok(StoredSignature {
                        account_id: row.get(0)?,
                        html: row.get(1)?,
                        enabled: row.get::<_, i64>(2)? != 0,
                        updated_at: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(row.unwrap_or(StoredSignature {
            account_id,
            html: String::new(),
            // 没有记录时按启用处理：空签名启用等于什么都没加。
            enabled: true,
            updated_at: String::new(),
        }))
    }

    /// 列出所有已存在的签名行（设置同步导出用；无签名的账号不会出现）。
    pub fn list_signatures(&self) -> Result<Vec<StoredSignature>, StoreError> {
        let mut stmt = self
            .conn()
            .prepare("SELECT account_id, html, enabled, updated_at FROM signature ORDER BY account_id ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok(StoredSignature {
                account_id: row.get(0)?,
                html: row.get(1)?,
                enabled: row.get::<_, i64>(2)? != 0,
                updated_at: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
    /// 写入或更新一个账号的签名。
    pub fn save_signature(&self, account_id: i64, html: &str, enabled: bool) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO signature (account_id, html, enabled, updated_at) \
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')) \
             ON CONFLICT(account_id) DO UPDATE SET \
                 html = excluded.html, enabled = excluded.enabled, updated_at = excluded.updated_at",
            rusqlite::params![account_id, html, i64::from(enabled)],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{NewOutbox, OutboxKind, OutboxState};
    use crate::Store;

    fn migrated() -> Store {
        let mut store = Store::open_in_memory().expect("打开内存库");
        store.run_migrations().expect("迁移");
        store
    }

    fn account(store: &Store, email: &str) -> i64 {
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
            color: "#123456".to_string(),
            enabled: true,
            oauth_provider: None,
            oauth_client_id: String::new(),
        };
        store.insert_account(&draft, None).expect("插入账号").0
    }

    fn draft(account_id: i64, subject: &str) -> NewOutbox {
        NewOutbox {
            account_id,
            kind: OutboxKind::New,
            to_json: "[{\"name\":\"张三\",\"address\":\"z@example.com\"}]".to_string(),
            cc_json: "[]".to_string(),
            bcc_json: "[]".to_string(),
            subject: subject.to_string(),
            body_html: "<p>你好</p>".to_string(),
            body_text: "你好".to_string(),
            in_reply_to: None,
            references_json: "[]".to_string(),
            attachments_json: "[]".to_string(),
        }
    }

    #[test]
    fn 草稿可存改删入队() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let id = store
            .create_outbox(&draft(a, "初稿"), OutboxState::Draft)
            .expect("建草稿");
        let stored = store.get_outbox(id).expect("读").expect("存在");
        assert_eq!(stored.state, OutboxState::Draft);
        assert_eq!(stored.kind, OutboxKind::New);
        assert_eq!(stored.subject, "初稿");

        let mut edited = draft(a, "改过");
        edited.body_text = "改过的正文".to_string();
        assert!(store.update_outbox_draft(id, &edited).expect("更新"));
        assert_eq!(store.get_outbox(id).expect("读").expect("存在").subject, "改过");

        assert!(store.enqueue_outbox(id).expect("入队"));
        assert_eq!(
            store.get_outbox(id).expect("读").expect("存在").state,
            OutboxState::Queued
        );
        // 已入队的不再当草稿改。
        assert!(!store
            .update_outbox_draft(id, &draft(a, "再改"))
            .expect("拒绝更新"));
        // 已入队的不允许删除。
        assert!(!store.delete_outbox(id).expect("拒绝删除"));
        assert_eq!(store.count_pending_outbox().expect("待发计数"), 1);
    }

    #[test]
    fn 原子认领同一封信只有一次成功() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let id = store
            .create_outbox(&draft(a, "待发"), OutboxState::Queued)
            .expect("建待发");

        let first = store.claim_outbox(id).expect("第一次认领");
        assert!(first.is_some(), "第一次应拿到");
        let second = store.claim_outbox(id).expect("第二次认领");
        assert!(second.is_none(), "已认领的不应再被抢走");
        assert_eq!(first.unwrap().attempts, 1, "认领一次记一次尝试");
    }

    #[test]
    fn 临时失败重试两次后用尽() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let id = store
            .create_outbox(&draft(a, "会失败的"), OutboxState::Queued)
            .expect("建待发");

        // 第 1 次：认领（attempts=1）→ 临时失败 → 退回队列。
        assert!(store.claim_outbox(id).expect("认领1").is_some());
        assert_eq!(
            store.mark_outbox_failed(id, "连接超时", true).expect("失败1"),
            OutboxState::Queued
        );
        // 第 2 次：认领（attempts=2）→ 临时失败 → 退回队列。
        assert!(store.claim_outbox(id).expect("认领2").is_some());
        assert_eq!(
            store.mark_outbox_failed(id, "连接超时", true).expect("失败2"),
            OutboxState::Queued
        );
        // 第 3 次：认领（attempts=3）→ 临时失败但次数用尽 → 永久失败。
        assert!(store.claim_outbox(id).expect("认领3").is_some());
        assert_eq!(
            store.mark_outbox_failed(id, "连接超时", true).expect("失败3"),
            OutboxState::Failed
        );
        assert_eq!(store.get_outbox(id).expect("读").expect("存在").attempts, 3);
        // 永久错误立刻失败，不重试。
        let id2 = store
            .create_outbox(&draft(a, "地址错"), OutboxState::Queued)
            .expect("建待发2");
        assert!(store.claim_outbox(id2).expect("认领").is_some());
        assert_eq!(
            store
                .mark_outbox_failed(id2, "收件人被拒绝", false)
                .expect("永久失败"),
            OutboxState::Failed
        );
    }

    #[test]
    fn 用户重试失败件回队列并清零尝试() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let id = store
            .create_outbox(&draft(a, "重试"), OutboxState::Queued)
            .expect("建待发");
        assert!(store.claim_outbox(id).expect("认领").is_some());
        store.mark_outbox_failed(id, "拒绝", false).expect("永久失败");
        assert!(store.retry_outbox(id).expect("重试"));
        let stored = store.get_outbox(id).expect("读").expect("存在");
        assert_eq!(stored.state, OutboxState::Queued);
        assert_eq!(stored.attempts, 0);
        assert!(stored.last_error.is_none());
    }

    #[test]
    fn 发送成功后落sent与时间() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let id = store
            .create_outbox(&draft(a, "成功"), OutboxState::Queued)
            .expect("建待发");
        assert!(store.claim_outbox(id).expect("认领").is_some());
        store.mark_outbox_sent(id).expect("标记成功");
        let stored = store.get_outbox(id).expect("读").expect("存在");
        assert_eq!(stored.state, OutboxState::Sent);
        assert!(stored.sent_at.is_some());
        assert!(stored.last_error.is_none());
        assert_eq!(store.count_pending_outbox().expect("待发计数"), 0);
    }

    #[test]
    fn 重启收尾把残留发送中标失败() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let id = store
            .create_outbox(&draft(a, "中断"), OutboxState::Queued)
            .expect("建待发");
        assert!(store.claim_outbox(id).expect("认领").is_some());
        assert_eq!(store.recover_stuck_sending().expect("收尾"), 1);
        let stored = store.get_outbox(id).expect("读").expect("存在");
        assert_eq!(stored.state, OutboxState::Failed);
        assert!(stored.last_error.is_some());
        // 再收尾一次不应误伤。
        assert_eq!(store.recover_stuck_sending().expect("再收尾"), 0);
    }

    #[test]
    fn 认领下一封按时间最早() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let first = store
            .create_outbox(&draft(a, "第一封"), OutboxState::Queued)
            .expect("建1");
        let second = store
            .create_outbox(&draft(a, "第二封"), OutboxState::Queued)
            .expect("建2");
        let claimed = store.claim_next_outbox().expect("认领").expect("有货");
        assert_eq!(claimed.id, first, "先到先发");
        let next = store.claim_next_outbox().expect("认领").expect("有货");
        assert_eq!(next.id, second);
        assert!(store.claim_next_outbox().expect("认领").is_none());
    }

    #[test]
    fn 签名默认启用且可开关() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let default = store.get_signature(a).expect("读默认");
        assert!(default.html.is_empty());
        assert!(default.enabled, "没有记录时默认启用空签名");

        store.save_signature(a, "<p>此致</p>", true).expect("存签名");
        let saved = store.get_signature(a).expect("读签名");
        assert_eq!(saved.html, "<p>此致</p>");
        assert!(saved.enabled);

        store.save_signature(a, "<p>此致</p>", false).expect("关签名");
        let off = store.get_signature(a).expect("读签名");
        assert!(!off.enabled);
        // 更新不新增第二条。
        assert_eq!(
            store
                .raw_connection_for_test()
                .query_row(
                    "SELECT COUNT(*) FROM signature WHERE account_id = ?1",
                    rusqlite::params![a],
                    |row| row.get::<_, i64>(0),
                )
                .expect("计数"),
            1
        );
    }
}
