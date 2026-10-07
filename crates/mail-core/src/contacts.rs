//! 通讯录：联系人与分组的编排、校验，以及 CSV / vCard 的解析与生成。
//!
//! 口径（见 `docs/superpowers/specs/2026-10-06-contacts-workspace-design.md` v1.2）：
//! - 一份共用的通讯录，一个邮箱一条；写信补全也读这一份，并排除已隐藏的人；
//! - 导入导出只用用户亲手在系统对话框里选定的文件，不联网、不自动读写任何路径；
//! - 备注是纯文本，只存本地，不参与任何外发。

use std::fs;

use mail_store::{
    ContactDraft, ContactImportRow, ContactScope, StoredContact, StoredContactGroup, MAX_CONTACT_ROWS,
    MAX_GROUP_NAME_CHARS, MAX_NOTE_CHARS,
};

use crate::compose::lock_store;
use crate::engine::{EngineError, MailEngine};

/// 收件人自动补全一次最多返回多少条。
const MAX_CONTACT_HITS: usize = 20;

/// 导入文件大小上限（字节）。
pub const CONTACT_IMPORT_MAX_BYTES: usize = 5 * 1024 * 1024;

/// 一次导入最多多少条。
pub const CONTACT_IMPORT_MAX_ENTRIES: usize = 5000;

/// 预览里最多列几条坏行。
pub const CONTACT_IMPORT_MAX_PROBLEMS: usize = 20;

/// 导出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactExportKind {
    /// 逗号分隔，带 UTF-8 BOM。
    Csv,
    /// vCard 3.0。
    Vcf,
}

/// 导入时撞上已有邮箱怎么办。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactImportMode {
    /// 跳过（默认）。
    Skip,
    /// 用文件里的名字与备注覆盖。
    Overwrite,
}

/// 一条导入条目。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactImportEntry {
    /// 显示名。
    pub name: String,
    /// 邮箱。
    pub email: String,
    /// 备注。
    pub note: String,
    /// 分组名，空串表示未分组。
    pub group: String,
}

/// 一条读不出来的行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactProblem {
    /// 行号，从 1 开始（vCard 用卡片序号）。
    pub line: usize,
    /// 原因（中文，给用户看）。
    pub reason: String,
}

/// 导入预览。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactImportPreview {
    /// CSV 表头；不是 CSV 时为空。
    pub headers: Vec<String>,
    /// 认出来的邮箱列下标。
    pub email_column: Option<usize>,
    /// 解析出来的有效条目。
    pub entries: Vec<ContactImportEntry>,
    /// 读不出来的行（最多列 `CONTACT_IMPORT_MAX_PROBLEMS` 条）。
    pub problems: Vec<ContactProblem>,
    /// 有效条目里，邮箱库里已经有的有几条。
    pub duplicate_count: usize,
    /// 有效条目里，邮箱库里还没有的有几条。
    pub new_count: usize,
}

/// 导入落库结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContactImportOutcome {
    /// 新建几条。
    pub imported: usize,
    /// 跳过几条。
    pub skipped: usize,
    /// 覆盖几条。
    pub overwritten: usize,
    /// 坏行几条。
    pub invalid: usize,
    /// 顺手建了几个分组。
    pub groups_created: usize,
}

/// 导出结果：文本 + 导出了几条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactExport {
    /// 要写进文件的文本。
    pub text: String,
    /// 导出了几条联系人。
    pub count: usize,
}

/// 联系人条数快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContactCounts {
    /// 正常列表里的条数。
    pub active: i64,
    /// 已隐藏的条数。
    pub hidden: i64,
    /// 未分组的条数。
    pub ungrouped: i64,
}

/// 校验一个邮箱地址能不能收；只管格式空不空，不联网。
pub fn validate_contact_email(email: &str) -> Result<String, EngineError> {
    let email = email.trim();
    if email.is_empty() {
        return Err(EngineError::BadRequest("邮箱不能为空".to_string()));
    }
    if email.chars().any(char::is_whitespace) {
        return Err(EngineError::BadRequest(format!("邮箱里不能有空格：{email}")));
    }
    let mut parts = email.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if local.is_empty() || domain.is_empty() || parts.next().is_some() || !domain.contains('.') {
        return Err(EngineError::BadRequest(format!("邮箱格式不对：{email}")));
    }
    Ok(email.to_string())
}

