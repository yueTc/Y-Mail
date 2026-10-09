# em-master 邮件自动分类贴标签 — 设计规格（Spec）

| 项 | 值 |
|---|---|
| 文档版本 | v1.0 |
| 日期 | 2026-10-09 |
| 状态 | 已确认（2026-10-09 用户点头：类别预置一套、可改可删；把握不够归「其他」；标签显示在列表时间前面；jev 按新增站点类型接入；开关默认关，打开时一次性授权后新邮件自动外发） |
| 依据 | `docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md`（R10 AI 与翻译）；`docs/superpowers/specs/2026-10-08-notification-ai-verification-design.md`（一次性授权的先例） |
| 范围 | 新增 TypeSafe jev 站点类型、新增「邮件分类」AI 功能、本地标签体系、收件箱列表标签显示、搜索按标签筛选 |
| 变更记录 | v1.0（2026-10-09）：新增。 |

---

## 0. 需求溯源（五问）

1. **谁要的**：项目所有者本人。
2. **解决什么已发生的问题**：多个邮箱汇进统一收件箱后邮件混在一起，要按类别看只能自己翻。现在没有任何本地标签，只有文件夹和红旗。
3. **不做会怎样**：收件箱继续是一大堆混着的邮件，想只看某一类（比如只看账单）只能靠搜关键词。
4. **怎么算做成**：见第 5 节验收标准 R1–R23。打开开关后新邮件自动贴标签；标签显示在列表时间前面；搜索栏能按标签筛。
5. **为什么不是只改样式**：要新增第四种 AI 站点类型、新的 AI 功能、一整套本地标签表与读写，还要改收件箱列表和搜索，并且要把「新邮件正文自动外发」这条例外讲清楚。

---

## 1. 目标与边界

### 1.1 本规格包含

- 新增 AI 站点类型「TypeSafe jev」，与现有三种站点并列；密钥同样只进系统保险箱。
- 新增 AI 功能「邮件分类」，复用现有的功能级站点与模型选择。
- 新增本地标签体系。预置八条类别：客户、账单、通知、推广、招聘、同事、家人、其他。可改名、可增删、可调顺序。
- 新增「邮件自动分类」开关，默认关闭。打开时一次性授权，之后新邮件正文自动外发。
- 新邮件落地后自动调用 jev，给邮件贴一个标签；把握不够归「其他」。
- 邮件列表行在时间前面显示标签；会话折叠行显示最新一封的标签。
- 搜索栏新增 `label:` 筛选，可与 `from:` / `has:` / `is:` / `before:` 叠加。
- 读信界面可手动改标签。
- 手动分类命令，对指定邮件立即分类。

### 1.2 本规格不包含

- 不做「需要我回复吗」「多紧急」这类判断。这一轮只做分类。
- 不把标签同步回邮箱服务器。IMAP 没有跨服务商统一的标签概念，Gmail 的标签在 IMAP 里表现为文件夹；本地标签是纯本地数据。
- 不自动删邮件、不自动改文件夹、不自动标已读、不自动归档。
- 不给分类理由。模型只返回类别与把握程度，不返回解释文本。
- 不让 jev 承担翻译、摘要、润色、起草、通知识别；这些继续由现有站点承担。
- 不给 OpenAI 兼容 / DeepL / Ollama 站点开放分类能力；分类只认 jev。
- 不做 macOS / Linux 适配；项目当前以 Windows 为发布目标。

---

## 2. 架构决策（含反选理由）

### Decision D1：jev 作为第四种站点类型接入，复用现有站点体系

- **Reason**：密钥保管、外发授权、审计、一键关闭、功能级模型映射都能直接复用。用户明确要求 jev「像其他 LLM 一样填写」。
- **Alternatives Considered**：单独开一块「jev 设置」；不走站点体系、在分类功能里直接内置。
- **Rejected Because**：单开一块会把「站点」概念劈成两半，密钥与审计要写两遍；直接内置则完全复用不到外发授权与审计，是安全上的倒退。
- **Trade-offs Accepted**：后端要新写一套 jev 的请求与应答适配，因为它的接口形状与 OpenAI 兼容站点不同。

### Decision D2：标签是纯本地概念，只写本地库，不回写服务器

