# 通用决策模型站点接入 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在保留旧 TypeSafe jev 站点兼容的前提下，新增通用决策模型站点，支持 JEV、StartLux 及后续兼容 `state + questions + answers` 的服务。

**Architecture:** 存储层新增 `decision_api` 站点类型；`mail-ai` 把完整请求地址、请求构造和三类答案解析收口到统一决策协议；`mail-core` 继续只做业务编排；前端新增“决策模型站点”配置并让分类筛选同时接受新类型与旧 TypeSafe 类型。

**Tech Stack:** Rust 2021、rusqlite、tokio、serde_json、Tauri、React、TypeScript、Vitest、SQLite WAL。

**Spec:** `docs/superpowers/specs/2026-10-20-generic-decision-provider-design.md`

## Global Constraints

- 面向用户一律用中文大白话；先说结论，再说原因。
- 代码、命令、文件名可以保留英文；其余不要用英文旁白。
- 邮件正文是不可信内容，不得因正文触发发信、跳转、写库命令或执行命令。
- 密码、授权码、令牌、AI 密钥只许存进系统保险箱；数据库、日志、报错信息里都不许出现明文。
- 新决策站点默认不启用；外发前必须由现有确认流程明确确认。
- 正文最多外发前 2000 个字符，只发纯文本，不发附件，不加载远程图片。
- 新决策站点首版不支持图片输入、流式输出或自动拉取模型。
- 新决策站点地址是完整请求地址，客户端不得自动补 `/decisions` 或 `/systemone`。
- 旧 `typesafe` 站点地址继续沿用旧逻辑：根地址补 `/systemone`，已经带 `/systemone` 时不重复补。
- 每个任务只暂存本任务文件；当前工作区已有其它未提交改动，禁止整盘 `git add .`。
- 每批代码改动必须通过：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`npm run build`。
- 写文件使用 `[System.IO.File]::WriteAllText($path, $content, (New-Object System.Text.UTF8Encoding $false))`；UTF-8 无 BOM、LF。
- 提交信息用中文；分支保持 `codex/` 前缀；禁止提交 `*.db`、`.env`、密钥文件和构建产物。
- 当前分支：`codex/fix-typesafe-test-model`。不得碰 `D:\projects` 父目录仓库。

---

## 文件结构

- 数据与协议：`crates/mail-store/src/ai.rs`、`crates/mail-store/src/migrations/`、`crates/mail-ai/src/provider.rs`、`crates/mail-ai/src/client.rs`。
- 编排与外壳：`crates/mail-core/src/ai.rs`、`crates/mail-core/src/sync_settings.rs`、`src-tauri/src/commands.rs`。
- 前端：`src/api.ts`、`src/AiProviderDialog.tsx`、`src/AiPanel.tsx`、`src/i18n.en.ts` 及对应测试。
- 文档：旧分类规格、用户说明和 `.ai-memory/`。

---

### Task 1: 数据层增加决策站点类型与迁移

**Files:**
- Modify: `crates/mail-store/src/ai.rs`
- Create: `crates/mail-store/src/migrations/sql/0018_decision_provider_kind.sql`
- Modify: `crates/mail-store/src/migrations/mod.rs`
- Test: `crates/mail-store/src/ai.rs`
- Test: `crates/mail-store/src/migrations/mod.rs`

**Interfaces:**
- Consumes: 现有 `AiProviderKind`、`NewAiProvider`、迁移器。
- Produces: `AiProviderKind::DecisionApi`；`as_str()` 返回 `"decision_api"`；`parse()` 识别 `"decision_api"`、`"decision"`、`"generic_decision"`；迁移版本 18。

- [ ] **Step 1: 写失败测试**

在 `crates/mail-store/src/ai.rs` 测试模块加入：

```rust
#[test]
fn decision_api_类型可解析且可写回() {
    assert_eq!(
        AiProviderKind::parse("decision_api"),
        Some(AiProviderKind::DecisionApi)
    );
    assert_eq!(
        AiProviderKind::parse("decision"),
        Some(AiProviderKind::DecisionApi)
    );
    assert_eq!(AiProviderKind::DecisionApi.as_str(), "decision_api");
}
```

- [ ] **Step 2: 运行测试确认失败**

```powershell
cargo test -p mail-store decision_api_类型可解析且可写回 -- --nocapture
```

Expected: 编译失败，提示 `DecisionApi` 不存在。

- [ ] **Step 3: 最小实现枚举**

在 `AiProviderKind` 加入：

```rust
    /// 通用决策模型站点；只做邮件分类。
    #[serde(rename = "decision_api")]
    DecisionApi,
```

在 `as_str` 和 `parse` 加入：

```rust
Self::DecisionApi => "decision_api",
```

```rust
"decision_api" | "decision" | "generic_decision" => Some(Self::DecisionApi),
```

- [ ] **Step 4: 运行测试确认通过**

```powershell
cargo test -p mail-store decision_api_类型可解析且可写回 -- --nocapture
```

Expected: PASS。

- [ ] **Step 5: 写迁移失败测试**

在 `crates/mail-store/src/migrations/mod.rs` 测试模块加入：