/// 校验并整理一份联系人草稿。
fn normalize_draft(draft: &ContactDraft) -> Result<ContactDraft, EngineError> {
    let email = validate_contact_email(&draft.email)?;
    let note = draft.note.trim().to_string();
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err(EngineError::BadRequest(format!(
            "备注太长了，最多 {MAX_NOTE_CHARS} 个字"
        )));
    }
    Ok(ContactDraft {
        name: draft.name.trim().to_string(),
        email,
        note,
        group_id: draft.group_id,
    })
}

/// 校验分组名。
fn normalize_group_name(name: &str) -> Result<String, EngineError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(EngineError::BadRequest("分组名不能为空".to_string()));
    }
    if name.chars().count() > MAX_GROUP_NAME_CHARS {
        return Err(EngineError::BadRequest(format!(
            "分组名太长了，最多 {MAX_GROUP_NAME_CHARS} 个字"
        )));
    }
    Ok(name.to_string())
}

/// 把库里存的字符串换成导入格式。
fn to_entry(row: &StoredContact) -> ContactImportEntry {
    ContactImportEntry {
        name: row.name.clone(),
        email: row.email.clone(),
        note: row.note.clone(),
        group: row.group_name.clone().unwrap_or_default(),
    }
}

/// CSV 字段转义：含逗号 / 引号 / 换行的要用双引号包起来，内部引号翻倍。
fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// 生成 CSV 文本（RFC 4180 子集，带 UTF-8 BOM，CRLF 换行）。
pub fn export_csv(rows: &[ContactImportEntry]) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str("显示名,邮箱,分组,备注\r\n");
    for row in rows {
        out.push_str(&csv_escape(&row.name));
        out.push(',');
        out.push_str(&csv_escape(&row.email));
        out.push(',');
        out.push_str(&csv_escape(&row.group));
        out.push(',');
        out.push_str(&csv_escape(&row.note));
        out.push_str("\r\n");
    }
    out
}

/// vCard 的值转义。
fn vcard_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace(['\r', '\n'], "\\n")
}

/// vCard 的折行：一行不超过 75 个字节，续行以空格开头。
fn vcard_fold(line: &str) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for ch in line.chars() {
        let width = ch.len_utf8();
        if used + width > 73 {
            out.push_str("\r\n ");
            used = 1;
        }
        out.push(ch);
        used += width;
    }
    out
}

/// 生成 vCard 3.0 文本。
pub fn export_vcard(rows: &[ContactImportEntry]) -> String {
    let mut out = String::new();
    for row in rows {
        out.push_str(&vcard_fold("BEGIN:VCARD"));
        out.push_str("\r\n");
        out.push_str(&vcard_fold("VERSION:3.0"));
        out.push_str("\r\n");
        let display = if row.name.trim().is_empty() {
            row.email.trim()
        } else {
            row.name.trim()
        };
        out.push_str(&vcard_fold(&format!("FN:{}", vcard_escape(display))));
        out.push_str("\r\n");
        out.push_str(&vcard_fold(&format!("EMAIL:{}", vcard_escape(row.email.trim()))));
        out.push_str("\r\n");
        if !row.note.trim().is_empty() {
            out.push_str(&vcard_fold(&format!("NOTE:{}", vcard_escape(row.note.trim()))));
            out.push_str("\r\n");
        }
        if !row.group.trim().is_empty() {
            out.push_str(&vcard_fold(&format!(
                "CATEGORIES:{}",
                vcard_escape(row.group.trim())
            )));
            out.push_str("\r\n");
        }
        out.push_str(&vcard_fold("END:VCARD"));
        out.push_str("\r\n");
    }
    out
}

/// 按内容猜文件类型：以 `BEGIN:VCARD` 开头就算 vCard。
pub fn guess_kind(text: &str) -> ContactExportKind {
    if text.trim_start().to_uppercase().starts_with("BEGIN:VCARD") {
        ContactExportKind::Vcf
    } else {
        ContactExportKind::Csv
    }
}

/// 把 CSV 文本拆成一行行字段（RFC 4180 子集：双引号包裹、内部引号翻倍、字段里可以有换行）。
fn parse_csv_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(ch);
            }
            continue;
        }
        match ch {
            '"' => in_quotes = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(ch),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    // 整行都是空的直接丢掉。
    rows.retain(|item| item.iter().any(|cell| !cell.trim().is_empty()));
    rows
}