- **Reason**：IMAP 没有跨服务商统一的标签概念；强行映射成文件夹会污染用户服务器上的文件夹结构。
- **Alternatives Considered**：Gmail 账号用 Gmail 标签，其他账号用文件夹模拟。
- **Rejected Because**：会引入账号类型分支，还要处理服务器侧冲突与回滚，代价远超收益。
- **Trade-offs Accepted**：标签不跨设备同步。重装或换机器后要重新分类。这是本规格明确接受的限制。

### Decision D3：一封邮件一个标签；把握不够归「其他」

- **Reason**：用户说的是「归类」，是单选语义。单选也让列表显示、搜索筛选、手动修改都简单。
- **Alternatives Considered**：一封邮件多个标签。
- **Rejected Because**：用户明确表述为归类；多标签会让「把握不够」这条规则失去明确落点。
- **Trade-offs Accepted**：「其他」是唯一兜底标签，不允许删除（只允许改名）；其余七条可随意增删改。

### Decision D4：开关即长期授权，管新邮件；历史邮件走手动

- **Reason**：复用「通知智能识别」已经确立的例外模式（见 2026-10-08 规格 D1），用户已选择该模式。
- **Alternatives Considered**：每封新邮件逐次弹窗授权；只做手动分类。
- **Rejected Because**：逐封弹窗在实际使用中不可维护；只做手动则不满足「打开后自动」的要求。
- **Trade-offs Accepted**：这是对主规格 R10「每次调用前要求授权」的第二处明确例外。风险必须在开关打开时一次性讲清：发往哪个站点、哪个模型、是否本地、以后不再逐封确认。

### Decision D5：分类失败不贴标签、不打扰用户

- **Reason**：分类是增强能力，不是主流程。失败不该影响收信，也不该弹窗打断。
- **Alternatives Considered**：失败时贴「其他」；失败时弹提示。
- **Rejected Because**：把网络故障与「模型不确定」混成同一个标签，会让「其他」失去意义，用户分不清两者；弹窗会把后台失败变成前台噪音。
- **Trade-offs Accepted**：用户不会主动知道某封邮件分类失败了，只能从「没有标签」察觉。审计表里有记录可查。

### Decision D6：站点按能力过滤功能列表

- **Reason**：jev 只能做分类，做不了翻译 / 摘要 / 润色 / 起草 / 通知识别；反过来，聊天类站点做不了分类。不过滤的话用户会选到必然失败的组合。
- **Alternatives Considered**：不过滤，选了不支持的组合由后端报错。
- **Rejected Because**：让用户在界面上选出必然失败的组合是坏体验，也容易和「思考程度自动降级」那类静默兜底混淆。
- **Trade-offs Accepted**：站点能力要随新站点加入同步维护；以后新增站点类型必须显式声明支持哪些功能。

### Decision D7：模型名固定为 `jev-latest`，测试连接用试探请求

- **Reason**：TypeSafe 官方接口没有「拉取模型列表」这一项，模型名由请求体里的 `model` 字段指定，官方推荐值就是 `jev-latest`。
- **Alternatives Considered**：照搬现有「测试连接并拉取模型」流程。
- **Rejected Because**：jev 没有模型列表接口，照搬会必然失败。
- **Trade-offs Accepted**：jev 站点界面不提供「拉取模型」，改为一次极小的试探分类请求；模型名可手工改成锁定版本号（如 `jev-1.13.0`）。

### Decision D8：分类开关、门槛与标签都只存本机，不参与设置同步

- **Reason**：标签是本地数据（见 D2），分类结果也是本地数据。如果开关和门槛跟着账号同步跑到另一台机器，会出现「开关开着、标签却被改过」的错位。
- **Alternatives Considered**：照搬「通知智能识别」开关的做法，存进 `setting` 表并纳入设置同步的打包范围。
- **Rejected Because**：会造成跨设备状态不一致；标签本身不传，单独同步一个开关没有意义。
- **Trade-offs Accepted**：换机器后要在新机器上重设一次开关和门槛。这跟标签要重新跑是同一个代价，不额外增加负担。

---

## 3. 接口与数据

### 3.1 数据模型（迁移 `0013_mail_label_classification.sql`）

迁移做五件事。整条迁移由迁移器放在单独事务里执行，失败自动回滚。

