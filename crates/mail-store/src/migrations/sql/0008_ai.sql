-- 0008_ai：Wave 7 AI 与翻译。
-- 安全：密钥本体只进系统凭据库（keyring），本库只保存 api_key_ref 引用键；
-- 审计只记调用元数据，绝不记录邮件正文原文。

CREATE TABLE IF NOT EXISTS ai_provider (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    label          TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN ('openai_compatible', 'deepl', 'ollama')),
    base_url       TEXT NOT NULL,
    default_model  TEXT NOT NULL DEFAULT '',
    models_json    TEXT NOT NULL DEFAULT '[]',
    thinking_level TEXT NOT NULL DEFAULT 'off'
                   CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
    enabled        INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    api_key_ref    TEXT,
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_ai_provider_enabled ON ai_provider (enabled, id);

-- 功能级模型 / 思考程度覆盖：没配到就回退站点默认。
CREATE TABLE IF NOT EXISTS ai_model_map (
    function       TEXT PRIMARY KEY CHECK (function IN ('translate', 'summary', 'polish', 'draft')),
    provider_id    INTEGER NOT NULL REFERENCES ai_provider(id) ON DELETE CASCADE,
    model          TEXT NOT NULL DEFAULT '',
    thinking_level TEXT CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
    updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

-- 段落对齐译文缓存；切换显示模式不得重复请求模型。
CREATE TABLE IF NOT EXISTS ai_cache (
    cache_key   TEXT PRIMARY KEY,
    message_id  INTEGER REFERENCES message(id) ON DELETE CASCADE,
    function    TEXT NOT NULL,
    model       TEXT NOT NULL,
    target_lang TEXT NOT NULL DEFAULT '',
    source_hash TEXT NOT NULL,
    segments_json TEXT NOT NULL,
    thinking_downgraded INTEGER NOT NULL DEFAULT 0 CHECK (thinking_downgraded IN (0, 1)),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_ai_cache_message ON ai_cache (message_id, function, target_lang);

-- 调用审计：只记时间、功能、模型、是否外发；不记正文与密钥。
CREATE TABLE IF NOT EXISTS ai_audit (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    function   TEXT NOT NULL,
    provider_id INTEGER,
    provider_label TEXT NOT NULL DEFAULT '',
    model      TEXT NOT NULL DEFAULT '',
    target_host TEXT NOT NULL DEFAULT '',
    local      INTEGER NOT NULL DEFAULT 0 CHECK (local IN (0, 1)),
    outbound   INTEGER NOT NULL DEFAULT 1 CHECK (outbound IN (0, 1)),
    outcome    TEXT NOT NULL DEFAULT 'ok',
    detail     TEXT NOT NULL DEFAULT ''
) STRICT;

CREATE INDEX IF NOT EXISTS idx_ai_audit_created ON ai_audit (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_ai_audit_function ON ai_audit (function, created_at DESC);