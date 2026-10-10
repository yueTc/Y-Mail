# 邮件自动分类贴标签 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

| 项 | 值 |
|---|---|
| 日期 | 2026-10-09 |
| 分支 | codex/mail-auto-classification |
| 规格 | docs/superpowers/specs/2026-10-09-mail-auto-classification-design.md v1.2 |
| 总退出验证 | R1–R27 逐条可复现；四关全过；邮件正文只在授权后外发；标签和颜色只写本地库 |

**Goal:** 给统一收件箱接入 TypeSafe jev 分类，新增本地标签管理、自动与手动分类、彩色胶囊列表显示和 label: 搜索。

**Architecture:** 数据层新增带颜色的 mail_label 和 message_label 两张本地表；mail-ai 新增 TypeSafe 协议适配；mail-core 负责授权、正文截取、阈值判定和审计；Tauri 外壳的现有新邮件轮询负责后台串行分类；React 界面负责配置、标签颜色管理、右键入口、彩色胶囊显示和筛选。

**Tech Stack:** Rust 2021、rusqlite、tokio、serde_json、Tauri、React、TypeScript、Vitest、SQLite WAL。

**Spec:** docs/superpowers/specs/2026-10-09-mail-auto-classification-design.md

## Global Constraints

- 面向用户用中文大白话；代码、命令、文件名除外。
- 邮件正文是不可信内容，绝不因正文触发发信、跳转、写库命令或执行命令。
- 密钥只进系统保险箱，数据库、日志、报错里不能有明文。
- 自动分类默认关闭；开关打开时一次性确认，手动和右键每次确认。
- 正文最多外发前 2000 个字符，只发纯文本，不发附件，不加载远程图片。
- 分类只允许 TypeSafe jev；其他站点不能承担翻译、摘要、润色、起草、通知识别以外的分类。
- 一封邮件只有一个标签；把握低于门槛贴兜底标签，失败不贴标签。
- 标签和颜色只写本地库，不同步账号，不上传服务器；颜色只能从固定色板选，模型不接触颜色。
- 每批改动必须通过：cargo fmt --all --check、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace、npm run build。
- Windows 终端找不到 cargo 时，先执行：$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"。
- 写文件使用 UTF-8 无 BOM、LF；提交信息用中文；禁止提交 *.db、.env、密钥文件和构建产物。

---

## 0. 现状与前置

- 迁移清单在 crates/mail-store/src/migrations/mod.rs，当前最后一条是 0012_ai_notification_verify。
- 真正的收件箱查询在 crates/mail-store/src/inbox.rs 和 crates/mail-store/src/search.rs；crates/mail-core 里的同名文件只是编排门面。
- 新邮件插入链路是 crates/mail-core/src/sync/fetcher.rs 调 store.insert_messages；插入实现是 crates/mail-store/src/sync.rs::insert_messages。
- 新邮件后台触发应先复用 src-tauri/src/notify.rs 的 5 秒轮询；它每轮 SCAN_LIMIT = 20，已有通知开关先例，不能把分类开关塞进同步设置。
- 正文读取复用 crates/mail-core/src/reading.rs::get_message_body；分类编排不新建一套 MIME 解析。
- AI 功能映射在 crates/mail-store/src/ai.rs，AI 目标解析和授权在 crates/mail-core/src/ai.rs，站点协议在 crates/mail-ai/src/client.rs。
- 设置模型在 src-tauri/src/settings.rs 和 src-tauri/src/commands.rs；AppSettings derive Eq，门槛必须存 u8 百分比，不能存 f64。
- 共享开关在 src-tauri/src/state.rs；分类开关和门槛只在本机 settings.json，不进入 apply_synced_toggles。

## 1. 结构决策

- P-D1：标签表放 mail-store，不新建 crate。原因是邮件标签和 message 主键强关联，外键级联必须由唯一写库层保证。
- P-D2：新建 crates/mail-store/src/label.rs，集中放标签增删改查、排序、按邮件读取和设置邮件标签；不把 SQL 散到 mail-core。
- P-D3：TypeSafe 作为 mail-ai 的第四种 ProviderKind，协议适配留在 client.rs，分类业务编排留在 mail-core。
- P-D4：分类使用新的 AiFunction::Classify；resolve_ai_target 对 Classify 强制校验 TypeSafe 能力，无映射时不能回退到第一个启用站点。
- P-D5：自动分类走 AppState 的长授权和后台队列；手动与右键分类走 preview_classify_messages + classify_messages 的一次性授权。
- P-D6：后台队列挂在 notify.rs 的现有轮询里，每轮最多处理 20 封；收信线程只投编号，不等待模型。
- P-D7：邮件列表标签由 mail-store 查询直接带出，线程折叠行取最新一封的标签；前端只负责显示和点击搜索。
- P-D8：分类开关与门槛存本机 settings.json；TypeSafe 站点密钥仍走系统保险箱，标签数据走 SQLite。
- P-D9：标签颜色只存固定色板键；模型不返回颜色；前端用 data-label-color 加 CSS 色板映射显示，禁止接收任意样式或十六进制色值。

## 2. 规格与实际路径不一致的修正

规格 v1.2 里提到的下面这些路径在执行时按本表理解，禁止按旧路径改代码：

| 规格旧路径 | 实际路径 | 原因 |
|---|---|---|
| crates/mail-core/src/search.rs | crates/mail-store/src/search.rs | SQL 与搜索解析在存储层 |
| crates/mail-core/src/inbox.rs | crates/mail-store/src/inbox.rs | 收件箱 SQL 在存储层 |
| crates/mail-core/src/sync/worker.rs | crates/mail-core/src/sync/fetcher.rs + src-tauri/src/notify.rs | 新邮件插入在 fetcher，后台轮询在 notify |
| crates/mail-store/src/lib.rs 只导出新模块 | 同时补 pub mod label 和 pub use label::{...} | 其他 crate 只能从门面导入 |
| 分类目标默认取第一个启用站点 | crates/mail-core/src/ai.rs::resolve_ai_target 增加能力校验 | 防止误把 OpenAI/Ollama 当分类站点 |

---

## 3. Wave 与原子任务

### Wave A1：数据层

#### Task 1：迁移 0013 与标签表

**Files:**
- Create: crates/mail-store/src/migrations/sql/0013_mail_label_classification.sql
- Modify: crates/mail-store/src/migrations/mod.rs:1-80
- Test: crates/mail-store/src/migrations/mod.rs

**Interfaces:**
- Consumes: 现有迁移器 run_migrations，第 12 条迁移。
- Produces: mail_label、message_label 两张表；ai_provider 的 kind 放开 typesafe；ai_model_map 的 function 放开 classify；八条带颜色预置标签；color 只接受固定色板键。

- [x] **Step 1: 先写失败测试**

在 crates/mail-store/src/migrations/mod.rs 的测试模块里，把最新版本断言改成 13，并补：

    #[test]
    fn migration_0013_creates_mail_labels() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("mail.db")).expect("打开数据库");
        store.migrate().expect("跑迁移");
        let count: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM mail_label", [], |row| row.get(0))
            .expect("数标签");
        assert_eq!(count, 8);
        let fallback_color: String = store
            .conn()
            .query_row("SELECT color FROM mail_label WHERE is_fallback = 1", [], |row| row.get(0))
            .expect("读兜底标签颜色");
        assert_eq!(fallback_color, "gray");
        let distinct_colors: i64 = store
            .conn()
            .query_row("SELECT COUNT(DISTINCT color) FROM mail_label", [], |row| row.get(0))
            .expect("数标签颜色");
        assert_eq!(distinct_colors, 8);
        let kind: String = store
            .conn()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'ai_provider'",
                [],
                |row| row.get(0),
            )
            .expect("读建表 SQL");
        assert!(kind.contains("typesafe"), "站点约束要包含 typesafe：{kind}");
        let function: String = store
            .conn()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'ai_model_map'",
                [],
                |row| row.get(0),
            )
            .expect("读功能映射 SQL");
        assert!(function.contains("classify"), "功能约束要包含 classify：{function}");
    }

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-store migration_0013_creates_mail_labels -- --nocapture

