# em-master AI 站点设置弹窗与站点级代理 — 设计规格（Spec）

| 项 | 值 |
|---|---|
| 文档版本 | v1.0 |
| 日期 | 2026-10-12 |
| 状态 | 已确认（2026-10-12 用户确认：新增 / 编辑 AI 站点改成弹窗；AI 站点可自己选代理，采用与邮箱账号一致的三档策略「跟随全局 / 直连 / 指定代理」，默认「跟随全局」，老站点不受影响） |
| 依据 | `docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md`（R10 AI 与翻译）；`src/AccountDialog.tsx`（弹窗交互先例）；`crates/mail-core/src/proxies.rs`（代理决议 `resolve_route_with`）；`crates/mail-store/src/migrations/sql/0002_accounts_and_proxies.sql`（账号代理字段先例） |
| 范围 | AI 站点「新增 / 编辑」由页内内联表单改为弹窗；给 AI 站点增加站点级代理策略；后端所有 AI 外发按站点自己的策略解析代理 |
| 变更记录 | v1.0（2026-10-12）：新增。 |

---

## 0. 需求溯源（五问）

1. **谁要的**：项目所有者本人。
2. **解决什么已发生的问题**：
   - 新增 / 编辑站点直接铺在 AI 设置页里，表单长、挤占列表，想同时看列表和改一个站点很别扭。
   - AI 站点现在写死「跟随全局」代理：全局设了代理后，想让某个站点（例如本机 Ollama，或某个只在国内能直连的站点）直连，或单独走某个代理，做不到。
3. **不做会怎样**：设置页里表单和列表互相挤；需要分开走的站点只能全局改来改去，或者根本走不通。
4. **怎么算做成**：见第 5 节 R1–R12。点「新增 AI 站点」或某个站点的「编辑」弹出独立弹窗，改完关掉；弹窗里能选代理策略，选「指定代理」能挑一个具体代理；不选默认「跟随全局」，老站点行为不变。
5. **为什么不是只改样式**：站点级代理要改数据库表、数据读写结构、后端 9 处外发决议、命令参数和前端表单，牵动数据层、AI 层、命令层、界面层。

---

## 1. 目标与边界

### 1.1 本规格包含

- 新增 / 编辑 AI 站点改成弹窗，交互照抄账号弹窗。
- 给 AI 站点加站点级代理策略：跟随全局 / 直连 / 指定代理，默认跟随全局。
- 后端 AI 站点相关的所有外发（分类、翻译、摘要、润色、起草、通知识别、延迟检查、测试连接、拉模型）按站点自己的策略解析代理。
- 站点列表卡片显示该站点的代理策略。

### 1.2 本规格不包含

- 不改全局代理设置界面，也不改邮箱账号的代理策略（沿用现状）。
- 不改代理本身的增删改（仍在「代理」设置里管理）。
- 不改 AI 站点的其它字段，不改请求格式，不改密钥保管方式。
- 不给站点加「跟随系统」独立档：跟随全局已经覆盖（全局可以是系统代理）。
- 不做 macOS / Linux 适配，项目当前以 Windows 为发布目标。
- 版本号保持 0.3.0（本功能不单独升版本）。

---

## 2. 架构决策（含反选理由）

### Decision D1：站点设置改弹窗，照抄账号弹窗交互

- **Reason**：项目里已有成熟的账号弹窗（`src/AccountDialog.tsx`）：遮罩、点外关闭、Esc 关闭、Tab 焦点循环、关掉后焦点还原、有脏数据先问、忙时不许关。直接复用同一套交互，用户不用学两套。
- **Alternatives Considered**：继续用内联表单（用户明确要求改弹窗）；另写一套新弹窗（重复造轮子，交互不一致）。

### Decision D2：站点级代理沿用账号三档，不新造一套

- **Reason**：账号已有「跟随全局 / 直连 / 指定代理」三档，后端 `resolve_route_with` 也已支持。站点用同一套，语义统一、代码复用、界面一致。默认「跟随全局」等于现在的行为，老站点平移不变。
- **Alternatives Considered**：
  - 只给「走全局 / 直连」两档开关：省事，但没法给单个站点挂一个具体代理。
  - 站点只存一个代理编号、留空即直连：分不清「跟随全局」和「直连」，语义糊。
  - 加「跟随系统」第四档：和账号不一致，且全局设成系统代理就能覆盖，多余。