```rust
#[test]
fn 从十七号库升级后可写决策模型站点() {
    let mut store = Store::open_in_memory().expect("打开内存库");
    {
        let conn = store.raw_connection_for_test();
        conn.execute_batch(super::MIGRATION_TABLE_SQL).expect("建登记表");
        for migration in &MIGRATIONS[..17] {
            conn.execute_batch(migration.sql).expect("应用旧迁移");
            conn.execute(
                "INSERT INTO schema_migration (version, name, checksum) VALUES (?1, ?2, ?3)",
                rusqlite::params![migration.version, migration.name, checksum(migration.sql)],
            )
            .expect("登记旧迁移");
        }
        conn.execute(
            "INSERT INTO ai_provider (label, kind, base_url, default_model) \
             VALUES ('旧 JEV', 'typesafe', 'https://api.typesafe.ai/v1', 'jev-latest')",
            [],
        )
        .expect("插入旧决策站点");
    }

    store.run_migrations().expect("升级到十八号库");
    assert_eq!(
        store.applied_migration_versions().expect("读取版本"),
        vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18]
    );

    let conn = store.raw_connection_for_test();
    let old_kind: String = conn
        .query_row("SELECT kind FROM ai_provider WHERE label = '旧 JEV'", [], |row| row.get(0))
        .expect("旧站点应保留");
    assert_eq!(old_kind, "typesafe");
    conn.execute(
        "INSERT INTO ai_provider (label, kind, base_url, default_model) \
         VALUES ('StartLux', 'decision_api', 'https://api.bamboonode.dev/v1/decisions', \
                 'startlux-decision-27b-autothink')",
        [],
    )
    .expect("升级后应能写 decision_api 站点");
    assert!(conn.execute(
        "INSERT INTO ai_provider (label, kind, base_url) VALUES ('坏', 'nope', 'https://x')",
        [],
    ).is_err());
}
```

- [ ] **Step 6: 运行迁移测试确认失败**

```powershell
cargo test -p mail-store 从十七号库升级后可写决策模型站点 -- --nocapture
```

Expected: FAIL，因为迁移 18 不存在或 `CHECK` 仍拒绝 `decision_api`。

- [ ] **Step 7: 创建迁移 SQL**

创建 `crates/mail-store/src/migrations/sql/0018_decision_provider_kind.sql`：

```sql
-- 0018_decision_provider_kind：允许通用决策模型站点。
-- 旧 typesafe 数据原样保留；本迁移只放宽 kind 的 CHECK。
CREATE TABLE ai_provider_data_0018 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    label TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('openai_compatible', 'deepl', 'ollama', 'typesafe', 'decision_api')),
    base_url TEXT NOT NULL,
    default_model TEXT NOT NULL DEFAULT '',
    models_json TEXT NOT NULL DEFAULT '[]',
    thinking_level TEXT NOT NULL DEFAULT 'off' CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    api_key_ref TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    proxy_mode TEXT NOT NULL DEFAULT 'inherit' CHECK (proxy_mode IN ('inherit', 'direct', 'custom')),
    proxy_id INTEGER REFERENCES proxy(id) ON DELETE SET NULL
) STRICT;

INSERT INTO ai_provider_data_0018
    (id, label, kind, base_url, default_model, models_json, thinking_level,
     enabled, api_key_ref, created_at, updated_at, proxy_mode, proxy_id)
SELECT id, label, kind, base_url, default_model, models_json, thinking_level,
       enabled, api_key_ref, created_at, updated_at, proxy_mode, proxy_id
FROM ai_provider;

CREATE TABLE ai_model_map_data_0018 (
    function TEXT PRIMARY KEY,
    provider_id INTEGER NOT NULL,
    model TEXT NOT NULL DEFAULT '',
    thinking_level TEXT CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

INSERT INTO ai_model_map_data_0018
    (function, provider_id, model, thinking_level, updated_at)
SELECT function, provider_id, model, thinking_level, updated_at FROM ai_model_map;

DROP TABLE ai_model_map;
DROP TABLE ai_provider;
ALTER TABLE ai_provider_data_0018 RENAME TO ai_provider;

CREATE TABLE ai_model_map (
    function TEXT PRIMARY KEY CHECK (function IN ('translate', 'summary', 'polish', 'draft', 'notification_verify', 'classify')),
    provider_id INTEGER NOT NULL REFERENCES ai_provider(id) ON DELETE CASCADE,
    model TEXT NOT NULL DEFAULT '',
    thinking_level TEXT CHECK (thinking_level IN ('off', 'low', 'medium', 'high')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

INSERT INTO ai_model_map (function, provider_id, model, thinking_level, updated_at)
SELECT function, provider_id, model, thinking_level, updated_at FROM ai_model_map_data_0018;
DROP TABLE ai_model_map_data_0018;
CREATE INDEX IF NOT EXISTS idx_ai_provider_enabled ON ai_provider (enabled, id);
CREATE INDEX IF NOT EXISTS idx_ai_provider_proxy ON ai_provider (proxy_id);
```

- [ ] **Step 8: 注册迁移并更新版本断言**

在 `MIGRATIONS` 末尾追加：

```rust
Migration {
    version: 18,
    name: "0018_decision_provider_kind",
    sql: include_str!("sql/0018_decision_provider_kind.sql"),
},
```

把所有版本断言末项从 `17` 改为 `18`。用下面命令检查遗漏：

```powershell
rg -n "16, 17\]" crates/mail-store/src/migrations/mod.rs
```

Expected: 没有仍然停在 17 的断言。

- [ ] **Step 9: 运行数据层测试**

```powershell
cargo test -p mail-store decision_api -- --nocapture
cargo test -p mail-store 从十七号库升级后可写决策模型站点 -- --nocapture
```

Expected: 全部 PASS。

- [ ] **Step 10: 提交**

```powershell
git add -- crates/mail-store/src/ai.rs crates/mail-store/src/migrations/mod.rs crates/mail-store/src/migrations/sql/0018_decision_provider_kind.sql
git commit -m "数据：增加通用决策模型站点类型"
```

---

### Task 2: `mail-ai` 支持完整决策地址和通用响应