Expected: FAIL，提示找不到 mail_label 表或迁移版本仍是 12。

- [x] **Step 3: 写迁移 SQL**

新建 crates/mail-store/src/migrations/sql/0013_mail_label_classification.sql，内容至少包含：

    -- 迁移器已开启 PRAGMA foreign_keys = ON，且整条迁移在事务里执行。
    -- 不能先 DROP TABLE ai_provider，否则会被 ai_model_map 的外键挡住。
    -- 顺序固定为：先把映射搬到无外键临时表，再删旧子表、旧父表，
    -- 重建父表后创建带外键的新子表，最后回填映射数据。
    CREATE TABLE ai_provider_0013 (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        label TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('openai_compatible', 'deepl', 'ollama', 'typesafe')),
        base_url TEXT NOT NULL,
        default_model TEXT NOT NULL DEFAULT '',
        models TEXT NOT NULL DEFAULT '[]',
        thinking_level TEXT CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
        enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
        api_key_ref TEXT,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    INSERT INTO ai_provider_0013
        SELECT id, label, kind, base_url, default_model, models, thinking_level, enabled, api_key_ref, created_at, updated_at
        FROM ai_provider;

    CREATE TABLE ai_model_map_data_0013 (
        function TEXT PRIMARY KEY,
        provider_id INTEGER NOT NULL,
        model TEXT NOT NULL DEFAULT '',
        thinking_level TEXT CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    INSERT INTO ai_model_map_data_0013
        SELECT function, provider_id, model, thinking_level, updated_at FROM ai_model_map;
    DROP TABLE ai_model_map;
    DROP TABLE ai_provider;

    ALTER TABLE ai_provider_0013 RENAME TO ai_provider;

    CREATE TABLE ai_model_map (
        function TEXT PRIMARY KEY
            CHECK (function IN ('translate', 'summary', 'polish', 'draft', 'notification_verify', 'classify')),
        provider_id INTEGER NOT NULL REFERENCES ai_provider(id) ON DELETE CASCADE,
        model TEXT NOT NULL DEFAULT '',
        thinking_level TEXT CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    INSERT INTO ai_model_map
        SELECT function, provider_id, model, thinking_level, updated_at FROM ai_model_map_data_0013;
    DROP TABLE ai_model_map_data_0013;

    CREATE INDEX IF NOT EXISTS idx_ai_provider_enabled ON ai_provider (enabled, id);

    CREATE TABLE IF NOT EXISTS mail_label (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        color TEXT NOT NULL CHECK (color IN ('gray', 'red', 'orange', 'amber', 'green', 'teal', 'blue', 'indigo', 'purple', 'pink')),
        sort_order INTEGER NOT NULL DEFAULT 0,
        is_fallback INTEGER NOT NULL DEFAULT 0 CHECK (is_fallback IN (0, 1)),
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;
    CREATE UNIQUE INDEX IF NOT EXISTS idx_mail_label_name ON mail_label (name);
    CREATE UNIQUE INDEX IF NOT EXISTS idx_mail_label_fallback ON mail_label (is_fallback) WHERE is_fallback = 1;

    CREATE TABLE IF NOT EXISTS message_label (
        message_id INTEGER PRIMARY KEY REFERENCES message(id) ON DELETE CASCADE,
        label_id INTEGER NOT NULL REFERENCES mail_label(id) ON DELETE CASCADE,
        source TEXT NOT NULL CHECK (source IN ('auto', 'manual')),
        confidence REAL,
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;
    CREATE INDEX IF NOT EXISTS idx_message_label_label ON message_label (label_id);

    INSERT INTO mail_label (name, color, sort_order, is_fallback) VALUES
        ('客户', 'blue', 10, 0), ('账单', 'orange', 20, 0), ('通知', 'teal', 30, 0), ('推广', 'purple', 40, 0),
        ('招聘', 'green', 50, 0), ('同事', 'indigo', 60, 0), ('家人', 'pink', 70, 0), ('其他', 'gray', 1000, 1);

- [x] **Step 4: 注册迁移并把所有版本断言改成 13**

在 crates/mail-store/src/migrations/mod.rs 的静态迁移数组末尾加：

    Migration {
        version: 13,
        name: "mail_label_classification",
        sql: include_str!("sql/0013_mail_label_classification.sql"),
    },

同时更新最新支持版本、迁移数量、测试里的版本数组和 supported_version() 断言。

- [x] **Step 5: 跑迁移测试**

Run: cargo test -p mail-store migration_0013_creates_mail_labels -- --nocapture

Expected: PASS；另外跑一次 cargo test -p mail-store migrations -- --nocapture，全部通过。

- [x] **Step 6: 提交**

    git add crates/mail-store/src/migrations/mod.rs crates/mail-store/src/migrations/sql/0013_mail_label_classification.sql
    git commit -m "数据：新增邮件标签与分类功能迁移"

---

#### Task 2：标签数据类型与基本增删改查

**Files:**
- Create: crates/mail-store/src/label.rs
- Modify: crates/mail-store/src/lib.rs:1-55
- Test: crates/mail-store/src/label.rs

**Interfaces:**
- Consumes: Store、StoreError、message 主键。
- Produces: MailLabel、MailLabelSource、MAIL_LABEL_COLORS；list_mail_labels、create_mail_label、rename_mail_label、delete_mail_label、reorder_mail_labels、set_mail_label_color。

- [x] **Step 1: 写失败测试**

在 crates/mail-store/src/label.rs 底部写：

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::connection::Store;

        fn store() -> Store {
            let dir = tempfile::tempdir().expect("临时目录");
            let store = Store::open(dir.keep().join("mail.db")).expect("开库");
            store.migrate().expect("迁移");
            store
        }

        #[test]
        fn labels_can_be_created_renamed_deleted_and_reordered() {
            let store = store();
            let initial = store.list_mail_labels().expect("初始标签");
            assert_eq!(initial.len(), 8);
            assert_eq!(initial.iter().find(|label| label.name == "其他").expect("兜底").color, "gray");
            let extra = store.create_mail_label("项目").expect("新增");
            let after_create = store.list_mail_labels().expect("再查");
            assert_eq!(after_create.len(), 9);
            assert_eq!(after_create.iter().find(|label| label.id == extra).expect("新标签").color, "red");
            store.rename_mail_label(extra, "项目邮件").expect("改名");
            let mut order: Vec<i64> = initial.iter().map(|label| label.id).collect();
            order.insert(0, extra);
            store.reorder_mail_labels(&order).expect("排序");
            let after = store.list_mail_labels().expect("查排序");
            assert_eq!(after[0].id, extra);
            store.delete_mail_label(extra).expect("删除");
            assert_eq!(store.list_mail_labels().expect("删后").len(), 8);
        }

        #[test]
        fn fallback_label_cannot_be_deleted() {
            let store = store();
            let fallback = store
                .list_mail_labels()
                .expect("标签")
                .into_iter()
                .find(|label| label.is_fallback)
                .expect("兜底");
            let err = store.delete_mail_label(fallback.id).expect_err("不能删");
            assert!(matches!(err, StoreError::InvalidData(_)));
        }

        #[test]
        fn fallback_label_color_is_fixed_gray() {
            let store = store();
            let fallback = store
                .list_mail_labels()
                .expect("标签")
                .into_iter()
                .find(|label| label.is_fallback)
                .expect("兜底");
            let err = store.set_mail_label_color(fallback.id, "red").expect_err("不能改兜底颜色");
            assert!(matches!(err, StoreError::InvalidData(_)));
        }
    }

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-store labels_can_be_created_renamed_deleted_and_reordered fallback_label_cannot_be_deleted fallback_label_color_is_fixed_gray -- --nocapture

Expected: FAIL，提示 label 模块、颜色字段或方法不存在。

- [x] **Step 3: 定义结构与方法签名**

在 crates/mail-store/src/label.rs 写：

    use crate::connection::Store;
    use crate::error::StoreError;

    pub const MAIL_LABEL_COLORS: [&str; 10] = [
        "gray", "red", "orange", "amber", "green", "teal", "blue", "indigo", "purple", "pink",
    ];

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct MailLabel {
        pub id: i64,
        pub name: String,
        pub color: String,
        pub sort_order: i64,
        pub is_fallback: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MailLabelSource { Auto, Manual }

    impl MailLabelSource {
        pub fn as_str(self) -> &'static str {
            match self { Self::Auto => "auto", Self::Manual => "manual" }
        }
    }

在 Store 的 impl 块实现：list_mail_labels、create_mail_label、rename_mail_label、delete_mail_label、reorder_mail_labels、set_mail_label_color。list_mail_labels 必须返回 color。create_mail_label 按 MAIL_LABEL_COLORS 顺序挑第一个尚未使用的颜色；全部用过时用当前标签数量对色板长度取模。set_mail_label_color 只接受固定色板键，并在写入前校验；删除标签前先查 is_fallback，是兜底标签返回 StoreError::InvalidData("兜底标签不能删除")；兜底标签改色返回 StoreError::InvalidData("兜底标签颜色不能修改")。reorder_mail_labels 要求传入全部标签 ID、不能重复，并在事务里按顺序更新 sort_order。所有 SQL 用参数绑定。

- [x] **Step 4: 导出模块**

在 crates/mail-store/src/lib.rs 加 pub mod label;，并在 pub use 区加 MailLabel、MailLabelSource、MAIL_LABEL_COLORS。

- [x] **Step 5: 跑测试**

Run: cargo test -p mail-store labels_can_be_created_renamed_deleted_and_reordered fallback_label_cannot_be_deleted fallback_label_color_is_fixed_gray -- --nocapture

Expected: PASS。

- [x] **Step 6: 提交**

    git add crates/mail-store/src/label.rs crates/mail-store/src/lib.rs
    git commit -m "数据：实现本地分类标签增删改查"

---

#### Task 3：邮件标签读写与级联

**Files:**
- Modify: crates/mail-store/src/label.rs
- Modify: crates/mail-store/src/lib.rs
- Test: crates/mail-store/src/label.rs

**Interfaces:**
- Consumes: MailLabel、MailLabelSource、message 表、mail_label 表。
- Produces: StoredMessageLabel；get_message_label、set_message_label、clear_message_label、list_message_ids_with_label。

- [x] **Step 1: 写失败测试**

补三个测试：

    #[test]
    fn setting_a_label_replaces_the_previous_one() {
        // 先插入一封邮件，再分别贴两个标签；断言最终只查到一个，且 name 是第二个。
    }

    #[test]
    fn deleting_a_label_clears_its_message_rows() {
        // 建自定义标签 -> 贴到邮件 -> 删标签 -> get_message_label 返回 None。
    }

    #[test]
    fn deleting_a_message_clears_its_label_row() {
        // 贴标签 -> 删 message -> 直接数 message_label 为 0。
    }

测试里的 message 插入复用已有 test helper 的 SQL 字段；不要用内存桩，直接跑真实迁移。

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-store setting_a_label_replaces_the_previous_one deleting_a_label_clears_its_message_rows deleting_a_message_clears_its_label_row -- --nocapture

Expected: FAIL，提示方法不存在或外键未级联。

- [x] **Step 3: 实现 Upsert 与读取**

在同一个 Store impl 块实现：

    pub fn set_message_label(
        &self,
        message_id: i64,
        label_id: i64,
        source: MailLabelSource,
        confidence: Option<f64>,
    ) -> Result<(), StoreError>

用 INSERT ... ON CONFLICT(message_id) DO UPDATE 写入 message_label；写入前校验 message 与 label 存在。

    pub fn get_message_label(&self, message_id: i64) -> Result<Option<(MailLabel, Option<f64>, MailLabelSource)>, StoreError>

    pub fn clear_message_label(&self, message_id: i64) -> Result<bool, StoreError>

    pub fn list_message_ids_with_label(&self, label_name: &str) -> Result<Vec<i64>, StoreError>

- [x] **Step 4: 跑测试**

Run: cargo test -p mail-store label -- --nocapture

Expected: 标签相关测试全部 PASS，包括级联。

- [x] **Step 5: 提交**

    git add crates/mail-store/src/label.rs crates/mail-store/src/lib.rs
    git commit -m "数据：实现邮件标签写入与级联清理"

---

#### Task 4：新邮件插入返回编号

**Files:**
- Modify: crates/mail-store/src/sync.rs:449-520
- Test: crates/mail-store/src/sync.rs

**Interfaces:**
- Consumes: NewMessage。
- Produces: insert_messages_returning_ids(&[NewMessage]) -> Result<Vec<i64>, StoreError>；旧 insert_messages 保持返回 usize。

- [x] **Step 1: 写失败测试**

在 crates/mail-store/src/sync.rs 测试模块补：

    #[test]
    fn inserting_messages_returns_only_new_ids() {
        let store = test_store();
        let batch = vec![message(1, 1, 100), message(1, 1, 101)];
        let ids = store.insert_messages_returning_ids(&batch).expect("首次插入");
        assert_eq!(ids.len(), 2);
        let again = store.insert_messages_returning_ids(&batch[..1]).expect("重复插入");
        assert!(again.is_empty());
        let count = store.insert_messages(&batch[..1]).expect("旧接口");
        assert_eq!(count, 0);
    }

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-store inserting_messages_returns_only_new_ids -- --nocapture

Expected: FAIL，方法不存在。

- [x] **Step 3: 实现新接口**

把现有插入循环改成同一个事务里记录 last_insert_rowid()，返回成功写入的编号；如果现有 SQL 用 INSERT OR IGNORE，用 changes() 判断是否真的插入。旧接口：

    pub fn insert_messages(&self, messages: &[NewMessage]) -> Result<usize, StoreError> {
        Ok(self.insert_messages_returning_ids(messages)?.len())
    }

- [x] **Step 4: 跑测试**

Run: cargo test -p mail-store inserting_messages_returns_only_new_ids -- --nocapture

Expected: PASS；再跑 cargo test -p mail-store sync:: -- --nocapture 确认旧断言不变。

- [x] **Step 5: 提交**

    git add crates/mail-store/src/sync.rs
    git commit -m "数据：新邮件插入返回本地邮件编号"

---

### Wave A2：AI 层

#### Task 5：ProviderKind 与能力声明

**Files:**
- Modify: crates/mail-ai/src/provider.rs:8-61
- Modify: crates/mail-store/src/ai.rs:17-50
- Modify: crates/mail-core/src/ai.rs:13-55
- Test: crates/mail-ai/src/provider.rs、crates/mail-core/src/ai.rs

**Interfaces:**
- Consumes: 现有三类站点枚举、AiFunction。
- Produces: mail-ai::ProviderFunction；ProviderKind::TypeSafe；ProviderKind::supports(ProviderFunction) -> bool；AiProviderKind::TypeSafe；mail-core::provider_function(AiFunction) -> ProviderFunction。

- [x] **Step 1: 写失败测试**

在 crates/mail-ai/src/provider.rs 测试模块补：

    #[test]
    fn typesafe_only_supports_classification() {
        assert!(ProviderKind::TypeSafe.supports(ProviderFunction::Classification));
        assert!(!ProviderKind::TypeSafe.supports(ProviderFunction::Translation));
        assert!(!ProviderKind::OpenAiCompatible.supports(ProviderFunction::Classification));
        assert!(ProviderKind::Ollama.supports(ProviderFunction::Summary));
    }

mail-ai 不依赖 mail-store，所以这里禁止引用 AiFunction。固定方案：在 crates/mail-ai/src/provider.rs 定义 ProviderFunction；在 crates/mail-core/src/ai.rs 写 provider_function(AiFunction) -> ProviderFunction 做一对一映射。不要改 Cargo.toml 制造反向依赖。

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-ai typesafe_only_supports_classification -- --nocapture

Expected: FAIL，TypeSafe 不存在。

- [x] **Step 3: 加枚举与映射**

定义 ProviderFunction，至少包含 Translation、Summary、Polish、Draft、NotificationVerify、Classification；ProviderKind 与 AiProviderKind 都加 TypeSafe。as_str/parse 接受 typesafe，label 返回「TypeSafe jev」，default_base_url 返回 https://api.typesafe.ai/v1。ProviderKind::supports 严格按下面这张表实现：

    OpenAI 兼容：翻译、摘要、润色、起草、通知识别
    DeepL：翻译
    Ollama：翻译、摘要、润色、起草、通知识别
    TypeSafe：邮件分类

- [x] **Step 4: 跑测试**

Run: cargo test -p mail-ai typesafe_only_supports_classification -- --nocapture

Expected: PASS；站点枚举往返测试也 PASS。

- [x] **Step 5: 提交**

    git add crates/mail-ai/src/provider.rs crates/mail-store/src/ai.rs crates/mail-core/src/ai.rs
    git commit -m "AI：新增 TypeSafe jev 站点与能力声明"

---

#### Task 6：jev 请求与应答适配

**Files:**
- Modify: crates/mail-ai/src/client.rs:1-250
- Modify: crates/mail-ai/src/error.rs:1-60
- Test: crates/mail-ai/src/client.rs

**Interfaces:**
- Consumes: Endpoint、ProxyRoute、Secret、model、正文纯文本、标签名数组。
- Produces: ClassifyOutcome { choice: String, confidence: f64 }；classify_mail(route, endpoint, model, state, choices, timeout)。

- [x] **Step 1: 写失败测试**

在 client.rs 测试模块补：

    #[tokio::test]
    async fn typesafe_classify_sends_systemone_request_and_parses_answer() {
        let port = fake_server(
            "200 OK",
            r#"{"answers":{"category":{"choice":"账单","confidence":0.91}}}"#,
        ).await;
        let endpoint = Endpoint {
            kind: ProviderKind::TypeSafe,
            base_url: &format!("http://127.0.0.1:{port}/v1"),
            api_key: None,
        };
        let out = classify_mail(None, &endpoint, "jev-latest", "本月账单已出", &["账单".into(), "其他".into()], Duration::from_secs(15)).await.expect("分类");
        assert_eq!(out.choice, "账单");
        assert!((out.confidence - 0.91).abs() < 0.0001);
    }

再加两个失败用例：confidence 超过 1、choice 不在候选里，都返回 AiError::Protocol。

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-ai typesafe_classify_sends_systemone_request_and_parses_answer -- --nocapture

Expected: FAIL，classify_mail 不存在。

- [x] **Step 3: 实现请求**

classify_mail 只接受 TypeSafe；请求体固定为：

    {
      "state": state,
      "model": model,
      "questions": [{
        "name": "category",
        "type": "choice",
        "criteria": "请只从候选标签中选择最匹配的一项",
        "choices": choices
      }]
    }

URL 是 join_url(endpoint.base_url, "systemone")，认证头复用 Bearer。解析 answers.category.choice 与 answers.category.confidence；校验 choice 在 choices 里、confidence 在 0..=1、不能为空。错误正文继续走 status_error 清洗。

- [x] **Step 4: 跑测试**

Run: cargo test -p mail-ai typesafe_classify -- --nocapture

Expected: PASS。

- [x] **Step 5: 提交**

    git add crates/mail-ai/src/client.rs crates/mail-ai/src/error.rs
    git commit -m "AI：接入 TypeSafe jev 分类请求"

---

#### Task 7：测试连接与分类探针

**Files:**
- Modify: crates/mail-ai/src/prompt.rs
- Modify: crates/mail-core/src/ai.rs:402-430、1011-1060
- Modify: src-tauri/src/commands.rs:3444-3463
- Test: crates/mail-core/src/ai.rs

**Interfaces:**
- Consumes: TypeSafe classify_mail。
- Produces: test_ai_provider 对 TypeSafe 返回 vec!["jev-latest"]；固定探针不读真实邮件。

- [x] **Step 1: 写失败测试**

补：

    #[test]
    fn typesafe_test_connection_returns_fixed_model_name() {
        // 用假 TypeSafe 响应启动本地 HTTP 服务，调 engine.test_ai_provider，
        // 断言返回 vector 等于 ["jev-latest"]，且请求体里 state 是固定中文探针。
    }

探针文本写成常量，不含用户数据：

    const TYPESAFE_PROBE_STATE: &str = "这是一封测试邮件：您的账单已生成。";

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-core typesafe_test_connection_returns_fixed_model_name -- --nocapture

Expected: FAIL，TypeSafe 走到 list_models 分支。

- [x] **Step 3: 在 test_ai_provider 分支**

在 crates/mail-core/src/ai.rs::test_ai_provider 开头判断 kind == TypeSafe：调 classify_mail，成功返回 vec!["jev-latest".to_string()]；失败返回清洗后的错误。不要拉取模型。修 src-tauri/src/commands.rs 的类型解析错误文案，加入 typesafe。

- [x] **Step 4: 跑测试**

Run: cargo test -p mail-core typesafe_test_connection_returns_fixed_model_name -- --nocapture

Expected: PASS。

- [x] **Step 5: 提交**

    git add crates/mail-ai/src/prompt.rs crates/mail-core/src/ai.rs src-tauri/src/commands.rs
    git commit -m "AI：TypeSafe 使用固定探针测试连接"

---

#### Task 8：新增 classify 功能映射与严格目标选择

**Files:**
- Modify: crates/mail-store/src/ai.rs:87-120
- Modify: crates/mail-core/src/ai.rs:541-660、1088-1128
- Modify: src/api.ts:552-580
- Test: crates/mail-store/src/ai.rs、crates/mail-core/src/ai.rs

**Interfaces:**
- Consumes: AiFunction、AiModelMapEntry、ProviderKind 能力表。
- Produces: AiFunction::Classify；resolve_ai_target(Classify) 只接受启用且 TypeSafe 的映射。

- [x] **Step 1: 写失败测试**

补：

    #[test]
    fn classify_never_falls_back_to_a_chat_provider() {
        // 库里只放一个 OpenAiCompatible 启用站点，不写 classify 映射。
        // resolve_ai_target(AiFunction::Classify) 必须报 EngineError::BadRequest，不得返回该站点。
    }

    #[test]
    fn classify_uses_configured_typesafe_provider() {
        // 建 TypeSafe 站点 + classify 映射，断言 target.kind == TypeSafe。
    }

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-core classify_never_falls_back_to_a_chat_provider -- --nocapture

Expected: FAIL，当前无映射会取第一个启用站点。

- [x] **Step 3: 加 Classify 与能力校验**

AiFunction 加 Classify，as_str/parse 支持 classify。resolve_ai_target 先按 function 调 provider_kind 能力判断；Classify 没有显式映射时返回 EngineError::BadRequest("邮件分类还没有配置 TypeSafe jev 站点")，不得走 find(first enabled)。有映射但站点不支持该功能时返回 EngineError::BadRequest。

- [x] **Step 4: 跑测试**

Run: cargo test -p mail-core classify_ -- --nocapture

Expected: PASS；再跑 cargo test -p mail-store ai -- --nocapture。

- [x] **Step 5: 提交**

    git add crates/mail-store/src/ai.rs crates/mail-core/src/ai.rs src/api.ts
    git commit -m "AI：新增邮件分类功能映射与能力约束"

---

### Wave A3：编排与命令

#### Task 9：批量一次性授权结构

**Files:**
- Modify: crates/mail-core/src/ai.rs:149-230、500-660
- Test: crates/mail-core/src/ai.rs

**Interfaces:**
- Consumes: AiAuthorizationPreview、PendingAiAuthorization、内容哈希、邮件编号列表。
- Produces: preview_classify_messages、consume_classify_authorization；令牌带 message_ids 与每封 content_hash。

- [x] **Step 1: 写失败测试**

补两封邮件：

    #[test]
    fn classify_authorization_is_one_time_and_bound_to_message_hashes() {
        // 预览两封 -> token 长度非空；
        // 修改第二封正文 -> 执行必须拒绝；
        // 第一次成功消费后第二次必须拒绝。
    }

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-core classify_authorization -- --nocapture

Expected: FAIL，预览和消费方法不存在。

- [x] **Step 3: 扩展授权结构**

给 PendingAiAuthorization 增加：

    pub message_ids: Vec<i64>,
    pub content_hashes: Vec<String>,

普通翻译等单封功能填长度 1 的向量。预览返回目标邮件编号、站点、完整域名、模型、local、每封 content_hash 和 token。执行时令牌一次性，重新读取每封正文，逐封比较哈希；目标数量、顺序、内容任一不对就拒绝，绝不部分执行。

- [x] **Step 4: 跑测试**

Run: cargo test -p mail-core classify_authorization -- --nocapture

Expected: PASS。

- [x] **Step 5: 提交**

    git add crates/mail-core/src/ai.rs
    git commit -m "安全：增加批量分类一次性授权"

---

#### Task 10：分类编排与阈值落标签

**Files:**
- Modify: crates/mail-core/src/ai.rs
- Modify: crates/mail-core/src/reading.rs
- Modify: crates/mail-store/src/label.rs
- Test: crates/mail-core/src/ai.rs

**Interfaces:**
- Consumes: get_message_body、classify_mail、list_mail_labels、set_message_label。
- Produces: classify_message(message_id, threshold) -> Result<ClassifyOutcome, EngineError>；只写标签，不通知识别，不发信。

- [x] **Step 1: 写失败测试**

用假 TypeSafe 服务器覆盖：

    - confidence 0.90 + threshold 0.75 -> 贴模型选中的标签；
    - confidence 0.40 + threshold 0.75 -> 贴兜底标签；
    - HTTP 500 / 超时 / choice 不在标签 -> 不贴标签，返回可审计错误，不影响下一次分类。

测试只构造最小 message 与 body，不依赖真实邮箱。

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-core classify_message_ -- --nocapture

Expected: FAIL，方法不存在。

- [x] **Step 3: 实现编排**

流程必须严格按规格 4.2：

    1. 取邮件正文纯文本，trim，空则跳过；
    2. 截取 chars().take(2000)；
    3. 从 mail_label 读取所有标签名，兜底标签必须存在；
    4. 调 classify_mail；
    5. confidence >= threshold 且 choice 匹配时贴该标签；否则贴 is_fallback 标签；
    6. 记录 ai_audit，不记录正文；
    7. 任何模型错误只返回错误，不写标签。

自动流程在写入前检查已有 message_label；手动流程允许覆盖。

- [x] **Step 4: 跑测试**

Run: cargo test -p mail-core classify_message_ -- --nocapture

Expected: PASS，且断言 ai_audit 不含正文片段。

- [x] **Step 5: 提交**

    git add crates/mail-core/src/ai.rs crates/mail-core/src/reading.rs crates/mail-store/src/label.rs
    git commit -m "分类：实现阈值判定与本地落标签"

---

#### Task 11：新邮件后台队列接线

**Files:**
- Modify: crates/mail-core/src/sync/fetcher.rs:302-340
- Modify: src-tauri/src/notify.rs:30-210
- Modify: src-tauri/src/state.rs:1-125
- Modify: crates/mail-core/src/engine.rs
- Test: crates/mail-core/src/sync/fetcher.rs、src-tauri/src/notify.rs

**Interfaces:**
- Consumes: insert_messages_returning_ids、classify_message、AppState 分类开关与门槛。
- Produces: MailEngine::enqueue_classify_ids、drain_classify_queue；notify 轮询每轮最多处理 20 封。

- [x] **Step 1: 写失败测试**

在 mail-core 补：

    #[test]
    fn new_message_ids_are_enqueued_without_waiting_for_ai() {
        // fetcher 插入 2 封新邮件，立即断言队列里收到 2 个编号；
        // 用阻塞假服务器证明收信插入调用没有等待模型响应。
    }

在 src-tauri/src/notify.rs 补纯函数测试：

    #[test]
    fn classify_batch_is_capped_at_twenty() {
        let ids: Vec<i64> = (1..=25).collect();
        assert_eq!(take_classify_batch(ids), vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]);
    }

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-core new_message_ids_are_enqueued_without_waiting_for_ai -- --nocapture

Expected: FAIL，队列方法不存在。

- [x] **Step 3: 插入后投编号，轮询里消费**

fetcher.rs 调 insert_messages_returning_ids；拿到 ids 后只做 enqueue，不做 await 模型。队列实现用 std::sync::Mutex<VecDeque<i64>>，锁内只 push；notify.rs 每轮从队列取最多 20 个，逐个调用 engine.classify_message。分类开关关时只保留队列不处理；门槛从 Arc<AtomicU8> 读取并除以 100 变 0.50–0.99。存储锁绝不跨 await。

- [x] **Step 4: 跑测试**

Run: cargo test -p mail-core new_message_ids_are_enqueued_without_waiting_for_ai -- --nocapture
Run: cargo test -p ymail classify_batch_is_capped_at_twenty -- --nocapture

Expected: PASS。src-tauri/Cargo.toml 已确认 package 名是 ymail，命令直接使用 -p ymail，不再按其他包名替换。

- [x] **Step 5: 提交**

    git add crates/mail-core/src/sync/fetcher.rs crates/mail-core/src/engine.rs src-tauri/src/notify.rs src-tauri/src/state.rs
    git commit -m "分类：新邮件后台队列串行分类"

---

#### Task 12：标签命令与分类预览命令

**Files:**
- Modify: src-tauri/src/commands.rs:1-120、1590-1705
- Modify: src-tauri/src/lib.rs:60-165
- Modify: src/api.ts:200-310、820-870
- Test: src-tauri/src/commands.rs

**Interfaces:**
- Consumes: Store 标签接口、MailEngine 分类授权接口。
- Produces: list_mail_labels、create_mail_label、rename_mail_label、delete_mail_label、reorder_mail_labels、set_mail_label_color、preview_classify_messages、classify_messages、set_message_label；MailLabelDto 增加 color；InboxMessageDto 增加 labelId、labelName、labelColor。

- [x] **Step 1: 写失败测试**

在 commands.rs 测试模块补：

    #[test]
    fn label_dto_never_contains_secret_fields() {
        let dto = MailLabelDto { id: 1, name: "账单".into(), color: "orange".into(), sort_order: 10, is_fallback: false };
        let text = serde_json::to_string(&dto).expect("序列化");
        assert!(!text.contains("api_key"));
        assert!(!text.contains("secret"));
    }

在 src/api.ts 的类型测试里断言 InboxMessage 有 labelId: number | null、labelName: string | null 和 labelColor: string | null；MailLabel 有 color: string。

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p ymail label_dto_never_contains_secret_fields -- --nocapture
Run: npm run test -- --run src/api.test.ts

Expected: FAIL，DTO 或类型不存在。

- [x] **Step 3: 加 DTO 与命令**

命令参数全部用 camelCase 的 serde rename_all。create/rename/delete/reorder 只接受标签 ID 与名称；set_mail_label_color(label_id, color) 只接受固定色板键；所有名称 trim 后非空、长度不超过 64。preview_classify_messages(message_ids: Vec<i64>) 返回站点、域名、模型、local、每封 messageId/contentHash/token。classify_messages(token: String) 只消费服务端保存的待授权，不接受前端再传 messageIds。set_message_label(message_id, label_id: Option<i64>) 使用 source=manual。

- [x] **Step 4: 注册命令与列表字段**

在 src-tauri/src/lib.rs 的 invoke_handler 注册全部新命令。MailLabelDto 带出 color。InboxMessageDto::from_message 带出 label_id、label_name、label_color；没有标签时三个字段都输出 null。

- [x] **Step 5: 跑测试**

Run: cargo test -p ymail label_dto_never_contains_secret_fields -- --nocapture
Run: npm run test -- --run src/api.test.ts

Expected: PASS。

- [x] **Step 6: 提交**

    git add src-tauri/src/commands.rs src-tauri/src/lib.rs src/api.ts
    git commit -m "命令：新增标签管理与分类预览接口"

---

#### Task 13：收件箱查询带出标签

**Files:**
- Modify: crates/mail-store/src/inbox.rs:16-410
- Modify: crates/mail-store/src/search.rs:1-350
- Test: crates/mail-store/src/inbox.rs、crates/mail-store/src/search.rs

**Interfaces:**
- Consumes: message_label、mail_label。
- Produces: InboxMessage.label_id、InboxMessage.label_name、InboxMessage.label_color；INBOX_COLUMNS 从 19 列变 22 列；线程最新一封带标签和颜色。

- [x] **Step 1: 写失败测试**

补：

    #[test]
    fn inbox_thread_uses_latest_message_label() {
        // 同一 thread 两封邮件，旧邮件账单、新邮件通知；
        // 断言 list_inbox_threads 返回的最新标签 name 是“通知”、color 是“teal”。
    }

    #[test]
    fn search_column_count_matches_inbox_columns() {
        assert_eq!(INBOX_COLUMNS_COUNT, 22);
        assert_eq!(INBOX_COLUMNS_COUNT, crate::inbox::INBOX_COLUMNS.split(',').count());
    }

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-store inbox_thread_uses_latest_message_label search_column_count_matches_inbox_columns -- --nocapture

Expected: FAIL，字段/列数不存在。

- [x] **Step 3: 改列与 JOIN**

INBOX_COLUMNS 末尾加：

    ml.label_id, l.name, l.color

THREAD_BASE 同步加三列，并在 FROM message m 后补：

    LEFT JOIN message_label ml ON ml.message_id = m.id
    LEFT JOIN mail_label l ON l.id = ml.label_id

线程窗口按新列位置取值：tkey 变 22，rn/message_count/unread_count 顺延。row_to_message 读取 19/20/21 标签下标，row_to_thread 同步改下标。get_inbox_message、list_inbox_messages、list_thread_messages、count 相关查询都补相同 JOIN，避免列错位。

- [x] **Step 4: 跑测试**

Run: cargo test -p mail-store inbox -- --nocapture
Run: cargo test -p mail-store search -- --nocapture

Expected: PASS，特别是折叠行和搜索分页。

- [x] **Step 5: 提交**

    git add crates/mail-store/src/inbox.rs crates/mail-store/src/search.rs
    git commit -m "收件箱：查询带出标签并保持线程最新标签"

---

#### Task 14：label: 搜索解析与数据库过滤

**Files:**
- Modify: crates/mail-store/src/search.rs
- Modify: crates/mail-core/src/search.rs:1-120
- Modify: src/api.ts:800-850
- Test: crates/mail-store/src/search.rs、crates/mail-core/src/search.rs

**Interfaces:**
- Consumes: SearchQuery、list_message_ids_with_label。
- Produces: SearchQuery.label: Option<String>；label: 可与 from:/has:/is:/before: 叠加。

- [x] **Step 1: 写失败测试**

补：

    #[test]
    fn label_filter_is_case_insensitive_for_ascii_and_exact_for_chinese() {
        // label:账单 命中账单；label:BILLS 命中 bills；label:bill 不命中 bills。
    }

    #[test]
    fn label_only_search_skips_fts() {
        // 只传 label:账单 不创建/读取 FTS 命中，仍返回普通数据库结果。
    }

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-store label_filter -- --nocapture

Expected: FAIL，解析器把 label: 当普通关键词。

- [x] **Step 3: 扩展解析与 SQL**

解析器新增 label: 前缀，值去首尾空白。SQL 过滤条件用 EXISTS：

    EXISTS (
        SELECT 1 FROM message_label ml
        JOIN mail_label l ON l.id = ml.label_id
        WHERE ml.message_id = m.id AND (l.name = ? OR LOWER(l.name) = LOWER(?))
    )

只有 label:、没有关键词时走普通数据库过滤；有其它关键词时把 EXISTS 叠加到 FTS 结果的 SQL 条件里。

- [x] **Step 4: 跑测试**

Run: cargo test -p mail-store label_filter -- --nocapture
Run: cargo test -p mail-core search -- --nocapture

Expected: PASS。

- [x] **Step 5: 提交**

    git add crates/mail-store/src/search.rs crates/mail-core/src/search.rs src/api.ts
    git commit -m "搜索：支持 label 标签筛选"

---

### Wave A4：界面

#### Task 15：分类开关、门槛与站点能力

**Files:**
- Modify: src-tauri/src/settings.rs:31-75、120-240
- Modify: src-tauri/src/state.rs:20-120
- Modify: src-tauri/src/lib.rs:210-390
- Modify: src/AiPanel.tsx:45-90、700-870
- Modify: src/api.ts:540-590、970-1040
- Modify: src/i18n.en.ts
- Test: src-tauri/src/settings.rs、src-tauri/src/state.rs、src/__tests__/ai.test.tsx

**Interfaces:**
- Consumes: AppSettings、AppState、AiProviderKind::TypeSafe、AiFunction::Classify。
- Produces: classify_enabled、classify_confidence_percent；AI 面板只列支持分类的站点。

- [x] **Step 1: 写失败测试**

settings.rs 补：

    #[test]
    fn classify_threshold_percent_is_clamped_to_50_99() {
        let mut settings = AppSettings::default();
        settings.classify_confidence_percent = 10;
        assert_eq!(settings.normalized_classify_confidence_percent(), 75);
        settings.classify_confidence_percent = 120;
        assert_eq!(settings.normalized_classify_confidence_percent(), 75);
        settings.classify_confidence_percent = 80;
        assert_eq!(settings.normalized_classify_confidence_percent(), 80);
    }

React 测试断言分类行只出现 TypeSafe，OpenAI 站点不出现在候选。

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p ymail classify_threshold_percent_is_clamped_to_50_99 -- --nocapture
Run: npm run test -- --run src/__tests__/ai.test.tsx

Expected: FAIL。

- [x] **Step 3: 加字段与原子状态**

AppSettings 加：

    pub classify_enabled: bool,
    pub classify_confidence_percent: u8,

默认 false / 75。AppState 加 Arc<AtomicBool> classify_enabled、Arc<AtomicU8> classify_confidence_percent；save_settings 更新两者。confirm 弹窗中的门槛展示为百分数，内部转 0.50–0.99。apply_synced_toggles 不加这两个字段。

- [x] **Step 4: 接前端表单**

AiPanel 的站点类型新增「TypeSafe jev」；TypeSafe 不显示拉模型按钮，显示测试连接；模型默认 jev-latest，可改。FUNCTION_ORDER 增加邮件分类。功能级站点下拉按 supports(function) 过滤。打开自动分类开关先弹确认，确认后才调用保存。

- [x] **Step 5: 跑测试**

Run: cargo test -p ymail classify_threshold_percent_is_clamped_to_50_99 -- --nocapture
Run: npm run test -- --run src/__tests__/ai.test.tsx

Expected: PASS。

- [x] **Step 6: 提交**

    git add src-tauri/src/settings.rs src-tauri/src/state.rs src-tauri/src/lib.rs src/AiPanel.tsx src/api.ts src/i18n.en.ts src/__tests__/ai.test.tsx
    git commit -m "界面：加入分类开关门槛与 jev 配置"

---

#### Task 16：AI 功能里的分类标签管理

**Files:**
- Modify: src/AiPanel.tsx
- Modify: src/api.ts
- Modify: src/i18n.en.ts
- Modify: src/index.css
- Test: src/__tests__/ai.test.tsx

**Interfaces:**
- Consumes: list_mail_labels、create_mail_label、rename_mail_label、delete_mail_label、reorder_mail_labels、set_mail_label_color。
- Produces: 「分类标签管理」区；新增、改名、改颜色、删除、上下调序；每项有胶囊预览；兜底标签删除和改色按钮禁用。

- [x] **Step 1: 写失败测试**

React 测试断言：

    - AI 功能页出现「分类标签管理」标题；
    - 初始八条标签；
    - 每条标签有 data-label-preview，颜色与 color 一致；
    - 「其他」的删除按钮和颜色选择器 disabled；
    - 把「账单」改成红色后调用 set_mail_label_color 并重新拉回红色标签；
    - 点新增后出现新标签；
    - 点改名后名称变化；
    - 点上移后顺序变化。

- [x] **Step 2: 跑测试确认失败**

Run: npm run test -- --run src/__tests__/ai.test.tsx

Expected: FAIL，标题不存在。

- [x] **Step 3: 实现管理区**

放在 AI 功能页功能映射之后、审计之前。颜色选择器只列 MAIL_LABEL_COLORS 的十项，不提供任意取色器或十六进制输入；每项用同样的胶囊样式做预览，预览节点带 data-label-preview={label.color}。名称输入 trim 后为空不提交，超过 64 字符给中文错误。删除前弹确认，提示该标签下邮件会变成没有标签；兜底标签禁用删除和改色。所有成功操作重新拉 list_mail_labels，不靠本地猜顺序或颜色。

- [x] **Step 4: 跑测试**

Run: npm run test -- --run src/__tests__/ai.test.tsx

Expected: PASS。

- [x] **Step 5: 提交**

    git add src/AiPanel.tsx src/api.ts src/i18n.en.ts src/index.css src/__tests__/ai.test.tsx
    git commit -m "界面：在 AI 功能里增加分类标签管理"

---

#### Task 17：列表标签、点击筛选与搜索提示

**Files:**
- Modify: src/InboxPanel.tsx:330-480、1400-1475
- Modify: src/api.ts:219-310
- Modify: src/i18n.en.ts
- Modify: src/index.css
- Test: src/__tests__/flaggedView.test.tsx、src/__tests__/inbox.test.tsx

**Interfaces:**
- Consumes: InboxMessage.labelId、labelName、labelColor；SearchQuery label；MAIL_LABEL_COLORS。
- Produces: 彩色完整胶囊放在时间前面；点标签设置 label:标签名；搜索提示包含 label:。

- [x] **Step 1: 写失败测试**

测试数据带 labelName: "账单"、labelColor: "orange"，断言标签节点在 .inbox-row-time 的前一个兄弟节点，且带 data-label-color="orange"；点击后搜索框值为 label:账单，并触发列表筛选。无标签时不渲染胶囊。平铺、子邮件和会话折叠三种行都要覆盖。

- [x] **Step 2: 跑测试确认失败**

Run: npm run test -- --run src/__tests__/flaggedView.test.tsx src/__tests__/inbox.test.tsx

Expected: FAIL。

- [x] **Step 3: 实现显示与点击**

邮件行结构：

    {message.labelName && message.labelColor ? (
      <span className="inbox-row-label" data-label-color={message.labelColor}>
        {message.labelName}
      </span>
    ) : null}
    <time className="inbox-row-time">{formatDate(message.dateUtc)}</time>

胶囊只包裹标签文字，整块紧挨时间放在时间前面，不额外换行。标签点击 stopPropagation，设置搜索为 label:标签名；搜索提示改成「搜索邮件，支持 from: / has: / is: / before: / label:」。在 src/index.css 的 .inbox-row-time 前补 .inbox-row-label 的完整圆角、内边距、边框和文字样式，并为十种 data-label-color 分别写浅色与深色两组映射；颜色值只来自 CSS，不拼到 DOM 的 style 属性里。

- [x] **Step 4: 跑测试**

Run: npm run test -- --run src/__tests__/flaggedView.test.tsx src/__tests__/inbox.test.tsx

Expected: PASS。

- [x] **Step 5: 提交**

    git add src/InboxPanel.tsx src/api.ts src/i18n.en.ts src/index.css src/__tests__/flaggedView.test.tsx src/__tests__/inbox.test.tsx
    git commit -m "界面：邮件标签显示在时间前并支持点选筛选"

---

#### Task 18：收件箱右键立即分类

**Files:**
- Modify: src/InboxPanel.tsx:330-480、1400-1475
- Modify: src/api.ts
- Modify: src/i18n.en.ts
- Test: src/__tests__/inbox.test.tsx

**Interfaces:**
- Consumes: preview_classify_messages、classify_messages。
- Produces: 单封右键「立即分类」；确认弹窗；成功后刷新列表。

- [x] **Step 1: 写失败测试**

React 测试：

    - 右键一封邮件出现「立即分类」；
    - 点击后确认框含站点、域名、模型、local 和目标主题；
    - 取消不调用 classify_messages；
    - 确认后只传该邮件编号，成功后刷新列表。

- [x] **Step 2: 跑测试确认失败**

Run: npm run test -- --run src/__tests__/inbox.test.tsx

Expected: FAIL。

- [x] **Step 3: 实现单封菜单与确认**

只把当前 row.message.id 放进 preview 参数。菜单项点击后先 preview，再用返回信息组装确认框；用户确认后调 classify_messages(token)。关闭菜单、点别处、按 Escape 都取消。当前不做多选，不留批量入口。

- [x] **Step 4: 跑测试**

Run: npm run test -- --run src/__tests__/inbox.test.tsx

Expected: PASS。

- [x] **Step 5: 提交**

    git add src/InboxPanel.tsx src/api.ts src/i18n.en.ts src/__tests__/inbox.test.tsx
    git commit -m "界面：收件箱右键支持单封立即分类"

---

#### Task 19：读信界面手动改标签

**Files:**
- Modify: src/MessageReader.tsx
- Modify: src/api.ts
- Modify: src/i18n.en.ts
- Modify: src/index.css
- Test: src/__tests__/reader.test.tsx

**Interfaces:**
- Consumes: list_mail_labels、set_message_label。
- Produces: 读信页标签下拉；选标签或清空后刷新列表。

- [x] **Step 1: 写失败测试**

断言下拉列出八条标签并保留每条 color 数据，能选「账单」，能选「清除标签」，保存后调用 set_message_label 并触发列表刷新。

- [x] **Step 2: 跑测试确认失败**

Run: npm run test -- --run src/__tests__/reader.test.tsx

Expected: FAIL。

- [x] **Step 3: 实现下拉**

下拉值用 labelId 的字符串，清空传 null。保存成功后只刷新列表，不重载正文。失败显示中文提示，不乐观改界面。

- [x] **Step 4: 跑测试**

Run: npm run test -- --run src/__tests__/reader.test.tsx

Expected: PASS。

- [x] **Step 5: 提交**

    git add src/MessageReader.tsx src/api.ts src/i18n.en.ts src/index.css src/__tests__/reader.test.tsx
    git commit -m "界面：读信页支持手动修改标签"

---

### Wave A5：收尾

#### Task 20：安全走查与审计断言

**Files:**
- Modify: crates/mail-core/src/ai.rs
- Modify: src-tauri/src/notify.rs
- Test: crates/mail-core/src/ai.rs、src-tauri/src/notify.rs

**Interfaces:**
- Consumes: ai_audit、分类编排、确认流程。
- Produces: 正文不入审计、密钥不入错误、关闭开关立即停止。

- [x] **Step 1: 写失败测试**

补：

    - 分类成功与失败后查 ai_audit，断言不含正文前 20 个字符；
    - 用假服务器返回包含密钥的错误正文，断言错误字符串不含密钥；
    - classify_enabled=false 时后台消费函数直接返回，不调用模型。

- [x] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-core ai_audit -- --nocapture

Expected: 至少一条失败，定位泄漏点。

- [x] **Step 3: 修正最小实现**

所有 tracing::debug 只记 message_id、provider_id、model、错误类别。错误文案继续用 AiError 的清洗结果。开关关闭分支放在读正文之前。

- [x] **Step 4: 跑测试**

Run: cargo test -p mail-core ai_audit -- --nocapture

Expected: PASS。

- [x] **Step 5: 提交**

    git add crates/mail-core/src/ai.rs src-tauri/src/notify.rs
    git commit -m "安全：补齐分类审计与关闭开关测试"

---

#### Task 21：四关验证

**Files:**
- No new files.
- Modify only if 四关发现真实问题。

- [x] **Step 1: 格式化检查**

Run: cargo fmt --all --check

Expected: 退出码 0。

- [x] **Step 2: Rust 静态检查**

Run: cargo clippy --workspace --all-targets -- -D warnings

Expected: 退出码 0，无 warning。

- [x] **Step 3: Rust 测试**

Run: cargo test --workspace

Expected: 全部 PASS。

- [x] **Step 4: 前端构建**

Run: npm run build

Expected: 退出码 0。

- [x] **Step 5: 提交修复**

    git add -A
    git commit -m "验证：邮件自动分类贴标签四关通过"

#### Task 22：更新记忆并汇报

**Files:**
- Modify: .ai-memory/handoff.md
- Modify: .ai-memory/project_memory.md
- Modify: .ai-memory/20261010/daily.md

- [x] **Step 1: 写交接**

记录已完成范围、相关文件、未完成项、风险、四关命令与结果。不要贴密钥、数据库文件或正文。

- [x] **Step 2: 自查文件**

Run: git status --short
Expected: 只能看到三个记忆文件修改；没有 *.db、.env、构建产物。

- [x] **Step 3: 提交**

    git add .ai-memory/handoff.md .ai-memory/project_memory.md .ai-memory/20261010/daily.md
    git commit -m "记忆：记录邮件自动分类实现结果"

---

## 4. 测试策略

- 数据层全部走真实 SQLite 临时库，不用假 SQL；覆盖旧库升级、带颜色预置标签、颜色校验、自动分配、兜底固定灰色、唯一兜底、级联删除。
- AI 协议用本地假 HTTP 服务，覆盖 200、400、500、超时、confidence 非法、choice 非法。
- 授权测试必须覆盖令牌一次性、内容和邮件编号绑定、取消不读取正文。
- 收件箱 SQL 测平铺、线程折叠、搜索分页、无标签、标签改名和颜色变化后的显示。
- React 测试按任务文件分别覆盖彩色胶囊位置、颜色管理、点选筛选、右键单封、取消不发、读信修改。
- 四关是总门禁，任何一项失败都不能进入收尾提交。

## 5. 风险与回退

- jev 中文准确率没验：先脱敏样本实测；不满意时保持开关默认关，不影响旧功能。
- 正文 2000 字符可能不够：门槛和截取长度都留在 mail-core 常量；回退只需调常量。
- 后台轮询分类可能拖慢：队列每轮最多 20，单封 15 秒超时；回退时清空队列并关开关即可。
- 迁移失败：0013 在事务里执行；回退只回滚迁移，不写半成品表。
- 标签误删：删除前弹确认；删除只清本地关联，不回写服务器。
- 命令契约变化：前端只通过 src/api.ts 封装；回退一个命令时同步删注册和类型。
- 固定色板在浅色和深色主题下的对比度没做真机观感验证：上线前逐色看一遍；不满意时只改 src/index.css 的映射，不改数据库颜色键。

## 6. 检查点

- A1 完成后：cargo test -p mail-store 全绿；迁移版本 13。
- A2 完成后：cargo test -p mail-ai、cargo test -p mail-core 的分类测试全绿。
- A3 完成后：分类授权、线程标签、label: 搜索和命令测试全绿。
- A4 完成后：npm run test 相关前端测试全绿。
- A5 完成后：四关全绿，.ai-memory 更新。

## 7. 总退出验证（R1–R27）

| 需求 | 验证任务 | 具体证据 |
|---|---|---|
| R1 | Task 5、15 | TypeSafe 站点可选、Key 只存保险箱、枚举测试通过 |
| R2 | Task 7、15 | 测试连接固定探针；无拉模型入口；默认 jev-latest |
| R3 | Task 8、15 | 邮件分类功能行只列支持分类的站点 |
| R4 | Task 15 | classify_enabled 默认 false |
| R5 | Task 8、15 | 无 TypeSafe 映射或模型为空时拒绝打开 |
| R6 | Task 15、18 | 打开开关与右键都弹确认，确认后才落盘/外发 |
| R7 | Task 11、20 | 开关关闭时消费函数在读正文前返回 |
| R8 | Task 11、13 | 新邮件入队；正文 chars().take(2000) |
| R9 | Task 10 | confidence 门槛上下分支测试 |
| R10 | Task 10、20 | 超时/网络/非法不贴标签且不弹窗 |
| R11 | Task 10、11 | 自动流程检查已有标签并跳过 |
| R12 | Task 13、17 | 列表 SQL 带标签，DOM 标签在时间前 |
| R13 | Task 13 | 线程最新一封标签测试 |
| R14 | Task 14 | label: 与 from:/has:/is:/before: 叠加测试 |
| R15 | Task 17 | 点击标签设置 label: 搜索 |
| R16 | Task 2、16 | 标签管理区增删改查与排序测试 |
| R17 | Task 2、16 | 兜底标签删除返回错误，按钮 disabled |
| R18 | Task 3 | 删除标签级联清空 message_label |
| R19 | Task 19 | 读信下拉设置与清空标签并刷新 |
| R20 | Task 20 | 审计字段无正文、无密钥 |
| R21 | Task 8、15、20 | 一键关闭 AI 后分类停止，标签保留 |
| R22 | Task 9、18 | 手动分类预览与一次性执行 |
| R23 | Task 15 | 门槛默认 75，范围 50–99，越界回 75 |
| R24 | Task 9、18 | 右键菜单、单封确认、取消不外发 |
| R25 | Task 18 | 只传当前 message.id，不提供多选入口 |
| R26 | Task 2、12、16 | 固定色板校验、颜色修改命令、管理区预览和兜底禁改测试通过；颜色不进入 AI 请求 |
| R27 | Task 1、13、17 | 预置颜色落库；收件箱查询带出 label_color；胶囊紧挨时间之前且十色浅深主题映射存在 |

## 8. 执行交接

计划写完并提交后，按 writing-plans 的交接规则，选一种：

1. 子智能体驱动（推荐）：每个任务换一个子智能体，任务之间双重审查，适合这批跨 Rust、Tauri、React 的改动。
2. 当前会话分批执行：按 Wave 分批，在 A1、A2、A3、A4、A5 各设检查点。

两种方式都必须先读本规格和本计划；遇到规格与代码冲突时停下问用户，不猜。
