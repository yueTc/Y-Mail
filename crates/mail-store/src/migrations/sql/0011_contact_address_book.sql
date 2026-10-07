-- 通讯录升级为可管理的地址簿（规格 see docs/superpowers/specs/2026-10-06-contacts-workspace-design.md v1.2）。
--
-- 核心变化：一个邮箱 = 一条记录。老表按「账号 + 邮箱」分行，同一个人跟两个账号来往就会占两行，
-- 分组 / 备注 / 隐藏这些「对人的属性」无处安放，所以这里按邮箱归并后重建。
--
-- 安全：迁移前先把老数据原样留一份到 contact_backup_0011（不自动清理）；
-- 整条迁移由迁移器放在单独事务里执行，失败自动回滚。

-- 分组：名字唯一，排序号给将来的手动排序留位。
CREATE TABLE IF NOT EXISTS contact_group (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_contact_group_name ON contact_group (name);

-- 老数据留档：合并写错也捞得回来。
CREATE TABLE contact_backup_0011 AS SELECT * FROM contact;

-- 新表：一个邮箱一条。
CREATE TABLE contact_v2 (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    email        TEXT NOT NULL,
    name         TEXT NOT NULL DEFAULT '',
    note         TEXT NOT NULL DEFAULT '',
    group_id     INTEGER REFERENCES contact_group(id) ON DELETE SET NULL,
    source       TEXT NOT NULL DEFAULT 'auto' CHECK (source IN ('auto', 'manual')),
    hidden       INTEGER NOT NULL DEFAULT 0 CHECK (hidden IN (0, 1)),
    last_used_at TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
) STRICT;

-- 同一个邮箱只留一条：有名字的优先，其次名字长的，再次最近联系的，最后编号小的。
INSERT INTO contact_v2 (email, name, last_used_at, created_at, updated_at)
SELECT email, name, last_used_at, created_at, created_at FROM (
    SELECT email, name, last_used_at, created_at,
           ROW_NUMBER() OVER (
               PARTITION BY lower(email)
               ORDER BY (trim(name) = '') ASC, length(name) DESC,
                        COALESCE(last_used_at, '') DESC, id ASC
           ) AS rn
    FROM contact
) WHERE rn = 1;

-- 换表：老 contact 表连同它的索引一起丢掉。
DROP TABLE contact;
DROP INDEX IF EXISTS idx_contact_account_email;
DROP INDEX IF EXISTS idx_contact_global_email;
DROP INDEX IF EXISTS idx_contact_email;
ALTER TABLE contact_v2 RENAME TO contact;

CREATE UNIQUE INDEX idx_contact_email ON contact (lower(email));
CREATE INDEX idx_contact_group ON contact (group_id);
CREATE INDEX idx_contact_hidden ON contact (hidden);