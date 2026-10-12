-- 0017_ai_provider_proxy：给 AI 站点加站点级代理策略。
-- 默认 inherit（跟随全局），老站点迁移后行为不变；custom 时才用 proxy_id。

ALTER TABLE ai_provider ADD COLUMN proxy_mode TEXT NOT NULL DEFAULT 'inherit'
    CHECK (proxy_mode IN ('inherit', 'direct', 'custom'));
ALTER TABLE ai_provider ADD COLUMN proxy_id INTEGER REFERENCES proxy(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_ai_provider_proxy ON ai_provider (proxy_id);