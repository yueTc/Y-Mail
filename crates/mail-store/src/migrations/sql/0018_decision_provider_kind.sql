-- 0018_decision_provider_kind：允许通用决策模型站点。
-- 旧 typesafe 数据原样保留；本迁移只放宽 kind 的 CHECK。
CREATE TABLE ai_provider_data_0018 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    label TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('openai_compatible', 'deepl', 'ollama', 'typesafe', 'decision_api')),
    base_url TEXT NOT NULL,
    default_model TEXT NOT NULL DEFAULT '',
    models_json TEXT NOT NULL DEFAULT '[]',
    thinking_level TEXT NOT NULL DEFAULT 'off' CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    api_key_ref TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    proxy_mode TEXT NOT NULL DEFAULT 'inherit' CHECK (proxy_mode IN ('inherit', 'direct', 'custom')),
    proxy_id INTEGER REFERENCES proxy(id) ON DELETE SET NULL
) STRICT;

INSERT INTO ai_provider_data_0018
    (id, label, kind, base_url, default_model, models_json, thinking_level,
     enabled, api_key_ref, created_at, updated_at, proxy_mode, proxy_id)
SELECT id, label, kind, base_url, default_model, models_json, thinking_level,
       enabled, api_key_ref, created_at, updated_at, proxy_mode, proxy_id
FROM ai_provider;

CREATE TABLE ai_model_map_data_0018 (
    function TEXT PRIMARY KEY,
    provider_id INTEGER NOT NULL,
    model TEXT NOT NULL DEFAULT '',
    thinking_level TEXT CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

INSERT INTO ai_model_map_data_0018
    (function, provider_id, model, thinking_level, updated_at)
SELECT function, provider_id, model, thinking_level, updated_at FROM ai_model_map;

DROP TABLE ai_model_map;
DROP TABLE ai_provider;
ALTER TABLE ai_provider_data_0018 RENAME TO ai_provider;

CREATE TABLE ai_model_map (
    function TEXT PRIMARY KEY CHECK (function IN ('translate', 'summary', 'polish', 'draft', 'notification_verify', 'classify')),
    provider_id INTEGER NOT NULL REFERENCES ai_provider(id) ON DELETE CASCADE,
    model TEXT NOT NULL DEFAULT '',
    thinking_level TEXT CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

INSERT INTO ai_model_map (function, provider_id, model, thinking_level, updated_at)
SELECT function, provider_id, model, thinking_level, updated_at FROM ai_model_map_data_0018;
DROP TABLE ai_model_map_data_0018;
CREATE INDEX IF NOT EXISTS idx_ai_provider_enabled ON ai_provider (enabled, id);
CREATE INDEX IF NOT EXISTS idx_ai_provider_proxy ON ai_provider (proxy_id);
