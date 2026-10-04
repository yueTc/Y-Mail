//! 全文搜索（Wave 5）。
//!
//! 设计要点：
//! - 关键词走 FTS5（trigram 分词，中文按子串命中）；1–2 个字符的短词走 `LIKE` 兜底，
//!   因为 trigram 至少需要 3 个字符才能建索引；
//! - `from:` 折进 FTS 的发件人列；`has:attachment` / `is:unread` / `before:` 走普通 SQL 过滤；
//! - 搜索跨全部文件夹；结果复用收件箱的行结构，前端不用再学一套字段；
//! - 高亮片段用 FTS 的 `snippet()` 生成，控制字符 \x01 / \x02 只在本模块内出现，
//!   解析成结构化片段后交给前端当普通文本渲染（不用 HTML 注入）。
//!
//! 安全：关键词只作为 SQL 参数或 FTS 查询串片段，绝不拼接进原始 SQL 文本。

use rusqlite::ToSql;

use crate::connection::Store;
use crate::error::StoreError;
use crate::inbox::{row_to_message, InboxMessage, INBOX_COLUMNS};

/// 高亮片段的控制字符：FTS 的 snippet() 用它标记命中范围。
const HL_START: char = '\u{1}';
const HL_END: char = '\u{2}';

/// 一段可展示的检索片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetSegment {
    /// 片段文本。
    pub text: String,
    /// 是否是命中的关键词。
    pub highlighted: bool,
}

/// 一条搜索结果：邮件 + 高亮片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// 命中的邮件（字段与收件箱一致）。
    pub message: InboxMessage,
    /// 高亮片段；空表示没有可用片段。
    pub snippet: Vec<SnippetSegment>,
}

/// 一页搜索结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPage {
    /// 本页结果。
    pub items: Vec<SearchHit>,
    /// 符合条件的总条数。
    pub total: i64,
    /// 本页跳过的条数。
    pub offset: i64,
    /// 本页最大条数。
    pub limit: i64,
}

/// 搜索条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// 原始查询串，例如 `发票 from:alice has:attachment`。
    pub raw: String,
    /// 只看某个账号；None 表示全部账号。
    pub account_id: Option<i64>,
    /// 跳过条数。
    pub offset: i64,
    /// 最多返回条数。
    pub limit: i64,
}

impl SearchQuery {
    /// 组装一条查询；limit 会被压到 1..=500。
    pub fn new(raw: impl Into<String>, account_id: Option<i64>, offset: i64, limit: i64) -> Self {
        Self {
            raw: raw.into(),
            account_id,
            offset: offset.max(0),
            limit: limit.clamp(1, 500),
        }
    }
}

/// 解析后的查询条件（纯数据，便于单测）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParsedQuery {
    /// 关键词（不含任何前缀）。
    pub keywords: Vec<String>,
    /// `from:` 里的发件人关键词。
    pub from: Vec<String>,
    /// `has:attachment`。
    pub has_attachment: bool,
    /// `is:unread`。
    pub unread_only: bool,
    /// `before:YYYY-MM-DD`。
    pub before: Option<String>,
}

impl ParsedQuery {
    /// 是否一条有效查询（至少有一个关键词、发件人或过滤条件）。
    pub(crate) fn is_effective(&self) -> bool {
        !self.keywords.is_empty()
            || !self.from.is_empty()
            || self.has_attachment
            || self.unread_only
            || self.before.is_some()
    }
}