**Files:**
- Modify: `crates/mail-ai/src/provider.rs`
- Modify: `crates/mail-ai/src/client.rs`
- Modify: `crates/mail-ai/src/lib.rs`
- Test: `crates/mail-ai/src/provider.rs`
- Test: `crates/mail-ai/src/client.rs`

**Interfaces:**
- Consumes: `AiProviderKind::DecisionApi` 已存在。
- Produces: `ProviderKind::DecisionApi`；`ProviderKind::is_decision(self) -> bool`；`fn decision_endpoint_url(base_url: &str, kind: ProviderKind) -> String`；分类函数同时接受 `TypeSafe` 和 `DecisionApi`。

- [ ] **Step 1: 写站点类型失败测试**

在 `crates/mail-ai/src/provider.rs` 测试模块加入：

```rust
#[test]
fn 决策模型类型可解析且同时支持分类() {
    let kind = ProviderKind::parse("decision_api").expect("应能解析");
    assert_eq!(kind, ProviderKind::DecisionApi);
    assert_eq!(kind.as_str(), "decision_api");
    assert_eq!(kind.label(), "决策模型站点");
    assert!(kind.supports(ProviderFunction::Classification));
    assert!(!kind.supports(ProviderFunction::Summary));
    assert!(kind.is_decision());
    assert!(ProviderKind::TypeSafe.is_decision());
}
```

- [ ] **Step 2: 运行测试确认失败**

```powershell
cargo test -p mail-ai 决策模型类型可解析且同时支持分类 -- --nocapture
```

Expected: 编译失败，提示 `DecisionApi` 或 `is_decision` 不存在。

- [ ] **Step 3: 实现 `ProviderKind`**

加入变体：

```rust
    /// 通用决策模型站点；只做邮件分类。
    DecisionApi,
```

补齐映射：

```rust
Self::DecisionApi => "decision_api",
"decision_api" | "decision" | "generic_decision" => Some(Self::DecisionApi),
Self::DecisionApi => "决策模型站点",
Self::DecisionApi => "https://api.bamboonode.dev/v1/decisions",
Self::TypeSafe | Self::DecisionApi => matches!(function, F::Classification),
```

增加：

```rust
    /// 是不是决策模型站点。
    pub fn is_decision(self) -> bool {
        matches!(self, Self::TypeSafe | Self::DecisionApi)
    }
```

- [ ] **Step 4: 运行类型测试确认通过**

```powershell
cargo test -p mail-ai 决策模型类型可解析且同时支持分类 -- --nocapture
```

Expected: PASS。

- [ ] **Step 5: 写地址处理失败测试**

在 `crates/mail-ai/src/client.rs` 测试模块加入：

```rust
#[test]
fn 通用决策站点完整地址原样使用() {
    assert_eq!(
        decision_endpoint_url(
            "https://api.bamboonode.dev/v1/decisions",
            ProviderKind::DecisionApi,
        ),
        "https://api.bamboonode.dev/v1/decisions"
    );
    assert_eq!(
        decision_endpoint_url("https://api.typesafe.ai/v1", ProviderKind::TypeSafe),
        "https://api.typesafe.ai/v1/systemone"
    );
}
```

- [ ] **Step 6: 运行测试确认失败**

```powershell
cargo test -p mail-ai 通用决策站点完整地址原样使用 -- --nocapture
```

Expected: FAIL，提示 `decision_endpoint_url` 不存在。

- [ ] **Step 7: 实现地址函数并替换调用**

把 `systemone_url` 改为：

```rust
fn decision_endpoint_url(base_url: &str, kind: ProviderKind) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    match kind {
        ProviderKind::TypeSafe => {
            if trimmed.to_ascii_lowercase().ends_with("/systemone") {
                trimmed.to_string()
            } else {
                join_url(trimmed, "systemone")
            }
        }
        ProviderKind::DecisionApi => trimmed.to_string(),
        _ => trimmed.to_string(),
    }
}
```

在两个分类函数中把类型检查改为：

```rust
if !endpoint.kind.is_decision() {
    return Err(AiError::Unsupported("邮件分类"));
}
```

地址改为：

```rust
let url = decision_endpoint_url(endpoint.base_url, endpoint.kind);
```

- [ ] **Step 8: 运行地址测试确认通过**

```powershell
cargo test -p mail-ai 通用决策站点完整地址原样使用 -- --nocapture
```

Expected: PASS。

- [ ] **Step 9: 写响应解析失败测试**

加入：

```rust
#[test]
fn 决策站点是否题兼容布尔值和类型别名() {
    let answer = serde_json::json!({"type":"bool","noul":true});
    assert_eq!(
        parse_boolean_answer(&answer, Some("bool")),
        Some(GroupAnswer::Boolean { probability: 1.0 })
    );
    assert!(parse_boolean_answer(
        &serde_json::json!({"type":"noul","value":2}),
        Some("noul")
    ).is_none());
}

#[test]
fn 决策站点选择题可从概率推导把握程度() {
    let question = GroupQuestion {
        group_id: 1,
        kind: GroupQuestionKind::Choice,
        instructions: "分类".to_string(),
        criteria: vec!["账单".to_string(), "其他".to_string()],
        score_min: None,
        score_max: None,
    };
    let answer = serde_json::json!({
        "choice":"账单",
        "probabilities":{"账单":0.9,"其他":0.1}
    });
    assert_eq!(
        parse_choice_answer(&answer, None, &question),
        Some(GroupAnswer::Choice {
            label: "账单".to_string(),
            confidence: 0.9,
        })
    );
}
```

- [ ] **Step 10: 运行测试确认失败**

```powershell
cargo test -p mail-ai 决策站点是否题兼容布尔值和类型别名 -- --nocapture
cargo test -p mail-ai 决策站点选择题可从概率推导把握程度 -- --nocapture
```