**（1）重建 `ai_provider`，放开 `kind` 约束。**

现有约束是 `CHECK (kind IN ('openai_compatible', 'deepl', 'ollama'))`，插入 `typesafe` 会被拒。SQLite 改不了已有 CHECK，按官方建议重建：建新表 → 搬数据 → 删旧表 → 改名。新约束加上 `'typesafe'`。

**（2）重建 `ai_model_map`，放开 `function` 约束。**

`0012` 已把约束放宽到 `('translate', 'summary', 'polish', 'draft', 'notification_verify')`。这一条再加 `'classify'`。做法与 `0012` 相同。

**（3）建标签表 `mail_label`。**

```sql
CREATE TABLE IF NOT EXISTS mail_label (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    sort_order  INTEGER NOT NULL DEFAULT 0,
    is_fallback INTEGER NOT NULL DEFAULT 0 CHECK (is_fallback IN (0, 1)),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_mail_label_name ON mail_label (name);
CREATE UNIQUE INDEX IF NOT EXISTS idx_mail_label_fallback ON mail_label (is_fallback) WHERE is_fallback = 1;
```

`is_fallback = 1` 唯一一条，就是「其他」。那个偏索引保证兜底标签有且只有一个。

**（4）建邮件-标签对应表 `message_label`。**

```sql
CREATE TABLE IF NOT EXISTS message_label (
    message_id INTEGER PRIMARY KEY REFERENCES message(id) ON DELETE CASCADE,
    label_id   INTEGER NOT NULL REFERENCES mail_label(id) ON DELETE CASCADE,
    source     TEXT NOT NULL CHECK (source IN ('auto', 'manual')),
    confidence REAL,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_message_label_label ON message_label (label_id);
```

`message_id` 做主键，从结构上保证一封邮件只有一个标签（见 D3）。`ON DELETE CASCADE` 保证删邮件或删标签时不留下悬空记录。

**（5）预置八条标签。**

```sql
INSERT INTO mail_label (name, sort_order, is_fallback) VALUES
    ('客户', 10, 0), ('账单', 20, 0), ('通知', 30, 0), ('推广', 40, 0),
    ('招聘', 50, 0), ('同事', 60, 0), ('家人', 70, 0), ('其他', 1000, 1);
```

### 3.2 jev 站点适配（`crates/mail-ai`）

- `ProviderKind` 新增 `TypeSafe`，存库标识 `"typesafe"`，界面名「TypeSafe jev」，默认地址 `https://api.typesafe.ai/v1`。
- 新增能力声明：站点类型 → 支持的功能集合。
  - OpenAI 兼容：翻译、摘要、润色、起草、通知识别。
  - DeepL：翻译。
  - 本机 Ollama：翻译、摘要、润色、起草、通知识别。
  - TypeSafe jev：邮件分类。
- 新增 jev 请求适配：
  - 地址：`POST {base_url}/systemone`。
  - 请求体：`{ state, model, questions }`。`state` 是邮件正文纯文本；`questions` 里放一个问题 `category`，类型是 `choice`，`criteria` 是当前所有标签（标签名作选项）。
  - 应答：取 `answers.category.choice` 与 `answers.category.confidence`。
  - 认证：`Authorization: Bearer <密钥>`，密钥取自系统保险箱。
- 超时 15 秒，与「通知识别」一致。
- jev 站点不提供「拉取模型列表」；「测试连接」改为一次极小的试探分类请求。

### 3.3 新增 AI 功能 `classify`（`crates/mail-core`）

- `AiFunction` 新增 `Classify`，存库标识 `"classify"`。
- 复用现有功能级模型映射：站点 + 模型 + 思考程度。jev 不支持思考程度，选择时按现有「不支持则自动降级」规则处理，并在界面标注。
- 复用现有一次性外发授权预览结构（明示站点、模型、域名、是否本地）。

### 3.4 命令与前端契约

新增后端命令：

| 命令 | 作用 |
|---|---|
| `list_mail_labels` | 取标签列表（含排序与是否兜底） |
| `create_mail_label` | 新增标签 |
| `rename_mail_label` | 改名 |
| `delete_mail_label` | 删除（兜底标签拒绝删除） |
| `reorder_mail_labels` | 调顺序 |
| `classify_messages` | 对指定邮件立即分类 |
| `set_message_label` | 手动设置或清除某封邮件的标签 |

