# 邮件标签组（多维度并行判断）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

| 项 | 值 |
|---|---|
| 日期 | 2026-10-11 |
| 分支 | codex/mail-label-groups（从 codex/mail-auto-classification 拉出） |
| 规格 | docs/superpowers/specs/2026-10-11-mail-label-groups-design.md v1.0 |
| 前置 | 邮件自动分类贴标签已完成（迁移 0013、0014、0015 已在） |
| 总退出验证 | R1–R34 逐条可复现；四关全过；正文只在授权后外发；组、标签、分数、门槛只写本地库 |

**Goal:** 把「一封邮件一个标签」升级成「一封邮件多个标签组各一条结果」，一次 jev 请求问完所有开启的组，新增「需要我回复」和「紧急程度」两个默认组，搜索统一改成 tag:组名:值。

**Architecture:** 数据层新增 mail_label_group 表、给 mail_label 加 group_id、重建 message_label 成（邮件, 组）结构并搬家老数据；mail-ai 改成一次请求多问题、支持 choice / noul / score 三种题型；mail-core 负责多组编排、门槛换算、锁定判断和审计；Tauri 外壳的现有新邮件轮询继续串行处理；React 界面负责标签组管理、多胶囊显示、右键按组修改和 tag: 搜索。

**Tech Stack:** Rust 2021、rusqlite、tokio、serde_json、Tauri、React、TypeScript、Vitest、SQLite WAL。

**Spec:** docs/superpowers/specs/2026-10-11-mail-label-groups-design.md

## Global Constraints

- 面向用户用中文大白话；代码、命令、文件名除外。
- 邮件正文是不可信内容，绝不因正文触发发信、跳转、写库命令或执行命令。
- 密钥只进系统保险箱，数据库、日志、报错里不能有明文。
- 自动分类默认关闭；总开关打开时一次性确认，手动和右键默认每次确认（免确认开关除外）。
- 正文最多外发前 2000 个字符，只发纯文本，不发附件，不加载远程图片。
- 分类只允许 TypeSafe jev；其他站点不能承担分类。
- 一次请求携带所有已开启组的问题；不按组多次发送。
- 一封邮件每个组最多一条结果；手动改过（manual）的组锁定，自动流程不覆盖。
- 选择题低于门槛有兜底归兜底、没兜底空着；是否题始终出一条；打分题低于门槛空着。
- 组、标签、颜色、分数、门槛只写本地库，不同步账号，不上传服务器；颜色只能从固定色板选，模型不接触颜色。
- 搜索统一 tag:组名:值；label: 删除，不做兼容。
- 每批改动必须通过：cargo fmt --all --check、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace、npm run build。
- Windows 终端找不到 cargo 时，先执行：$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"。
- 写文件使用 UTF-8 无 BOM、LF；提交信息用中文；禁止提交 *.db、.env、密钥文件和构建产物。

---

## 0. 现状与前置

- 迁移清单在 crates/mail-store/src/migrations/mod.rs，当前最后一条是 0015_folder_child_create_blocked；本功能用 0016。
- 分类数据层在 crates/mail-store/src/label.rs（MailLabel、MailLabelSource、mail_label / message_label 读写）。
- 收件箱 SQL 在 crates/mail-store/src/inbox.rs，搜索解析与 SQL 在 crates/mail-store/src/search.rs。
- 分类编排在 crates/mail-core/src/ai.rs（classify_message、classify_message_if_untagged、classify_messages、preview_classify_messages、resolve_ai_target）。
- jev 协议在 crates/mail-ai/src/client.rs（systemone_url、ClassifyOutcome、classify_mail）与 crates/mail-ai/src/provider.rs（ProviderKind::TypeSafe、ProviderFunction::Classification）。
- 后台轮询在 src-tauri/src/notify.rs（SCAN_LIMIT、CLASSIFY_BATCH_LIMIT、drain_classify_queue）；开关与门槛是 Arc<AtomicBool>/Arc<AtomicU8>。
- 命令在 src-tauri/src/commands.rs；设置模型在 src-tauri/src/settings.rs；前端封装在 src/api.ts。
- 界面：标签管理在 src/AiPanel.tsx（分类标签管理区），列表胶囊在 src/InboxPanel.tsx，读信下拉在 src/MessageReader.tsx，颜色映射在 src/index.css。

## 1. 结构决策

