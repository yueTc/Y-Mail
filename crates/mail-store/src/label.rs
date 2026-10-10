//! 本地邮件分类标签。
//!
//! 标签是纯本地数据：只写这个库，不回写邮件服务器，也不参与账号同步。
//! 每封邮件最多一个标签（`message_label.message_id` 是主键，见设计规格 D3）。
//! 颜色只存固定色板键，不存十六进制色值；模型不接触颜色。

use rusqlite::OptionalExtension;

use crate::connection::Store;
use crate::error::StoreError;

/// 固定色板。数据库只存这些键，界面负责映射成随主题变化的色值。
pub const MAIL_LABEL_COLORS: [&str; 10] = [
    "gray", "red", "orange", "amber", "green", "teal", "blue", "indigo", "purple", "pink",
];

/// 标签名的最大字符数。
pub const MAX_LABEL_NAME_CHARS: usize = 64;

/// 一条本地标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailLabel {
    /// 主键。
    pub id: i64,
    /// 标签名（唯一）。
    pub name: String,
    /// 固定色板键，例如 `orange`。
    pub color: String,
    /// 显示顺序，小的在前。
    pub sort_order: i64,
    /// 是否是兜底标签「其他」。有且只有一条。
    pub is_fallback: bool,
}

/// 邮件标签的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailLabelSource {
    /// 自动或手动分类流程写的。
    Auto,
    /// 用户在读信界面手改的。
    Manual,
}

impl MailLabelSource {
    /// 存库用的标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }

    /// 从存库标识解析；未知值返回 `None`。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "auto" => Some(Self::Auto),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }
}

const SELECT_LABEL: &str = "SELECT id, name, color, sort_order, is_fallback FROM mail_label";

fn row_to_label(row: &rusqlite::Row<'_>) -> rusqlite::Result<MailLabel> {
    Ok(MailLabel {
        id: row.get(0)?,
        name: row.get(1)?,
        color: row.get(2)?,
        sort_order: row.get(3)?,
        is_fallback: row.get::<_, i64>(4)? != 0,
    })
}

/// 校验并规整标签名：去首尾空白、非空、不超过 64 个字符。
fn normalized_label_name(name: &str) -> Result<String, StoreError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(StoreError::InvalidData("标签名不能为空".to_string()));
    }
    if trimmed.chars().count() > MAX_LABEL_NAME_CHARS {
        return Err(StoreError::InvalidData(format!(
            "标签名不能超过 {MAX_LABEL_NAME_CHARS} 个字符"
        )));
    }
    Ok(trimmed.to_string())
}

/// 把「标签名重复」的唯一索引报错翻译成可读错误，其它原样透传。
fn map_label_name_error(err: rusqlite::Error) -> StoreError {
    if let rusqlite::Error::SqliteFailure(_, Some(message)) = &err {
        if message.contains("idx_mail_label_name") || message.contains("mail_label.name") {
            return StoreError::InvalidData("已经有同名标签".to_string());
        }
    }
    StoreError::from(err)
}

impl Store {
    /// 列出全部标签，按 `sort_order` 升序、主键升序。
    pub fn list_mail_labels(&self) -> Result<Vec<MailLabel>, StoreError> {
        let sql = format!("{SELECT_LABEL} ORDER BY sort_order ASC, id ASC");
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut labels = Vec::new();
        while let Some(row) = rows.next()? {
            labels.push(row_to_label(row)?);
        }
        Ok(labels)
    }

    /// 按主键取一条标签。
    pub fn get_mail_label(&self, id: i64) -> Result<Option<MailLabel>, StoreError> {
        let sql = format!("{SELECT_LABEL} WHERE id = ?1");
        self.conn()
            .query_row(&sql, [id], row_to_label)
            .optional()
            .map_err(StoreError::from)
    }

    /// 取兜底标签（有且只有一条）。
    pub fn fallback_mail_label(&self) -> Result<Option<MailLabel>, StoreError> {
        let sql = format!("{SELECT_LABEL} WHERE is_fallback = 1");
        self.conn()
            .query_row(&sql, [], row_to_label)
            .optional()
            .map_err(StoreError::from)
    }

