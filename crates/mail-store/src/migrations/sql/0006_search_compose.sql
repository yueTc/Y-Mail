-- 0006_search_compose：Wave 5 全文搜索与写信（草稿/发件队列）。
-- 说明：
--   message_fts 是自含内容的 FTS5 表（rowid = message.id），正文多存一份换来简单可靠；
--   分词用 trigram，利于中文子串匹配；不足 3 个字符的关键词由上层走 LIKE 兜底；
--   outbox 保存写信草稿与发件队列；contact 保存收发信人地址簿；signature 保存账号签名。
-- 安全：正文来自不可信邮件，只作为检索与展示用文本，不得由它触发任何动作。

CREATE VIRTUAL TABLE IF NOT EXISTS message_fts USING fts5(
    subject,
    from_name,
    from_addr,
    body_text,
    tokenize = 'trigram'
);

-- 新邮件入库：正文可能还没缓存，先写空串，正文到位后再由 message_body 触发器补。
CREATE TRIGGER IF NOT EXISTS message_fts_ai AFTER INSERT ON message BEGIN
    INSERT INTO message_fts (rowid, subject, from_name, from_addr, body_text)
    VALUES (
        new.id,
        new.subject,
        new.from_name,
        new.from_addr,
        COALESCE((SELECT text_plain FROM message_body WHERE message_id = new.id), '')
    );
END;

-- 邮件元数据更新：重建该行的检索内容。
CREATE TRIGGER IF NOT EXISTS message_fts_au AFTER UPDATE ON message BEGIN
    DELETE FROM message_fts WHERE rowid = old.id;
    INSERT INTO message_fts (rowid, subject, from_name, from_addr, body_text)
    VALUES (
        new.id,
        new.subject,
        new.from_name,
        new.from_addr,
        COALESCE((SELECT text_plain FROM message_body WHERE message_id = new.id), '')
    );
END;

-- 邮件删除：同步删掉检索行。
CREATE TRIGGER IF NOT EXISTS message_fts_ad AFTER DELETE ON message BEGIN
    DELETE FROM message_fts WHERE rowid = old.id;
END;

-- 正文写入/更新：把正文并入检索内容。
CREATE TRIGGER IF NOT EXISTS message_body_fts_ai AFTER INSERT ON message_body BEGIN
    DELETE FROM message_fts WHERE rowid = new.message_id;
    INSERT INTO message_fts (rowid, subject, from_name, from_addr, body_text)
    SELECT m.id, m.subject, m.from_name, m.from_addr, COALESCE(new.text_plain, '')
    FROM message m WHERE m.id = new.message_id;
END;

CREATE TRIGGER IF NOT EXISTS message_body_fts_au AFTER UPDATE ON message_body BEGIN
    DELETE FROM message_fts WHERE rowid = new.message_id;
    INSERT INTO message_fts (rowid, subject, from_name, from_addr, body_text)
    SELECT m.id, m.subject, m.from_name, m.from_addr, COALESCE(new.text_plain, '')
    FROM message m WHERE m.id = new.message_id;
END;

-- 存量邮件回填（升级场景）。
INSERT INTO message_fts (rowid, subject, from_name, from_addr, body_text)
SELECT m.id, m.subject, m.from_name, m.from_addr, COALESCE(b.text_plain, '')
FROM message m
LEFT JOIN message_body b ON b.message_id = m.id;

-- 发件队列：草稿与待发邮件共用一张表，state 走固定状态机。
CREATE TABLE IF NOT EXISTS outbox (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id       INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    kind             TEXT NOT NULL CHECK (kind IN ('new', 'reply', 'forward')),
    to_json          TEXT NOT NULL DEFAULT '[]',
    cc_json          TEXT NOT NULL DEFAULT '[]',
    bcc_json         TEXT NOT NULL DEFAULT '[]',
    subject          TEXT NOT NULL DEFAULT '',
    body_html        TEXT NOT NULL DEFAULT '',
    body_text        TEXT NOT NULL DEFAULT '',
    in_reply_to      TEXT,
    references_json  TEXT NOT NULL DEFAULT '[]',
    attachments_json TEXT NOT NULL DEFAULT '[]',
    state            TEXT NOT NULL DEFAULT 'draft'
                     CHECK (state IN ('draft', 'queued', 'sending', 'sent', 'failed')),
    attempts         INTEGER NOT NULL DEFAULT 0,
    last_error       TEXT,
    created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    sent_at          TEXT
) STRICT;

CREATE INDEX IF NOT EXISTS idx_outbox_state ON outbox (state, updated_at);
CREATE INDEX IF NOT EXISTS idx_outbox_account ON outbox (account_id, updated_at DESC);

-- 地址簿：account_id 为空表示跨账号共用的全局联系人。
CREATE TABLE IF NOT EXISTS contact (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id   INTEGER REFERENCES account(id) ON DELETE CASCADE,
    name         TEXT NOT NULL DEFAULT '',
    email        TEXT NOT NULL,
    last_used_at TEXT,
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

-- 同一账号内按小写邮箱唯一；全局联系人另按小写邮箱唯一。
CREATE UNIQUE INDEX IF NOT EXISTS idx_contact_account_email
    ON contact (account_id, lower(email)) WHERE account_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_contact_global_email
    ON contact (lower(email)) WHERE account_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_contact_email ON contact (email);

-- 账号签名：每个账号最多一条。
CREATE TABLE IF NOT EXISTS signature (
    account_id INTEGER PRIMARY KEY REFERENCES account(id) ON DELETE CASCADE,
    html       TEXT NOT NULL DEFAULT '',
    enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;