Expected: FAIL，因为当前解析器要求 `type` 且不认布尔值。

- [ ] **Step 11: 实现通用响应解析**

三个解析函数改为可选类型：

```rust
fn parse_choice_answer(
    answer: &Value,
    answer_type: Option<&str>,
    question: &GroupQuestion,
) -> Option<GroupAnswer>
```

```rust
fn parse_boolean_answer(answer: &Value, answer_type: Option<&str>) -> Option<GroupAnswer>
```

```rust
fn parse_score_answer(
    answer: &Value,
    answer_type: Option<&str>,
    question: &GroupQuestion,
) -> Option<GroupAnswer>
```

选择题加入：

```rust
if answer_type.is_some_and(|kind| kind != "choice") {
    return None;
}
let confidence = answer_confidence(answer)?;
```

是否题加入：

```rust
if answer_type.is_some_and(|kind| !matches!(kind, "noul" | "bool")) {
    return None;
}
let probability = answer
    .get("noul")
    .or_else(|| answer.get("probability"))
    .and_then(|value| {
        value.as_f64().or_else(|| {
            value.as_bool().map(|flag| if flag { 1.0 } else { 0.0 })
        })
    })?;
if !valid_confidence(probability) {
    return None;
}
```

打分题加入：

```rust
if answer_type.is_some_and(|kind| kind != "score") {
    return None;
}
let confidence = answer.get("confidence").and_then(Value::as_f64)?;
if !valid_confidence(confidence) {
    return None;
}
```

增加：

```rust
fn answer_confidence(answer: &Value) -> Option<f64> {
    if let Some(confidence) = answer.get("confidence").and_then(Value::as_f64) {
        return valid_confidence(confidence).then_some(confidence);
    }
    let probabilities = answer.get("probabilities")?.as_object()?;
    probabilities
        .values()
        .filter_map(Value::as_f64)
        .filter(|value| valid_confidence(*value))
        .max_by(f64::total_cmp)
}
```

打分题没有概率时，整数读取改为：

```rust
answer
    .get("score")
    .and_then(Value::as_i64)
    .or_else(|| answer.get("level").and_then(Value::as_i64))
```

`classify_mail_groups` 中改为：

```rust
let answer_type = answer.get("type").and_then(Value::as_str);
```

- [ ] **Step 12: 禁止决策站点走聊天和模型列表**

在 `list_models` 顶部改为：

```rust
if endpoint.kind.translation_only() || endpoint.kind.is_decision() {
    return Err(AiError::Unsupported("拉取模型列表"));
}
```

在 `chat` 顶部加入：

```rust
if endpoint.kind.is_decision() {
    return Err(AiError::Unsupported("摘要与润色"));
}
```

- [ ] **Step 13: 运行 `mail-ai` 全部测试**

```powershell
cargo test -p mail-ai -- --nocapture
```

Expected: 全部 PASS。

- [ ] **Step 14: 提交**

```powershell
git add -- crates/mail-ai/src/provider.rs crates/mail-ai/src/client.rs crates/mail-ai/src/lib.rs
git commit -m "AI：接入通用决策模型协议"
```

---

### Task 3: `mail-core` 支持决策站点编排

**Files:**
- Modify: `crates/mail-core/src/ai.rs`
- Test: `crates/mail-core/src/ai.rs`

**Interfaces:**
- Consumes: `AiProviderKind::DecisionApi`、`ProviderKind::DecisionApi`、`ProviderKind::is_decision()`。
- Produces: `provider_kind(AiProviderKind::DecisionApi) -> ProviderKind::DecisionApi`；新决策站点保存时要求默认模型非空；测试连接使用固定探针；分类错误提示改为通用决策模型。

- [ ] **Step 1: 写保存校验失败测试**

在 `crates/mail-core/src/ai.rs` 测试模块加入：

```rust
#[tokio::test]
async fn 通用决策站点必须填写模型() {
    let dir = tempfile::tempdir().expect("临时目录");
    let engine = engine(&dir);
    let input = AiProviderInput {
        id: None,
        label: "决策".to_string(),
        kind: AiProviderKind::DecisionApi,
        base_url: "https://api.bamboonode.dev/v1/decisions".to_string(),
        default_model: "   ".to_string(),
        models: Vec::new(),
        thinking_level: AiThinkingLevel::Off,
        enabled: true,
        proxy: AccountProxyMode::Direct,
    };
    let error = engine
        .save_ai_provider(&input, None)
        .expect_err("空模型应被拒绝");
    assert!(format!("{error}").contains("决策模型站点必须填写模型"));
}
```


- [ ] **Step 2: 运行测试确认失败**

```powershell
cargo test -p mail-core 通用决策站点必须填写模型 -- --nocapture
```

Expected: FAIL，当前会允许空模型或提示文案不符。

- [ ] **Step 3: 实现类型映射和保存校验**

在 `provider_kind` 加入：

```rust
AiProviderKind::DecisionApi => ProviderKind::DecisionApi,
```

在 `save_ai_provider_inner` 的地址规整后加入：

```rust
let default_model = input.default_model.trim();
if input.kind == AiProviderKind::DecisionApi && default_model.is_empty() {
    return Err(EngineError::BadRequest(
        "决策模型站点必须填写模型".to_string(),
    ));
}
```

构造 `NewAiProvider` 时使用：

```rust
default_model: default_model.to_string(),
```

- [ ] **Step 4: 运行测试确认通过**

```powershell
cargo test -p mail-core 通用决策站点必须填写模型 -- --nocapture
```

Expected: PASS。

