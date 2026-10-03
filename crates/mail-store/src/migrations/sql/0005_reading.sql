-- 0005_reading：Wave 4 读信与附件表。
-- 说明：
--   message_body 缓存每封邮件懒加载的正文（纯文本 + 清洗后的 HTML），一封一行；
--   attachment 记录附件元数据与本地保存状态，原件按需下载。
-- 安全：正文与 HTML 都来自不可信邮件，写入前已经过 mail-mime 白名单清洗。
-- 状态取值固定，禁止新增：attachment.state ∈ {pending, downloading, downloaded, failed}。

CREATE TABLE IF NOT EXISTS message_body (
    message_id     INTEGER PRIMARY KEY REFERENCES message(id) ON DELETE CASCADE,
    text_plain     TEXT,
    html_sanitized TEXT,
    fetched_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE TABLE IF NOT EXISTS attachment (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id  INTEGER NOT NULL REFERENCES message(id) ON DELETE CASCADE,
    part_index  INTEGER NOT NULL,
    filename    TEXT NOT NULL DEFAULT '',
    mime_type   TEXT NOT NULL DEFAULT '',
    size        INTEGER NOT NULL DEFAULT 0,
    content_id  TEXT,
    is_inline   INTEGER NOT NULL DEFAULT 0 CHECK (is_inline IN (0, 1)),
    local_path  TEXT,
    state       TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'downloading', 'downloaded', 'failed'))
) STRICT;

-- 同一封邮件里同一个分片下标只允许一条记录，重复解析时按它做替换。
CREATE UNIQUE INDEX IF NOT EXISTS idx_attachment_message_part ON attachment (message_id, part_index);
CREATE INDEX IF NOT EXISTS idx_attachment_message ON attachment (message_id);
