//! 通讯录：联系人、分组的读写。
//!
//! 口径（见 `docs/superpowers/specs/2026-10-06-contacts-workspace-design.md` v1.2）：
//! - 一个邮箱一条记录，比较时不分大小写；不同账号共用同一本通讯录；
//! - 删除 = 置 `hidden`，进「已隐藏」，可恢复；彻底删除只有界面在「已隐藏」里才提供；
//! - `source` 记来源：`auto` 是同步收发件人时顺手登记的，`manual` 是用户建或改过的。
//!   同步写入绝不覆盖 `manual` 行的名字，也绝不把 `hidden` 行放出来；
//! - 备注只存本地，纯文本，不参与任何外发。

use rusqlite::{params, OptionalExtension};

use crate::{Store, StoreError};

/// 一次最多读多少条联系人（硬上限，调用方再夹一层）。
pub const MAX_CONTACT_ROWS: usize = 5000;

/// 备注最长多少个字符。
pub const MAX_NOTE_CHARS: usize = 2000;

/// 分组名最长多少个字符。
pub const MAX_GROUP_NAME_CHARS: usize = 20;

/// 联系人是怎么进通讯录的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactSource {
    /// 同步收发件人时顺手登记进来的。
    Auto,
    /// 用户自己建的，或者用户改过所以接管下来的。
    Manual,
}

impl ContactSource {
    /// 存库用的字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            ContactSource::Auto => "auto",
            ContactSource::Manual => "manual",
        }
    }

    /// 从库里的字符串还原；认不出来一律当自动收集。
    pub fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("manual") {
            ContactSource::Manual
        } else {
            ContactSource::Auto
        }
    }
}

/// 要读哪一批联系人。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactScope {
    /// 正常列表。
    Active,
    /// 「已隐藏」列表。
    Hidden,
}

impl ContactScope {
    /// 对应库里 `hidden` 的取值。
    fn hidden_flag(self) -> i64 {
        match self {
            ContactScope::Active => 0,
            ContactScope::Hidden => 1,
        }
    }
}

/// 通讯录里的一个人。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredContact {
    /// 主键。
    pub id: i64,
    /// 显示名，空串表示没填。
    pub name: String,
    /// 邮箱地址。
    pub email: String,
    /// 本地备注，纯文本。
    pub note: String,
    /// 所属分组；None 表示未分组。
    pub group_id: Option<i64>,
    /// 分组名（连表取出来的，没分组就是 None）。
    pub group_name: Option<String>,
    /// 来源。
    pub source: ContactSource,
    /// 是否已隐藏。
    pub hidden: bool,
    /// 最近一次收发信时间。
    pub last_used_at: Option<String>,
    /// 建库时间。
    pub created_at: String,
    /// 最近一次修改时间。
    pub updated_at: String,
}

/// 一个分组；成员数只数没被隐藏的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredContactGroup {
    /// 主键。
    pub id: i64,
    /// 分组名。
    pub name: String,
    /// 排序号。
    pub sort_order: i64,
    /// 组内没被隐藏的成员数。
    pub member_count: i64,
}

/// 新建 / 修改联系人时提交的字段。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactDraft {
    /// 显示名，可空。
    pub name: String,
    /// 邮箱地址。
    pub email: String,
    /// 备注。
    pub note: String,
    /// 所属分组。
    pub group_id: Option<i64>,
}

/// 批量导入时的一行。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactImportRow {
    /// 显示名，可空。
    pub name: String,
    /// 邮箱地址。
    pub email: String,
    /// 备注。
    pub note: String,
    /// 分组名；None 或空串表示未分组。
    pub group: Option<String>,
}

/// 批量导入的结果统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContactImportStats {
    /// 新建了几条。
    pub imported: usize,
    /// 因为邮箱已存在而跳过几条。
    pub skipped: usize,
    /// 覆盖更新了几条。
    pub overwritten: usize,
    /// 顺手建了几个新分组。
    pub groups_created: usize,
}