### Decision D3：站点级代理只影响该站点的 AI 外发，不碰邮箱收发

- **Reason**：代理策略挂在站点上，只在解析这个站点的请求路线时生效；邮箱账号的收发代理仍按账号自己的策略。
- **Alternatives Considered**：站点代理反过来影响邮箱（越界，乱）。

### Decision D4：站点指定的代理被删后自动退回「跟随全局」

- **Reason**：`proxy_id` 用 `ON DELETE SET NULL`。删掉一个被站点指定的代理后，站点读到的是「custom 但没有编号」，此时退回「跟随全局」，界面也显示「跟随全局」，绝不因为一个悬空引用让 AI 完全用不了。
- **Alternatives Considered**：报错卡死（用户会一脸懵）；继续当 custom 但没编号（路线不可解析）。

---

## 3. 接口与数据

### 3.1 数据模型（新增迁移 `0017_ai_provider_proxy.sql`）

给 `ai_provider` 加两列，照抄 `account` 的代理字段：

```sql
ALTER TABLE ai_provider ADD COLUMN proxy_mode TEXT NOT NULL DEFAULT 'inherit'
    CHECK (proxy_mode IN ('inherit', 'direct', 'custom'));
ALTER TABLE ai_provider ADD COLUMN proxy_id INTEGER REFERENCES proxy(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_ai_provider_proxy ON ai_provider (proxy_id);
```

- `proxy_mode`：`inherit` / `direct` / `custom`，默认 `inherit`（老站点自动跟随全局）。
- `proxy_id`：只在 `custom` 时有效，指向 `proxy.id`。
- 迁移只加列、不动老数据；注册到 `crates/mail-store/src/migrations/mod.rs`（追加在 0016 之后）。

### 3.2 代理决议

- `StoredAiProvider` 与 `NewAiProvider` 增加 `proxy: AccountProxyMode` 字段（用 `mail-domain` 的 `AccountProxyMode`）。
- 存库：按 `proxy.as_str()` 写 `proxy_mode`，`Custom(id)` 才写 `proxy_id`。
- 读库：用 `AccountProxyMode::parse(proxy_mode, proxy_id)`；`custom` 但编号为空 → 退回 `InheritGlobal`（见 D4）。
- 后端 `crates/mail-core/src/ai.rs` 里现有 9 处 `self.resolve_route(AccountProxyMode::InheritGlobal)?` 一律改成按该站点自己的策略：
  - 有 `AiTarget` 的地方：`AiTarget` 增加 `proxy: AccountProxyMode` 字段（由站点的 `StoredAiProvider.proxy` 带出），改成 `self.resolve_route(target.proxy)?`。覆盖分类、翻译、摘要、润色、起草、通知识别、延迟检查。
  - `refresh_ai_provider_models(id)`：站点已存，直接用该站点的 `proxy`。
  - `test_ai_provider(...)`：站点可能还没保存，新增入参 `proxy: AccountProxyMode` 由界面传入。
- `custom` 策略在保存和测试时先校验代理存在（照 `set_global_proxy_mode` 的做法），不存在就报「找不到这个代理」。

### 3.3 弹窗交互（照抄 `src/AccountDialog.tsx`）

- 半透明遮罩铺满窗口；点遮罩空白处关闭；弹窗居中、最大高度 90vh、内容可滚。
- `role="dialog"`、`aria-modal="true"`、标题带 `aria-labelledby`。
- 打开时焦点落到第一个输入框；关闭时焦点还给触发按钮。
- Esc 关闭；Tab 在弹窗内循环，不跑到后面的页面。
- 有没保存的内容时关闭先问一句「表单里还有没保存的内容，确定放弃吗？」。
- 正在测试连接 / 保存时不许关（避免半截请求把状态搞乱）。
- 新增和编辑共用同一个弹窗组件；标题随情况显示「新增 AI 站点 / 编辑 AI 站点」。