- P-D1：组表与结果表都放 mail-store 的 label.rs，不新建 crate；外键级联必须由唯一写库层保证。
- P-D2：message_label 主键改成（message_id, group_id），保证一封邮件每组只有一条结果；重跑就是覆盖。
- P-D3：打分题只存上下限，档位描述由代码生成，不入库；避免描述漂移。
- P-D4：一次请求多问题用 questions 对象，键是 g<组编号>；应答按同一键取回，再逐组校验类型。
- P-D5：手动改过（source = manual）的组锁定；自动流程和「立即分类」都跳过它，其它组照常。
- P-D6：组开关、门槛、上下限、排序存 mail_label_group 表；总开关与免确认开关继续存 settings.json。
- P-D7：列表结果由 mail-store 查询直接带出 groupResults 数组，线程折叠行取最新一封；前端只显示和点击。
- P-D8：打分胶囊颜色按数值在区间里的相对位置分 5 档；前端用 data-score-band 加 CSS 映射，禁止接收任意色值。
- P-D9：搜索解析新增 tag: 分支，删掉 label: 分支；组名里禁止英文冒号。

## 2. 规格与实际路径的对应

| 规格写法 | 实际路径 | 说明 |
|---|---|---|
| crates/mail-store/src/label.rs 只有标签 | 同文件加组与组结果读写 | 组和标签强关联，放同一模块 |
| 迁移 0016 | crates/mail-store/src/migrations/sql/0016_mail_label_groups.sql | 0014、0015 已被文件夹功能占用 |
| src/api.ts 的 labelId/labelName/labelColor | 换成 groupResults 数组 | 一封邮件多组结果 |
| crates/mail-ai/src/client.rs 的 classify_mail | 新增多问题版本 classify_mail_groups | 一次请求问完所有组 |

---

## 3. Wave 与原子任务

### Wave B1：数据层

#### Task 1：迁移 0016 与标签组表

**Files:**
- Create: crates/mail-store/src/migrations/sql/0016_mail_label_groups.sql
- Modify: crates/mail-store/src/migrations/mod.rs
- Test: crates/mail-store/src/migrations/mod.rs

**Interfaces:**
- Consumes: 现有迁移器 run_migrations，第 15 条迁移之后。
- Produces: mail_label_group 表；mail_label 加 group_id 与组内唯一约束；message_label 重建为（邮件, 组）；三个默认组；老数据搬家。

- [ ] **Step 1: 先写失败测试**

在 crates/mail-store/src/migrations/mod.rs 测试模块里把最新版本断言改成 16，并补两条：`migration_0016_creates_label_groups`（默认三组、类型顺序 choice/boolean/score）、`migration_0016_keeps_old_labels_in_content_group`（八条老标签都在内容分类组、兜底仍有一个）。

- [ ] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-store migration_0016 -- --nocapture
Expected: FAIL，mail_label_group 表不存在。

- [ ] **Step 3: 写迁移**

写 crates/mail-store/src/migrations/sql/0016_mail_label_groups.sql，按顺序：

1. 建 mail_label_group（id、name、kind、enabled、confidence_threshold、score_min、score_max、sort_order、created_at、updated_at），kind 限制 choice/boolean/score，name 唯一。
2. 插三个默认组：内容分类（choice，门槛 0.75，排序 10）、需要我回复（boolean，门槛 0.5，排序 20）、紧急程度（score，1–5，门槛 0.75，排序 30）；enabled 全为 0。
3. 重建 mail_label：加 group_id 列（NOT NULL REFERENCES mail_label_group(id) ON DELETE CASCADE），把现有八条标签全部指向内容分类组；旧索引 idx_mail_label_name 换成 idx_mail_label_group_name(group_id, name)；idx_mail_label_fallback 换成 idx_mail_label_group_fallback(group_id) WHERE is_fallback = 1。
4. 插「需要我回复」组两条标签：需要回复（red，排序 10）、无需回复（gray，排序 20），都不是兜底。
5. 重建 message_label：新结构（message_id, group_id, label_id, score_value, source, confidence, updated_at），主键（message_id, group_id），CHECK ((label_id IS NULL) <> (score_value IS NULL))。
6. 搬老数据：老 message_label 的每条记录带内容分类组号写进新表，score_value 为 NULL。
7. 建 idx_message_label_group 与 idx_message_label_label。

迁移全程在一个事务里；失败整体回滚。

- [ ] **Step 4: 跑测试确认通过**

Run: cargo test -p mail-store migration_0016 -- --nocapture
Expected: PASS，两条都过。

- [ ] **Step 5: 提交**

    git add crates/mail-store/src/migrations/sql/0016_mail_label_groups.sql crates/mail-store/src/migrations/mod.rs
    git commit -m "数据：新增标签组表并搬家老标签"

#### Task 2：标签组数据类型与增删改查

**Files:**
- Modify: crates/mail-store/src/label.rs
- Modify: crates/mail-store/src/lib.rs
- Test: crates/mail-store/src/label.rs

**Interfaces:**
- Consumes: Task 1 的 mail_label_group 表。
- Produces: LabelGroupKind 枚举、LabelGroup 结构、list_label_groups / create_label_group / rename_label_group / set_label_group_enabled / set_label_group_threshold / delete_label_group / reorder_label_groups。

- [ ] **Step 1: 先写失败测试**