修改：

- 邮件列表查询（`InboxMessage`）增加标签字段：`labelId` 与 `labelName`；没有标签时为 `null`。
- 搜索解析新增 `label:` 分支。
- AI 站点与功能的返回结构增加「支持的功能」，供界面过滤。

### 3.5 开关与门槛存哪

- 「邮件自动分类」开关与「把握程度门槛」照现有做法存进本机 `settings.json`（`src-tauri/src/settings.rs`）：开关默认 `false`，门槛默认 `0.75`。
- 按 D8，这两个字段**不进** `setting` 表、**不进**设置同步的打包范围。
- 老配置里缺这两个字段时按默认值处理，不报错。
- 界面改开关或门槛后立刻落盘，并同步更新外壳里的共享状态（照现有 `notify_ai_enabled` 的写法）。

---

## 4. 行为

### 4.1 设置页行为（AI 功能内）

- 站点类型下拉新增「TypeSafe jev」，从三项变四项。
- jev 站点表单：地址（预填 `https://api.typesafe.ai/v1`，可改）、密钥（只进系统保险箱）、模型（默认 `jev-latest`）、启用开关。「拉取模型」按钮不出现，换成「测试连接」。
- 功能级模型区新增一行「邮件分类」。这一行的站点下拉**只列支持分类的站点**（当前就是 jev）。
- 新增「邮件自动分类」开关，默认关闭。
- 开关旁新增「把握程度门槛」数字输入，默认 `0.75`，允许范围 `0.50`–`0.99`。
- 新增「分类标签管理」区：列出所有标签，可改名、删除、新增、上下调序。兜底标签「其他」的删除入口禁用，并有说明。

### 4.2 自动分类流程

打开开关时的确认弹窗必须写清：

- 新邮件正文会自动发给所选的 AI。
- 发往哪个站点（含完整域名）、用哪个模型、是不是本地。
- 开启后不再逐封确认。
- 关闭开关后立即停止自动外发。

确认后才落盘为开启；取消则保持关闭。

开关打开后，每轮新邮件扫描中，对每封新落地邮件按以下顺序处理：

1. 检查分类开关是否打开；没打开直接跳过。
2. 检查该邮件是否已经有标签；有就跳过（不覆盖，包括用户手动改过的）。
3. 检查是否存在可用的分类站点与模型；没有则跳过，不报错。
4. 从本地库读正文纯文本。正文为空、读取失败或只有附件时跳过。
5. 只取正文前 2000 个字符发出去。正文是 HTML 时先按现有切段逻辑取纯文本；远程图片不加载，附件不发送。
6. 调用 jev，单封最长等 15 秒。
7. 解析结果：
   - 拿到合法的选择结果且把握 ≥ 门槛：贴它选中的标签，来源记 `auto`。
   - 拿到合法结果但把握 < 门槛：贴兜底标签「其他」。
   - 超时、网络失败、模型报错、结果不合法：**不贴标签**，记一条审计，继续下一封。
8. 后台串行或小并发处理，不阻塞收信；每轮最多处理 20 封，剩下的下一轮继续。

### 4.3 手动分类

- 收件箱里可选中若干邮件，点「分类」，只对选中的邮件执行 4.2 的第 4–7 步。
- 手动分类不受自动开关影响，不需要先开开关。
- 每次执行前弹一次确认，写明发往的站点、模型、是否本地、本批邮件数量。确认后执行。
- 手动分类会覆盖已有标签。
- 手动分类的结果来源记 `auto`；只有用户在读信界面手改才算 `manual`。

### 4.4 标签的显示与手动修改

- 邮件列表行：在时间那一格前面插入标签。没有标签就不显示。标签用一个小胶囊样式，配色沿用现有账号色标体系，不引入新配色。
- 会话折叠行：显示该会话最新一封邮件的标签。
- 列表里的标签可点击，点击等于执行对应的 `label:` 搜索。
- 读信界面加一个标签下拉，可改成任意标签或清空。改完立刻刷新列表。
- 手动改过的记录来源记 `manual`；自动流程本来就会跳过已有标签的邮件（见 4.2 第 2 步），不会再覆盖它。