### 3.4 命令与前端契约

- `AiProviderDraftDto` 增加 `proxy: AccountProxyDto`（复用现有账号代理 DTO：`mode` + 可选 `proxyId`），缺省 `{ mode: "inherit" }`。
- `AiProviderDto` 增加 `proxy: AccountProxyDto`，列表卡片用它显示策略。
- `test_ai_provider` 命令增加 `proxy: AccountProxyDto`（可选，缺省 `inherit`），供未保存的站点测试连接时指定路线。
- 前端 `src/api.ts`：`AiProviderDraft` 与 `AiProvider` 增加 `proxy: AccountProxy`（复用账号那套类型：`{ mode, proxyId? }`）；`testAiProvider` 增加 `proxy` 可选参数。
- 前端 `src/AiPanel.tsx`：
  - 代理策略下拉复用账号的 `PROXY_MODE_LABELS`；选「指定代理」时再出现一个代理下拉，选项来自 `api.listProxies()`，标签形如「名字（socks5 host:port）」；没代理时给提示并禁止保存 custom。
  - 站点卡片加一行显示代理策略（复用 `PROXY_MODE_LABELS`；指定代理时显示代理名）。

---

## 4. 行为

### 4.1 站点弹窗行为

- 点列表下方「新增 AI 站点」→ 弹出空白弹窗；点某站点的「编辑」→ 弹出带该站点数据的弹窗。
- 弹窗内容与现在的内联表单字段一致：站点名称、站点类型、站点地址、CDKey/API Key、默认模型、默认思考程度、模型列表、启用、代理策略。按钮：保存、测试连接（或「测试连接并拉取模型」）、取消。
- 保存成功 → 关弹窗、刷新列表、提示「已保存「名字」」。
- 取消 / 点外 / Esc → 关弹窗；有未保存内容先确认。
- 测试连接用弹窗里当前填的地址、密钥和代理策略。

### 4.2 站点代理行为

- 默认「跟随全局」：行为和现在完全一样。
- 「直连」：这个站点的请求不走任何代理。
- 「指定代理」：这个站点的请求固定走选中的那个代理（含它自己的账号密码，密码仍在保险箱）。
- 全局「一键关闭 AI」后所有站点停止外发，已有结果保留（沿用现状，不受代理改动影响）。
- 删站点照旧清密钥与缓存；删代理后引用它的站点退回「跟随全局」。

---

## 5. 验收标准

- **R1**（MUST）点「新增 AI 站点」在整个应用里弹出独立弹窗，不再于列表下方展开内联表单。
- **R2**（MUST）点某站点的「编辑」弹出同一个弹窗，标题为「编辑 AI 站点」，字段预填该站点现值。
- **R3**（MUST）弹窗支持：点遮罩空白关闭、Esc 关闭、Tab 焦点循环、打开焦点进第一个输入框、关闭焦点还原到触发按钮。
- **R4**（MUST）弹窗有未保存内容时关闭先确认；正在测试 / 保存时不许关。
- **R5**（MUST）弹窗里有「代理策略」三档：跟随全局 / 直连 / 指定代理；选「指定代理」时出现代理下拉；新建默认「跟随全局」。
- **R6**（MUST）选「指定代理」却没挑代理时保存被拦下并给中文提示；挑了不存在的代理后端也报错。
- **R7**（MUST）站点列表卡片显示该站点的代理策略；指定代理时显示代理名。
- **R8**（MUST）老站点（迁移前）读出来是「跟随全局」，行为与现在一致。
- **R9**（MUST）站点级代理只影响该站点的 AI 外发；邮箱账号的收发代理不受影响。
- **R10**（MUST）测试连接 / 拉模型 / 分类 / 翻译 / 摘要 / 润色 / 起草 / 通知识别 / 延迟检查都按站点自己的策略解析代理。
- **R11**（MUST）删掉一个被站点指定的代理后，该站点退回「跟随全局」，不会因为悬空引用报错或卡死。
- **R12**（SHOULD）版本号保持 0.3.0，7 个版本文件保持一致。

---

