-- 0009_mcp：Wave 8 外部 Agent 接入（MCP）的审计与开关。
-- 说明：
--   mcp_audit 只记工具名、账号范围、参数摘要（哈希）与状态，绝不落正文、凭据或原始参数；
--   MCP 开关复用通用 setting 表：mcp.enabled = 0/1，mcp.write_tools_enabled = 0/1；
--   写工具（建草稿）默认关闭，发送类工具不进入 v1 工具集。
-- 安全：本表只用于审计与排查，内容来自本地生成，不来自不可信的邮件正文。

CREATE TABLE IF NOT EXISTS mcp_audit (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    tool          TEXT NOT NULL,
    account_scope TEXT NOT NULL DEFAULT 'all',
    args_digest   TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL,
    ts            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_mcp_audit_ts ON mcp_audit (ts DESC, id DESC);

INSERT OR IGNORE INTO setting (key, value) VALUES ('mcp.enabled', '0');
INSERT OR IGNORE INTO setting (key, value) VALUES ('mcp.write_tools_enabled', '0');