/// 按表头猜邮箱在第几列。
fn guess_email_column(headers: &[String]) -> Option<usize> {
    headers.iter().position(|item| {
        let head = item.trim().to_lowercase();
        head.contains("email") || head.contains("mail") || head.contains("邮箱") || head.contains("邮件")
    })
}

/// 解析 CSV。
fn parse_csv(text: &str, email_column: Option<usize>) -> ContactImportPreview {
    let mut preview = ContactImportPreview::default();
    let rows = parse_csv_rows(text);
    let Some(headers) = rows.first().cloned() else {
        return preview;
    };
    preview.headers = headers.clone();
    let column = email_column.or_else(|| guess_email_column(&headers));
    let Some(column) = column else {
        preview.problems.push(ContactProblem {
            line: 1,
            reason: "没找到邮箱列，请手动指定哪一列是邮箱".to_string(),
        });
        return preview;
    };
    preview.email_column = Some(column);

    for (index, row) in rows.iter().enumerate().skip(1) {
        let line = index + 1;
        let email = row.get(column).map(|value| value.trim()).unwrap_or_default();
        if email.is_empty() {
            preview.problems.push(ContactProblem {
                line,
                reason: "邮箱为空".to_string(),
            });
            continue;
        }
        let name = row
            .first()
            .map(|value| value.trim())
            .filter(|value| *value != email);
        let note = row
            .iter()
            .skip(column + 1)
            .last()
            .map(|value| value.trim())
            .unwrap_or_default();
        preview.entries.push(ContactImportEntry {
            name: name.unwrap_or_default().to_string(),
            email: email.to_string(),
            note: note.to_string(),
            group: guess_group(&headers, row),
        });
    }
    preview
}

/// 表头里带「分组 / 组 / group / categories」的那一列。
fn guess_group(headers: &[String], row: &[String]) -> String {
    headers
        .iter()
        .position(|head| {
            let head = head.trim().to_lowercase();
            head.contains("group") || head.contains("categories") || head.contains("分组") || head == "组"
        })
        .and_then(|index| row.get(index))
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// vCard 的续行拼回上一行。
fn unfold_vcard(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = out.last_mut() {
                last.push_str(&line[1..]);
            }
            continue;
        }
        out.push(line.to_string());
    }
    out
}