/// 把一行查出来的结果拼成联系人。
fn row_to_contact(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredContact> {
    let source_text: String = row.get(6)?;
    Ok(StoredContact {
        id: row.get(0)?,
        name: row.get(1)?,
        email: row.get(2)?,
        note: row.get(3)?,
        group_id: row.get(4)?,
        group_name: row.get(5)?,
        source: ContactSource::parse(&source_text),
        hidden: row.get::<_, i64>(7)? != 0,
        last_used_at: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

impl Store {
    /// 按关键字列联系人；`scope` 决定看正常列表还是「已隐藏」列表。
    ///
    /// 关键字为空表示不过滤；非空按名字或邮箱做包含匹配（沿用 SQLite `LIKE` 的口径）。
    /// 排序：最近联系时间倒序，其次编号升序（界面还会按拼音重排一次）。
    pub fn list_contacts(
        &self,
        keyword: &str,
        limit: usize,
        scope: ContactScope,
    ) -> Result<Vec<StoredContact>, StoreError> {
        let keyword = keyword.trim();
        let pattern = format!("%{keyword}%");
        let sql = "SELECT c.id, c.name, c.email, c.note, c.group_id, g.name, c.source, c.hidden, \
                       c.last_used_at, c.created_at, c.updated_at \
                 FROM contact c LEFT JOIN contact_group g ON g.id = c.group_id \
                 WHERE c.hidden = ?1 AND (?2 = '' OR c.name LIKE ?3 OR c.email LIKE ?3) \
                 ORDER BY COALESCE(c.last_used_at, '') DESC, c.id ASC \
                 LIMIT ?4";
        let mut stmt = self.conn().prepare(sql)?;
        let rows = stmt.query_map(
            params![
                scope.hidden_flag(),
                keyword,
                pattern,
                limit.min(MAX_CONTACT_ROWS) as i64
            ],
            row_to_contact,
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 读一位联系人；不存在返回 None。
    pub fn get_contact(&self, id: i64) -> Result<Option<StoredContact>, StoreError> {
        let sql = "SELECT c.id, c.name, c.email, c.note, c.group_id, g.name, c.source, c.hidden, \
                       c.last_used_at, c.created_at, c.updated_at \
                 FROM contact c LEFT JOIN contact_group g ON g.id = c.group_id \
                 WHERE c.id = ?1";
        Ok(self
            .conn()
            .query_row(sql, params![id], row_to_contact)
            .optional()?)
    }

    /// 数一数某个范围内的联系人有几个。
    pub fn count_contacts(&self, scope: ContactScope) -> Result<i64, StoreError> {
        let count: i64 = self.conn().query_row(
            "SELECT count(*) FROM contact WHERE hidden = ?1",
            params![scope.hidden_flag()],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// 数一数没有归属任何分组的正常联系人。
    pub fn count_ungrouped_contacts(&self) -> Result<i64, StoreError> {
        let count: i64 = self.conn().query_row(
            "SELECT count(*) FROM contact WHERE hidden = 0 AND group_id IS NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// 新建一位联系人（来源记为手动）。邮箱撞车返回 `ContactEmailTaken`。
    pub fn create_contact(&self, draft: &ContactDraft) -> Result<i64, StoreError> {
        let email = draft.email.trim();
        if self.contact_id_by_email(email)?.is_some() {
            return Err(StoreError::ContactEmailTaken {
                email: email.to_string(),
            });
        }
        self.conn().execute(
            "INSERT INTO contact (email, name, note, group_id, source, hidden, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 'manual', 0, \
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![email, draft.name.trim(), draft.note.trim(), draft.group_id],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// 修改一位联系人；改完来源记为手动（这条记录归用户管了）。
    pub fn update_contact(&self, id: i64, draft: &ContactDraft) -> Result<bool, StoreError> {
        let email = draft.email.trim();
        if let Some(other) = self.contact_id_by_email(email)? {
            if other != id {
                return Err(StoreError::ContactEmailTaken {
                    email: email.to_string(),
                });
            }
        }
        let changed = self.conn().execute(
            "UPDATE contact SET email = ?2, name = ?3, note = ?4, group_id = ?5, source = 'manual', \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ?1",
            params![id, email, draft.name.trim(), draft.note.trim(), draft.group_id],
        )?;
        Ok(changed > 0)
    }

    /// 隐藏一位联系人（软删）。返回是否真的改到了行。
    pub fn hide_contact(&self, id: i64) -> Result<bool, StoreError> {
        self.set_contact_hidden(id, 1)
    }

    /// 把一位已隐藏的联系人放回正常列表。
    pub fn restore_contact(&self, id: i64) -> Result<bool, StoreError> {
        self.set_contact_hidden(id, 0)
    }

    fn set_contact_hidden(&self, id: i64, hidden: i64) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "UPDATE contact SET hidden = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ?1",
            params![id, hidden],
        )?;
        Ok(changed > 0)
    }

    /// 彻底删掉一位联系人。
    ///
    /// 自动收集的行删掉之后，下次同步拿到同一地址还会重新建一条；界面要提示这一点。
    pub fn purge_contact(&self, id: i64) -> Result<bool, StoreError> {
        let changed = self
            .conn()
            .execute("DELETE FROM contact WHERE id = ?1", params![id])?;
        Ok(changed > 0)
    }

    /// 清掉所有「同步自动收集、且没被藏起来」的联系人；手动加的一条都不动。
    pub fn clear_auto_contacts(&self) -> Result<usize, StoreError> {
        let changed = self
            .conn()
            .execute("DELETE FROM contact WHERE source = 'auto' AND hidden = 0", [])?;
        Ok(changed)
    }

    /// 列出全部分组，附上没被隐藏的成员数。
    pub fn list_contact_groups(&self) -> Result<Vec<StoredContactGroup>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT g.id, g.name, g.sort_order, \
                    (SELECT count(*) FROM contact c WHERE c.group_id = g.id AND c.hidden = 0) \
             FROM contact_group g ORDER BY g.sort_order ASC, g.name ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StoredContactGroup {
                id: row.get(0)?,
                name: row.get(1)?,
                sort_order: row.get(2)?,
                member_count: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 新建分组；重名返回 `ContactGroupNameTaken`。
    pub fn create_contact_group(&self, name: &str) -> Result<i64, StoreError> {
        let name = name.trim();
        if self.contact_group_id_by_name(name)?.is_some() {
            return Err(StoreError::ContactGroupNameTaken {
                name: name.to_string(),
            });
        }
        self.conn()
            .execute("INSERT INTO contact_group (name) VALUES (?1)", params![name])?;
        Ok(self.conn().last_insert_rowid())
    }

    /// 给分组改名；重名返回 `ContactGroupNameTaken`。
    pub fn rename_contact_group(&self, id: i64, name: &str) -> Result<bool, StoreError> {
        let name = name.trim();
        if let Some(other) = self.contact_group_id_by_name(name)? {
            if other != id {
                return Err(StoreError::ContactGroupNameTaken {
                    name: name.to_string(),
                });
            }
        }
        let changed = self.conn().execute(
            "UPDATE contact_group SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(changed > 0)
    }

    /// 删分组：组内联系人回到「未分组」，联系人本身一条都不删。
    pub fn delete_contact_group(&self, id: i64) -> Result<bool, StoreError> {
        let tx = self.conn().unchecked_transaction()?;
        tx.execute(
            "UPDATE contact SET group_id = NULL WHERE group_id = ?1",
            params![id],
        )?;
        let changed = tx.execute("DELETE FROM contact_group WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(changed > 0)
    }

    /// 同步收发件人时登记一位联系人（来源记为自动）。
    ///
    /// 两条铁律：不覆盖手动记录的名字；不把已经藏起来的记录重新放出来。
    pub fn upsert_contact(&self, name: &str, email: &str) -> Result<(), StoreError> {
        let email = email.trim();
        if email.is_empty() {
            return Ok(());
        }
        match self.contact_id_by_email(email)? {
            Some(id) => {
                self.conn().execute(
                    "UPDATE contact SET \
                         name = CASE WHEN ?2 = '' THEN name \
                                     WHEN source = 'manual' THEN name \
                                     WHEN hidden = 1 THEN name ELSE ?2 END, \
                         last_used_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), \
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
                     WHERE id = ?1",
                    params![id, name.trim()],
                )?;
            }
            None => {
                self.conn().execute(
                    "INSERT INTO contact (email, name, source, hidden, last_used_at, created_at, updated_at) \
                     VALUES (?1, ?2, 'auto', 0, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), \
                             strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                    params![email, name.trim()],
                )?;
            }
        }
        Ok(())
    }

    /// 批量登记；跳过已隐藏的行由 `upsert_contact` 自己保证。
    pub fn upsert_contacts(&self, people: &[(String, String)]) -> Result<(), StoreError> {
        for (name, email) in people {
            self.upsert_contact(name, email)?;
        }
        Ok(())
    }
    /// 批量导入联系人：整批放在一个事务里，要么全成要么全回滚。
    ///
    /// 邮箱撞上已有记录时，`overwrite = false` 就跳过，`true` 就用文件里的名字与备注覆盖。
    /// 导入进来的都是手动来源（用户主动导的，同步不许再改它们）。
    pub fn import_contacts(
        &self,
        rows: &[ContactImportRow],
        overwrite: bool,
    ) -> Result<ContactImportStats, StoreError> {
        let mut stats = ContactImportStats::default();
        let tx = self.conn().unchecked_transaction()?;
        for row in rows {
            let email = row.email.trim();
            if email.is_empty() {
                continue;
            }
            let group_id = match row.group.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                Some(name) => {
                    let existing: Option<i64> = tx
                        .query_row(
                            "SELECT id FROM contact_group WHERE lower(name) = lower(?1)",
                            params![name],
                            |row| row.get(0),
                        )
                        .optional()?;
                    match existing {
                        Some(id) => Some(id),
                        None => {
                            tx.execute("INSERT INTO contact_group (name) VALUES (?1)", params![name])?;
                            stats.groups_created += 1;
                            Some(tx.last_insert_rowid())
                        }
                    }
                }
                None => None,
            };

            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM contact WHERE lower(email) = lower(?1)",
                    params![email],
                    |row| row.get(0),
                )
                .optional()?;

            match existing {
                Some(_) if !overwrite => stats.skipped += 1,
                Some(id) => {
                    tx.execute(
                        "UPDATE contact SET name = ?2, note = ?3, group_id = ?4, source = 'manual', \
                             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
                         WHERE id = ?1",
                        params![id, row.name.trim(), row.note.trim(), group_id],
                    )?;
                    stats.overwritten += 1;
                }
                None => {
                    tx.execute(
                        "INSERT INTO contact (email, name, note, group_id, source, hidden, \
                             created_at, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, 'manual', 0, \
                                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), \
                                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                        params![email, row.name.trim(), row.note.trim(), group_id],
                    )?;
                    stats.imported += 1;
                }
            }
        }
        tx.commit()?;
        Ok(stats)
    }

    /// 按邮箱找主键（不分大小写）。
    fn contact_id_by_email(&self, email: &str) -> Result<Option<i64>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id FROM contact WHERE lower(email) = lower(?1)",
                params![email],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// 按名字找分组主键（不分大小写）。
    fn contact_group_id_by_name(&self, name: &str) -> Result<Option<i64>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id FROM contact_group WHERE lower(name) = lower(?1)",
                params![name],
                |row| row.get(0),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::{ContactDraft, ContactScope, ContactSource, StoredContact};
    use crate::Store;

    /// 一个跑完全部迁移的内存库。
    fn migrated() -> Store {
        let mut store = Store::open_in_memory().expect("打开内存库");
        store.run_migrations().expect("迁移");
        store
    }

    /// 造一个联系人草稿。
    fn draft(name: &str, email: &str) -> ContactDraft {
        ContactDraft {
            name: name.to_string(),
            email: email.to_string(),
            note: String::new(),
            group_id: None,
        }
    }

    /// 找一条联系人。
    fn find(store: &Store, email: &str) -> StoredContact {
        store
            .list_contacts(email, 10, ContactScope::Active)
            .expect("列联系人")
            .into_iter()
            .find(|item| item.email.eq_ignore_ascii_case(email))
            .expect("找得到这位联系人")
    }

    #[test]
    fn 迁移十一把同一邮箱合并成一条并留备份() {
        // 手工搭一个 v10 的库：跑到 0010 为止，再塞进老结构的重复邮箱。
        let conn = rusqlite::Connection::open_in_memory().expect("开内存库");
        // 老表上挂着 account_id 外键，这里只验合并逻辑，不陪造账号数据。
        conn.execute_batch("PRAGMA foreign_keys = OFF;").expect("关外键");
        for migration in crate::migrations::MIGRATIONS
            .iter()
            .filter(|item| item.version <= 10)
        {
            conn.execute_batch(migration.sql).expect("跑老迁移");
        }
        conn.execute_batch(
            "INSERT INTO contact (account_id, name, email, last_used_at) VALUES \
                 (1, '', 'a@example.com', '2026-01-01T00:00:00.000Z'), \
                 (2, '阿里', 'A@example.com', '2026-02-01T00:00:00.000Z'), \
                 (2, 'Bob', 'b@example.com', NULL);",
        )
        .expect("塞老数据");
        let before: i64 = conn
            .query_row("SELECT count(*) FROM contact", [], |row| row.get(0))
            .expect("数行");
        assert_eq!(before, 3);

        let sql = crate::migrations::MIGRATIONS
            .iter()
            .find(|item| item.version == 11)
            .expect("0011 已登记")
            .sql;
        conn.execute_batch(sql).expect("跑 0011");

        let after: i64 = conn
            .query_row("SELECT count(*) FROM contact", [], |row| row.get(0))
            .expect("数行");
        assert_eq!(after, 2, "同一个邮箱合并成一条");
        let backup: i64 = conn
            .query_row("SELECT count(*) FROM contact_backup_0011", [], |row| row.get(0))
            .expect("数备份");
        assert_eq!(backup, 3, "老数据原样留档");

        let (name, note, source, hidden): (String, String, String, i64) = conn
            .query_row(
                "SELECT name, note, source, hidden FROM contact WHERE lower(email) = 'a@example.com'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("读合并后的行");
        assert_eq!(name, "阿里", "有名字的那条胜出");
        assert_eq!(note, "");
        assert_eq!(source, "auto");
        assert_eq!(hidden, 0);

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(contact)")
            .expect("读表结构")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("列名")
            .map(|item| item.expect("取值"))
            .collect();
        assert!(!columns.iter().any(|item| item == "account_id"), "老列已去掉");
        for column in ["note", "group_id", "source", "hidden", "updated_at"] {
            assert!(columns.iter().any(|item| item == column), "缺少 {column}");
        }
    }

    #[test]
    fn 新建联系人后来源是手动且邮箱不许撞车() {
        let store = migrated();
        let id = store
            .create_contact(&draft("鲍勃", "Bob@example.com"))
            .expect("建联系人");
        assert!(id > 0);

        let saved = find(&store, "bob@example.com");
        assert_eq!(saved.name, "鲍勃");
        assert_eq!(saved.source, ContactSource::Manual);
        assert!(!saved.hidden);

        let again = store.create_contact(&draft("另一个人", "bob@EXAMPLE.com"));
        assert!(matches!(again, Err(crate::StoreError::ContactEmailTaken { .. })));
        assert_eq!(store.count_contacts(ContactScope::Active).expect("数"), 1);
    }

    #[test]
    fn 编辑联系人会接管这条记录并挡住撞车的邮箱() {
        let store = migrated();
        store
            .upsert_contact("信件署名", "z@example.com")
            .expect("同步登记");
        store.create_contact(&draft("李四", "l@example.com")).expect("建");

        let synced = find(&store, "z@example.com");
        assert_eq!(synced.source, ContactSource::Auto);

        let mut edit = draft("张三", "z@example.com");
        edit.note = "上次报价 3 万".to_string();
        assert!(store.update_contact(synced.id, &edit).expect("改"));
        let edited = find(&store, "z@example.com");
        assert_eq!(edited.name, "张三");
        assert_eq!(edited.note, "上次报价 3 万");
        assert_eq!(edited.source, ContactSource::Manual, "改过就归用户管");

        // 再同步一次，名字不许被信件署名顶掉。
        store.upsert_contact("别的署名", "z@example.com").expect("再同步");
        assert_eq!(find(&store, "z@example.com").name, "张三");

        // 改成别人已经在用的邮箱要拦住。
        let taken = store.update_contact(synced.id, &draft("张三", "L@example.com"));
        assert!(matches!(taken, Err(crate::StoreError::ContactEmailTaken { .. })));
    }

    #[test]
    fn 隐藏的联系人不出现在正常列表也不被同步放回来() {
        let store = migrated();
        store.upsert_contact("张三", "z@example.com").expect("同步登记");
        let id = find(&store, "z@example.com").id;

        assert!(store.hide_contact(id).expect("隐藏"));
        assert_eq!(store.count_contacts(ContactScope::Active).expect("数"), 0);
        assert_eq!(store.count_contacts(ContactScope::Hidden).expect("数"), 1);

        store
            .upsert_contact("张三改名了", "z@example.com")
            .expect("再同步");
        let hidden = store
            .list_contacts("", 10, ContactScope::Hidden)
            .expect("看已隐藏");
        assert_eq!(hidden.len(), 1, "同步不许把它放出来");
        assert_eq!(hidden[0].name, "张三", "同步也不许改它的名字");
        assert_eq!(store.count_contacts(ContactScope::Active).expect("数"), 0);

        assert!(store.restore_contact(id).expect("恢复"));
        assert_eq!(store.count_contacts(ContactScope::Active).expect("数"), 1);
    }

    #[test]
    fn 彻底删掉自动收集的联系人再同步会重新出现() {
        let store = migrated();
        store.upsert_contact("张三", "z@example.com").expect("同步登记");
        let id = find(&store, "z@example.com").id;
        assert!(store.purge_contact(id).expect("彻底删"));
        assert_eq!(store.count_contacts(ContactScope::Active).expect("数"), 0);

        store.upsert_contact("张三", "z@example.com").expect("再同步");
        assert_eq!(store.count_contacts(ContactScope::Active).expect("数"), 1);
    }

    #[test]
    fn 分组能建能改名删了以后成员回未分组() {
        let store = migrated();
        let group = store.create_contact_group("客户").expect("建分组");
        assert!(matches!(
            store.create_contact_group(" 客户 "),
            Err(crate::StoreError::ContactGroupNameTaken { .. })
        ));

        let mut contact = draft("鲍勃", "bob@example.com");
        contact.group_id = Some(group);
        store.create_contact(&contact).expect("建联系人");
        assert_eq!(store.list_contact_groups().expect("列分组")[0].member_count, 1);
        assert_eq!(store.count_ungrouped_contacts().expect("数未分组"), 0);

        assert!(store.rename_contact_group(group, "老客户").expect("改名"));
        assert_eq!(store.list_contact_groups().expect("列分组")[0].name, "老客户");

        assert!(store.delete_contact_group(group).expect("删分组"));
        assert!(store.list_contact_groups().expect("列分组").is_empty());
        assert_eq!(store.count_ungrouped_contacts().expect("数未分组"), 1);
        assert_eq!(store.count_contacts(ContactScope::Active).expect("数"), 1);
        assert!(find(&store, "bob@example.com").group_id.is_none());
    }

    #[test]
    fn 清空自动收集只删没被藏起来的自动记录() {
        let store = migrated();
        store.upsert_contact("自动甲", "a@example.com").expect("同步");
        store.upsert_contact("自动乙", "b@example.com").expect("同步");
        let hidden_id = find(&store, "b@example.com").id;
        store.hide_contact(hidden_id).expect("藏起来");
        store
            .create_contact(&draft("手动丙", "c@example.com"))
            .expect("手动建");

        assert_eq!(store.clear_auto_contacts().expect("清"), 1);
        assert_eq!(store.count_contacts(ContactScope::Active).expect("数"), 1);
        assert_eq!(store.count_contacts(ContactScope::Hidden).expect("数"), 1);
        let left = store.list_contacts("", 10, ContactScope::Active).expect("列");
        assert_eq!(left[0].email, "c@example.com", "手动的必须留着");
    }
}