/// 解析查询串；无法识别的前缀按普通关键词处理（用户不会因此搜不到东西）。
pub(crate) fn parse_query(raw: &str) -> ParsedQuery {
    let mut parsed = ParsedQuery::default();
    for token in raw.split_whitespace() {
        if let Some(value) = token.strip_prefix("from:") {
            let value = value.trim_matches('"');
            if !value.is_empty() {
                parsed.from.push(value.to_string());
            }
            continue;
        }
        if let Some(value) = token.strip_prefix("has:") {
            if value.eq_ignore_ascii_case("attachment") {
                parsed.has_attachment = true;
                continue;
            }
        }
        if let Some(value) = token.strip_prefix("is:") {
            if value.eq_ignore_ascii_case("unread") {
                parsed.unread_only = true;
                continue;
            }
        }
        if let Some(value) = token.strip_prefix("before:") {
            if is_iso_date(value) {
                parsed.before = Some(value.to_string());
                continue;
            }
        }
        if !token.is_empty() {
            parsed.keywords.push(token.to_string());
        }
    }
    parsed
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

/// 字符数（不是字节数）：trigram 看的是字符。
fn char_count(value: &str) -> usize {
    value.chars().count()
}

/// 把关键词包成 FTS5 短语；引号加倍转义，保证不会被当成查询语法。
fn fts_phrase(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// 由解析结果拼出 SQL 过滤片段与参数。
struct FilterBuilder {
    sql: Vec<String>,
    params: Vec<Box<dyn ToSql>>,
}

impl FilterBuilder {
    fn push_param<T: ToSql + 'static>(&mut self, value: T) -> usize {
        self.params.push(Box::new(value));
        self.params.len()
    }
}

/// 查询只在一处拼装，两个分支（FTS / LIKE）共用同样的过滤条件。
fn build_filter(parsed: &ParsedQuery, use_fts: bool) -> FilterBuilder {
    let mut builder = FilterBuilder {
        sql: Vec::new(),
        params: Vec::new(),
    };

    if use_fts {
        let mut terms: Vec<String> = parsed.keywords.iter().map(|item| fts_phrase(item)).collect();
        for value in &parsed.from {
            terms.push(format!("{{from_name from_addr}} : {}", fts_phrase(value)));
        }
        let index = builder.push_param(terms.join(" "));
        builder.sql.push(format!("message_fts MATCH ?{index}"));
    } else {
        for keyword in &parsed.keywords {
            let index = builder.push_param(format!("%{keyword}%"));
            builder.sql.push(format!(
                "(m.subject LIKE ?{index} OR m.from_name LIKE ?{index} \
                 OR m.from_addr LIKE ?{index} \
                 OR EXISTS (SELECT 1 FROM message_body b \
                            WHERE b.message_id = m.id AND b.text_plain LIKE ?{index}))"
            ));
        }
        for value in &parsed.from {
            let index = builder.push_param(format!("%{value}%"));
            builder.sql.push(format!(
                "(m.from_name LIKE ?{index} OR m.from_addr LIKE ?{index})"
            ));
        }
    }

    if parsed.has_attachment {
        builder.sql.push("m.has_attachments = 1".to_string());
    }
    if parsed.unread_only {
        builder.sql.push("m.is_read = 0".to_string());
    }
    if let Some(before) = &parsed.before {
        let index = builder.push_param(format!("{before}T00:00:00Z"));
        builder.sql.push(format!("m.date_utc < ?{index}"));
    }
    builder
}

/// 把 `snippet()` 的带标记文本拆成结构化片段。
pub(crate) fn split_snippet(raw: &str) -> Vec<SnippetSegment> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut highlighted = false;
    for ch in raw.chars() {
        match ch {
            HL_START => {
                if !current.is_empty() {
                    segments.push(SnippetSegment {
                        text: std::mem::take(&mut current),
                        highlighted,
                    });
                }
                highlighted = true;
            }
            HL_END => {
                if !current.is_empty() {
                    segments.push(SnippetSegment {
                        text: std::mem::take(&mut current),
                        highlighted,
                    });
                }
                highlighted = false;
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        segments.push(SnippetSegment {
            text: current,
            highlighted,
        });
    }
    segments
}

impl Store {
    /// 全文搜索一页邮件，按时间倒序。
    pub fn search_messages(&self, query: &SearchQuery) -> Result<SearchPage, StoreError> {
        let parsed = parse_query(&query.raw);
        if !parsed.is_effective() {
            return Ok(SearchPage {
                items: Vec::new(),
                total: 0,
                offset: query.offset,
                limit: query.limit,
            });
        }

        let short_keyword = parsed.keywords.iter().any(|item| char_count(item) < 3);
        let short_from = parsed.from.iter().any(|item| char_count(item) < 3);
        let has_text_terms = !parsed.keywords.is_empty() || !parsed.from.is_empty();
        let use_fts = has_text_terms && !short_keyword && !short_from;

        let mut filter = build_filter(&parsed, use_fts);
        let mut where_parts = std::mem::take(&mut filter.sql);
        if let Some(account_id) = query.account_id {
            let index = filter.push_param(account_id);
            where_parts.push(format!("m.account_id = ?{index}"));
        }
        let where_sql = where_parts.join(" AND ");

        // 计数与取数共用同一份过滤参数；计数参数放前面，取数再加两个分页参数。
        let count_sql = format!(
            "SELECT COUNT(*) FROM message m \
             JOIN folder f ON f.id = m.folder_id \
             JOIN account a ON a.id = m.account_id \
             {} \
             WHERE {where_sql}",
            fts_join(use_fts)
        );
        let count_params: Vec<&dyn ToSql> = filter.params.iter().map(|item| item.as_ref()).collect();
        let total: i64 = self
            .conn()
            .query_row(&count_sql, count_params.as_slice(), |row| row.get(0))?;

        let snippet_expr = snippet_expression(&parsed, use_fts, &mut filter);
        let mut list_params = filter.params;
        let limit_index = list_params.len() + 1;
        let offset_index = list_params.len() + 2;
        list_params.push(Box::new(query.limit));
        list_params.push(Box::new(query.offset));

        let list_sql = format!(
            "SELECT {INBOX_COLUMNS}, {snippet_expr} \
             FROM message m \
             JOIN folder f ON f.id = m.folder_id \
             JOIN account a ON a.id = m.account_id \
             {} \
             WHERE {where_sql} \
             ORDER BY m.date_utc DESC, m.id DESC \
             LIMIT ?{limit_index} OFFSET ?{offset_index}",
            fts_join(use_fts)
        );

        let mut stmt = self.conn().prepare(&list_sql)?;
        let list_refs: Vec<&dyn ToSql> = list_params.iter().map(|item| item.as_ref()).collect();
        let mut rows = stmt.query(list_refs.as_slice())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            let message = row_to_message(row)?;
            let raw_snippet: String = row.get(INBOX_COLUMN_COUNT)?;
            let snippet = if use_fts {
                split_snippet(&raw_snippet)
            } else {
                fallback_snippet(&message, &parsed, &raw_snippet)
            };
            items.push(SearchHit { message, snippet });
        }

        Ok(SearchPage {
            items,
            total,
            offset: query.offset,
            limit: query.limit,
        })
    }
}

/// 收件箱行结构的列数（`INBOX_COLUMNS` 的列数）。
const INBOX_COLUMN_COUNT: usize = 19;

fn fts_join(use_fts: bool) -> &'static str {
    if use_fts {
        "JOIN message_fts ON message_fts.rowid = m.id"
    } else {
        ""
    }
}