- [ ] **Step 5: 写测试连接失败测试**

加入：

```rust
#[tokio::test]
async fn 通用决策站点测试连接使用完整地址() {
    let (port, captured) = fake_capture_server(
        r#"{"answers":{"category":{"type":"choice","choice":"账单","confidence":0.99}}}"#,
    )
    .await;
    let base = format!("http://127.0.0.1:{port}/v1/decisions");
    let dir = tempfile::tempdir().expect("临时目录");
    let engine = engine(&dir);
    let models = engine
        .test_ai_provider(
            AiProviderKind::DecisionApi,
            &base,
            "startlux-decision-27b-autothink",
            Some(&Secret::new("test-key")),
            None,
            AccountProxyMode::Direct,
        )
        .await
        .expect("测试连接");
    assert_eq!(models, vec!["startlux-decision-27b-autothink"]);
    let request = captured.recv().await.expect("收到请求");
    assert!(request.starts_with("POST /v1/decisions "), "{request}");
    assert!(request.contains("\"model\":\"startlux-decision-27b-autothink\""));
}
```

复用同文件已有 TypeSafe 测试服务的 helper；路径断言改为 `/v1/decisions`。

- [ ] **Step 6: 运行测试确认失败**

```powershell
cargo test -p mail-core 通用决策站点测试连接使用完整地址 -- --nocapture
```

Expected: FAIL，因为 `test_ai_provider` 只把 TypeSafe 当决策站点。

- [ ] **Step 7: 实现测试连接和分类目标**

把 `test_ai_provider` 中两处：

```rust
kind == AiProviderKind::TypeSafe
```

改为：

```rust
provider_kind(kind).is_decision()
```

固定探针分支继续调用 `classify_mail`，返回：

```rust
return Ok(vec![model.to_string()]);
```

把分类缺少映射的提示：

```rust
"邮件分类还没有配置 TypeSafe jev 站点"
```

改为：

```rust
"邮件分类还没有配置决策模型站点"
```

把能力判断前注释里的“只认 TypeSafe jev”改为“只认决策模型站点”。

- [ ] **Step 8: 运行测试确认通过**

```powershell
cargo test -p mail-core 通用决策站点测试连接使用完整地址 -- --nocapture
```

Expected: PASS。

- [ ] **Step 9: 运行分类相关测试**

```powershell
cargo test -p mail-core classify -- --nocapture
cargo test -p mail-core decision -- --nocapture
```

Expected: 全部 PASS。

- [ ] **Step 10: 提交**

```powershell
git add -- crates/mail-core/src/ai.rs
git commit -m "分类：支持通用决策模型站点"
```

---

### Task 4: Tauri 命令和设置同步兼容新类型

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Modify: `crates/mail-core/src/sync_settings.rs`
- Modify: `crates/mail-store/src/settings_import.rs`
- Test: `crates/mail-core/src/sync_settings.rs`
- Test: `crates/mail-store/src/settings_import.rs`

**Interfaces:**
- Consumes: `AiProviderKind::DecisionApi`。
- Produces: 命令错误文案包含 `decision_api`；站点 DTO 的 `supportedFunctions` 对 `decision_api` 返回 `["classify"]`；设置快照可以序列化和导入 `decision_api`。

- [ ] **Step 1: 写快照往返失败测试**

在 `crates/mail-core/src/sync_settings.rs` 测试模块加入：

```rust
#[test]
fn 设置快照保留通用决策站点() {
    let dir = tempfile::tempdir().expect("临时目录");
    let (engine, _secrets) = engine(dir.path());
    let input = AiProviderInput {
        id: None,
        label: "StartLux".to_string(),
        kind: AiProviderKind::DecisionApi,
        base_url: "https://api.bamboonode.dev/v1/decisions".to_string(),
        default_model: "startlux-decision-27b-autothink".to_string(),
        models: vec!["startlux-decision-27b-autothink".to_string()],
        thinking_level: AiThinkingLevel::Off,
        enabled: true,
        proxy: AccountProxyMode::Direct,
    };
    engine.save_ai_provider(&input, Some(&Secret::new("fake-key"))).expect("保存站点");
    let snapshot = engine
        .export_settings_snapshot("device-a", "本机", 1)
        .expect("导出快照");
    assert_eq!(snapshot.ai_providers.len(), 1);
    assert_eq!(snapshot.ai_providers[0].kind, AiProviderKind::DecisionApi);
    let plan = super::build_import_plan(&snapshot);
    assert_eq!(plan.ai_providers[0].provider.kind, AiProviderKind::DecisionApi);
    assert_eq!(
        plan.ai_providers[0].provider.base_url,
        "https://api.bamboonode.dev/v1/decisions"
    );
}
```

在 `crates/mail-core/src/sync_settings.rs` 测试模块补充导入：

```rust
use crate::ai::AiProviderInput;
```

`engine(dir.path())` 返回 `(MailEngine, Arc<MemorySecretStore>)`，测试里只用引擎，所以用 `_secrets` 解构。

- [ ] **Step 2: 运行测试确认失败**

```powershell
cargo test -p mail-core 设置快照保留通用决策站点 -- --nocapture
```

Expected: FAIL，因为新类型尚未接入或测试辅助构造未编译。

- [ ] **Step 3: 更新 Tauri 文案与注释**

把 DTO 类型注释改为：

```rust
/// 站点类型：openai_compatible / deepl / ollama / typesafe / decision_api。
```

把两处错误文案改为：

```rust
"AI 站点类型只能是 openai_compatible、deepl、ollama、typesafe 或 decision_api"
```

把功能注释改为：

```rust
/// 这个站点类型支持的功能标识（邮件分类支持决策模型站点）。
```

