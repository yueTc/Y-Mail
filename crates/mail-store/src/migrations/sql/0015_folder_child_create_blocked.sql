-- 0015_folder_child_create_blocked：记住服务商明确拒绝过的「不能建子文件夹」。
-- 有的服务商（例如 QQ 邮箱的「订阅邮件」）在 LIST 里不返回 \NoInferiors，
-- 只有真发 CREATE 才回 NO。把这次拒绝落到本地，界面下次不再重复开放
-- 「新建子文件夹」入口，免得用户反复撞同一个墙。
-- 用整数 0/1 存布尔；STRICT 表要求显式类型。

ALTER TABLE folder ADD COLUMN child_create_blocked INTEGER NOT NULL DEFAULT 0;