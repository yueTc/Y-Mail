-- 0016_mail_label_groups：把「一封邮件一个标签」升级成「标签组 + 标签」。
--
-- 本迁移做七件事（全程在一个事务里，失败整体回滚）：
-- 1) 建标签组表 mail_label_group，预置三个默认组（默认全关）；
-- 2) 加「需要我回复」组的两个标签；
-- 3) 把老 message_label 先搬到无外键临时表，保住老结果；
-- 4) 重建 mail_label，加 group_id，八条老标签全部指向「内容分类」组；
-- 5) 重建 message_label 成（邮件, 组）结构，主键（message_id, group_id）；
-- 6) 把老结果带「内容分类」组号搬回新表，score_value 为空；
-- 7) 建新索引。
--
-- 安全：标签、颜色、分组只存本机；本迁移不涉及密钥、不涉及外发。
-- SQLite 改不了已有表的列与约束，按官方建议重建表。
-- 外键默认打开，所以先把子表数据搬到无外键临时表，再按「子先父后」顺序删表，
-- 否则 DROP 父表会级联把老结果一起删掉。

CREATE TABLE mail_label_group (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    name                 TEXT NOT NULL,
    kind                 TEXT NOT NULL CHECK (kind IN ('choice', 'boolean', 'score')),
    enabled              INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    confidence_threshold REAL NOT NULL DEFAULT 0.75,
    score_min            INTEGER,
    score_max            INTEGER,
    sort_order           INTEGER NOT NULL DEFAULT 0,
    created_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

-- 组表与标签表都要一个唯一索引，名字不能重，所以组表这个加后缀区分。
CREATE UNIQUE INDEX idx_mail_label_group_unique_name ON mail_label_group (name);

INSERT INTO mail_label_group (name, kind, enabled, confidence_threshold, score_min, score_max, sort_order) VALUES
    ('内容分类', 'choice', 0, 0.75, NULL, NULL, 10),
    ('需要我回复', 'boolean', 0, 0.5, NULL, NULL, 20),
    ('紧急程度', 'score', 0, 0.75, 1, 5, 30);

-- 先把老结果搬到无外键的临时表，保证父表重建时不被级联删掉。
CREATE TABLE message_label_backup_0016 AS
    SELECT message_id, label_id, source, confidence, updated_at FROM message_label;

-- 重建标签表：加 group_id，老标签全部归到「内容分类」组，颜色与排序原样保留。
CREATE TABLE mail_label_new_0016 (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id    INTEGER NOT NULL REFERENCES mail_label_group(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    color       TEXT NOT NULL CHECK (color IN ('gray', 'red', 'orange', 'amber', 'green', 'teal', 'blue', 'indigo', 'purple', 'pink')),
    sort_order  INTEGER NOT NULL DEFAULT 0,
    is_fallback INTEGER NOT NULL DEFAULT 0 CHECK (is_fallback IN (0, 1)),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

INSERT INTO mail_label_new_0016
    (id, group_id, name, color, sort_order, is_fallback, created_at, updated_at)
    SELECT id,
           (SELECT id FROM mail_label_group WHERE name = '内容分类'),
           name, color, sort_order, is_fallback, created_at, updated_at
    FROM mail_label;

INSERT INTO mail_label_new_0016 (group_id, name, color, sort_order, is_fallback) VALUES
    ((SELECT id FROM mail_label_group WHERE name = '需要我回复'), '需要回复', 'red', 10, 0),
    ((SELECT id FROM mail_label_group WHERE name = '需要我回复'), '无需回复', 'gray', 20, 0);

-- 子表先删（老结果已备份），再删父表，避免级联删数据。
DROP TABLE message_label;
DROP TABLE mail_label;

ALTER TABLE mail_label_new_0016 RENAME TO mail_label;

CREATE UNIQUE INDEX idx_mail_label_group_name ON mail_label (group_id, name);
CREATE UNIQUE INDEX idx_mail_label_group_fallback ON mail_label (group_id) WHERE is_fallback = 1;

-- 新结果表：一封邮件每个组最多一条结果。
CREATE TABLE message_label (
    message_id  INTEGER NOT NULL REFERENCES message(id) ON DELETE CASCADE,
    group_id    INTEGER NOT NULL REFERENCES mail_label_group(id) ON DELETE CASCADE,
    label_id    INTEGER REFERENCES mail_label(id) ON DELETE CASCADE,
    score_value INTEGER,
    source      TEXT NOT NULL CHECK (source IN ('auto', 'manual')),
    confidence  REAL,
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (message_id, group_id),
    CHECK ((label_id IS NULL) <> (score_value IS NULL))
) STRICT;

INSERT INTO message_label
    (message_id, group_id, label_id, score_value, source, confidence, updated_at)
    SELECT b.message_id,
           (SELECT id FROM mail_label_group WHERE name = '内容分类'),
           b.label_id,
           NULL,
           b.source,
           b.confidence,
           b.updated_at
    FROM message_label_backup_0016 b;

DROP TABLE message_label_backup_0016;

CREATE INDEX idx_message_label_group ON message_label (group_id);
CREATE INDEX idx_message_label_label ON message_label (label_id);