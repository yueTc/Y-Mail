-- 0014_folder_attributes：保存服务器 LIST 返回的文件夹属性。
-- 界面据此判断哪些文件夹可以新建子文件夹、重命名或删除。

ALTER TABLE folder ADD COLUMN attributes TEXT NOT NULL DEFAULT '';