在 crates/mail-store/src/label.rs 测试模块补三条：`label_group_crud_and_uniqueness`（建组、重名报错、改名、开关、门槛、删组）、`score_group_validates_range`（上限小于下限、上下限相等、档位超过 10 都要报错，1–5 通过）、`group_name_rejects_colon`（组名含英文冒号要报错）。

- [ ] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-store label_group -- --nocapture
Expected: FAIL，LabelGroupKind 不存在。

- [ ] **Step 3: 实现**

在 label.rs 加：

- `pub enum LabelGroupKind { Choice, Boolean, Score }`，as_str 用 choice / boolean / score，parse 反向。
- `pub struct LabelGroup { id, name, kind, enabled, confidence_threshold, score_min, score_max, sort_order }`。
- list_label_groups 按 sort_order 升序；create_label_group 校验名字非空、≤64 字、不含英文冒号、不重名；Score 类型必须有 min < max 且档位数（max-min+1）在 2–10；Choice/Boolean 的 min/max 必须为空。
- rename / set_enabled / set_threshold（0.5–0.99）/ delete / reorder（必须传全部组编号，去重、齐全，否则报错）。
- 在 crates/mail-store/src/lib.rs 导出新类型与函数。

- [ ] **Step 4: 跑测试确认通过**

Run: cargo test -p mail-store label_group -- --nocapture
Expected: PASS。

- [ ] **Step 5: 提交**

    git add crates/mail-store/src/label.rs crates/mail-store/src/lib.rs
    git commit -m "数据：标签组增删改查与校验"
#### Task 3：组内标签与组结果读写

**Files:**
- Modify: crates/mail-store/src/label.rs
- Test: crates/mail-store/src/label.rs

**Interfaces:**
- Consumes: Task 1、Task 2。
- Produces: 组内标签增删改查（带 group_id）、set_message_group_value / clear_message_group_value / list_message_group_results / message_group_result。

- [ ] **Step 1: 先写失败测试**

补三条：`message_group_result_round_trip`（写一条内容分类结果、读回来、再清空）、`score_group_result_stores_number`（打分题写 3 读回 3，写 9 越界要拒）、`deleting_group_clears_results`（删组后该组结果清零）。

- [ ] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-store message_group -- --nocapture
Expected: FAIL，set_message_group_value 不存在。

- [ ] **Step 3: 实现**

- 改造现有标签函数：create_mail_label / rename / set_color / delete / reorder 都带 group_id；新增 list_group_labels(group_id)；组内名字唯一、组内最多一个兜底。
- set_message_group_value(message_id, group_id, label_id, score_value, source, confidence)：
  - Choice / Boolean：label_id 必须属于该组，score_value 必须为空。
  - Score：score_value 必须在 [min, max]，label_id 必须为空。
  - 用 INSERT ... ON CONFLICT(message_id, group_id) DO UPDATE 覆盖。
- clear_message_group_value：删该（邮件, 组）记录，返回是否有删除。
- list_message_group_results(message_id)：按组排序返回该邮件全部结果。
- 旧 set_message_label / get_message_label / clear_message_label 的调用点全部改成新函数，不允许留着写旧结构。

- [ ] **Step 4: 跑测试确认通过**

Run: cargo test -p mail-store message_group -- --nocapture
Expected: PASS。

- [ ] **Step 5: 提交**

    git add crates/mail-store/src/label.rs
    git commit -m "数据：组内标签与组结果读写"

#### Task 4：收件箱与搜索按组带出结果

**Files:**
- Modify: crates/mail-store/src/inbox.rs
- Modify: crates/mail-store/src/search.rs
- Test: crates/mail-store/src/inbox.rs
- Test: crates/mail-store/src/search.rs

**Interfaces:**
- Consumes: Task 3。
- Produces: InboxMessage.groupResults；tag: 搜索解析与 SQL；删除 label: 分支。

- [ ] **Step 1: 先写失败测试**

inbox.rs 补：`inbox_message_carries_group_results_in_order`（一封邮件两个组各有结果，查询后顺序是内容分类在前、紧急程度在后）、`thread_row_uses_latest_message_results`（同线程两封，折叠行取最新一封的结果）。

search.rs 补：`tag_search_filters_by_group_and_label`（两封分别贴账单和通知，tag:内容分类:账单 只回第一封）、`tag_search_supports_score_comparison`（紧急程度 2 和 5 两封，tag:紧急程度:>=3 只回第二封，tag:紧急程度:2 只回第一封）、`label_prefix_is_no_longer_recognized`（label:账单 不再当筛选）、`tag_search_rejects_unknown_group`（组名不存在给中文提示，不静默返回空）。

- [ ] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-store tag_search -- --nocapture
Expected: FAIL，tag: 未实现。

- [ ] **Step 3: 实现**