- [ ] **Step 4: 更新设置导入测试**

在 `crates/mail-store/src/settings_import.rs` 现有导入计划测试里新增一个 `decision_api` 站点，保留原有 `openai_compatible` 用例。断言：

```rust
assert_eq!(
    store.list_ai_providers().expect("列站点")[1].kind,
    AiProviderKind::DecisionApi
);
```

- [ ] **Step 5: 运行外壳与同步测试**

```powershell
cargo test -p em-master command -- --nocapture
cargo test -p mail-core 设置快照保留通用决策站点 -- --nocapture
cargo test -p mail-store 导入计划 -- --nocapture
```

Expected: 全部 PASS；如果外壳过滤名没有匹配，改跑 `cargo test -p em-master --lib -- --nocapture`。

- [ ] **Step 6: 提交**

```powershell
git add -- src-tauri/src/commands.rs crates/mail-core/src/sync_settings.rs crates/mail-store/src/settings_import.rs
git commit -m "同步：支持通用决策站点类型"
```

---

### Task 5: 前端新增决策模型站点入口

**Files:**
- Modify: `src/api.ts`
- Modify: `src/AiProviderDialog.tsx`
- Modify: `src/AiPanel.tsx`
- Modify: `src/i18n.en.ts`
- Test: `src/__tests__/ai.test.tsx`
- Test: `src/__tests__/aiProviderApi.test.ts`

**Interfaces:**
- Consumes: 后端 `kind: "decision_api"`、完整地址、模型编号。
- Produces: `AiProviderKind` 包含 `"decision_api"`；`isDecisionKind(kind: AiProviderKind): boolean`；新建站点不提供旧 `"typesafe"`，编辑旧站点时仍保留；分类站点筛选使用 `supportedFunctions.includes("classify")`。

- [ ] **Step 1: 写前端 API 失败测试**

在 `src/__tests__/aiProviderApi.test.ts` 加入：

```ts
it("通用决策站点测试连接传递完整地址和模型", async () => {
  const result = await api.testAiProvider(
    "decision_api",
    "https://api.bamboonode.dev/v1/decisions",
    undefined,
    9,
    { mode: "direct" },
    "startlux-decision-27b-autothink",
  );
  expect(invoke).toHaveBeenCalledWith("test_ai_provider", {
    kind: "decision_api",
    baseUrl: "https://api.bamboonode.dev/v1/decisions",
    id: 9,
    proxy: { mode: "direct" },
    model: "startlux-decision-27b-autothink",
  });
  expect(result).toEqual(["relay/jev-custom"]);
});
```

- [ ] **Step 2: 运行测试确认失败**

```powershell
npm test -- --run src/__tests__/aiProviderApi.test.ts
```

Expected: TypeScript 或测试失败，`"decision_api"` 不在联合类型中。

- [ ] **Step 3: 更新前端类型**

在 `src/api.ts` 把：

```ts
export type AiProviderKind = "openai_compatible" | "deepl" | "ollama" | "typesafe";
```

改为：

```ts
export type AiProviderKind =
  | "openai_compatible"
  | "deepl"
  | "ollama"
  | "typesafe"
  | "decision_api";
```

把注释改成：

```ts
/** AI 站点类型：OpenAI 兼容、DeepL、本机 Ollama、旧 TypeSafe 和通用决策模型。 */
```

- [ ] **Step 4: 运行 API 测试确认通过**

```powershell
npm test -- --run src/__tests__/aiProviderApi.test.ts
```

Expected: PASS。

- [ ] **Step 5: 写分类站点筛选失败测试**

在 `src/__tests__/ai.test.tsx` 的分类测试里增加：

```tsx
const decisionProvider: AiProvider = {
  id: 3,
  label: "StartLux",
  kind: "decision_api",
  baseUrl: "https://api.bamboonode.dev/v1/decisions",
  defaultModel: "startlux-decision-27b-autothink",
  models: ["startlux-decision-27b-autothink"],
  thinkingLevel: "off",
  enabled: true,
  hasKey: true,
  supportedFunctions: ["classify"],
  proxy: { mode: "inherit" },
};
```

把原测试改为：

```tsx
it("邮件分类站点同时列出新决策站点和旧 TypeSafe", async () => {
  vi.mocked(api.listAiProviders).mockResolvedValue([
    openAiProvider,
    typeSafeProvider,
    decisionProvider,
  ]);
  vi.mocked(api.listAiModelMaps).mockResolvedValue([]);
  vi.mocked(api.listAiAudit).mockResolvedValue([]);
  vi.mocked(api.listLabelGroups).mockResolvedValue([]);
  render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: "展开邮件分类" }));
  const select = await screen.findByLabelText("邮件分类站点");
  const texts = within(select).getAllByRole("option").map((option) => option.textContent ?? "");
  expect(texts.some((text) => text.includes("StartLux"))).toBe(true);
  expect(texts.some((text) => text.includes("jev"))).toBe(true);
  expect(texts.some((text) => text.includes("中转站"))).toBe(false);
});
```

把“没有站点”断言改为：

```tsx
expect(
  await screen.findByText(
    "还没有可用的决策模型站点：请先到上面的「AI 站点」里新增一个，再回来选。",
  ),
).toBeTruthy();
```

- [ ] **Step 6: 写弹窗预填失败测试**

在 `src/__tests__/ai.test.tsx` 的站点弹窗测试区加入：

