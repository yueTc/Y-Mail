-- 0012_ai_notification_verify：给「通知智能识别」放开 ai_model_map 的功能取值。
--
-- 背景：0008 建 ai_model_map 时，function 列的 CHECK 只允许
-- translate / summary / polish / draft。新增的「通知智能识别」功能标识
-- notification_verify 插不进去，界面报
-- 「CHECK constraint failed: function IN ('translate', 'summary', 'polish', 'draft')」。
-- 历史迁移不许改，所以这里新开一条把约束放开。
--
-- SQLite 改不了已有的 CHECK，按官方建议重建表：建新表 → 搬数据 → 删旧表 → 改名。
-- 整条迁移由迁移器放在单独事务里执行，失败自动回滚。

CREATE TABLE ai_model_map_0012 (
    function       TEXT PRIMARY KEY
                   CHECK (function IN ('translate', 'summary', 'polish', 'draft', 'notification_verify')),
    provider_id    INTEGER NOT NULL REFERENCES ai_provider(id) ON DELETE CASCADE,
    model          TEXT NOT NULL DEFAULT '',
    thinking_level TEXT CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
    updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

INSERT INTO ai_model_map_0012 (function, provider_id, model, thinking_level, updated_at)
    SELECT function, provider_id, model, thinking_level, updated_at FROM ai_model_map;

DROP TABLE ai_model_map;

ALTER TABLE ai_model_map_0012 RENAME TO ai_model_map;