### 4.5 搜索

- 新增 `label:` 筛选，例如 `label:客户`。
- 按标签名精确匹配。标签名是英文时忽略大小写。
- 可与现有四种筛选叠加，例如 `发票 label:账单 from:alice`。
- 只有标签筛选、没有关键词时，走普通数据库过滤，不进全文检索。
- 搜索栏的提示文案补上 `label:`。

### 4.6 类别管理

- 改名：立刻生效；已贴该标签的邮件跟着显示新名字。
- 新增：新标签立刻出现在分类选项和列表筛选里。
- 删除：该标签下的邮件标签一并清除（外键级联），这些邮件变成「没有标签」。
- 调序：只影响设置里和下拉里的显示顺序。
- 兜底标签「其他」：不能删，可以改名。改名不影响兜底逻辑（按 `is_fallback` 判断，不按名字）。

---

## 5. 验收标准

- **R1**（MUST）AI 设置站点类型下拉出现「TypeSafe jev」；填密钥后可保存，密钥只进系统保险箱，不入库不入日志。
- **R2**（MUST）jev 站点提供「测试连接」，不提供「拉取模型」；模型默认 `jev-latest`，可手工改。
- **R3**（MUST）功能级模型区出现「邮件分类」一行；该行的站点下拉只列支持分类的站点。
- **R4**（MUST）「邮件自动分类」开关默认关闭。
- **R5**（MUST）站点或模型没配好时不能打开开关，给出中文提示。
- **R6**（MUST）打开开关时弹一次确认，写清站点、域名、模型、是否本地、「以后不再逐封确认」；确认后才落盘。
- **R7**（MUST）开关关闭时，后台不发送任何邮件正文。
- **R8**（MUST）开关打开后，新邮件落地自动分类；每封只发正文前 2000 个字符。
- **R9**（MUST）把握 ≥ 门槛时贴模型选中的标签；把握 < 门槛时贴「其他」。
- **R10**（MUST）超时、网络失败、结果不合法时不贴标签，不影响收信，不弹窗打扰。
- **R11**（MUST）自动流程跳过已经有标签的邮件，不覆盖用户手动改过的结果。
- **R12**（MUST）邮件列表行在时间前面显示标签；没有标签时不显示占位。
- **R13**（MUST）会话折叠行显示该会话最新一封邮件的标签。
- **R14**（MUST）搜索栏支持 `label:标签名`，并能与 `from:` / `has:` / `is:` / `before:` 叠加。
- **R15**（MUST）点列表里的标签，等于执行对应的 `label:` 搜索。
- **R16**（MUST）设置里能查看、改名、新增、删除、排序标签。
- **R17**（MUST）兜底标签「其他」不能被删除；可以改名。
- **R18**（MUST）删除标签会清除该标签下所有邮件的标签记录，不留悬空。
- **R19**（MUST）读信界面能手动改标签或清空，改完列表立刻刷新。
- **R20**（MUST）审计记录功能、站点、模型、是否外发、结果标签，不记正文。
- **R21**（MUST）设置里点「一键关闭 AI」后分类停止；已贴的标签保留。
- **R22**（MUST）手动分类只对指定邮件生效，执行前弹一次外发确认。
- **R23**（SHOULD）「把握程度门槛」输入默认 0.75，范围 0.50–0.99，改了立刻对后续分类生效。

---

## 6. 安全与隐私

- 密钥只进系统保险箱；数据库、日志、报错信息里都不出现明文。
- 正文只发前 2000 个字符；只发正文纯文本，不发附件、不加载远程图片。
- 模型的返回只当纯文本处理，**不触发发送、跳转、删邮件、改文件夹等任何动作**；分类功能唯一的写动作是给邮件贴标签，而且只写本地库。
- 默认关闭；开关关闭立即停止自动外发。
- 审计只记时间、功能、站点、模型、是否外发、结果标签，不记正文。
- 标签是本地数据，不参与账号同步、不上传。

---

## 7. File Changes