- inbox.rs：INBOX_COLUMNS 去掉 ml.label_id / l.name / l.color，改成查完后按 message_id 批量取 message_label + mail_label + mail_label_group，组装 groupResults 数组（按组 sort_order 升序）。
- row_to_message 相应改成先收基础字段，再由上层填充 groupResults；线程折叠行取最新一封的结果。
- search.rs：删掉 label: 分支；新增 tag: 解析：
  - 形式 tag:组名:值；组名必写，组名不存在报中文错。
  - 值以 >= 或 <= 开头时按分数比较，只对打分题组有效。
  - 其它情况按标签名精确匹配（英文忽略大小写），只对选择题/是否题组有效。
- 同步改 src/api.ts 的 InboxMessage 类型（前端行为在 Task 11 做，这里先改类型）。

- [ ] **Step 4: 跑测试确认通过**

Run: cargo test -p mail-store -- --nocapture
Expected: PASS，含旧 inbox / search 用例（按新结构改断言）。

- [ ] **Step 5: 提交**

    git add crates/mail-store/src/inbox.rs crates/mail-store/src/search.rs
    git commit -m "数据：列表带出多组结果与 tag 搜索"

### Wave B2：AI 层

#### Task 5：jev 多问题请求与三种题型解析

**Files:**
- Modify: crates/mail-ai/src/client.rs
- Modify: crates/mail-ai/src/prompt.rs
- Test: crates/mail-ai/src/client.rs

**Interfaces:**
- Consumes: 现有 systemone_url、auth_headers、request、ProviderKind::TypeSafe。
- Produces: GroupQuestion 结构、GroupAnswer 枚举、classify_mail_groups 函数。

- [ ] **Step 1: 先写失败测试**

补三条：`score_levels_are_generated_by_range`（1–5 出 5 条、1–10 出 10 条、1–11 返回空）、`multi_question_request_sends_all_groups_and_parses_three_types`（假服务端断言请求体里 questions 有 g1 choice、g2 noul、g3 score，返回三类答案并解析）、`multi_question_skips_group_with_wrong_type`（某组 type 对不上只跳过该组）。

- [ ] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-ai multi_question -- --nocapture
Expected: FAIL，classify_mail_groups 不存在。

- [ ] **Step 3: 实现**

- prompt.rs 加 score_levels(min, max) -> Vec<String>：按档位数取固定描述表；2–10 档有效，越界返回空。
- client.rs：
  - `pub enum GroupAnswer { Choice { label: String, confidence: f64 }, Boolean { probability: f64 }, Score { level: i64, confidence: f64 } }`
  - `pub async fn classify_mail_groups(route, endpoint, model, state, questions: &[GroupQuestion], timeout) -> Result<Vec<(i64, GroupAnswer)>, AiError>`，返回（组编号, 答案）。
  - 请求体按规格 3.2 节形状组装；choice 的 criteria 是「标签名 → null」；noul 只有 instructions；score 的 criteria 是有序档位描述数组。
  - 解析：按 g<编号> 取 answer；type 对不上跳过该组；choice 不在候选里跳过；confidence / 概率越界跳过；score 的 level 不在 [min, max] 跳过。
  - 旧的单问题 classify_mail 在 Task 6 改完编排后再删。

- [ ] **Step 4: 跑测试确认通过**

Run: cargo test -p mail-ai -- --nocapture
Expected: PASS。

- [ ] **Step 5: 提交**

    git add crates/mail-ai/src/client.rs crates/mail-ai/src/prompt.rs
    git commit -m "AI：jev 一次请求多问题与三种题型解析"

#### Task 6：分类编排改成按组落结果

**Files:**
- Modify: crates/mail-core/src/ai.rs
- Test: crates/mail-core/src/ai.rs

**Interfaces:**
- Consumes: Task 3、Task 5。
- Produces: classify_message_groups（单封多组）、classify_messages 改成多组、classify_message_if_untagged 改成多组。

- [ ] **Step 1: 先写失败测试**

补六条：`choice_group_below_threshold_falls_back`（把握 0.6、门槛 0.75、有兜底 → 落兜底）、`choice_group_below_threshold_without_fallback_writes_nothing`、`boolean_group_always_writes_one_of_two`（0.9 落第一条、0.2 落第二条）、`score_group_takes_highest_probability_level`（最高档是第 3 档 → 落 3）、`manual_group_is_not_overwritten`（已 manual 的组跳过、其它组照写）、`request_failure_writes_nothing`（超时 → 一组都不写，记审计）。

- [ ] **Step 2: 跑测试确认失败**

Run: cargo test -p mail-core classify_message_groups -- --nocapture
Expected: FAIL，新函数不存在。

- [ ] **Step 3: 实现**