    /// 新增标签，返回新主键。
    ///
    /// 颜色按固定色板顺序挑第一个尚未使用的；全部用过时按当前标签数量取模循环。
    /// 排序值放在所有非兜底标签之后、兜底标签之前。
    pub fn create_mail_label(&self, name: &str) -> Result<i64, StoreError> {
        let name = normalized_label_name(name)?;
        let tx = self.conn().unchecked_transaction()?;

        let mut used = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT color FROM mail_label")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                used.push(row.get::<_, String>(0)?);
            }
        }
        let total = used.len() as i64;
        let color = MAIL_LABEL_COLORS
            .iter()
            .find(|candidate| !used.iter().any(|used| used == *candidate))
            .copied()
            .unwrap_or_else(|| MAIL_LABEL_COLORS[(total as usize) % MAIL_LABEL_COLORS.len()]);

        let max_order: i64 = tx.query_row(
            "SELECT COALESCE(MAX(sort_order), 0) FROM mail_label WHERE is_fallback = 0",
            [],
            |row| row.get(0),
        )?;

        tx.execute(
            "INSERT INTO mail_label (name, color, sort_order, is_fallback) VALUES (?1, ?2, ?3, 0)",
            rusqlite::params![name, color, max_order + 10],
        )
        .map_err(map_label_name_error)?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    /// 给标签改名。兜底标签也可以改名。
    pub fn rename_mail_label(&self, id: i64, name: &str) -> Result<(), StoreError> {
        let name = normalized_label_name(name)?;
        let changed = self
            .conn()
            .execute(
                "UPDATE mail_label
                 SET name = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1",
                rusqlite::params![id, name],
            )
            .map_err(map_label_name_error)?;
        if changed == 0 {
            return Err(StoreError::InvalidData("标签不存在".to_string()));
        }
        Ok(())
    }

    /// 删除标签；兜底标签拒绝删除。邮件上的关联记录由外键级联清掉。
    pub fn delete_mail_label(&self, id: i64) -> Result<(), StoreError> {
        let fallback = self
            .get_mail_label(id)?
            .ok_or_else(|| StoreError::InvalidData("标签不存在".to_string()))?
            .is_fallback;
        if fallback {
            return Err(StoreError::InvalidData("兜底标签不能删除".to_string()));
        }
        self.conn()
            .execute("DELETE FROM mail_label WHERE id = ?1", [id])?;
        Ok(())
    }

    /// 修改标签颜色，只接受固定色板键；兜底标签颜色固定灰色，拒绝修改。
    pub fn set_mail_label_color(&self, id: i64, color: &str) -> Result<(), StoreError> {
        if !MAIL_LABEL_COLORS.contains(&color) {
            return Err(StoreError::InvalidData("颜色只能从固定色板里选".to_string()));
        }
        let label = self
            .get_mail_label(id)?
            .ok_or_else(|| StoreError::InvalidData("标签不存在".to_string()))?;
        if label.is_fallback {
            return Err(StoreError::InvalidData("兜底标签颜色不能修改".to_string()));
        }
        self.conn().execute(
            "UPDATE mail_label
             SET color = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![id, color],
        )?;
        Ok(())
    }

    /// 重新排序；必须传入全部标签主键且不能重复，按传入顺序写 `sort_order`。
    pub fn reorder_mail_labels(&self, ids: &[i64]) -> Result<(), StoreError> {
        let mut existing: Vec<i64> = {
            let mut stmt = self.conn().prepare("SELECT id FROM mail_label")?;
            let mut rows = stmt.query([])?;
            let mut collected = Vec::new();
            while let Some(row) = rows.next()? {
                collected.push(row.get::<_, i64>(0)?);
            }
            collected
        };

        let mut given = ids.to_vec();
        given.sort_unstable();
        given.dedup();
        if given.len() != ids.len() {
            return Err(StoreError::InvalidData("排序里不能有重复标签".to_string()));
        }
        existing.sort_unstable();
        if given != existing {
            return Err(StoreError::InvalidData("排序必须包含全部标签".to_string()));
        }

        let tx = self.conn().unchecked_transaction()?;
        for (index, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE mail_label
                 SET sort_order = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1",
                rusqlite::params![id, (index as i64 + 1) * 10],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Store;

    fn store() -> Store {
        let dir = tempfile::tempdir().expect("临时目录");
        let mut store = Store::open(dir.keep().join("mail.db")).expect("开库");
        store.run_migrations().expect("迁移");
        store
    }

    #[test]
    fn labels_can_be_created_renamed_deleted_and_reordered() {
        let store = store();
        let initial = store.list_mail_labels().expect("初始标签");
        assert_eq!(initial.len(), 8);
        let fallback = initial.iter().find(|label| label.name == "其他").expect("兜底");
        assert_eq!(fallback.color, "gray");
        assert!(fallback.is_fallback);

        let extra = store.create_mail_label("项目").expect("新增");
        let after_create = store.list_mail_labels().expect("再查");
        assert_eq!(after_create.len(), 9);
        let created = after_create
            .iter()
            .find(|label| label.id == extra)
            .expect("新标签");
        assert_eq!(created.color, "red");
        assert!(!created.is_fallback);

        store.rename_mail_label(extra, "项目邮件").expect("改名");
        let renamed = store
            .list_mail_labels()
            .expect("查改名")
            .into_iter()
            .find(|label| label.id == extra)
            .expect("改名后仍在");
        assert_eq!(renamed.name, "项目邮件");

        let mut order: Vec<i64> = initial.iter().map(|label| label.id).collect();
        order.insert(0, extra);
        store.reorder_mail_labels(&order).expect("排序");
        let after = store.list_mail_labels().expect("查排序");
        assert_eq!(after[0].id, extra);

        store.delete_mail_label(extra).expect("删除");
        assert_eq!(store.list_mail_labels().expect("删后").len(), 8);
    }

    #[test]
    fn new_label_colors_follow_the_palette_order() {
        let store = store();
        // 初始八条用掉 blue / orange / teal / purple / green / indigo / pink / gray，
        // 新标签按色板顺序应拿到第一个没用过的 red，然后是 amber。
        let first = store.create_mail_label("项目").expect("新增一");
        let second = store.create_mail_label("报销").expect("新增二");
        let labels = store.list_mail_labels().expect("列表");
        let color_of = |id: i64| {
            labels
                .iter()
                .find(|label| label.id == id)
                .expect("找到标签")
                .color
                .clone()
        };
        assert_eq!(color_of(first), "red");
        assert_eq!(color_of(second), "amber");
    }

    #[test]
    fn creating_a_label_rejects_blank_or_duplicate_names() {
        let store = store();
        assert!(matches!(
            store.create_mail_label("   ").expect_err("空白名"),
            StoreError::InvalidData(_)
        ));
        assert!(matches!(
            store.create_mail_label("客户").expect_err("重名"),
            StoreError::InvalidData(_)
        ));
        let long = "很长".repeat(40);
        assert!(matches!(
            store.create_mail_label(&long).expect_err("超长"),
            StoreError::InvalidData(_)
        ));
    }

    #[test]
    fn fallback_label_cannot_be_deleted() {
        let store = store();
        let fallback = store
            .list_mail_labels()
            .expect("标签")
            .into_iter()
            .find(|label| label.is_fallback)
            .expect("兜底");
        let err = store.delete_mail_label(fallback.id).expect_err("不能删");
        assert!(matches!(err, StoreError::InvalidData(_)));
    }

    #[test]
    fn fallback_label_color_is_fixed_gray() {
        let store = store();
        let fallback = store
            .list_mail_labels()
            .expect("标签")
            .into_iter()
            .find(|label| label.is_fallback)
            .expect("兜底");
        let err = store
            .set_mail_label_color(fallback.id, "red")
            .expect_err("不能改兜底颜色");
        assert!(matches!(err, StoreError::InvalidData(_)));
        // 兜底标签仍然可以改名。
        store.rename_mail_label(fallback.id, "其他邮件").expect("改名");
        let renamed = store
            .list_mail_labels()
            .expect("再查")
            .into_iter()
            .find(|label| label.id == fallback.id)
            .expect("兜底仍在");
        assert_eq!(renamed.name, "其他邮件");
        assert!(renamed.is_fallback);
        assert_eq!(renamed.color, "gray");
    }

    #[test]
    fn set_color_rejects_unknown_palette_key() {
        let store = store();
        let label = store.create_mail_label("项目").expect("新增");
        let err = store
            .set_mail_label_color(label, "#ff0000")
            .expect_err("只接受固定色板");
        assert!(matches!(err, StoreError::InvalidData(_)));
        store.set_mail_label_color(label, "pink").expect("合法颜色");
        let color = store
            .list_mail_labels()
            .expect("列表")
            .into_iter()
            .find(|row| row.id == label)
            .expect("标签")
            .color;
        assert_eq!(color, "pink");
    }

    #[test]
    fn reorder_requires_all_ids_once() {
        let store = store();
        let ids: Vec<i64> = store
            .list_mail_labels()
            .expect("标签")
            .iter()
            .map(|label| label.id)
            .collect();
        assert!(matches!(
            store.reorder_mail_labels(&ids[..3]).expect_err("少传"),
            StoreError::InvalidData(_)
        ));
        let mut dup = ids.clone();
        dup[1] = dup[0];
        assert!(matches!(
            store.reorder_mail_labels(&dup).expect_err("重复"),
            StoreError::InvalidData(_)
        ));
    }
}
