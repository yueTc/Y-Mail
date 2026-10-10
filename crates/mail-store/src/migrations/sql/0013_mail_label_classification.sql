-- 0013_mail_label_classification：邮件自动分类与本地彩色标签。
--
-- 本迁移做五件事：
-- 1) 重建 ai_provider，让 kind 允许 typesafe；
-- 2) 重建 ai_model_map，让 function 允许 classify；
-- 3) 建本地标签表 mail_label（带固定色板颜色）；
-- 4) 建邮件标签对应表 message_label；
-- 5) 预置八条带颜色的标签。
--
-- SQLite 改不了已有的 CHECK，按官方建议重建表。
-- 不能先 DROP TABLE ai_provider，否则会被 ai_model_map 的外键挡住。
-- 顺序固定为：先把映射搬到无外键临时表，再删旧子表、旧父表，
-- 重建父表后创建带外键的新子表，最后回填映射数据。
-- 整条迁移由迁移器放在单独事务里执行，失败自动回滚。
--
-- 安全：api_key_ref 仍只是保险箱引用键，密钥本体不落库。
-- 标签与颜色只存本地，不参与账号同步，也不发给模型。

CREATE TABLE ai_provider_0013 (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    label          TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN ('openai_compatible', 'deepl', 'ollama', 'typesafe')),
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

INSERT INTO ai_provider_0013
    (id, label, kind, base_url, default_model, models_json, thinking_level, enabled, api_key_ref, created_at, updated_at)
    SELECT id, label, kind, base_url, default_model, models_json, thinking_level, enabled, api_key_ref, created_at, updated_at
    FROM ai_provider;

CREATE TABLE ai_model_map_data_0013 (
    function       TEXT PRIMARY KEY,
    provider_id    INTEGER NOT NULL,
    model          TEXT NOT NULL DEFAULT '',
    thinking_level TEXT CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
    updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

INSERT INTO ai_model_map_data_0013 (function, provider_id, model, thinking_level, updated_at)
    SELECT function, provider_id, model, thinking_level, updated_at FROM ai_model_map;

DROP TABLE ai_model_map;
DROP TABLE ai_provider;

ALTER TABLE ai_provider_0013 RENAME TO ai_provider;

CREATE TABLE ai_model_map (
    function       TEXT PRIMARY KEY
                   CHECK (function IN ('translate', 'summary', 'polish', 'draft', 'notification_verify', 'classify')),
    provider_id    INTEGER NOT NULL REFERENCES ai_provider(id) ON DELETE CASCADE,
    model          TEXT NOT NULL DEFAULT '',
    thinking_level TEXT CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
    updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

INSERT INTO ai_model_map (function, provider_id, model, thinking_level, updated_at)
    SELECT function, provider_id, model, thinking_level, updated_at FROM ai_model_map_data_0013;

DROP TABLE ai_model_map_data_0013;

CREATE INDEX IF NOT EXISTS idx_ai_provider_enabled ON ai_provider (enabled, id);

CREATE TABLE IF NOT EXISTS mail_label (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    color       TEXT NOT NULL CHECK (color IN ('gray', 'red', 'orange', 'amber', 'green', 'teal', 'blue', 'indigo', 'purple', 'pink')),
    sort_order  INTEGER NOT NULL DEFAULT 0,
    is_fallback INTEGER NOT NULL DEFAULT 0 CHECK (is_fallback IN (0, 1)),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_mail_label_name ON mail_label (name);
CREATE UNIQUE INDEX IF NOT EXISTS idx_mail_label_fallback ON mail_label (is_fallback) WHERE is_fallback = 1;

CREATE TABLE IF NOT EXISTS message_label (
    message_id INTEGER PRIMARY KEY REFERENCES message(id) ON DELETE CASCADE,
    label_id   INTEGER NOT NULL REFERENCES mail_label(id) ON DELETE CASCADE,
    source     TEXT NOT NULL CHECK (source IN ('auto', 'manual')),
    confidence REAL,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_message_label_label ON message_label (label_id);

INSERT INTO mail_label (name, color, sort_order, is_fallback) VALUES
    ('客户', 'blue', 10, 0), ('账单', 'orange', 20, 0),
    ('通知', 'teal', 30, 0), ('推广', 'purple', 40, 0),
    ('招聘', 'green', 50, 0), ('同事', 'indigo', 60, 0),
    ('家人', 'pink', 70, 0), ('其他', 'gray', 1000, 1);