- 取 enabled 组（按 sort_order）；没有就返回「没有开启任何标签组」。
- 校验每组能出结果：Choice 至少一条标签；Boolean 正好两条；Score 上下限合法。
- 读正文、截 2000 字符、算哈希。
- 组 questions 发一次 classify_mail_groups。
- 逐组换算：Choice（把握够落选中标签，不够有兜底落兜底、没兜底跳过）；Boolean（概率够落第一条，否则第二条）；Score（把握够落数字，不够跳过）。
- 写库前查该（邮件, 组）是不是 manual；是就跳过。
- 写审计：组名 + 结果摘要。
- preview_classify_messages 增加「本次将开启哪些组」字段。
- 删掉旧的单问题 classify_mail 与只贴一个标签的分支。

- [ ] **Step 4: 跑测试确认通过**

Run: cargo test -p mail-core -- --nocapture
Expected: PASS。

- [ ] **Step 5: 提交**

    git add crates/mail-core/src/ai.rs crates/mail-ai/src/client.rs
    git commit -m "业务：分类编排改成按组落结果"

#### Task 7：后台轮询与开关接线

**Files:**
- Modify: src-tauri/src/notify.rs
- Modify: src-tauri/src/state.rs
- Modify: src-tauri/src/lib.rs
- Test: src-tauri/src/notify.rs

**Interfaces:**
- Consumes: Task 6。
- Produces: 后台按新多组流程处理；不再依赖全局门槛 AtomicU8。

- [ ] **Step 1: 先写失败测试**

补一条：`classify_drain_uses_group_thresholds`（关掉总开关时不处理；开着时调新编排）。

- [ ] **Step 2: 跑测试确认失败**

Run: cargo test -p em-master classify_drain -- --nocapture
Expected: FAIL 或断言不符。

- [ ] **Step 3: 实现**

- drain_classify_queue 改成调 classify_message_groups；删掉对全局 percent 的依赖（门槛从组表读）。
- spawn 的签名相应简化；lib.rs 的注册同步改。
- 保持每轮最多 20 封、单封 15 秒超时。

- [ ] **Step 4: 跑测试确认通过**

Run: cargo test -p em-master -- --nocapture
Expected: PASS。

- [ ] **Step 5: 提交**

    git add src-tauri/src/notify.rs src-tauri/src/state.rs src-tauri/src/lib.rs
    git commit -m "后台：轮询改用多组分类流程"
### Wave B3：命令与设置

#### Task 8：Tauri 命令层

**Files:**
- Modify: src-tauri/src/commands.rs
- Modify: src-tauri/src/lib.rs
- Test: src-tauri/src/commands.rs

**Interfaces:**
- Consumes: Task 3、Task 6。
- Produces: list_label_groups、create_label_group、rename_label_group、set_label_group_enabled、set_label_group_threshold、delete_label_group、reorder_label_groups、set_message_group_value、clear_message_group_value；改造后的标签命令与分类命令。

- [ ] **Step 1: 先写失败测试**

补两条：`label_group_commands_round_trip`（建组 → 列表 → 改名 → 开关 → 删组）、`set_message_group_value_rejects_foreign_label`（拿 A 组的标签往 B 组写要报错）。

- [ ] **Step 2: 跑测试确认失败**

Run: cargo test -p em-master label_group_commands -- --nocapture
Expected: FAIL，命令不存在。

- [ ] **Step 3: 实现**

- 新增组命令，DTO 带 id、name、kind、enabled、confidenceThreshold、scoreMin、scoreMax、sortOrder、labels。
- 标签命令（create / rename / set_color / delete / reorder）增加 groupId 参数。
- 删掉 list_mail_labels 与 set_message_label；换成 list_label_groups 与 set_message_group_value / clear_message_group_value。
- preview_classify_messages 返回里加 enabledGroups（组名 + 类型）。
- classify_messages 返回改成按邮件分组的各组结果。
- 在 src-tauri/src/lib.rs 的 invoke_handler 里增删对应注册。
- 错误文案全中文，不吐英文原文与堆栈。

- [ ] **Step 4: 跑测试确认通过**

Run: cargo test -p em-master -- --nocapture
Expected: PASS。

- [ ] **Step 5: 提交**

    git add src-tauri/src/commands.rs src-tauri/src/lib.rs
    git commit -m "命令：新增标签组命令并改造标签与分类命令"

#### Task 9：设置模型与老门槛搬家

**Files:**
- Modify: src-tauri/src/settings.rs
- Modify: src-tauri/src/commands.rs
- Test: src-tauri/src/settings.rs

**Interfaces:**
- Consumes: Task 2、Task 8。
- Produces: 总开关与免确认开关继续生效；老的全局门槛一次性搬进「内容分类」组。

- [ ] **Step 1: 先写失败测试**

补一条：`old_threshold_migrates_into_content_group_once`（老配置 80 → 内容分类组门槛变 0.8，标记置位；再跑一次不覆盖用户后来改的值）。

- [ ] **Step 2: 跑测试确认失败**

Run: cargo test -p em-master threshold_migrates -- --nocapture
Expected: FAIL，标记字段不存在。

