//! 本地邮件分类标签。
//!
//! 标签是纯本地数据：只写这个库，不回写邮件服务器，也不参与账号同步。
//! 每封邮件在每个标签组里最多一条结果（`message_label` 的主键是邮件和组）。
//! 颜色只存固定色板键，不存十六进制色值；模型不接触颜色。

use std::collections::HashMap;

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
    /// 所属标签组。
    pub group_id: i64,
    /// 标签名（同一组内唯一）。
    pub name: String,
    /// 固定色板键，例如 `orange`。
    pub color: String,
    /// 显示顺序，小的在前。
    pub sort_order: i64,
    /// 是否是所在组的兜底标签。每组最多一条。
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

/// 组名的最大字符数。
pub const MAX_GROUP_NAME_CHARS: usize = 64;

/// 把握门槛的最小值。
pub const MIN_GROUP_THRESHOLD: f64 = 0.5;

/// 把握门槛的最大值。
pub const MAX_GROUP_THRESHOLD: f64 = 0.99;

/// 标签组类型，对应 jev 的三种题型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelGroupKind {
    /// 选择题：从一组标签里选一个。
    Choice,
    /// 是否题：返回 0 到 1 的概率，落两条标签之一。
    Boolean,
    /// 打分题：返回各档位概率，落一个分数。
    Score,
}

impl LabelGroupKind {
    /// 存库用的标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Choice => "choice",
            Self::Boolean => "boolean",
            Self::Score => "score",
        }
    }

    /// 从存库标识解析；未知值返回 `None`。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "choice" => Some(Self::Choice),
            "boolean" => Some(Self::Boolean),
            "score" => Some(Self::Score),
            _ => None,
        }
    }
}

/// 一个标签组（一个判断维度）。
#[derive(Debug, Clone, PartialEq)]
pub struct LabelGroup {
    /// 主键。
    pub id: i64,
    /// 组名（全局唯一，不含英文冒号）。
    pub name: String,
    /// 组类型。
    pub kind: LabelGroupKind,
    /// 这一组是否参与自动分类。
    pub enabled: bool,
    /// 这一组的把握门槛。
    pub confidence_threshold: f64,
    /// 打分题下限，其它类型为空。
    pub score_min: Option<i64>,
    /// 打分题上限，其它类型为空。
    pub score_max: Option<i64>,
    /// 显示顺序，小的在前。
    pub sort_order: i64,
}

/// 一封邮件在某个标签组里的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct MessageGroupResult {
    /// 所属标签组。
    pub group: LabelGroup,
    /// 选择题或是否题命中的标签；打分题为空。
    pub label: Option<MailLabel>,
    /// 打分题的数字；选择题或是否题为空。
    pub score_value: Option<i64>,
    /// 这条结果是自动写的还是用户手改的。
    pub source: MailLabelSource,
    /// 模型或人工给的把握程度。
    pub confidence: Option<f64>,
}

const SELECT_GROUP: &str =
    "SELECT id, name, kind, enabled, confidence_threshold, score_min, score_max, sort_order FROM mail_label_group";

fn row_to_group(row: &rusqlite::Row<'_>) -> rusqlite::Result<LabelGroup> {
    let kind_raw: String = row.get(2)?;
    Ok(LabelGroup {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: LabelGroupKind::parse(&kind_raw).unwrap_or(LabelGroupKind::Choice),
        enabled: row.get::<_, i64>(3)? != 0,
        confidence_threshold: row.get(4)?,
        score_min: row.get(5)?,
        score_max: row.get(6)?,
        sort_order: row.get(7)?,
    })
}

/// 校验并规整组名：去首尾空白、非空、不超过 64 个字符、不含英文冒号。
fn normalized_group_name(name: &str) -> Result<String, StoreError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(StoreError::InvalidData("组名不能为空".to_string()));
    }
    if trimmed.chars().count() > MAX_GROUP_NAME_CHARS {
        return Err(StoreError::InvalidData(format!(
            "组名不能超过 {MAX_GROUP_NAME_CHARS} 个字符"
        )));
    }
    if trimmed.contains(':') {
        return Err(StoreError::InvalidData("组名不能包含英文冒号".to_string()));
    }
    Ok(trimmed.to_string())
}

/// 校验把握门槛：必须在 0.5 到 0.99 之间。
fn validate_group_threshold(value: f64) -> Result<(), StoreError> {
    if value.is_nan() || !(MIN_GROUP_THRESHOLD..=MAX_GROUP_THRESHOLD).contains(&value) {
        return Err(StoreError::InvalidData(format!(
            "把握门槛必须在 {MIN_GROUP_THRESHOLD} 到 {MAX_GROUP_THRESHOLD} 之间"
        )));
    }
    Ok(())
}

/// 校验打分题的上下限：必须同时填、下限小于上限、档位数在 2 到 10 之间。
fn validate_score_range(min: Option<i64>, max: Option<i64>) -> Result<(), StoreError> {
    match (min, max) {
        (Some(min), Some(max)) => {
            if min >= max {
                return Err(StoreError::InvalidData("打分下限必须小于上限".to_string()));
            }
            let levels = max - min + 1;
            if !(2..=10).contains(&levels) {
                return Err(StoreError::InvalidData("打分档位必须在 2 到 10 之间".to_string()));
            }
            Ok(())
        }
        _ => Err(StoreError::InvalidData("打分题必须同时填下限和上限".to_string())),
    }
}

/// 把「组名重复」的唯一索引报错翻译成可读错误，其它原样透传。
fn map_group_name_error(err: rusqlite::Error) -> StoreError {
    if let rusqlite::Error::SqliteFailure(_, Some(message)) = &err {
        if message.contains("mail_label_group.name") || message.contains("idx_mail_label_group_unique_name") {
            return StoreError::InvalidData("已经有同名标签组".to_string());
        }
    }
    StoreError::from(err)
}