```tsx
it("选择通用决策站点时预填完整地址和 StartLux 模型", async () => {
  render(
    <AiProviderDialog
      open
      provider={null}
      onClose={vi.fn()}
      onSaved={vi.fn()}
    />,
  );
  const dialog = await screen.findByRole("dialog");
  fireEvent.change(within(dialog).getByLabelText("AI 站点类型"), {
    target: { value: "decision_api" },
  });
  expect((within(dialog).getByLabelText("AI 站点地址") as HTMLInputElement).value)
    .toBe("https://api.bamboonode.dev/v1/decisions");
  expect((within(dialog).getByLabelText("默认模型") as HTMLInputElement).value)
    .toBe("startlux-decision-27b-autothink");
  expect(within(dialog).getByRole("button", { name: "测试连接" })).toBeTruthy();
});
```

- [ ] **Step 7: 运行前端测试确认失败**

```powershell
npm test -- --run src/__tests__/ai.test.tsx
```

Expected: FAIL，因为界面还没有“决策模型站点”选项和通用文案。

- [ ] **Step 8: 更新弹窗实现**

在 `src/AiProviderDialog.tsx` 增加：

```tsx
export function isDecisionKind(kind: AiProviderKind): boolean {
  return kind === "decision_api" || kind === "typesafe";
}
```

把 `KIND_LABEL` 改为：

```tsx
export const KIND_LABEL: Record<AiProviderKind, string> = {
  openai_compatible: "OpenAI 兼容站点",
  deepl: "DeepL",
  ollama: "本机 Ollama",
  typesafe: "旧版 TypeSafe jev（兼容）",
  decision_api: "决策模型站点",
};
```

站点类型选项改为：

```tsx
{(Object.keys(KIND_LABEL) as AiProviderKind[])
  .filter((kind) => kind !== "typesafe" || editing.kind === "typesafe")
  .map((kind) => (
    <option key={kind} value={kind}>
      {t(KIND_LABEL[kind])}
    </option>
  ))}
```

切换类型时预填改为：

```tsx
const preset =
  kind === "decision_api"
    ? {
        baseUrl: "https://api.bamboonode.dev/v1/decisions",
        defaultModel: "startlux-decision-27b-autothink",
      }
    : kind === "typesafe"
      ? { baseUrl: "https://api.typesafe.ai/v1", defaultModel: "jev-latest" }
      : undefined;
```

地址提示改为：

```tsx
editing.kind === "decision_api"
  ? t("填写完整请求地址，例如 https://api.bamboonode.dev/v1/decisions")
  : editing.kind === "typesafe"
    ? t("填到 /v1 为止，例如 https://api.typesafe.ai/v1，不要带 /systemone")
    : t("例如 https://api.example.com/v1，本机 Ollama 可填 http://127.0.0.1:11434")
```

把测试连接逻辑中所有 `editing.kind === "typesafe"` 改为：

```tsx
isDecisionKind(editing.kind)
```

包括空模型拦截、测试模型参数、成功文案、覆盖模型列表判断、测试模型选择器和按钮文案。

- [ ] **Step 9: 更新分类面板实现**

在 `src/AiPanel.tsx` 引入：

```tsx
import AiProviderDialog, { isDecisionKind } from "./AiProviderDialog";
```

把校验：

```tsx
if (provider.kind !== "typesafe") {
```

改成：

```tsx
if (!isDecisionKind(provider.kind)) {
```

提示改成：

```tsx
return t("请先在「邮件分类」里选择决策模型站点，再打开组开关。");
```

```tsx
return t("邮件分类只支持决策模型站点。");
```

把“还没有 TypeSafe jev 站点”改成：

```tsx
t("还没有可用的决策模型站点：请先到上面的「AI 站点」里新增一个，再回来选。")
```

自动分类开关提示改成：

```tsx
t("请先在「邮件分类」里选择决策模型站点，再打开自动分类。")
```

- [ ] **Step 10: 更新英文文案**

在 `src/i18n.en.ts` 增加：

```ts
"决策模型站点": "Decision model site",
"旧版 TypeSafe jev（兼容）": "Legacy TypeSafe jev (compatible)",
"填写完整请求地址，例如 https://api.bamboonode.dev/v1/decisions":
  "Enter the full request URL, e.g. https://api.bamboonode.dev/v1/decisions",
"请先在「邮件分类」里选择决策模型站点，再打开组开关。":
  "Choose a decision model site under Email classification before enabling a group.",
"邮件分类只支持决策模型站点。":
  "Email classification supports decision model sites only.",
"还没有可用的决策模型站点：请先到上面的「AI 站点」里新增一个，再回来选。":
  "No decision model site yet: add one under AI sites above, then come back and choose it.",
"请先在「邮件分类」里选择决策模型站点，再打开自动分类。":
  "Choose a decision model site under Email classification before enabling automatic classification.",
```

保留旧词条，避免旧界面或测试断链；新文案优先使用。

- [ ] **Step 11: 运行前端定向测试**

```powershell
npm test -- --run src/__tests__/aiProviderApi.test.ts src/__tests__/ai.test.tsx
```

Expected: 全部 PASS。

- [ ] **Step 12: 提交**

```powershell
git add -- src/api.ts src/AiProviderDialog.tsx src/AiPanel.tsx src/i18n.en.ts src/__tests__/ai.test.tsx src/__tests__/aiProviderApi.test.ts
git commit -m "界面：增加通用决策模型站点"
```

---

### Task 6: 更新规格引用、用户说明和全量验证

**Files:**
- Modify: `docs/superpowers/specs/2026-10-11-mail-label-groups-design.md`
- Modify（如存在相关段落）: `docs/user-guide.md`
- Memory: `.ai-memory/handoff.md`
- Memory: `.ai-memory/project_memory.md`
- Create or Modify: `.ai-memory/20261020/daily.md`

**Interfaces:**
- Consumes: 前五个任务完成后的实现。
- Produces: 旧规格不再与实现冲突；四关证据进入交接记忆。