- [ ] **Step 3: 实现**

- settings.rs 加 `classify_confidence_migrated: bool`（默认 false），保留 classify_enabled 与 classify_skip_manual_confirm。
- 启动初始化时（AppState 建立、迁移跑完之后）读一次：没搬过就按老百分比写「内容分类」组门槛，再把标记置位；搬过就不再动。
- 老字段 classify_confidence_percent 保留可读，避免老配置报错；界面不再显示这个全局门槛。
- 总开关、免确认开关的读写命令保持原样。

- [ ] **Step 4: 跑测试确认通过**

Run: cargo test -p em-master -- --nocapture
Expected: PASS。

- [ ] **Step 5: 提交**

    git add src-tauri/src/settings.rs src-tauri/src/commands.rs
    git commit -m "设置：老门槛一次性搬进内容分类组"

#### Task 10：前端接口契约

**Files:**
- Modify: src/api.ts
- Test: src/__tests__/settingsStorage.test.tsx

**Interfaces:**
- Consumes: Task 8、Task 9。
- Produces: LabelGroup / LabelGroupKind / GroupResult 类型；组命令封装；InboxMessage.groupResults；分类命令新返回。

- [ ] **Step 1: 先写失败测试**

补一条：`inbox_message_exposes_group_results`（造一封带两组结果的邮件，断言 groupResults 顺序与字段）。

- [ ] **Step 2: 跑测试确认失败**

Run: npm test -- settingsStorage
Expected: FAIL，类型与字段不存在。

- [ ] **Step 3: 实现**

- InboxMessage 去掉 labelId / labelName / labelColor，加 groupResults: GroupResult[]。
- 新增类型 LabelGroupKind（"choice" | "boolean" | "score"）、LabelGroup、GroupResult（groupId、groupName、kind、labelId、labelName、labelColor、scoreValue）。
- 新增命令封装：listLabelGroups、createLabelGroup、renameLabelGroup、setLabelGroupEnabled、setLabelGroupThreshold、deleteLabelGroup、reorderLabelGroups、setMessageGroupValue、clearMessageGroupValue。
- 标签命令加 groupId 参数；删掉 listMailLabels 与 setMessageLabel。
- previewClassifyMessages 返回加 enabledGroups；classifyMessages 返回改成按邮件的各组结果。

- [ ] **Step 4: 跑测试确认通过**

Run: npm test -- settingsStorage
Expected: PASS。

- [ ] **Step 5: 提交**

    git add src/api.ts
    git commit -m "前端：标签组接口契约"

### Wave B4：界面

#### Task 11：标签组管理界面

**Files:**
- Modify: src/AiPanel.tsx
- Test: src/__tests__/ai.test.tsx

**Interfaces:**
- Consumes: Task 10。
- Produces: 「分类标签管理」升级成「标签组管理」。

- [ ] **Step 1: 先写失败测试**

补四条：`label_group_manager_lists_groups_and_labels`（三个默认组都列出来）、`new_group_requires_kind_specific_fields`（打分题必须填上下限、是否题不用填标签）、`group_threshold_saved_per_group`、`fallback_label_cannot_be_deleted_or_recolored`。

- [ ] **Step 2: 跑测试确认失败**

Run: npm test -- ai
Expected: FAIL，界面上还是老的分类标签管理。

- [ ] **Step 3: 实现**

- 区块标题改「标签组管理」，说明文案改成「组就是判断维度，一封邮件每组各出一个结果」。
- 组列表：组名、类型、开关、门槛、上下限（打分题）、组内标签与颜色胶囊、上移下移。
- 新建组：选类型；选择题填名字后加标签；是否题自动两条；打分题填上下限（2–10 档）。
- 组内标签：增删改、改颜色、排序、指定兜底；兜底不能删、不能改色，可改名。
- 保留总开关与免确认开关；总开关关闭时给一行说明「组开关打开也不外发」。
- 错误提示全中文。

- [ ] **Step 4: 跑测试确认通过**

Run: npm test -- ai
Expected: PASS。

- [ ] **Step 5: 提交**

    git add src/AiPanel.tsx
    git commit -m "界面：标签组管理"

#### Task 12：多胶囊显示与打分颜色

**Files:**
- Modify: src/InboxPanel.tsx
- Modify: src/MessageReader.tsx
- Modify: src/index.css
- Test: src/__tests__/inbox.test.tsx
- Test: src/__tests__/reader.test.tsx

**Interfaces:**
- Consumes: Task 10。
- Produces: 列表、搜索结果、会话折叠行、读信页按组显示多个胶囊；打分胶囊按数值分 5 档颜色。

- [ ] **Step 1: 先写失败测试**

补四条：`rows_show_group_pills_before_time`（两组结果都显示，且在时间元素前面，顺序按组）、`thread_row_shows_latest_group_pills`、`score_pill_uses_band_color`（1 灰、3 黄、5 红）、`reader_shows_group_pills`。

