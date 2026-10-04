//! MCP（外部 Agent 接入）的本地审计与开关存取（Wave 8）。
//!
//! 安全约定：
//! - 审计只存工具名、账号范围、参数摘要（哈希）与状态，绝不存正文、凭据或原始参数；
//! - 开关复用 `setting` 表的 `mcp.enabled` / `mcp.write_tools_enabled`，默认都是关闭；
//! - 本模块只读 / 写这两类数据，不碰凭据保险箱，也不接触邮件正文。

use rusqlite::OptionalExtension;

use crate::connection::Store;
use crate::error::StoreError;
use crate::inbox::{row_to_message, InboxMessage, INBOX_COLUMNS};

/// MCP 总开关的 setting 键。
pub const MCP_ENABLED_KEY: &str = "mcp.enabled";
/// MCP 写工具（建草稿）独立开关的 setting 键。
pub const MCP_WRITE_TOOLS_KEY: &str = "mcp.write_tools_enabled";

/// 一条 MCP 审计记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAuditRecord {
    /// 主键。
    pub id: i64,
    /// 工具名（例如 `search_messages`）。
    pub tool: String,
    /// 账号范围：`all` 或账号编号文本。
    pub account_scope: String,
    /// 参数摘要（SHA-256 十六进制小写）；空表示这次调用没有参数。
    pub args_digest: String,
    /// 状态：`ok` / `rejected` / `error`。
    pub status: String,
    /// 记录时间（UTC ISO-8601）。
    pub ts: String,
}

fn row_to_audit(row: &rusqlite::Row<'_>) -> rusqlite::Result<McpAuditRecord> {
    Ok(McpAuditRecord {
        id: row.get(0)?,
        tool: row.get(1)?,
        account_scope: row.get(2)?,
        args_digest: row.get(3)?,
        status: row.get(4)?,
        ts: row.get(5)?,
    })
}

impl Store {
    /// 写一条 MCP 审计。
    pub fn insert_mcp_audit(
        &self,
        tool: &str,
        account_scope: &str,
        args_digest: &str,
        status: &str,
    ) -> Result<i64, StoreError> {
        self.conn().execute(
            "INSERT INTO mcp_audit (tool, account_scope, args_digest, status) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![tool, account_scope, args_digest, status],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// 按时间倒序查最近若干条审计。
    pub fn list_mcp_audit(&self, limit: i64) -> Result<Vec<McpAuditRecord>, StoreError> {
        let limit = limit.clamp(1, 500);
        let mut stmt = self.conn().prepare(
            "SELECT id, tool, account_scope, args_digest, status, ts FROM mcp_audit \
             ORDER BY ts DESC, id DESC LIMIT ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![limit])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push(row_to_audit(row)?);
        }
        Ok(items)
    }

    /// 审计总条数。
    pub fn count_mcp_audit(&self) -> Result<i64, StoreError> {
        let count: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM mcp_audit", [], |row| row.get(0))?;
        Ok(count)
    }

    /// 清空审计记录（供用户清理历史用）。
    pub fn clear_mcp_audit(&self) -> Result<usize, StoreError> {
        Ok(self.conn().execute("DELETE FROM mcp_audit", [])?)
    }

    /// MCP 总开关是否打开；默认关闭。
    pub fn mcp_enabled(&self) -> Result<bool, StoreError> {
        Ok(self
            .get_setting(MCP_ENABLED_KEY)?
            .is_some_and(|value| value == "1"))
    }

    /// 设置 MCP 总开关。
    pub fn set_mcp_enabled(&self, enabled: bool) -> Result<(), StoreError> {
        self.set_setting(MCP_ENABLED_KEY, if enabled { "1" } else { "0" })
    }

    /// MCP 写工具开关是否打开；默认关闭。
    pub fn mcp_write_tools_enabled(&self) -> Result<bool, StoreError> {
        Ok(self
            .get_setting(MCP_WRITE_TOOLS_KEY)?
            .is_some_and(|value| value == "1"))
    }

    /// 设置 MCP 写工具开关。
    pub fn set_mcp_write_tools_enabled(&self, enabled: bool) -> Result<(), StoreError> {
        self.set_setting(MCP_WRITE_TOOLS_KEY, if enabled { "1" } else { "0" })
    }
}

/// 按编号读一封邮件的展示元数据（只读）；不存在返回 None。
///
/// 供 MCP 只读工具使用：只取收件箱视图那一组字段，不含正文、不含服务器定位。
impl Store {
    /// 读一封邮件的展示元数据。
    pub fn get_message_view(&self, message_id: i64) -> Result<Option<InboxMessage>, StoreError> {
        let sql = format!(
            "SELECT {INBOX_COLUMNS} FROM message m \
             JOIN folder f ON f.id = m.folder_id \
             JOIN account a ON a.id = m.account_id \
             WHERE m.id = ?1"
        );
        self.conn()
            .query_row(&sql, rusqlite::params![message_id], row_to_message)
            .optional()
            .map_err(StoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    fn migrated() -> Store {
        let mut store = Store::open_in_memory().expect("打开内存库");
        store.run_migrations().expect("迁移");
        store
    }

    #[test]
    fn 迁移后审计为空且开关默认关闭() {
        let store = migrated();
        assert_eq!(store.count_mcp_audit().expect("计数"), 0);
        assert!(store.list_mcp_audit(50).expect("列表").is_empty());
        assert!(!store.mcp_enabled().expect("总开关"));
        assert!(!store.mcp_write_tools_enabled().expect("写工具开关"));
    }

    #[test]
    fn 审计可按时间倒序查询并计数() {
        let store = migrated();
        store
            .insert_mcp_audit("list_accounts", "all", "aaa", "ok")
            .expect("写审计");
        store
            .insert_mcp_audit("search_messages", "1", "bbb", "ok")
            .expect("写审计");
        store
            .insert_mcp_audit("create_draft", "1", "ccc", "rejected")
            .expect("写审计");

        assert_eq!(store.count_mcp_audit().expect("计数"), 3);
        let items = store.list_mcp_audit(2).expect("列表");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].tool, "create_draft", "先返回最新一条");
        assert_eq!(items[0].status, "rejected");
        assert_eq!(items[1].tool, "search_messages");
    }

    #[test]
    fn 开关可反复切换() {
        let store = migrated();
        store.set_mcp_enabled(true).expect("打开");
        assert!(store.mcp_enabled().expect("读取"));
        store.set_mcp_write_tools_enabled(true).expect("打开写工具");
        assert!(store.mcp_write_tools_enabled().expect("读取"));
        store.set_mcp_enabled(false).expect("关闭");
        assert!(!store.mcp_enabled().expect("读取"));
    }

    #[test]
    fn 清空审计可用() {
        let store = migrated();
        store
            .insert_mcp_audit("list_accounts", "all", "", "ok")
            .expect("写审计");
        assert_eq!(store.clear_mcp_audit().expect("清空"), 1);
        assert_eq!(store.count_mcp_audit().expect("计数"), 0);
    }
}
