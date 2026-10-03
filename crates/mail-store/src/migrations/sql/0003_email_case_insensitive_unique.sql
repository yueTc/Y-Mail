-- 0003_email_case_insensitive_unique：邮箱地址查重不区分大小写。
-- 背景：业务侧 email_taken 用 lower(email) 比较，而 0002 的唯一索引区分大小写，
-- 会出现「业务说重复、数据库放行」或反过来的裂缝。这里把索引改成基于 lower(email)，
-- 让数据库约束与业务规则一致。历史迁移 0002 已在本机应用，不允许改动。
--
-- 说明：SQLite 的 lower() 只处理 ASCII 大小写；邮箱域名的本地部分按兼容性保持原样存储。

DROP INDEX IF EXISTS idx_account_email;
CREATE UNIQUE INDEX IF NOT EXISTS idx_account_email ON account (lower(email));