const SELECT_LABEL: &str = "SELECT id, group_id, name, color, sort_order, is_fallback FROM mail_label";

fn row_to_label(row: &rusqlite::Row<'_>) -> rusqlite::Result<MailLabel> {
    Ok(MailLabel {
        id: row.get(0)?,
        group_id: row.get(1)?,
        name: row.get(2)?,
        color: row.get(3)?,
        sort_order: row.get(4)?,
        is_fallback: row.get::<_, i64>(5)? != 0,
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
        if message.contains("idx_mail_label_group_name")
            || message.contains("idx_mail_label_name")
            || message.contains("mail_label.name")
        {
            return StoreError::InvalidData("已经有同名标签".to_string());
        }
    }
    StoreError::from(err)
}

const SELECT_GROUP_RESULT: &str = "
SELECT g.id, g.name, g.kind, g.enabled, g.confidence_threshold, g.score_min, g.score_max, g.sort_order,
       l.id, l.group_id, l.name, l.color, l.sort_order, l.is_fallback,
       ml.score_value, ml.source, ml.confidence
FROM message_label ml
JOIN mail_label_group g ON g.id = ml.group_id
LEFT JOIN mail_label l ON l.id = ml.label_id";

fn row_to_group_result(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageGroupResult> {
    row_to_group_result_at(row, 0)
}

/// 从第 `offset` 列开始读一条组结果；批量查询在前面多带了一列邮件编号。
fn row_to_group_result_at(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<MessageGroupResult> {
    let kind_raw: String = row.get(offset + 2)?;
    let group = LabelGroup {
        id: row.get(offset)?,
        name: row.get(offset + 1)?,
        kind: LabelGroupKind::parse(&kind_raw).unwrap_or(LabelGroupKind::Choice),
        enabled: row.get::<_, i64>(offset + 3)? != 0,
        confidence_threshold: row.get(offset + 4)?,
        score_min: row.get(offset + 5)?,
        score_max: row.get(offset + 6)?,
        sort_order: row.get(offset + 7)?,
    };
    let label_id: Option<i64> = row.get(offset + 8)?;
    let label = match label_id {
        Some(id) => Some(MailLabel {
            id,
            group_id: row.get(offset + 9)?,
            name: row.get(offset + 10)?,
            color: row.get(offset + 11)?,
            sort_order: row.get(offset + 12)?,
            is_fallback: row.get::<_, i64>(offset + 13)? != 0,
        }),
        None => None,
    };
    let source_raw: String = row.get(offset + 15)?;
    Ok(MessageGroupResult {
        group,
        label,
        score_value: row.get(offset + 14)?,
        source: MailLabelSource::parse(&source_raw).unwrap_or(MailLabelSource::Auto),
        confidence: row.get(offset + 16)?,
    })
}

/// 批量取组结果用的查询：比单条版本多带一列邮件编号，排在所有字段前面。
const SELECT_GROUP_RESULT_BY_MESSAGE: &str = "
SELECT ml.message_id, g.id, g.name, g.kind, g.enabled, g.confidence_threshold, g.score_min, g.score_max, g.sort_order,
       l.id, l.group_id, l.name, l.color, l.sort_order, l.is_fallback,
       ml.score_value, ml.source, ml.confidence
FROM message_label ml
JOIN mail_label_group g ON g.id = ml.group_id
LEFT JOIN mail_label l ON l.id = ml.label_id";
impl Store {
    /// 取默认的「内容分类」组编号；只给旧接口过渡用。
    pub(crate) fn legacy_content_group_id(&self) -> Result<i64, StoreError> {
        self.conn()
            .query_row(
                "SELECT id FROM mail_label_group WHERE name = '内容分类'",
                [],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::InvalidData("找不到内容分类标签组".to_string()))
    }

    /// 过渡兼容：旧接口只处理内容分类组。
    pub fn list_mail_labels(&self) -> Result<Vec<MailLabel>, StoreError> {
        self.list_group_labels(self.legacy_content_group_id()?)
    }

    /// 列出某个组里的标签，按 `sort_order` 升序、主键升序。
    pub fn list_group_labels(&self, group_id: i64) -> Result<Vec<MailLabel>, StoreError> {
        if self.get_label_group(group_id)?.is_none() {
            return Err(StoreError::InvalidData("标签组不存在".to_string()));
        }
        let sql = format!("{SELECT_LABEL} WHERE group_id = ?1 ORDER BY sort_order ASC, id ASC");
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query([group_id])?;
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

    /// 过渡兼容：取内容分类组的兜底标签。
    pub fn fallback_mail_label(&self) -> Result<Option<MailLabel>, StoreError> {
        self.fallback_group_label(self.legacy_content_group_id()?)
    }

    /// 取某个组的兜底标签。
    pub fn fallback_group_label(&self, group_id: i64) -> Result<Option<MailLabel>, StoreError> {
        let sql = format!("{SELECT_LABEL} WHERE group_id = ?1 AND is_fallback = 1");
        self.conn()
            .query_row(&sql, [group_id], row_to_label)
            .optional()
            .map_err(StoreError::from)
    }

    /// 过渡兼容：在内容分类组新增标签。
    pub fn create_mail_label(&self, name: &str) -> Result<i64, StoreError> {
        self.create_group_label(self.legacy_content_group_id()?, name)
    }

    /// 在某个组新增标签，返回新主键。
    ///
    /// 颜色按固定色板顺序挑组内第一个尚未使用的；全部用过时按组内标签数量取模循环。
    /// 排序值放在组内所有非兜底标签之后、兜底标签之前。
    pub fn create_group_label(&self, group_id: i64, name: &str) -> Result<i64, StoreError> {
        let name = normalized_label_name(name)?;
        let tx = self.conn().unchecked_transaction()?;
        let group_exists = tx
            .query_row("SELECT 1 FROM mail_label_group WHERE id = ?1", [group_id], |_| {
                Ok(true)
            })
            .optional()?
            .unwrap_or(false);
        if !group_exists {
            return Err(StoreError::InvalidData("标签组不存在".to_string()));
        }

        let mut used = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT color FROM mail_label WHERE group_id = ?1")?;
            let mut rows = stmt.query([group_id])?;
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
            "SELECT COALESCE(MAX(sort_order), 0)
             FROM mail_label
             WHERE group_id = ?1 AND is_fallback = 0",
            [group_id],
            |row| row.get(0),
        )?;

        tx.execute(
            "INSERT INTO mail_label (group_id, name, color, sort_order, is_fallback)
             VALUES (?1, ?2, ?3, ?4, 0)",
            rusqlite::params![group_id, name, color, max_order + 10],
        )
        .map_err(map_label_name_error)?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    /// 过渡兼容：按标签主键改名。
    pub fn rename_mail_label(&self, id: i64, name: &str) -> Result<(), StoreError> {
        let label = self
            .get_mail_label(id)?
            .ok_or_else(|| StoreError::InvalidData("标签不存在".to_string()))?;
        self.rename_group_label(label.group_id, id, name)
    }

    /// 给某个组内的标签改名；兜底标签也可以改名。
    pub fn rename_group_label(&self, group_id: i64, id: i64, name: &str) -> Result<(), StoreError> {
        let name = normalized_label_name(name)?;
        let changed = self
            .conn()
            .execute(
                "UPDATE mail_label
                 SET name = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1 AND group_id = ?2",
                rusqlite::params![id, group_id, name],
            )
            .map_err(map_label_name_error)?;
        if changed == 0 {
            return Err(StoreError::InvalidData("标签不存在或不属于该组".to_string()));
        }
        Ok(())
    }

    /// 过渡兼容：按标签主键删除。
    pub fn delete_mail_label(&self, id: i64) -> Result<(), StoreError> {
        let label = self
            .get_mail_label(id)?
            .ok_or_else(|| StoreError::InvalidData("标签不存在".to_string()))?;
        self.delete_group_label(label.group_id, id)
    }

    /// 删除某个组里的标签；兜底标签拒绝删除。邮件结果由外键级联清掉。
    pub fn delete_group_label(&self, group_id: i64, id: i64) -> Result<(), StoreError> {
        let label = self
            .get_mail_label(id)?
            .ok_or_else(|| StoreError::InvalidData("标签不存在".to_string()))?;
        if label.group_id != group_id {
            return Err(StoreError::InvalidData("标签不存在或不属于该组".to_string()));
        }
        if label.is_fallback {
            return Err(StoreError::InvalidData("兜底标签不能删除".to_string()));
        }
        self.conn().execute(
            "DELETE FROM mail_label WHERE id = ?1 AND group_id = ?2",
            rusqlite::params![id, group_id],
        )?;
        Ok(())
    }

    /// 过渡兼容：按标签主键改颜色。
    pub fn set_mail_label_color(&self, id: i64, color: &str) -> Result<(), StoreError> {
        let label = self
            .get_mail_label(id)?
            .ok_or_else(|| StoreError::InvalidData("标签不存在".to_string()))?;
        self.set_group_label_color(label.group_id, id, color)
    }

    /// 修改某个组内标签颜色，只接受固定色板键；兜底标签颜色固定灰色，拒绝修改。
    pub fn set_group_label_color(&self, group_id: i64, id: i64, color: &str) -> Result<(), StoreError> {
        if !MAIL_LABEL_COLORS.contains(&color) {
            return Err(StoreError::InvalidData("颜色只能从固定色板里选".to_string()));
        }
        let label = self
            .get_mail_label(id)?
            .ok_or_else(|| StoreError::InvalidData("标签不存在".to_string()))?;
        if label.group_id != group_id {
            return Err(StoreError::InvalidData("标签不存在或不属于该组".to_string()));
        }
        if label.is_fallback {
            return Err(StoreError::InvalidData("兜底标签颜色不能修改".to_string()));
        }
        self.conn().execute(
            "UPDATE mail_label
             SET color = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND group_id = ?2",
            rusqlite::params![id, group_id, color],
        )?;
        Ok(())
    }

    /// 指定或取消某个组内标签的兜底。
    ///
    /// 同一组最多一个兜底：设为兜底时先把该组原有兜底清掉，颜色固定灰色；可改名。
    pub fn set_group_label_fallback(
        &self,
        group_id: i64,
        id: i64,
        is_fallback: bool,
    ) -> Result<(), StoreError> {
        let label = self
            .get_mail_label(id)?
            .ok_or_else(|| StoreError::InvalidData("标签不存在".to_string()))?;
        if label.group_id != group_id {
            return Err(StoreError::InvalidData("标签不存在或不属于该组".to_string()));
        }
        let tx = self.conn().unchecked_transaction()?;
        if is_fallback {
            tx.execute(
                "UPDATE mail_label
                 SET is_fallback = 0, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE group_id = ?1 AND is_fallback = 1",
                [group_id],
            )?;
            tx.execute(
                "UPDATE mail_label
                 SET is_fallback = 1, color = 'gray',
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1 AND group_id = ?2",
                rusqlite::params![id, group_id],
            )?;
        } else {
            tx.execute(
                "UPDATE mail_label
                 SET is_fallback = 0, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1 AND group_id = ?2",
                rusqlite::params![id, group_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// 过渡兼容：重排内容分类组里的全部标签。
    pub fn reorder_mail_labels(&self, ids: &[i64]) -> Result<(), StoreError> {
        self.reorder_group_labels(self.legacy_content_group_id()?, ids)
    }

    /// 重新排序某个组内的标签；必须传入该组全部标签主键且不能重复。
    pub fn reorder_group_labels(&self, group_id: i64, ids: &[i64]) -> Result<(), StoreError> {
        let mut existing: Vec<i64> = {
            let mut stmt = self
                .conn()
                .prepare("SELECT id FROM mail_label WHERE group_id = ?1")?;
            let mut rows = stmt.query([group_id])?;
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
            return Err(StoreError::InvalidData("排序必须包含该组全部标签".to_string()));
        }

        let tx = self.conn().unchecked_transaction()?;
        for (index, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE mail_label
                 SET sort_order = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1 AND group_id = ?2",
                rusqlite::params![id, group_id, (index as i64 + 1) * 10],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

impl Store {
    /// 列出全部标签组，按 `sort_order` 升序、主键升序。
    pub fn list_label_groups(&self) -> Result<Vec<LabelGroup>, StoreError> {
        let sql = format!("{SELECT_GROUP} ORDER BY sort_order ASC, id ASC");
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut groups = Vec::new();
        while let Some(row) = rows.next()? {
            groups.push(row_to_group(row)?);
        }
        Ok(groups)
    }

    /// 按主键取一个标签组。
    pub fn get_label_group(&self, id: i64) -> Result<Option<LabelGroup>, StoreError> {
        let sql = format!("{SELECT_GROUP} WHERE id = ?1");
        self.conn()
            .query_row(&sql, [id], row_to_group)
            .optional()
            .map_err(StoreError::from)
    }

    /// 新建标签组，返回新主键。是否题默认门槛 0.5，其余类型默认 0.75。
    pub fn create_label_group(
        &self,
        name: &str,
        kind: LabelGroupKind,
        score_min: Option<i64>,
        score_max: Option<i64>,
    ) -> Result<i64, StoreError> {
        let name = normalized_group_name(name)?;
        match kind {
            LabelGroupKind::Score => validate_score_range(score_min, score_max)?,
            LabelGroupKind::Choice | LabelGroupKind::Boolean => {
                if score_min.is_some() || score_max.is_some() {
                    return Err(StoreError::InvalidData("只有打分题才能填上下限".to_string()));
                }
            }
        }
        let default_threshold = match kind {
            LabelGroupKind::Boolean => 0.5,
            _ => 0.75,
        };

        let tx = self.conn().unchecked_transaction()?;
        let max_order: i64 = tx.query_row(
            "SELECT COALESCE(MAX(sort_order), 0) FROM mail_label_group",
            [],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO mail_label_group
                 (name, kind, enabled, confidence_threshold, score_min, score_max, sort_order)
             VALUES (?1, ?2, 0, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                name,
                kind.as_str(),
                default_threshold,
                score_min,
                score_max,
                max_order + 10
            ],
        )
        .map_err(map_group_name_error)?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    /// 给标签组改名。
    pub fn rename_label_group(&self, id: i64, name: &str) -> Result<(), StoreError> {
        let name = normalized_group_name(name)?;
        let changed = self
            .conn()
            .execute(
                "UPDATE mail_label_group
                 SET name = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1",
                rusqlite::params![id, name],
            )
            .map_err(map_group_name_error)?;
        if changed == 0 {
            return Err(StoreError::InvalidData("标签组不存在".to_string()));
        }
        Ok(())
    }

    /// 开或关一个标签组。
    pub fn set_label_group_enabled(&self, id: i64, enabled: bool) -> Result<(), StoreError> {
        let changed = self.conn().execute(
            "UPDATE mail_label_group
             SET enabled = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![id, i64::from(enabled)],
        )?;
        if changed == 0 {
            return Err(StoreError::InvalidData("标签组不存在".to_string()));
        }
        Ok(())
    }

    /// 改一个标签组的把握门槛，范围 0.5 到 0.99。
    pub fn set_label_group_threshold(&self, id: i64, threshold: f64) -> Result<(), StoreError> {
        validate_group_threshold(threshold)?;
        let changed = self.conn().execute(
            "UPDATE mail_label_group
             SET confidence_threshold = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![id, threshold],
        )?;
        if changed == 0 {
            return Err(StoreError::InvalidData("标签组不存在".to_string()));
        }
        Ok(())
    }

    /// 删除标签组；组内标签与该组所有邮件结果由外键级联清掉。
    pub fn delete_label_group(&self, id: i64) -> Result<(), StoreError> {
        let changed = self
            .conn()
            .execute("DELETE FROM mail_label_group WHERE id = ?1", [id])?;
        if changed == 0 {
            return Err(StoreError::InvalidData("标签组不存在".to_string()));
        }
        Ok(())
    }

    /// 重新排序；必须传入全部组主键且不能重复，按传入顺序写 `sort_order`。
    pub fn reorder_label_groups(&self, ids: &[i64]) -> Result<(), StoreError> {
        let mut existing: Vec<i64> = {
            let mut stmt = self.conn().prepare("SELECT id FROM mail_label_group")?;
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
            return Err(StoreError::InvalidData("排序里不能有重复标签组".to_string()));
        }
        existing.sort_unstable();
        if given != existing {
            return Err(StoreError::InvalidData("排序必须包含全部标签组".to_string()));
        }

        let tx = self.conn().unchecked_transaction()?;
        for (index, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE mail_label_group
                 SET sort_order = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1",
                rusqlite::params![id, (index as i64 + 1) * 10],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

impl Store {
    /// 过渡兼容：把标签写进它所属的组。
    pub fn set_message_label(
        &self,
        message_id: i64,
        label_id: i64,
        source: MailLabelSource,
        confidence: Option<f64>,
    ) -> Result<(), StoreError> {
        let label = self
            .get_mail_label(label_id)?
            .ok_or_else(|| StoreError::InvalidData("标签不存在".to_string()))?;
        self.set_message_group_value(
            message_id,
            label.group_id,
            Some(label_id),
            None,
            source,
            confidence,
        )
    }

    /// 给一封邮件的某个组写结果；同一组已有结果就整行覆盖。
    ///
    /// 选择题和是否题只能写组内标签；打分题只能写组内上下限之间的整数。
    pub fn set_message_group_value(
        &self,
        message_id: i64,
        group_id: i64,
        label_id: Option<i64>,
        score_value: Option<i64>,
        source: MailLabelSource,
        confidence: Option<f64>,
    ) -> Result<(), StoreError> {
        let message_exists = self
            .conn()
            .query_row("SELECT 1 FROM message WHERE id = ?1", [message_id], |_| Ok(true))
            .optional()?
            .unwrap_or(false);
        if !message_exists {
            return Err(StoreError::InvalidData("邮件不存在".to_string()));
        }

        let group = self
            .get_label_group(group_id)?
            .ok_or_else(|| StoreError::InvalidData("标签组不存在".to_string()))?;

        if let Some(value) = confidence {
            if value.is_nan() || !(0.0..=1.0).contains(&value) {
                return Err(StoreError::InvalidData("把握程度必须在 0 到 1 之间".to_string()));
            }
        }

        match group.kind {
            LabelGroupKind::Choice | LabelGroupKind::Boolean => {
                if score_value.is_some() {
                    return Err(StoreError::InvalidData("选择题和是否题只能写标签".to_string()));
                }
                let label_id = label_id
                    .ok_or_else(|| StoreError::InvalidData("选择题和是否题必须写一个标签".to_string()))?;
                let belongs = self
                    .conn()
                    .query_row(
                        "SELECT 1 FROM mail_label WHERE id = ?1 AND group_id = ?2",
                        rusqlite::params![label_id, group_id],
                        |_| Ok(true),
                    )
                    .optional()?
                    .unwrap_or(false);
                if !belongs {
                    return Err(StoreError::InvalidData("标签不存在或不属于该组".to_string()));
                }
            }
            LabelGroupKind::Score => {
                if label_id.is_some() {
                    return Err(StoreError::InvalidData("打分题只能写数字".to_string()));
                }
                let score =
                    score_value.ok_or_else(|| StoreError::InvalidData("打分题必须写数字".to_string()))?;
                let (min, max) = match (group.score_min, group.score_max) {
                    (Some(min), Some(max)) => (min, max),
                    _ => {
                        return Err(StoreError::InvalidData(
                            "打分题缺少上下限，不能写结果".to_string(),
                        ))
                    }
                };
                if !(min..=max).contains(&score) {
                    return Err(StoreError::InvalidData(format!("打分必须在 {min} 到 {max} 之间")));
                }
            }
        }

        self.conn().execute(
            "INSERT INTO message_label
                 (message_id, group_id, label_id, score_value, source, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(message_id, group_id) DO UPDATE SET
                 label_id = excluded.label_id,
                 score_value = excluded.score_value,
                 source = excluded.source,
                 confidence = excluded.confidence,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            rusqlite::params![
                message_id,
                group_id,
                label_id,
                score_value,
                source.as_str(),
                confidence
            ],
        )?;
        Ok(())
    }

    /// 过渡兼容：只读内容分类组的结果。
    pub fn get_message_label(
        &self,
        message_id: i64,
    ) -> Result<Option<(MailLabel, Option<f64>, MailLabelSource)>, StoreError> {
        let content = self.legacy_content_group_id()?;
        match self.message_group_result(message_id, content)? {
            Some(result) => match result.label {
                Some(label) => Ok(Some((label, result.confidence, result.source))),
                None => Ok(None),
            },
            None => Ok(None),
        }
    }

    /// 读一封邮件某个组的结果；没有结果返回 `None`。
    pub fn message_group_result(
        &self,
        message_id: i64,
        group_id: i64,
    ) -> Result<Option<MessageGroupResult>, StoreError> {
        let sql = format!("{SELECT_GROUP_RESULT} WHERE ml.message_id = ?1 AND ml.group_id = ?2");
        self.conn()
            .query_row(&sql, rusqlite::params![message_id, group_id], row_to_group_result)
            .optional()
            .map_err(StoreError::from)
    }

    /// 过渡兼容：只清内容分类组的结果。
    pub fn clear_message_label(&self, message_id: i64) -> Result<bool, StoreError> {
        self.clear_message_group_value(message_id, self.legacy_content_group_id()?)
    }

    /// 清空一封邮件某个组的结果；返回是否真的删掉了一行。
    pub fn clear_message_group_value(&self, message_id: i64, group_id: i64) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "DELETE FROM message_label WHERE message_id = ?1 AND group_id = ?2",
            rusqlite::params![message_id, group_id],
        )?;
        Ok(changed > 0)
    }

    /// 列出某封邮件的全部组结果，按组排序升序。
    pub fn list_message_group_results(&self, message_id: i64) -> Result<Vec<MessageGroupResult>, StoreError> {
        let sql =
            format!("{SELECT_GROUP_RESULT} WHERE ml.message_id = ?1 ORDER BY g.sort_order ASC, g.id ASC");
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query([message_id])?;
        let mut results = Vec::new();
        while let Some(row) = rows.next()? {
            results.push(row_to_group_result(row)?);
        }
        Ok(results)
    }

    /// 批量取一批邮件的全部组结果，按邮件编号分组；每组内按组的显示顺序升序。
    ///
    /// 供收件箱、搜索列表一次取完，避免每封邮件各查一遍。没有结果的邮件不出现在返回值里。
    pub fn group_results_for_messages(
        &self,
        message_ids: &[i64],
    ) -> Result<HashMap<i64, Vec<MessageGroupResult>>, StoreError> {
        let mut grouped: HashMap<i64, Vec<MessageGroupResult>> = HashMap::new();
        if message_ids.is_empty() {
            return Ok(grouped);
        }
        let mut unique = message_ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        let placeholders = std::iter::repeat_n("?", unique.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "{SELECT_GROUP_RESULT_BY_MESSAGE} WHERE ml.message_id IN ({placeholders}) \
             ORDER BY ml.message_id ASC, g.sort_order ASC, g.id ASC"
        );
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(unique.iter()))?;
        while let Some(row) = rows.next()? {
            let message_id: i64 = row.get(0)?;
            grouped
                .entry(message_id)
                .or_default()
                .push(row_to_group_result_at(row, 1)?);
        }
        Ok(grouped)
    }

    /// 过渡兼容：按内容分类组的标签名精确取邮件编号（升序）。
    pub fn list_message_ids_with_label(&self, label_name: &str) -> Result<Vec<i64>, StoreError> {
        let content = self.legacy_content_group_id()?;
        let mut stmt = self.conn().prepare(
            "SELECT ml.message_id
             FROM message_label ml
             JOIN mail_label l ON l.id = ml.label_id
             WHERE ml.group_id = ?1 AND l.name = ?2
             ORDER BY ml.message_id ASC",
        )?;
        let mut rows = stmt.query(rusqlite::params![content, label_name])?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next()? {
            ids.push(row.get::<_, i64>(0)?);
        }
        Ok(ids)
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
    fn fallback_can_be_assigned_moved_and_cleared() {
        let store = store();
        let original = store.fallback_mail_label().expect("查兜底").expect("初始兜底");
        let group_id = original.group_id;
        let candidate = store
            .list_group_labels(group_id)
            .expect("组内标签")
            .into_iter()
            .find(|label| !label.is_fallback)
            .expect("非兜底标签")
            .id;

        // 换兜底：新标签顶上，旧兜底让位，颜色固定灰色，组内只有一个兜底。
        store
            .set_group_label_fallback(group_id, candidate, true)
            .expect("设为兜底");
        let moved = store.get_mail_label(candidate).expect("查新兜底").expect("在");
        assert!(moved.is_fallback);
        assert_eq!(moved.color, "gray");
        let previous = store.get_mail_label(original.id).expect("查旧兜底").expect("在");
        assert!(!previous.is_fallback);
        let fallback_count = store
            .list_group_labels(group_id)
            .expect("组内标签")
            .iter()
            .filter(|label| label.is_fallback)
            .count();
        assert_eq!(fallback_count, 1);

        // 取消兜底：该组不再有兜底。
        store
            .set_group_label_fallback(group_id, candidate, false)
            .expect("取消兜底");
        assert!(store.fallback_group_label(group_id).expect("查兜底").is_none());
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

    fn seeded_messages(store: &Store, uids: &[u32]) -> Vec<i64> {
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
        let account_id = store.insert_account(&draft, None).expect("插入账号").0;
        let folder_id = store
            .upsert_folder(account_id, "INBOX", "/", mail_domain::FolderKind::Inbox)
            .expect("插入文件夹");
        let batch: Vec<crate::sync::NewMessage> = uids
            .iter()
            .map(|uid| crate::sync::NewMessage {
                account_id,
                folder_id,
                uid: *uid,
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
            })
            .collect();
        store.insert_messages(&batch).expect("插入邮件");
        let conn = store.conn();
        uids.iter()
            .map(|uid| {
                conn.query_row(
                    "SELECT id FROM message WHERE uid = ?1",
                    [i64::from(*uid)],
                    |row| row.get(0),
                )
                .expect("读邮件编号")
            })
            .collect()
    }

    fn label_id(store: &Store, name: &str) -> i64 {
        store
            .list_mail_labels()
            .expect("标签列表")
            .into_iter()
            .find(|label| label.name == name)
            .expect("找到标签")
            .id
    }

    #[test]
    fn setting_a_label_replaces_the_previous_one() {
        let store = store();
        let message = seeded_messages(&store, &[100])[0];
        let customer = label_id(&store, "客户");
        let bill = label_id(&store, "账单");

        store
            .set_message_label(message, customer, MailLabelSource::Auto, Some(0.9))
            .expect("贴第一个标签");
        let first = store.get_message_label(message).expect("查").expect("有标签");
        assert_eq!(first.0.name, "客户");
        assert_eq!(first.2, MailLabelSource::Auto);
        assert_eq!(first.1, Some(0.9));

        store
            .set_message_label(message, bill, MailLabelSource::Manual, None)
            .expect("换标签");
        let second = store.get_message_label(message).expect("查").expect("有标签");
        assert_eq!(second.0.name, "账单");
        assert_eq!(second.2, MailLabelSource::Manual);

        let rows: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM message_label WHERE message_id = ?1",
                [message],
                |row| row.get(0),
            )
            .expect("数标签行");
        assert_eq!(rows, 1, "一封邮件只能有一行标签");
    }

    #[test]
    fn deleting_a_label_clears_its_message_rows() {
        let store = store();
        let message = seeded_messages(&store, &[200])[0];
        let custom = store.create_mail_label("项目").expect("新增标签");
        store
            .set_message_label(message, custom, MailLabelSource::Auto, Some(0.8))
            .expect("贴标签");

        store.delete_mail_label(custom).expect("删标签");
        assert!(store.get_message_label(message).expect("查").is_none());
    }

    #[test]
    fn deleting_a_message_clears_its_label_row() {
        let store = store();
        let message = seeded_messages(&store, &[300])[0];
        let bill = label_id(&store, "账单");
        store
            .set_message_label(message, bill, MailLabelSource::Manual, None)
            .expect("贴标签");

        store
            .conn()
            .execute("DELETE FROM message WHERE id = ?1", [message])
            .expect("删邮件");
        let rows: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM message_label", [], |row| row.get(0))
            .expect("数标签行");
        assert_eq!(rows, 0);
    }

    #[test]
    fn clearing_a_label_reports_whether_it_existed() {
        let store = store();
        let message = seeded_messages(&store, &[400])[0];
        let bill = label_id(&store, "账单");
        assert!(!store.clear_message_label(message).expect("没标签时清"));
        store
            .set_message_label(message, bill, MailLabelSource::Auto, None)
            .expect("贴标签");
        assert!(store.clear_message_label(message).expect("有标签时清"));
        assert!(store.get_message_label(message).expect("查").is_none());
    }

    #[test]
    fn list_message_ids_with_label_matches_by_name() {
        let store = store();
        let messages = seeded_messages(&store, &[500, 501]);
        let bill = label_id(&store, "账单");
        store
            .set_message_label(messages[0], bill, MailLabelSource::Auto, None)
            .expect("贴第一封");

        let found = store.list_message_ids_with_label("账单").expect("按标签取邮件");
        assert_eq!(found, vec![messages[0]]);
        assert!(store
            .list_message_ids_with_label("不存在的标签")
            .expect("不存在")
            .is_empty());
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

    fn default_group_id(store: &Store, name: &str) -> i64 {
        store
            .list_label_groups()
            .expect("标签组列表")
            .into_iter()
            .find(|group| group.name == name)
            .expect("找到标签组")
            .id
    }

    fn group_label_id(store: &Store, group_id: i64, name: &str) -> i64 {
        store
            .list_group_labels(group_id)
            .expect("组内标签列表")
            .into_iter()
            .find(|label| label.name == name)
            .expect("找到组内标签")
            .id
    }

    #[test]
    fn message_group_result_round_trip() {
        let store = store();
        let message = seeded_messages(&store, &[600])[0];
        let content = default_group_id(&store, "内容分类");
        let bill = group_label_id(&store, content, "账单");

        store
            .set_message_group_value(
                message,
                content,
                Some(bill),
                None,
                MailLabelSource::Auto,
                Some(0.9),
            )
            .expect("写内容分类结果");

        let found = store
            .message_group_result(message, content)
            .expect("读结果")
            .expect("有结果");
        assert_eq!(found.group.id, content);
        assert_eq!(found.label.as_ref().expect("标签").name, "账单");
        assert_eq!(found.score_value, None);
        assert_eq!(found.source, MailLabelSource::Auto);
        assert_eq!(found.confidence, Some(0.9));

        let all = store.list_message_group_results(message).expect("列出全部组结果");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].group.id, content);

        assert!(store
            .clear_message_group_value(message, content)
            .expect("清空结果"));
        assert!(store
            .message_group_result(message, content)
            .expect("再读")
            .is_none());
    }

    #[test]
    fn score_group_result_stores_number() {
        let store = store();
        let message = seeded_messages(&store, &[601])[0];
        let urgency = default_group_id(&store, "紧急程度");

        store
            .set_message_group_value(
                message,
                urgency,
                None,
                Some(3),
                MailLabelSource::Manual,
                Some(0.8),
            )
            .expect("写 3 分");

        let found = store
            .message_group_result(message, urgency)
            .expect("读结果")
            .expect("有结果");
        assert_eq!(found.score_value, Some(3));
        assert_eq!(found.label, None);
        assert_eq!(found.source, MailLabelSource::Manual);

        assert!(matches!(
            store
                .set_message_group_value(message, urgency, None, Some(9), MailLabelSource::Manual, None,)
                .expect_err("9 分越界"),
            StoreError::InvalidData(_)
        ));
    }

    #[test]
    fn deleting_group_clears_results() {
        let store = store();
        let message = seeded_messages(&store, &[602])[0];
        let group = store
            .create_label_group("临时维度", LabelGroupKind::Choice, None, None)
            .expect("建组");
        let label = store.create_group_label(group, "临时标签").expect("建标签");
        store
            .set_message_group_value(message, group, Some(label), None, MailLabelSource::Manual, None)
            .expect("写结果");
        assert!(store
            .message_group_result(message, group)
            .expect("读结果")
            .is_some());

        store.delete_label_group(group).expect("删组");

        let rows: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM message_label WHERE group_id = ?1",
                [group],
                |row| row.get(0),
            )
            .expect("数组结果");
        assert_eq!(rows, 0);
        assert!(store.get_mail_label(label).expect("查标签").is_none());
    }
    #[test]
    fn label_group_crud_and_uniqueness() {
        let store = store();

        let groups = store.list_label_groups().expect("初始组");
        assert_eq!(groups.len(), 3, "应有三个默认组");
        assert_eq!(groups[0].name, "内容分类");
        assert_eq!(groups[0].kind, LabelGroupKind::Choice);

        let id = store
            .create_label_group("项目维度", LabelGroupKind::Choice, None, None)
            .expect("建组");
        let created = store
            .list_label_groups()
            .expect("再查")
            .into_iter()
            .find(|group| group.id == id)
            .expect("新组应在列表里");
        assert_eq!(created.name, "项目维度");
        assert_eq!(created.kind, LabelGroupKind::Choice);
        assert!(!created.enabled, "新组默认关闭");
        assert_eq!(created.confidence_threshold, 0.75);
        assert_eq!(created.score_min, None);
        assert_eq!(created.score_max, None);
        assert_eq!(created.sort_order, 40, "新组应排在最后");

        assert!(matches!(
            store
                .create_label_group("项目维度", LabelGroupKind::Choice, None, None)
                .expect_err("重名组应被拒"),
            StoreError::InvalidData(_)
        ));
        assert!(matches!(
            store
                .create_label_group("内容分类", LabelGroupKind::Choice, None, None)
                .expect_err("与默认组重名应被拒"),
            StoreError::InvalidData(_)
        ));

        store.rename_label_group(id, "项目类型").expect("改名");
        let renamed = store
            .list_label_groups()
            .expect("查改名")
            .into_iter()
            .find(|group| group.id == id)
            .expect("改名后仍在");
        assert_eq!(renamed.name, "项目类型");

        store.set_label_group_enabled(id, true).expect("开组");
        let enabled = store
            .list_label_groups()
            .expect("查开关")
            .into_iter()
            .find(|group| group.id == id)
            .expect("组在")
            .enabled;
        assert!(enabled);
        store.set_label_group_enabled(id, false).expect("关组");

        store.set_label_group_threshold(id, 0.6).expect("改门槛");
        let threshold = store
            .list_label_groups()
            .expect("查门槛")
            .into_iter()
            .find(|group| group.id == id)
            .expect("组在")
            .confidence_threshold;
        assert_eq!(threshold, 0.6);
        assert!(matches!(
            store.set_label_group_threshold(id, 0.4).expect_err("门槛太低"),
            StoreError::InvalidData(_)
        ));
        assert!(matches!(
            store.set_label_group_threshold(id, 1.0).expect_err("门槛太高"),
            StoreError::InvalidData(_)
        ));

        store.delete_label_group(id).expect("删组");
        assert!(store
            .list_label_groups()
            .expect("删后")
            .iter()
            .all(|group| group.id != id));

        let ids: Vec<i64> = store
            .list_label_groups()
            .expect("组")
            .iter()
            .map(|group| group.id)
            .collect();
        assert!(matches!(
            store.reorder_label_groups(&ids[..2]).expect_err("少传应被拒"),
            StoreError::InvalidData(_)
        ));
        let mut reversed = ids.clone();
        reversed.reverse();
        store.reorder_label_groups(&reversed).expect("排序");
        let after = store.list_label_groups().expect("查排序");
        assert_eq!(after.iter().map(|group| group.id).collect::<Vec<_>>(), reversed);
    }

    #[test]
    fn score_group_validates_range() {
        let store = store();

        assert!(matches!(
            store
                .create_label_group("坏打分", LabelGroupKind::Score, Some(5), Some(1))
                .expect_err("上下限倒挂应被拒"),
            StoreError::InvalidData(_)
        ));
        assert!(matches!(
            store
                .create_label_group("坏打分", LabelGroupKind::Score, Some(3), Some(3))
                .expect_err("上下限相等应被拒"),
            StoreError::InvalidData(_)
        ));
        assert!(matches!(
            store
                .create_label_group("坏打分", LabelGroupKind::Score, Some(1), Some(11))
                .expect_err("档位超过十条应被拒"),
            StoreError::InvalidData(_)
        ));
        assert!(matches!(
            store
                .create_label_group("坏打分", LabelGroupKind::Score, Some(1), None)
                .expect_err("只给下限应被拒"),
            StoreError::InvalidData(_)
        ));

        let id = store
            .create_label_group("打分维度", LabelGroupKind::Score, Some(1), Some(5))
            .expect("1 到 5 应通过");
        let group = store
            .list_label_groups()
            .expect("组")
            .into_iter()
            .find(|group| group.id == id)
            .expect("在");
        assert_eq!(group.kind, LabelGroupKind::Score);
        assert_eq!(group.score_min, Some(1));
        assert_eq!(group.score_max, Some(5));

        assert!(matches!(
            store
                .create_label_group("坏选择", LabelGroupKind::Choice, Some(1), Some(5))
                .expect_err("选择题不该有上下限"),
            StoreError::InvalidData(_)
        ));
        assert!(matches!(
            store
                .create_label_group("坏是否", LabelGroupKind::Boolean, Some(1), Some(5))
                .expect_err("是否题不该有上下限"),
            StoreError::InvalidData(_)
        ));
    }

    #[test]
    fn group_name_rejects_colon() {
        let store = store();
        assert!(matches!(
            store
                .create_label_group("项目:类型", LabelGroupKind::Choice, None, None)
                .expect_err("含英文冒号应被拒"),
            StoreError::InvalidData(_)
        ));
        assert!(matches!(
            store
                .create_label_group("   ", LabelGroupKind::Choice, None, None)
                .expect_err("空名应被拒"),
            StoreError::InvalidData(_)
        ));
        let long = "很长".repeat(40);
        assert!(matches!(
            store
                .create_label_group(&long, LabelGroupKind::Choice, None, None)
                .expect_err("超长应被拒"),
            StoreError::InvalidData(_)
        ));

        let id = store
            .create_label_group("正常组", LabelGroupKind::Choice, None, None)
            .expect("建组");
        assert!(matches!(
            store
                .rename_label_group(id, "改:名")
                .expect_err("改名含冒号应被拒"),
            StoreError::InvalidData(_)
        ));
    }
}