## 6. 安全与隐私

- 代理密码只在建连前一刻从系统保险箱取出，不落库、不进日志（沿用 `resolve_route_with` 现状）。
- AI 密钥继续只进系统保险箱；数据库只存引用键。
- 本次改动不新增任何外发路径；代理只改变「怎么连出去」，不改变「发什么」。
- 站点代理策略是本地数据，不参与账号同步、不上传。
- 代理策略校验在后端兜底：即使界面被绕过，非法组合也拦得住。

---

## 7. File Changes

| 文件 | 类型 | 说明 |
|---|---|---|
| `crates/mail-store/src/migrations/sql/0017_ai_provider_proxy.sql` | new | 给 ai_provider 加 proxy_mode / proxy_id |
| `crates/mail-store/src/migrations/mod.rs` | edit | 注册 0017 |
| `crates/mail-store/src/ai.rs` | edit | StoredAiProvider / NewAiProvider 加 proxy；读写语句带上两列 |
| `crates/mail-core/src/ai.rs` | edit | AiProviderInput/View 加 proxy；AiTarget 加 proxy；9 处 InheritGlobal 改成按站点策略；test_ai_provider 加 proxy 入参 |
| `crates/mail-core/src/proxies.rs` | edit | 如需，暴露「按模式校验代理存在」的小工具（复用现有逻辑） |
| `src-tauri/src/commands.rs` | edit | AiProviderDraftDto / AiProviderDto 加 proxy；test_ai_provider 加 proxy 参数 |
| `src/api.ts` | edit | AiProviderDraft / AiProvider 加 proxy；testAiProvider 加 proxy |
| `src/AiProviderDialog.tsx` | new | 站点弹窗（照抄 AccountDialog 交互） |
| `src/AiPanel.tsx` | edit | 去掉内联表单改用弹窗；卡片显示代理策略；代理下拉 |
| `src/index.css` | edit | 弹窗与卡片样式（如需） |
| `src/__tests__/*` | edit | 弹窗开关、代理策略、命令参数相关单测 |

---

## 8. Tasks（实现清单）

### Wave P1：数据层

- 写迁移 `0017_ai_provider_proxy.sql`，注册到 `mod.rs`。
- `mail-store::ai` 的 `StoredAiProvider` / `NewAiProvider` 加 `proxy`，读写带上两列。
- 单测：默认读成 inherit；custom 带编号能往返；custom 无编号退回 inherit；代理删除后站点退回 inherit。

### Wave P2：AI 层

- `AiProviderInput` / `AiProviderView` / `AiTarget` 加 `proxy`。
- 9 处 `resolve_route(InheritGlobal)` 改成按站点策略。
- `test_ai_provider` 加 `proxy` 入参；custom 先校验代理存在。
- 单测：直连站点不走代理；指定代理站点带密码走指定代理；未指定时跟随全局。

### Wave P3：命令层

- `AiProviderDraftDto` / `AiProviderDto` 加 `proxy`；`test_ai_provider` 命令加 `proxy`。
- 单测：命令入参缺省 inherit；custom 缺编号报错。

### Wave P4：界面层

- 新增 `AiProviderDialog.tsx`，交互照抄 `AccountDialog`。
- `AiPanel.tsx` 去掉内联表单改用弹窗；卡片显示代理策略；代理下拉取 `listProxies`。
- `api.ts` 类型与调用更新。
- 单测：点新增 / 编辑出弹窗、Esc 与点外关闭、代理下拉出现条件、保存带 proxy。

### Wave P5：收尾

- 版本号保持 0.3.0，核对 7 个版本文件一致。
- 四关全绿：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`npm run build`。
- 更新 `.ai-memory/`（handoff.md、project_memory.md、当日 daily.md）。

---

## 9. 未决与未验证

- **未验证**：真机在「指定代理」下测 jev / OpenAI 兼容站点的连通性；实现阶段先用假服务端覆盖路线选择，接真机需用户提供密钥后实测。
- **已确认**：站点级代理三档（跟随全局 / 直连 / 指定代理），默认跟随全局。
- **已确认**：版本号保持 0.3.0，不动版本文件。