-- 0004_folders_messages_sync：Wave 2 文件夹、邮件与同步任务表。
-- 说明：
--   folder 保存每个邮箱的文件夹映射、UIDVALIDITY / UIDNEXT 与补齐断点（synced_min_uid）；
--   message 保存邮件元数据（信封 + 标志）；正文懒加载留给 Wave 4；
--   sync_job 供界面查看每账号每文件夹的同步进度与失败原因。
-- UID 语义：message 以 (account_id, folder_id, uid) 唯一；服务端 UIDVALIDITY 变化时，
--   由引擎先清空该文件夹的 message 再重建索引，避免把两代 UID 混在一起。

CREATE TABLE IF NOT EXISTS folder (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id     INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    full_path      TEXT NOT NULL,
    delimiter      TEXT NOT NULL DEFAULT '/',
    kind           TEXT NOT NULL CHECK (kind IN ('inbox', 'sent', 'draft', 'trash', 'junk', 'custom')),
    uidvalidity    INTEGER,
    uidnext        INTEGER,
    synced_min_uid INTEGER,
    last_sync_at   TEXT,
    unread_count   INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_folder_account_path ON folder (account_id, full_path);
CREATE INDEX IF NOT EXISTS idx_folder_account_kind ON folder (account_id, kind);

CREATE TABLE IF NOT EXISTS message (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id        INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    folder_id         INTEGER NOT NULL REFERENCES folder(id) ON DELETE CASCADE,
    uid               INTEGER NOT NULL,
    message_id_header TEXT,
    thread_key        TEXT,
    subject           TEXT NOT NULL DEFAULT '',
    from_name         TEXT NOT NULL DEFAULT '',
    from_addr         TEXT NOT NULL DEFAULT '',
    to_json           TEXT NOT NULL DEFAULT '[]',
    cc_json           TEXT NOT NULL DEFAULT '[]',
    date_utc          TEXT NOT NULL DEFAULT '',
    size              INTEGER NOT NULL DEFAULT 0,
    has_attachments   INTEGER NOT NULL DEFAULT 0 CHECK (has_attachments IN (0, 1)),
    is_read           INTEGER NOT NULL DEFAULT 0 CHECK (is_read IN (0, 1)),
    is_flagged        INTEGER NOT NULL DEFAULT 0 CHECK (is_flagged IN (0, 1)),
    is_answered       INTEGER NOT NULL DEFAULT 0 CHECK (is_answered IN (0, 1)),
    is_draft          INTEGER NOT NULL DEFAULT 0 CHECK (is_draft IN (0, 1)),
    snippet           TEXT NOT NULL DEFAULT '',
    body_state        TEXT NOT NULL DEFAULT 'none' CHECK (body_state IN ('none', 'loading', 'ready', 'failed')),
    created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_message_uid ON message (account_id, folder_id, uid);
CREATE INDEX IF NOT EXISTS idx_message_account_date ON message (account_id, date_utc DESC);
CREATE INDEX IF NOT EXISTS idx_message_folder_uid ON message (folder_id, uid);
CREATE INDEX IF NOT EXISTS idx_message_thread ON message (thread_key);
CREATE INDEX IF NOT EXISTS idx_message_header_id ON message (account_id, message_id_header);

CREATE TABLE IF NOT EXISTS sync_job (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    folder_id  INTEGER REFERENCES folder(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL CHECK (kind IN ('initial', 'incremental', 'idle', 'backfill', 'body_fetch')),
    state      TEXT NOT NULL CHECK (state IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    progress   INTEGER NOT NULL DEFAULT 0,
    error      TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_sync_job_account ON sync_job (account_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_sync_job_folder_kind ON sync_job (folder_id, kind);