- [ ] **Step 2: 跑测试确认失败**

Run: npm test -- inbox
Expected: FAIL，界面还是单个标签。

- [ ] **Step 3: 实现**

- InboxPanel：MessageRow / SearchResultRow / ThreadRow 把单个标签按钮换成按 groupResults 渲染的胶囊组，统一放在 inbox-row-time 前面。
- 选择题/是否题胶囊复用 inbox-row-label 与 data-label-color；打分胶囊用新 class 与 data-score-band。
- MessageReader：读信页在时间位置附近显示同样的胶囊，点一下打开该组的手动修改。
- index.css：加 data-score-band 的 5 档颜色（浅色与深色各一套），多胶囊之间留间距，不换行挤压时间。
- 点列表胶囊触发对应的 tag: 搜索（接 Task 13）。

- [ ] **Step 4: 跑测试确认通过**

Run: npm test -- inbox reader
Expected: PASS。

- [ ] **Step 5: 提交**

    git add src/InboxPanel.tsx src/MessageReader.tsx src/index.css
    git commit -m "界面：多组胶囊与打分颜色"

#### Task 13：右键按组修改与 tag 搜索

**Files:**
- Modify: src/InboxPanel.tsx
- Modify: src/MessageReader.tsx
- Test: src/__tests__/flaggedView.test.tsx
- Test: src/__tests__/inbox.test.tsx

**Interfaces:**
- Consumes: Task 12。
- Produces: 右键菜单按组修改、清除；搜索改成 tag:。

- [ ] **Step 1: 先写失败测试**

补五条：`context_menu_lists_each_group_with_values`（选择题列标签、是否题列两条、打分题列 1–5）、`manual_set_locks_group`、`clear_group_unlocks`、`search_uses_tag_prefix`（点胶囊后搜索框是 tag:内容分类:账单）、`label_prefix_is_ignored`。

- [ ] **Step 2: 跑测试确认失败**

Run: npm test -- flaggedView
Expected: FAIL。

- [ ] **Step 3: 实现**

- 右键菜单：保留「立即分类」；下面按组列出可选值（选择题/是否题列标签，打分题列数字），每组带「清除本组结果」。
- 调 setMessageGroupValue / clearMessageGroupValue；改完刷新列表。
- 搜索：点胶囊设 tag:组名:值；搜索提示文案去掉 label:，改成 tag:。
- 免确认开关对「立即分类」继续生效。

- [ ] **Step 4: 跑测试确认通过**

Run: npm test -- flaggedView inbox
Expected: PASS。

- [ ] **Step 5: 提交**

    git add src/InboxPanel.tsx src/MessageReader.tsx
    git commit -m "界面：右键按组修改与 tag 搜索"

### Wave B5：收尾

#### Task 14：安全走查与审计断言

**Files:**
- Test: crates/mail-core/src/ai.rs
- Test: src-tauri/src/commands.rs

- [ ] **Step 1: 补安全断言**

- 总开关关闭时不读正文、不外发。
- 审计记录里没有正文、没有密钥、没有颜色和门槛。
- 组名/标签名里带换行、超长、冒号都被拒。
- 打分数字越界、标签不属于该组都被拒。

- [ ] **Step 2: 跑测试**

Run: cargo test -p mail-core -- --nocapture
Run: cargo test -p em-master -- --nocapture
Expected: PASS。

- [ ] **Step 3: 提交**

    git add crates/mail-core/src/ai.rs src-tauri/src/commands.rs
    git commit -m "验证：标签组安全与审计断言"

#### Task 15：四关验证

- [ ] **Step 1: 四关依次跑**

    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    npm run build

Expected: 四条全部退出码 0。日志存 .ai-memory/20261011/label-groups/。

- [ ] **Step 2: 前端测试**

Run: npm test
Expected: 全绿。

- [ ] **Step 3: 提交**

    git add -A
    git commit -m "验证：邮件标签组四关通过"

#### Task 16：更新记忆并汇报

**Files:**
- Modify: .ai-memory/handoff.md
- Modify: .ai-memory/project_memory.md
- Modify: .ai-memory/20261011/daily.md

- [ ] **Step 1: 写交接**

记录已完成范围、相关文件、未完成项、风险、四关命令与结果。不要贴密钥、数据库文件或正文。

- [ ] **Step 2: 自查文件**

Run: git status --short
Expected: 只能看到记忆文件修改；没有 *.db、.env、构建产物。

- [ ] **Step 3: 提交**

    git add .ai-memory/handoff.md .ai-memory/project_memory.md .ai-memory/20261011/daily.md
    git commit -m "记忆：记录邮件标签组实现结果"

---

## 4. 测试策略