| 文件 | 类型 | 说明 |
|---|---|---|
| `crates/mail-store/src/migrations/sql/0013_mail_label_classification.sql` | new | 重建 `ai_provider` 与 `ai_model_map`，建标签表，预置八条 |
| `crates/mail-store/src/migrations/mod.rs` | edit | 注册 0013 |
| `crates/mail-store/src/label.rs` | new | 标签增删改查、邮件标签读写、按标签取邮件 |
| `crates/mail-store/src/lib.rs` | edit | 导出新模块 |
| `crates/mail-ai/src/provider.rs` | edit | 新增 `TypeSafe` 类型、能力声明、默认地址 |
| `crates/mail-ai/src/client.rs` | edit | jev 请求与应答适配 |
| `crates/mail-ai/src/prompt.rs` | edit | 分类问题的构造 |
| `crates/mail-core/src/ai.rs` | edit | 新增 `Classify` 功能、分类编排、审计 |
| `crates/mail-core/src/search.rs` | edit | `label:` 筛选 |
| `crates/mail-core/src/inbox.rs` | edit | 列表查询带出标签字段 |
| `crates/mail-core/src/sync/worker.rs` | edit | 新邮件落地后排队分类 |
| `src-tauri/src/commands.rs` | edit | 注册新命令 |
| `src/api.ts` | edit | 新命令的类型与封装 |
| `src/AiPanel.tsx` | edit | 站点类型、分类功能行、开关、门槛、标签管理 |
| `src/InboxPanel.tsx` | edit | 列表行标签、点击筛选、搜索提示 |
| `src/MessageReader.tsx` | edit | 读信界面标签下拉 |
| `src/i18n.en.ts` | edit | 新文案 |
| `src-tauri/src/settings.rs` | edit | 新增分类开关与门槛字段（默认关、0.75） |
| `src-tauri/src/state.rs` | edit | 新增分类开关的共享状态 |
| `src-tauri/src/lib.rs` | edit | 启动时把设置灌进共享状态 |

---

## 8. Tasks（实现清单）

### Wave A1：数据层（无依赖）

- 写迁移 `0013`；用「旧库升级 → 新表可用」的用例验证。
- `mail-store` 标签读写：增删改查、排序、按邮件取标签、按标签筛邮件。
- 预置八条落库测试；兜底标签唯一性测试。
- 删标签级联清空测试。

### Wave A2：AI 层（依赖 A1）

- `ProviderKind::TypeSafe` 加能力声明加默认地址。
- jev 请求与应答适配；超时；错误归类。
- `AiFunction::Classify`；`ai_model_map` 能写能读 `classify`。
- 能力过滤：功能级模型列表按功能过滤站点。

### Wave A3：编排与命令（依赖 A2）

- 分类编排：取正文 → 调 jev → 阈值判定 → 落标签 → 审计。
- 新邮件自动触发（不阻塞收信、每轮上限 20 封）。
- 手动分类命令。
- `label:` 搜索筛选。
- 列表查询带出标签字段。
- 注册全部新命令。

### Wave A4：界面（依赖 A3）

- AI 设置：站点类型加 jev、分类功能行、自动开关加授权弹窗加门槛输入。
- 标签管理区。
- 列表行标签、点击筛选、搜索提示。
- 读信界面标签下拉。
- 文案本地化。

### Wave A5：收尾

- 测试与四关验证：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`npm run build`。
- 更新 `.ai-memory/`（handoff.md、project_memory.md、当日 daily.md）。

---

## 9. 未决与未验证

- **jev 对中文邮件的分类准确率没验。** TypeSafe 官方明确说英文效果最强、其他语言差一些。这是本功能最大的不确定项。计划先拿二三十封脱敏的真实中文邮件实测，再决定默认门槛与是否上线。
- **jev 的实际价格没验。** 官方自报输入约 0.042 美元 / 百万计量单位、输出免费，我们没有实测账单。
- **jev 的服务稳定性与限流策略没验。**
- **正文 2000 字符是否够分类**，没验。长邮件只取开头可能漏掉关键信息。
- **默认门槛 0.75 是否合适**，没验；这是照搬开源示例的默认值。R23 已经把它做成可调，先上线再调。
- **标签是否要按账号区分**（同一个「账单」标签跨所有账号）本轮按全局统一处理，未单独讨论。
- **标签集与分类开关都不跨设备同步**（见 D2、D8）。换机器后要重设开关、重新分类。