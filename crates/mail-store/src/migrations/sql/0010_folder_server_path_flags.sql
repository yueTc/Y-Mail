-- 0010_folder_server_path_flags：文件夹原始服务器名 + 红旗待同步标记。
-- folder.full_path 继续存展示名；server_path 存服务器原始名（可能是 Modified UTF-7）。
-- message.flag_pending 表示本地红旗状态尚未得到服务器确认，远端同步时不能覆盖。

ALTER TABLE folder ADD COLUMN server_path TEXT;

ALTER TABLE message ADD COLUMN flag_pending INTEGER NOT NULL DEFAULT 0 CHECK (flag_pending IN (0, 1));

CREATE INDEX IF NOT EXISTS idx_folder_account_server_path ON folder (account_id, server_path);
CREATE INDEX IF NOT EXISTS idx_message_flag_pending ON message (flag_pending) WHERE flag_pending = 1;