- 数据层全部走真实 SQLite 临时库：覆盖旧库升级、老标签搬家、组内唯一、组内唯一兜底、打分越界、级联删除、结果覆盖。
- AI 协议用本地假 HTTP 服务：覆盖一次请求多问题、choice / noul / score 三类解析、类型不匹配、confidence 越界、超时、非 200。
- 编排测试覆盖三类换算、门槛上下分支、有兜底与没兜底、manual 锁定、整批失败不写半套。
- 收件箱 SQL 测平铺、线程折叠、多组顺序、tag: 精确匹配与 >= / <=、组名不存在。
- React 测试覆盖组管理、多胶囊顺序与颜色、右键写入与清除、tag: 搜索、label: 失效。
- 四关是总门禁，任何一项失败都不能进入收尾提交。

## 5. 风险与回退

- jev 多问题一次请求的真实行为没验：先用假服务端覆盖协议，接真机需用户提供密钥后实测；不满意时可退回每组一次请求（改动集中在 mail-ai 与编排）。
- 打分题中文档位描述区分度没验：实现后用真实邮件抽样看，必要时只调 prompt.rs 的描述表。
- 数据搬家有风险：0016 全程单事务，失败整体回滚；迁移前提示用户备份数据库。
- 老 label: 查询失效是破坏性改动：规格已明确；上线时在搜索提示里写清新写法。
- 界面胶囊变多可能挤：先按「不换行、时间固定最右」实现；真机看着挤就改成超过三个折叠成「+N」，只动 CSS 与 InboxPanel。
- 组名带冒号会打乱解析：建组时就拒掉。

## 6. 检查点

- B1 完成后：cargo test -p mail-store 全绿；迁移版本 16。
- B2 完成后：cargo test -p mail-ai、cargo test -p mail-core 的分类测试全绿。
- B3 完成后：命令与设置测试全绿；老门槛搬家用例过。
- B4 完成后：npm test 前端相关用例全绿。
- B5 完成后：四关全绿，.ai-memory 更新。

## 7. 总退出验证（R1–R34）

| 需求 | 验证任务 | 具体证据 |
|---|---|---|
| R1 | Task 1 | 迁移后八条老标签在内容分类组，颜色排序不变，老结果带组号 |
| R2 | Task 1、11 | 默认三组，类型与开关正确 |
| R3 | Task 2、11 | 组管理界面增删改查与排序 |
| R4 | Task 2、11 | 三种类型新建校验 |
| R5 | Task 2、8 | 组名全局唯一、标签名组内唯一、冒号与超长拒绝 |
| R6 | Task 3 | 删组级联清标签与结果 |
| R7 | Task 7、9 | 每组开关；总开关关闭不外发 |
| R8 | Task 2、9、11 | 组门槛默认与范围，改完立即生效 |
| R9 | Task 6、8 | 组配不全时拒绝开启并给中文提示 |
| R10 | Task 11 | 开关确认弹窗写清站点域名模型与开启组 |
| R11 | Task 5、6 | 一次请求多问题断言 |
| R12 | Task 6 | 三类换算测试 |
| R13 | Task 6 | 选择题兜底与空着分支 |
| R14 | Task 6 | 打分题门槛分支 |
| R15 | Task 6 | 是否题两条分支 |
| R16 | Task 5、6 | 单组跳过与整批失败不写 |
| R17 | Task 6、7 | 失败不影响收信、不弹窗 |
| R18 | Task 7 | 只处理新邮件 |
| R19 | Task 6 | manual 锁定不覆盖 |
| R20 | Task 8、13 | 右键按组设置与清除 |
| R21 | Task 8、13 | 立即分类单封确认 |
| R22 | Task 9、13 | 免确认开关 |
| R23 | Task 12 | 多胶囊在时间前，按组顺序 |
| R24 | Task 4、12 | 线程折叠取最新一封 |
| R25 | Task 12 | 读信页胶囊与修改 |
| R26 | Task 12 | 标签颜色与打分 5 档颜色 |
| R27 | Task 12 | 浅深主题映射 |
| R28 | Task 4、13 | tag:组名:值 与组名不存在提示 |
| R29 | Task 4 | 等于与 >= / <= 比较 |
| R30 | Task 4、13 | label: 失效、提示改写 |
| R31 | Task 13 | 点胶囊触发 tag: 搜索 |
| R32 | Task 14 | 审计字段无正文无密钥 |
| R33 | Task 9 | 一键关闭 AI 后停止外发、结果保留 |
| R34 | Task 15 | 版本保持 0.3.0，7 个版本文件一致 |

## 8. 执行交接

计划写完并提交后，按 writing-plans 的交接规则，选一种：

1. 子智能体驱动（推荐）：每个任务换一个子智能体，任务之间双重审查，适合这批跨 Rust、Tauri、React 的改动。
2. 当前会话分批执行：按 Wave 分批，在 B1、B2、B3、B4、B5 各设检查点。

两种方式都必须先读本规格和本计划；遇到规格与代码冲突时停下问用户，不猜。