/// snippet 表达式；LIKE 分支要额外注册两个参数（匹配模式 + 原词用于定位窗口）。
fn snippet_expression(parsed: &ParsedQuery, use_fts: bool, filter: &mut FilterBuilder) -> String {
    if use_fts {
        return "snippet(message_fts, -1, char(1), char(2), '…', 16)".to_string();
    }
    let Some(keyword) = parsed.keywords.first() else {
        return "''".to_string();
    };
    let like_index = filter.push_param(format!("%{keyword}%"));
    let keyword_index = filter.push_param(keyword.clone());
    format!(
        "CASE WHEN EXISTS (SELECT 1 FROM message_body b \
                 WHERE b.message_id = m.id AND b.text_plain LIKE ?{like_index}) \
              THEN (SELECT substr(b.text_plain, \
                     max(1, instr(lower(b.text_plain), lower(?{keyword_index})) - 40), \
                     200) \
                    FROM message_body b \
                    WHERE b.message_id = m.id \
                      AND b.text_plain LIKE ?{like_index} LIMIT 1) \
              ELSE '' END"
    )
}

/// LIKE 分支的片段：优先正文窗口，其次主题。
fn fallback_snippet(message: &InboxMessage, parsed: &ParsedQuery, body_window: &str) -> Vec<SnippetSegment> {
    let source = if body_window.trim().is_empty() {
        message.subject.clone()
    } else {
        body_window.to_string()
    };
    if source.is_empty() {
        return Vec::new();
    }
    let lower = source.to_lowercase();
    // 大小写折叠可能改变字节长度，映射不回原串时只做无高亮展示，绝不越界切片。
    if lower.len() != source.len() {
        return vec![SnippetSegment {
            text: source,
            highlighted: false,
        }];
    }
    let mut hits: Vec<(usize, usize)> = Vec::new();
    for keyword in parsed.keywords.iter().chain(parsed.from.iter()) {
        let keyword_lower = keyword.to_lowercase();
        if keyword_lower.is_empty() {
            continue;
        }
        for (begin, _) in lower.match_indices(&keyword_lower) {
            let end = begin + keyword_lower.len();
            if end <= source.len() && source.is_char_boundary(begin) && source.is_char_boundary(end) {
                hits.push((begin, end));
            }
        }
    }
    if hits.is_empty() {
        return vec![SnippetSegment {
            text: source,
            highlighted: false,
        }];
    }
    hits.sort_unstable();
    let mut segments = Vec::new();
    let mut cursor = 0usize;
    for (begin, end) in hits {
        if begin < cursor {
            continue;
        }
        if begin > cursor {
            segments.push(SnippetSegment {
                text: source[cursor..begin].to_string(),
                highlighted: false,
            });
        }
        segments.push(SnippetSegment {
            text: source[begin..end].to_string(),
            highlighted: true,
        });
        cursor = end;
    }
    if cursor < source.len() {
        segments.push(SnippetSegment {
            text: source[cursor..].to_string(),
            highlighted: false,
        });
    }
    segments
}