- [ ] **Step 1: 更新旧分类规格**

在 `docs/superpowers/specs/2026-10-11-mail-label-groups-design.md` 顶部依据行后增加：

```markdown
> **2026-10-20 更新**：邮件分类站点已扩展为通用决策模型站点；本文件中“只认 TypeSafe jev”的表述由
> `2026-10-20-generic-decision-provider-design.md` 取代，旧 TypeSafe 站点继续兼容。
```

把正文中的“只认 TypeSafe jev”“jev 站点”“jev 请求”分别替换为“只认决策模型站点”“决策模型站点”“决策请求”。保留 JEV 官方协议例子，作为其中一种上游实现，不再当成唯一类型。

- [ ] **Step 2: 更新用户说明**

如果 `docs/user-guide.md` 有“新增 AI 站点”或“邮件分类”步骤，补一段：

```markdown
决策模型站点要填完整请求地址，例如 `https://api.bamboonode.dev/v1/decisions`，再填写模型编号，
例如 `startlux-decision-27b-autothink`。不要只填到 `/v1`，除非服务商明确说明 `/v1` 就是请求地址。
```

如果文件没有相关段落，不新增空章节。

- [ ] **Step 3: 运行前端定向测试**

```powershell
npm test -- --run src/__tests__/ai.test.tsx src/__tests__/aiProviderApi.test.ts
```

Expected: PASS。

- [ ] **Step 4: 运行后端定向测试**

```powershell
cargo test -p mail-store decision_api -- --nocapture
cargo test -p mail-ai decision -- --nocapture
cargo test -p mail-core decision -- --nocapture
```

Expected: PASS。

- [ ] **Step 5: 运行四关**

先确保当前窗口能找到 cargo：

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
```

依次运行：

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run build
```

Expected: 四条命令都退出码 0。失败时先修本功能相关错误，不得跳过测试或放宽警告。

- [ ] **Step 6: 检查文件格式与工作区**

```powershell
git diff --check
git status --short
```

Expected: 不出现本任务产生的 `*.db`、`.env`、密钥文件或构建产物；本功能修改文件为 UTF-8 无 BOM、LF；未跟踪目录保持不动。

- [ ] **Step 7: 更新记忆**

在 `.ai-memory/20261020/daily.md` 追加：

```markdown
## 通用决策模型站点实现

- 做完的：新增 `decision_api` 类型、迁移 18、完整地址请求、三题型兼容解析、测试连接、前端入口和分类筛选。
- 证据：填写四关实际结果和新增测试名称。
- 相关文件：填写本次实际改动文件。
- 没做完：真实 StartLux / JEV 成功调用是否验证，必须如实写明。
- 卡在哪：如果没有阻塞写“无”；有密钥或线路问题写清具体边界。
```

在 `.ai-memory/handoff.md` 与 `.ai-memory/project_memory.md` 追加对应摘要，不贴正文、不贴密钥。

- [ ] **Step 8: 提交文档**

```powershell
git add -- docs/superpowers/specs/2026-10-11-mail-label-groups-design.md docs/user-guide.md
git commit -m "文档：更新通用决策模型说明"
```

`.ai-memory/` 被 `.gitignore` 忽略，不纳入提交。

---

## 总退出验证

| 需求 | 验证任务 | 具体证据 |
|---|---|---|
| R1 | Task 1、2、3 | `decision_api` 可存、可解析、可分类 |
| R2 | Task 1、3 | 旧 `typesafe` 数据升级后保留并可测试 |
| R3 | Task 2 | 完整地址不追加路径；旧地址仍补 `/systemone` |
| R4 | Task 3、5 | 空模型前后端都拦截 |
| R5 | Task 2 | 一次请求含全部组问题 |
| R6 | Task 2 | `choice`、`noul`、`score` 请求形状测试 |
| R7 | Task 2 | 选项、概率、分数、把握程度校验测试 |
| R8 | Task 2 | 单个非法答案跳过，其他组保留 |
| R9 | Task 3 | 固定探针、当前模型、不读真实邮件 |
| R10 | Task 5 | “决策模型站点”和完整地址提示 |
| R11 | Task 5 | 分类下拉同时出现新类型和旧 TypeSafe |
| R12 | Task 2、3、5 | 密钥、确认、正文上限、图片拦截、审计不变 |
| R13 | Task 1、3、5 | 旧 TypeSafe 站点可读、可测、可分类 |
| R14 | Task 1、4 | 数据库和设置导入导出支持 `decision_api` |

## 真实接口验证边界

假服务端测试只能证明请求和解析符合设计。真实成功响应仍需用可用密钥单独联调：

```text
POST https://api.bamboonode.dev/v1/decisions
model: startlux-decision-27b-autothink
Authorization: Bearer <保险箱中的密钥>
```

另外可用：

```text
POST https://api.bamboonode.dev/jev/v1/decisions
model: typesafe/jev-1.14
```

如果用户没有提供密钥，收尾时必须明确写“真实成功调用未验”，不能把假服务端通过当成真实接入完成。

## 风险与回退

- 迁移 18 会重建站点表和映射表；必须有旧数据保留测试，失败就回滚整批迁移。
- StartLux 的 `model` 是否强制尚未真机确认；本实现按用户必须填写模型处理，避免静默使用错误模型。
- StartLux 成功响应字段尚未真机确认；假服务端覆盖官方和价格表描述的形状，真实字段不符时只调整解析器并补测试。
- 完整地址输入容易漏路径；界面示例和测试必须明确，不靠代码猜测。
- 当前工作区已有大量未提交改动；每个任务只暂存自己的文件，避免夹带。