/// vCard 的值反转义。
fn vcard_unescape(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// 解析 vCard 3.0 的常用子集：FN / N / EMAIL / NOTE / CATEGORIES。
fn parse_vcard(text: &str) -> ContactImportPreview {
    let mut preview = ContactImportPreview::default();
    let mut current: Option<ContactImportEntry> = None;
    let mut card_index = 0usize;

    for line in unfold_vcard(text) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let upper = trimmed.to_uppercase();
        if upper.starts_with("BEGIN:VCARD") {
            card_index += 1;
            current = Some(ContactImportEntry::default());
            continue;
        }
        if upper.starts_with("END:VCARD") {
            if let Some(entry) = current.take() {
                if entry.email.trim().is_empty() {
                    preview.problems.push(ContactProblem {
                        line: card_index,
                        reason: "这张卡片没有邮箱".to_string(),
                    });
                } else {
                    preview.entries.push(entry);
                }
            }
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        let Some((raw_name, raw_value)) = trimmed.split_once(':') else {
            continue;
        };
        let field = raw_name
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_uppercase();
        let value = vcard_unescape(raw_value.trim());
        match field.as_str() {
            "FN" if entry.name.is_empty() => entry.name = value,
            "N" if entry.name.is_empty() => {
                entry.name = value
                    .split(';')
                    .filter(|piece| !piece.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            "EMAIL" if entry.email.is_empty() => entry.email = value,
            "NOTE" => entry.note = value,
            "CATEGORIES" if entry.group.is_empty() => {
                entry.group = value.split(',').next().unwrap_or_default().trim().to_string()
            }
            _ => {}
        }
    }

    if let Some(entry) = current {
        if entry.email.trim().is_empty() {
            preview.problems.push(ContactProblem {
                line: card_index,
                reason: "这张卡片没有邮箱".to_string(),
            });
        } else {
            preview.entries.push(entry);
        }
    }
    preview
}

/// 按类型解析联系人文件内容。
pub fn parse_contacts(
    kind: ContactExportKind,
    text: &str,
    email_column: Option<usize>,
) -> ContactImportPreview {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    match kind {
        ContactExportKind::Csv => parse_csv(body, email_column),
        ContactExportKind::Vcf => parse_vcard(body),
    }
}

impl MailEngine {
    /// 通讯录列表。
    pub fn list_contacts(
        &self,
        keyword: &str,
        limit: usize,
        scope: ContactScope,
    ) -> Result<Vec<StoredContact>, EngineError> {
        let store = lock_store(&self.store);
        Ok(store.list_contacts(keyword, limit.min(MAX_CONTACT_ROWS), scope)?)
    }

    /// 收件人补全：只搜没被隐藏的人。
    pub fn search_contacts(&self, keyword: &str, limit: usize) -> Result<Vec<StoredContact>, EngineError> {
        let store = lock_store(&self.store);
        Ok(store.list_contacts(keyword, limit.min(MAX_CONTACT_HITS), ContactScope::Active)?)
    }

    /// 通讯录的条数快照。
    pub fn contact_counts(&self) -> Result<ContactCounts, EngineError> {
        let store = lock_store(&self.store);
        Ok(ContactCounts {
            active: store.count_contacts(ContactScope::Active)?,
            hidden: store.count_contacts(ContactScope::Hidden)?,
            ungrouped: store.count_ungrouped_contacts()?,
        })
    }

    /// 全部分组。
    pub fn list_contact_groups(&self) -> Result<Vec<StoredContactGroup>, EngineError> {
        let store = lock_store(&self.store);
        Ok(store.list_contact_groups()?)
    }

    /// 新建联系人。
    pub fn create_contact(&self, draft: &ContactDraft) -> Result<i64, EngineError> {
        let draft = normalize_draft(draft)?;
        let store = lock_store(&self.store);
        Ok(store.create_contact(&draft)?)
    }

    /// 修改联系人。
    pub fn update_contact(&self, id: i64, draft: &ContactDraft) -> Result<(), EngineError> {
        let draft = normalize_draft(draft)?;
        let store = lock_store(&self.store);
        if !store.update_contact(id, &draft)? {
            return Err(EngineError::BadRequest("这位联系人已经不在了".to_string()));
        }
        Ok(())
    }

    /// 隐藏一位联系人。
    pub fn hide_contact(&self, id: i64) -> Result<(), EngineError> {
        self.set_contact_hidden(id, true)
    }

    /// 把一位已隐藏的联系人放回来。
    pub fn restore_contact(&self, id: i64) -> Result<(), EngineError> {
        self.set_contact_hidden(id, false)
    }

    fn set_contact_hidden(&self, id: i64, hidden: bool) -> Result<(), EngineError> {
        let store = lock_store(&self.store);
        let changed = if hidden {
            store.hide_contact(id)?
        } else {
            store.restore_contact(id)?
        };
        if !changed {
            return Err(EngineError::BadRequest("这位联系人已经不在了".to_string()));
        }
        Ok(())
    }

    /// 彻底删掉一位联系人。
    pub fn purge_contact(&self, id: i64) -> Result<(), EngineError> {
        let store = lock_store(&self.store);
        if !store.purge_contact(id)? {
            return Err(EngineError::BadRequest("这位联系人已经不在了".to_string()));
        }
        Ok(())
    }

    /// 新建分组。
    pub fn create_contact_group(&self, name: &str) -> Result<i64, EngineError> {
        let name = normalize_group_name(name)?;
        let store = lock_store(&self.store);
        Ok(store.create_contact_group(&name)?)
    }

    /// 给分组改名。
    pub fn rename_contact_group(&self, id: i64, name: &str) -> Result<(), EngineError> {
        let name = normalize_group_name(name)?;
        let store = lock_store(&self.store);
        if !store.rename_contact_group(id, &name)? {
            return Err(EngineError::BadRequest("这个分组已经不在了".to_string()));
        }
        Ok(())
    }

    /// 删分组；组内联系人回到未分组。
    pub fn delete_contact_group(&self, id: i64) -> Result<(), EngineError> {
        let store = lock_store(&self.store);
        if !store.delete_contact_group(id)? {
            return Err(EngineError::BadRequest("这个分组已经不在了".to_string()));
        }
        Ok(())
    }

    /// 清空自动收集的联系人（手动的和已隐藏的都不动）。
    pub fn clear_auto_contacts(&self) -> Result<usize, EngineError> {
        let store = lock_store(&self.store);
        Ok(store.clear_auto_contacts()?)
    }

    /// 导出联系人，返回要写进文件的文本与条数。
    pub fn export_contacts(
        &self,
        kind: ContactExportKind,
        scope: ContactScope,
    ) -> Result<ContactExport, EngineError> {
        let rows = self.list_contacts("", MAX_CONTACT_ROWS, scope)?;
        let entries: Vec<ContactImportEntry> = rows.iter().map(to_entry).collect();
        let text = match kind {
            ContactExportKind::Csv => export_csv(&entries),
            ContactExportKind::Vcf => export_vcard(&entries),
        };
        Ok(ContactExport {
            text,
            count: entries.len(),
        })
    }

    /// 读一个导入文件并解析出预览；这一步只读文件，不写库。
    pub fn preview_contact_import(
        &self,
        path: &str,
        email_column: Option<usize>,
    ) -> Result<ContactImportPreview, EngineError> {
        let path = path.trim();
        if path.is_empty() {
            return Err(EngineError::BadRequest("没有选文件".to_string()));
        }
        let meta = fs::metadata(path)
            .map_err(|error| EngineError::BadRequest(format!("打不开这个文件：{error}")))?;
        if meta.len() as usize > CONTACT_IMPORT_MAX_BYTES {
            return Err(EngineError::BadRequest(format!(
                "文件太大了，最多 {} MB",
                CONTACT_IMPORT_MAX_BYTES / 1024 / 1024
            )));
        }
        let bytes =
            fs::read(path).map_err(|error| EngineError::BadRequest(format!("读不了这个文件：{error}")))?;
        let text = String::from_utf8(bytes).map_err(|_| {
            EngineError::BadRequest(
                "这个文件不是 UTF-8 编码，暂时读不了；请在导出方另存为 UTF-8 再试".to_string(),
            )
        })?;

        let kind = guess_kind(&text);
        let mut preview = parse_contacts(kind, &text, email_column);
        if preview.entries.len() > CONTACT_IMPORT_MAX_ENTRIES {
            return Err(EngineError::BadRequest(format!(
                "文件里联系人太多了，一次最多导 {CONTACT_IMPORT_MAX_ENTRIES} 条"
            )));
        }
        if preview.problems.len() > CONTACT_IMPORT_MAX_PROBLEMS {
            preview.problems.truncate(CONTACT_IMPORT_MAX_PROBLEMS);
        }

        let store = lock_store(&self.store);
        let mut duplicate = 0usize;
        for entry in &preview.entries {
            let existing = store
                .list_contacts(entry.email.trim(), MAX_CONTACT_ROWS, ContactScope::Active)?
                .into_iter()
                .any(|item| item.email.eq_ignore_ascii_case(entry.email.trim()));
            if existing {
                duplicate += 1;
            }
        }
        preview.duplicate_count = duplicate;
        preview.new_count = preview.entries.len().saturating_sub(duplicate);
        Ok(preview)
    }

    /// 把预览里确认过的条目落库（整批一个事务）。
    pub fn apply_contact_import(
        &self,
        entries: Vec<ContactImportEntry>,
        mode: ContactImportMode,
    ) -> Result<ContactImportOutcome, EngineError> {
        if entries.len() > CONTACT_IMPORT_MAX_ENTRIES {
            return Err(EngineError::BadRequest(format!(
                "一次最多导 {CONTACT_IMPORT_MAX_ENTRIES} 条"
            )));
        }
        let mut rows = Vec::with_capacity(entries.len());
        let mut invalid = 0usize;
        for entry in &entries {
            let email = entry.email.trim();
            if email.is_empty() || validate_contact_email(email).is_err() {
                invalid += 1;
                continue;
            }
            rows.push(ContactImportRow {
                name: entry.name.trim().to_string(),
                email: email.to_string(),
                note: entry.note.trim().chars().take(MAX_NOTE_CHARS).collect(),
                group: Some(entry.group.trim().to_string()).filter(|value| !value.is_empty()),
            });
        }
        let store = lock_store(&self.store);
        let stats = store.import_contacts(&rows, mode == ContactImportMode::Overwrite)?;
        Ok(ContactImportOutcome {
            imported: stats.imported,
            skipped: stats.skipped,
            overwritten: stats.overwritten,
            invalid,
            groups_created: stats.groups_created,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一条导出用的人。
    fn person(name: &str, email: &str, group: &str, note: &str) -> ContactImportEntry {
        ContactImportEntry {
            name: name.to_string(),
            email: email.to_string(),
            group: group.to_string(),
            note: note.to_string(),
        }
    }

    #[test]
    fn csv导出带字节顺序标记并且字段会转义() {
        let rows = vec![person("张三", "z@example.com", "客户", "带,逗号\n和换行")];
        let text = export_csv(&rows);
        assert!(text.starts_with('\u{feff}'), "开头要有 BOM");
        assert!(text.contains("\r\n"), "行尾用 CRLF");
        assert!(text.contains("\"带,逗号"), "含逗号的字段要被引号包起来");

        let back = parse_contacts(ContactExportKind::Csv, &text, None);
        assert!(back.problems.is_empty(), "不该有坏行：{:?}", back.problems);
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].name, "张三");
        assert_eq!(back.entries[0].email, "z@example.com");
        assert_eq!(back.entries[0].group, "客户");
        assert_eq!(back.entries[0].note, "带,逗号\n和换行");
    }

    #[test]
    fn csv认得引号翻倍与字段里的换行() {
        let text = "name,email,note\n\"李四\",\"l@example.com\",\"说了\"\"你好\"\"\n然后走了\"\n";
        let parsed = parse_contacts(ContactExportKind::Csv, text, None);
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].name, "李四");
        assert_eq!(parsed.entries[0].note, "说了\"你好\"\n然后走了");
    }

    #[test]
    fn csv邮箱为空的行走坏行而不是静默丢掉() {
        let text = "name,email\n张三,z@example.com\n没邮箱,\n,,\n";
        let parsed = parse_contacts(ContactExportKind::Csv, text, None);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.problems.len(), 1, "只该有一条坏行");
        assert_eq!(parsed.problems[0].line, 3);
        assert!(parsed.problems[0].reason.contains("邮箱"));
    }

    #[test]
    fn csv能按列号指定邮箱列() {
        let text = "名字,邮箱,分组\n张三,z@example.com,客户\n";
        let parsed = parse_contacts(ContactExportKind::Csv, text, Some(1));
        assert_eq!(parsed.email_column, Some(1));
        assert_eq!(parsed.entries[0].email, "z@example.com");
        assert_eq!(parsed.entries[0].group, "客户");
    }

    #[test]
    fn vcard能解析折行转义与分类() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:张\\,三\r\nEMAIL:z@example.com\r\nNOTE:第一行\\n第二行\r\nCATEGORIES:客户,同事\r\nEND:VCARD\r\n";
        let parsed = parse_contacts(ContactExportKind::Vcf, text, None);
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].name, "张,三");
        assert_eq!(parsed.entries[0].note, "第一行\n第二行");
        assert_eq!(parsed.entries[0].group, "客户", "分组取第一个");
    }

    #[test]
    fn vcard折行的长备注能拼回来() {
        let rows = vec![person(
            "长备注",
            "long@example.com",
            "",
            &"这是一段很长的备注，".repeat(20),
        )];
        let text = export_vcard(&rows);
        assert!(text.contains("\r\n "), "超长行要有续行");
        let back = parse_contacts(ContactExportKind::Vcf, &text, None);
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].note, rows[0].note, "折行要能拼回原样");
    }

    #[test]
    fn vcard没有邮箱的卡片算坏行() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:没邮箱\r\nEND:VCARD\r\n";
        let parsed = parse_contacts(ContactExportKind::Vcf, text, None);
        assert!(parsed.entries.is_empty());
        assert_eq!(parsed.problems.len(), 1);
    }

    #[test]
    fn 导出再导入内容一条不差() {
        let rows = vec![
            person("张三", "z@example.com", "客户", "上次报价 3 万"),
            person("", "l@example.com", "", ""),
        ];
        let kind = guess_kind(&export_vcard(&rows));
        assert_eq!(kind, ContactExportKind::Vcf);
        let parsed = parse_contacts(kind, &export_vcard(&rows), None);
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[0].name, "张三");
        assert_eq!(parsed.entries[0].note, "上次报价 3 万");
        assert_eq!(parsed.entries[0].group, "客户");
        assert_eq!(parsed.entries[1].email, "l@example.com");
    }

    #[test]
    fn 邮箱格式校验挡住明显不对的() {
        assert!(validate_contact_email("z@example.com").is_ok());
        assert!(
            validate_contact_email("  z@example.com  ").is_ok(),
            "两边空格该被去掉"
        );
        assert!(validate_contact_email("").is_err());
        assert!(validate_contact_email("没有圈a").is_err());
        assert!(validate_contact_email("a@b").is_err(), "域名要有点");
        assert!(validate_contact_email("a b@example.com").is_err());
        assert!(validate_contact_email("a@@example.com").is_err());
    }
}