#[cfg(test)]
mod tests {
    use mail_domain::FolderKind;

    use super::{parse_query, SearchQuery};
    use crate::sync::NewMessage;
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
        };
        store.insert_account(&draft, None).expect("插入账号").0
    }

    fn inbox(store: &Store, account_id: i64) -> i64 {
        store
            .upsert_folder(account_id, "INBOX", "/", FolderKind::Inbox)
            .expect("插入收件箱")
    }

    #[allow(clippy::too_many_arguments)]
    fn message(
        account_id: i64,
        folder_id: i64,
        uid: u32,
        subject: &str,
        from_name: &str,
        from_addr: &str,
        date_utc: &str,
        is_read: bool,
        has_attachments: bool,
    ) -> NewMessage {
        NewMessage {
            account_id,
            folder_id,
            uid,
            message_id_header: format!("<{uid}@example.com>"),
            thread_key: format!("t{uid}"),
            subject: subject.to_string(),
            from_name: from_name.to_string(),
            from_addr: from_addr.to_string(),
            to_json: "[]".to_string(),
            cc_json: "[]".to_string(),
            date_utc: date_utc.to_string(),
            size: 1000,
            has_attachments,
            is_read,
            is_flagged: false,
            is_answered: false,
            is_draft: false,
        }
    }

    /// 用原始连接捡回一封邮件的主键，供正文写入测试使用。
    fn message_id(store: &Store, uid: u32) -> i64 {
        store
            .raw_connection_for_test()
            .query_row(
                "SELECT id FROM message WHERE uid = ?1",
                rusqlite::params![i64::from(uid)],
                |row| row.get(0),
            )
            .expect("查邮件主键")
    }

    #[test]
    fn 解析查询串识别四种语法() {
        let parsed = parse_query("发票 from:alice has:attachment is:unread before:2026-10-01");
        assert_eq!(parsed.keywords, vec!["发票"]);
        assert_eq!(parsed.from, vec!["alice"]);
        assert!(parsed.has_attachment);
        assert!(parsed.unread_only);
        assert_eq!(parsed.before.as_deref(), Some("2026-10-01"));

        // 认不出来的前缀按普通关键词处理，用户不会因此搜不到东西。
        let fallback = parse_query("has:cc before:bad from:");
        assert_eq!(fallback.keywords, vec!["has:cc", "before:bad"]);
        assert!(fallback.from.is_empty(), "空的 from: 直接忽略");
    }

    #[test]
    fn 长关键词走全文索引并高亮片段() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let f = inbox(&store, a);
        store
            .insert_messages(&[
                message(
                    a,
                    f,
                    1,
                    "项目发票审核通知",
                    "张三",
                    "zhangsan@example.com",
                    "2026-10-04T10:00:00Z",
                    false,
                    false,
                ),
                message(
                    a,
                    f,
                    2,
                    "会议纪要",
                    "李四",
                    "lisi@example.com",
                    "2026-10-04T11:00:00Z",
                    false,
                    false,
                ),
            ])
            .expect("插入邮件");

        let page = store
            .search_messages(&SearchQuery::new("发票审核", None, 0, 50))
            .expect("搜索");
        assert_eq!(page.total, 1);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].message.subject, "项目发票审核通知");
        let highlighted: String = page.items[0]
            .snippet
            .iter()
            .filter(|seg| seg.highlighted)
            .map(|seg| seg.text.clone())
            .collect();
        assert!(
            highlighted.contains("发票审核"),
            "命中片段应带高亮，实际={highlighted:?}"
        );
    }

    #[test]
    fn 短词走兜底且能搜中文正文() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let f = inbox(&store, a);
        store
            .insert_messages(&[message(
                a,
                f,
                1,
                "普通主题",
                "张三",
                "zhangsan@example.com",
                "2026-10-04T10:00:00Z",
                false,
                false,
            )])
            .expect("插入邮件");
        let id = message_id(&store, 1);
        store
            .save_message_body(id, Some("这是一封关于发票的正文"), None)
            .expect("写正文");

        let page = store
            .search_messages(&SearchQuery::new("发票", None, 0, 50))
            .expect("搜索");
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].message.id, id);
    }

    #[test]
    fn 组合过滤按账号与条件收窄() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let b = account(&store, "b@example.com");
        let fa = inbox(&store, a);
        let fb = inbox(&store, b);
        store
            .insert_messages(&[
                message(
                    a,
                    fa,
                    1,
                    "季度报告",
                    "Alice",
                    "alice@example.com",
                    "2026-09-20T10:00:00Z",
                    false,
                    true,
                ),
                message(
                    a,
                    fa,
                    2,
                    "季度报告 草稿",
                    "Alice",
                    "alice@example.com",
                    "2026-10-02T10:00:00Z",
                    true,
                    false,
                ),
                message(
                    b,
                    fb,
                    1,
                    "季度报告 远程",
                    "Alice",
                    "alice@example.com",
                    "2026-09-01T10:00:00Z",
                    false,
                    true,
                ),
            ])
            .expect("插入邮件");

        // from: + has:attachment + before: 三者叠加。
        let page = store
            .search_messages(&SearchQuery::new(
                "季度报告 from:alice has:attachment before:2026-10-01",
                None,
                0,
                50,
            ))
            .expect("搜索");

        assert_eq!(page.total, 2, "两封附件邮件在截止日前");
        assert!(page.items.iter().all(|hit| hit.message.has_attachments));

        // 限定账号再叠加未读。
        let scoped = store
            .search_messages(&SearchQuery::new("is:unread", Some(a), 0, 50))
            .expect("搜索");
        assert_eq!(scoped.total, 1, "A 账号只有第一封未读");

        let unread_a = store
            .search_messages(&SearchQuery::new("季度报告 is:unread", Some(a), 0, 50))
            .expect("搜索");
        assert_eq!(unread_a.total, 1);
        assert_eq!(unread_a.items[0].message.uid, 1, "未读的是第一封");
    }

    #[test]
    fn 正文写入后能被全文检索命中() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let f = inbox(&store, a);
        store
            .insert_messages(&[message(
                a,
                f,
                1,
                "无关键词主题",
                "张三",
                "zhangsan@example.com",
                "2026-10-04T10:00:00Z",
                false,
                false,
            )])
            .expect("插入邮件");
        let id = message_id(&store, 1);
        let before = store
            .search_messages(&SearchQuery::new("季度预算", None, 0, 50))
            .expect("先搜");
        assert_eq!(before.total, 0);

        store
            .save_message_body(id, Some("附件里写着季度预算的分配方案"), None)
            .expect("写正文");

        let after = store
            .search_messages(&SearchQuery::new("季度预算", None, 0, 50))
            .expect("后搜");
        assert_eq!(after.total, 1);
        assert_eq!(after.items[0].message.id, id);
    }

    #[test]
    fn 一万封邮件搜索耗时低于一秒() {
        let store = migrated();
        let a = account(&store, "a@example.com");
        let f = inbox(&store, a);
        let batch: Vec<NewMessage> = (0..10_000u32)
            .map(|index| {
                message(
                    a,
                    f,
                    index + 1,
                    &format!("批量邮件 {index}"),
                    "系统",
                    "system@example.com",
                    "2026-10-04T10:00:00Z",
                    false,
                    false,
                )
            })
            .collect();
        store.insert_messages(&batch).expect("插入一万封");

        let started = std::time::Instant::now();
        let page = store
            .search_messages(&SearchQuery::new("批量邮件 9999", None, 0, 50))
            .expect("搜索");
        let elapsed = started.elapsed();
        assert!(page.total >= 1, "应至少命中一封");
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "一万封搜索应低于一秒，实际 {elapsed:?}"
        );
    }
}
