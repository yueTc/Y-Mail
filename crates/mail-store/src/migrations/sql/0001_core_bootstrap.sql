-- 0001_core_bootstrap：Wave 0 最小启动表。
-- 说明：本阶段只建立全局设置表，用来证明「迁移可执行、可登记」；
--      业务表（account / folder / message ...）自 Wave 1 起按规格 4.3 逐步添加。

CREATE TABLE IF NOT EXISTS setting (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
) STRICT;

INSERT OR IGNORE INTO setting (key, value) VALUES ('app.initialized', '1');