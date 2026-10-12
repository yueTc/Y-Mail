# AI 站点设置弹窗与站点级代理 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 AI 设置页里「新增 / 编辑 AI 站点」改成弹窗，并给每个 AI 站点加站点级代理策略（跟随全局 / 直连 / 指定代理），后端所有 AI 外发按站点自己的策略解析代理；同时把标签组默认收起、去掉「开启这一组」的文字只留开关。

**Architecture:** 前端新增 `AiProviderDialog.tsx`（照抄 `AccountDialog.tsx` 的弹窗交互），把 `AiPanel.tsx` 里的内联表单逻辑整体搬进去。数据库给 `ai_provider` 加 `proxy_mode` / `proxy_id` 两列；`mail-core` 的 `AiTarget` / 站点结构带上 `AccountProxyMode`，把 9 处写死的 `resolve_route(InheritGlobal)` 改成按站点策略解析。标签组只改 `AiPanel.tsx` 一行折叠判断与一处文字。

**Tech Stack:** Tauri 2 + Rust（`mail-store` / `mail-core` / `src-tauri`）+ React/TS（`AiPanel.tsx`、`AiProviderDialog.tsx`、`api.ts`）+ Vitest。

**Spec:**
- `docs/superpowers/specs/2026-10-12-ai-provider-dialog-and-proxy-design.md`（v1.0，R1–R12）
- `docs/superpowers/specs/2026-10-11-mail-label-groups-design.md`（v1.3，R35 更新：默认收起、开关不带文字）

## Global Constraints

- 版本号保持 0.3.0，7 个版本文件保持一致（本计划不改版本号）。
- 面向用户的文案一律中文，不新增英文旁白。
- 密钥只进系统保险箱；数据库只存引用键；代理密码只在建连前从保险箱取，不落库、不进日志。
- 本站点代理默认「跟随全局」，与现在行为一致；老站点迁移后自动是「跟随全局」。
- 分支保持 `codex/mail-auto-classification`；提交信息用中文。
- 终端若提示找不到 cargo，先执行：`$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"`。Rust 若缺 sidecar，先跑 `node scripts/build-mcp-sidecar.mjs`。
- 后端包名是 `ymail`，不是 `em-master`。
- 收尾四关必须全绿：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`npm run build`。
- 写文件用 UTF-8 无 BOM、LF 换行。

---

### Task 1: 标签组默认收起 + 去掉「开启这一组」文字

**Files:**
- Modify: `src/AiPanel.tsx`（折叠状态约 293–294、`toggleGroupCollapsed` 约 733–735、渲染约 1594 与 1650–1660）
- Test: `src/__tests__/ai.test.tsx`（约 1088–1110）

**Interfaces:**
- Produces: 进入设置页时所有标签组默认收起；组开关只渲染勾选框，不带可见文字；勾选框的无障碍名为 `开启或关闭{组名}`。

- [ ] **Step 1: 改测试锁住新行为（先红）**

把 `src/__tests__/ai.test.tsx` 里「单个组可折叠，排序和删除排在开启这一组前面」整段替换成：

```tsx
  it("标签组默认收起，排序和删除排在开关前面且开关不带文字", async () => {
    const { container } = prepare();
    await screen.findByText("邮件自动分类 · 标签组管理");
    await waitFor(() => expect(container.querySelector(".ai-label-group-head")).toBeTruthy());

    // 默认收起：门槛和组内标签一开始就看不到。
    expect(screen.queryByLabelText("内容分类门槛")).toBeNull();
    expect(container.querySelector(".ai-label-group-body")).toBeNull();

    const head = container.querySelector(".ai-label-group-head") as HTMLElement;
    const controls = Array.from(
      head.querySelectorAll("button[aria-label], input[type='checkbox'][aria-label]"),
    ).map((element) => element.getAttribute("aria-label"));
    expect(controls).toEqual([
      "展开内容分类",
      "上移内容分类",
      "下移内容分类",
      "删除内容分类",
      "开启或关闭内容分类",
    ]);
    // 开关不带可见文字。
    expect(head.textContent).not.toContain("开启这一组");

    fireEvent.click(screen.getByRole("button", { name: "展开内容分类" }));
    await waitFor(() => {
      expect(screen.getByLabelText("内容分类门槛")).toBeTruthy();
    });
    expect(screen.getByRole("button", { name: "折叠内容分类" })).toBeTruthy();
  });
```

并把 `it("组可以改名、下移和开关"...)`（约 1070）里的开关断言改成用新无障碍名：`screen.getByLabelText("开启或关闭需要我回复")`。

- [ ] **Step 2: 跑测试确认红**

Run: `npm run test -- run src/__tests__/ai.test.tsx`
Expected: FAIL，报默认收起不成立或找不到「展开内容分类」。

- [ ] **Step 3: 改前端状态与折叠判断**

在 `src/AiPanel.tsx`：

把折叠状态（约 293–294）改成存「展开」而非「收起」：

```tsx
  // 设置页里每一组可单独展开；默认全部收起，只影响界面，不写配置。
  const [expandedGroups, setExpandedGroups] = useState<Record<number, boolean>>({});
```

把 `toggleGroupCollapsed`（约 733–735）改成：

```tsx
  /** 单个标签组展开/收起：只影响设置界面，不改数据。 */
  const toggleGroupExpanded = useCallback((groupId: number) => {
    setExpandedGroups((old) => ({ ...old, [groupId]: !old[groupId] }));
  }, []);
```

把渲染里的 `const collapsed = collapsedGroups[group.id] === true;`（约 1594）改成：

```tsx
            const expanded = expandedGroups[group.id] === true;
            const collapsed = !expanded;
```

把折叠按钮的 `onClick={() => toggleGroupCollapsed(group.id)}`（约 1612）改成 `onClick={() => toggleGroupExpanded(group.id)}`。

- [ ] **Step 4: 去掉「开启这一组」文字**

把组头里那段开关（约 1650–1660）改成只留勾选框，并补无障碍名与悬停提示：

```tsx
                    <label className="checkbox ai-label-group-enable">
                      <input
                        type="checkbox"
                        aria-label={t("开启或关闭{0}", [group.name])}
                        title={t("开启或关闭{0}", [group.name])}
                        checked={group.enabled}
                        disabled={labelBusy}
                        onChange={(event) => void toggleGroup(group, event.target.checked)}
                      />
                    </label>
```

确认 `collapsedGroups` 这个名字在全文件里已无残留（用 `rg -n "collapsedGroups|toggleGroupCollapsed" src/AiPanel.tsx` 核对，应为空）。

- [ ] **Step 5: 跑测试确认绿**

Run: `npm run test -- run src/__tests__/ai.test.tsx`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add src/AiPanel.tsx src/__tests__/ai.test.tsx
git commit -m "界面：标签组默认收起并去掉开启文字"
```

---

### Task 2: 数据层——给 ai_provider 加代理两列

**Files:**
- New: `crates/mail-store/src/migrations/sql/0017_ai_provider_proxy.sql`
- Modify: `crates/mail-store/src/migrations/mod.rs`（迁移清单末尾追加）
- Modify: `crates/mail-store/src/ai.rs`（`StoredAiProvider` / `NewAiProvider` 结构与全部 `ai_provider` 读写语句）
- Test: `crates/mail-store/src/ai.rs` 的测试模块

**Interfaces:**
- Produces: `StoredAiProvider.proxy: AccountProxyMode`、`NewAiProvider.proxy: AccountProxyMode`；默认是 `AccountProxyMode::InheritGlobal`；`Custom(id)` 才写 `proxy_id`。

- [ ] **Step 1: 写迁移并注册**

新建 `crates/mail-store/src/migrations/sql/0017_ai_provider_proxy.sql`：

```sql
-- 0017_ai_provider_proxy：给 AI 站点加站点级代理策略。
-- 默认 inherit（跟随全局），老站点迁移后行为不变；custom 时才用 proxy_id。

ALTER TABLE ai_provider ADD COLUMN proxy_mode TEXT NOT NULL DEFAULT 'inherit'
    CHECK (proxy_mode IN ('inherit', 'direct', 'custom'));
ALTER TABLE ai_provider ADD COLUMN proxy_id INTEGER REFERENCES proxy(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_ai_provider_proxy ON ai_provider (proxy_id);
```

在 `crates/mail-store/src/migrations/mod.rs` 的 `MIGRATIONS` 末尾追加：

```rust
    Migration {
        version: 17,
        name: "0017_ai_provider_proxy",
        sql: include_str!("sql/0017_ai_provider_proxy.sql"),
    },
```

- [ ] **Step 2: 改存储结构与读写语句**

在 `crates/mail-store/src/ai.rs`：

- 顶部 `use mail_domain::account::AccountProxyMode;`（若尚未引入）。
- `StoredAiProvider` 增加字段：

```rust
    /// 站点级代理策略。
    pub proxy: AccountProxyMode,
```

- `NewAiProvider` 增加字段：

```rust
    /// 站点级代理策略。
    pub proxy: AccountProxyMode,
```

- 所有 `SELECT ... FROM ai_provider` 的列清单末尾加 `proxy_mode, proxy_id`；`row` 到结构体时用 `AccountProxyMode::parse(&mode, proxy_id).unwrap_or(AccountProxyMode::InheritGlobal)`（`custom` 但编号为空 → 退回 `inherit`，见规格 D4）。
- `insert_ai_provider` / `update_ai_provider` 的 INSERT / UPDATE 带上 `proxy_mode`、`proxy_id`：`proxy_mode = proxy.as_str()`；`proxy_id = match proxy { AccountProxyMode::Custom(id) => Some(id.0), _ => None }`。

- [ ] **Step 3: 补/改单测（先红后绿）**

在 `crates/mail-store/src/ai.rs` 测试模块里，把 `provider()` 夹具加 `proxy: AccountProxyMode::InheritGlobal`，并新增：

```rust
    #[test]
    fn 站点默认跟随全局且自定义代理可往返() {
        let store = store();
        let id = store.insert_ai_provider(&provider(), None).expect("新建");
        assert_eq!(
            store.get_ai_provider(id).expect("查询").expect("存在").proxy,
            AccountProxyMode::InheritGlobal
        );

        let mut custom = provider();
        custom.proxy = AccountProxyMode::Custom(mail_domain::proxy::ProxyId(7));
        assert!(store.update_ai_provider(id, &custom, None).expect("更新"));
        assert_eq!(
            store.get_ai_provider(id).expect("查询").expect("存在").proxy,
            AccountProxyMode::Custom(mail_domain::proxy::ProxyId(7))
        );
    }
```

Run（先补 PATH）:
```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test -p mail-store ai
```
Expected: PASS。

- [ ] **Step 4: 提交**

```bash
git add crates/mail-store/src/migrations/sql/0017_ai_provider_proxy.sql crates/mail-store/src/migrations/mod.rs crates/mail-store/src/ai.rs
git commit -m "数据：AI 站点增加站点级代理字段"
```

---

### Task 3: AI 层——按站点策略解析代理

**Files:**
- Modify: `crates/mail-core/src/ai.rs`（`AiProviderView` 约 132、`AiProviderInput` 约 155、`AiTarget` 约 100、`provider_view` 约 1793、9 处 `resolve_route(InheritGlobal)` 约 500/548/898/1276/1365/1428/1482/1531/1568、`test_ai_provider` 约 470–520）
- Modify: `crates/mail-core/src/proxies.rs`（如需暴露「校验代理存在」的小函数）
- Test: `crates/mail-core/src/ai.rs` 测试模块

**Interfaces:**
- Consumes: Task 2 的 `StoredAiProvider.proxy` / `NewAiProvider.proxy`。
- Produces: `AiProviderView.proxy: AccountProxyMode`、`AiProviderInput.proxy: AccountProxyMode`、`AiTarget.proxy: AccountProxyMode`；`test_ai_provider(kind, base_url, api_key, provider_id, proxy)`。

- [ ] **Step 1: 结构体加 proxy 字段**

- `AiTarget` 增加 `pub proxy: AccountProxyMode`，在 `resolve_ai_target` / `resolve_probe_target` 构造它时从站点带出。
- `AiProviderView` 增加 `pub proxy: AccountProxyMode`；`AiProviderInput` 增加 `pub proxy: AccountProxyMode`。
- `provider_view(provider)` 里补 `proxy: provider.proxy`。
- `save_ai_provider_inner` 构造 `NewAiProvider` 时带上 `proxy: input.proxy`；`custom` 但没编号时返回 `EngineError::BadRequest("选了「指定代理」就要挑一个代理".to_string())`；`custom` 且有编号时先 `self.store().get_proxy(id)?` 校验存在，不存在报 `EngineError::ProxyNotFound(id.0)`。

- [ ] **Step 2: 9 处决议改成按站点策略**

- 有 `target` / `AiTarget` 的 8 处：把 `let route = self.resolve_route(AccountProxyMode::InheritGlobal)?;` 改成 `let route = self.resolve_route(target.proxy)?;`（分类约 898、翻译约 1276、摘要/其它约 1365/1428/1482/1531、通知识别 1568 等，逐个核对变量名）。
- `test_ai_provider(...)`：新增入参 `proxy: AccountProxyMode`，函数体里 `let route = self.resolve_route(proxy)?;`；`custom` 同样先校验代理存在。
- `refresh_ai_provider_models(id)`：`let route = self.resolve_route(provider.proxy)?;`。
- 全部改完后 Run: `rg -n "resolve_route\(AccountProxyMode::InheritGlobal\)" crates/mail-core/src/ai.rs`；Expected: 只剩测试夹具或为空（业务代码里不应再有 9 处写死）。

- [ ] **Step 3: 补/改单测**

在 `crates/mail-core/src/ai.rs` 测试模块：
- 站点夹具补 `proxy: AccountProxyMode::InheritGlobal`。
- 新增用例：站点设成 `Custom(id)` 时，一次 AI 调用（可用现有假服务端）解析出的路线走对应代理；站点设成 `Direct` 时不走代理。
- 若已有「AI 走代理」相关用例，更新为按站点策略断言。

Run:
```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test -p ymail --lib ai
```
Expected: PASS。

- [ ] **Step 4: 提交**

```bash
git add crates/mail-core/src/ai.rs crates/mail-core/src/proxies.rs
git commit -m "AI：站点外发按站点自己的代理策略"
```

---

### Task 4: 命令层——DTO 带上代理

**Files:**
- Modify: `src-tauri/src/commands.rs`（`AiProviderDraftDto` 约 3294、`AiProviderDto` 约 3348、`impl AiProviderDraftDto::to_input` 约 3319、`impl AiProviderDto::from_view` 约 3372、`test_ai_provider` 约 3856）

**Interfaces:**
- Consumes: Task 3 的 `AiProviderInput.proxy` / `AiProviderView.proxy`。
- Produces: `AiProviderDraftDto.proxy: AccountProxyDto`（`#[serde(default)]`，缺省 `{ mode: "inherit" }`）、`AiProviderDto.proxy: AccountProxyDto`、`test_ai_provider(..., proxy: Option<AccountProxyDto>)`。

- [ ] **Step 1: DTO 加字段并映射**

- `AiProviderDraftDto` 增加：

```rust
    /// 站点级代理策略；缺省跟随全局。
    #[serde(default)]
    pub proxy: AccountProxyDto,
```

- `AiProviderDto` 增加 `pub proxy: AccountProxyDto`。
- `to_input` 里把 `proxy` 转成 `AccountProxyMode`（复用账号那条路径的转换逻辑；`custom` 缺 `proxy_id` 时返回 `CommandError::input("选了「指定代理」就要挑一个代理")`），填进 `AiProviderInput { proxy, .. }`。
- `from_view` 里把 `view.proxy` 转回 `AccountProxyDto`，填进 `AiProviderDto { proxy, .. }`。
- `AccountProxyDto` 需要能默认成 `inherit`：给 `AccountProxyDto` 加 `impl Default` 或在 `AiProviderDraftDto` 用 `#[serde(default = "default_account_proxy")]`；若账号那边已有等价默认函数，复用它。

- [ ] **Step 2: test_ai_provider 加参数**

```rust
pub async fn test_ai_provider(
    state: tauri::State<'_, AppState>,
    kind: String,
    base_url: String,
    api_key: Option<String>,
    id: Option<i64>,
    proxy: Option<AccountProxyDto>,
) -> Result<Vec<String>, CommandError> {
```

把 `proxy` 转成 `AccountProxyMode`（缺省 `InheritGlobal`），传给 `engine.test_ai_provider(kind, &base_url, secret.as_ref(), id, proxy_mode)`。

- [ ] **Step 3: 跑后端测试**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test -p ymail --lib ai
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: PASS，无未使用 / 类型警告。

- [ ] **Step 4: 提交**

```bash
git add src-tauri/src/commands.rs
git commit -m "命令：AI 站点命令带上代理策略"
```

---

### Task 5: 界面层——站点弹窗与代理选择

**Files:**
- New: `src/AiProviderDialog.tsx`
- Modify: `src/AiPanel.tsx`（`ProviderDraftState` / `emptyProvider` / `providerToDraft` / `parseModels` 约 99–150、`editing` / `apiKey` / `showApiKey` / `modelsHint` 状态约 267–270、`startCreate` / `startEdit` / `saveProvider` / `testDraft` 约 348–470、站点列表与内联表单约 1105–1300）
- Modify: `src/api.ts`（`AiProviderDraft` / `AiProvider` 约 601–618、`testAiProvider` 约 1174）
- Modify: `src/index.css`（如需）
- Test: `src/__tests__/ai.test.tsx`

**Interfaces:**
- Consumes: Task 4 的命令契约。
- Produces: `AiProviderDialog` 组件；`AiPanel` 里不再有内联站点表单；站点卡片显示代理策略。

- [ ] **Step 1: 改前端类型与调用**

`src/api.ts`：
- `AiProviderDraft` 与 `AiProvider` 各加 `proxy: AccountProxy;`。
- `testAiProvider` 增加可选 `proxy: AccountProxy` 参数，拼进 args。

- [ ] **Step 2: 新建弹窗组件**

新建 `src/AiProviderDialog.tsx`：把 `AiPanel.tsx` 的内联表单逻辑整体搬过来。
- 外壳照抄 `src/AccountDialog.tsx`：`BACKDROP_STYLE` / `DIALOG_STYLE` / `HEADER_STYLE` / `BODY_STYLE`、打开时焦点进第一个输入框、关闭时焦点还原、Esc 关闭、Tab 焦点循环、点遮罩空白关闭、脏数据确认、busy 时不许关。
- Props：`{ open: boolean; provider: AiProvider | null; onClose: () => void; onSaved: (saved: AiProvider) => void }`。
- 自持状态：`editing`（`emptyProvider()` 或 `providerToDraft(provider)`）、`apiKey`、`showApiKey`、`modelsHint`、`busy`、`proxies`（打开时 `api.listProxies()`）。
- 字段与现有内联表单一致，新增「代理策略」：下拉复用 `PROXY_MODE_LABELS`（`跟随全局 / 直连 / 指定代理`）；选「指定代理」时出现代理下拉，选项形如「名字（socks5 host:port）」，没代理时给提示。保存时把 `proxy: { mode, proxyId }` 填进 draft。
- 按钮：保存、测试连接（typesafe 显示「测试连接」，其它显示「测试连接并拉取模型」）、取消。测试连接把当前 `proxy` 传给 `api.testAiProvider`。
- 保存成功 → `onSaved(saved)` 并关弹窗。

- [ ] **Step 3: AiPanel 改用弹窗**

`src/AiPanel.tsx`：
- 删掉 `ProviderDraftState` / `emptyProvider` / `providerToDraft` / `parseModels` 与 `editing` / `apiKey` / `showApiKey` / `modelsHint` / `startCreate` / `startEdit` / `saveProvider` / `testDraft`。
- 新增：

```tsx
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingProvider, setEditingProvider] = useState<AiProvider | null>(null);
```

- 「新增 AI 站点」按钮：`onClick={() => { setEditingProvider(null); setDialogOpen(true); }}`。
- 站点卡片「编辑」按钮：`onClick={() => { setEditingProvider(provider); setDialogOpen(true); }}`。
- 站点卡片加一行显示代理策略：把 `provider.proxy.mode` 经 `PROXY_MODE_LABELS` 翻成中文；`custom` 时再查出代理名拼上（可从 `api.listProxies()` 结果里找，或用弹窗回传的 `AiProvider`）。
- 列表下方渲染 `<AiProviderDialog open={dialogOpen} provider={editingProvider} onClose={() => setDialogOpen(false)} onSaved={() => { setDialogOpen(false); void refresh(); }} />`。
- 删除原 `{editing && (<div className="form ai-provider-form">...</div>)}` 整块。

- [ ] **Step 4: 改测试（先红后绿）**

在 `src/__tests__/ai.test.tsx`：
- 把依赖内联表单的用例（如「新增站点」「编辑站点」「测试连接」相关）改成：先点按钮，再在弹窗里 `getByRole("dialog")` 内操作。
- 新增：点「新增 AI 站点」出现 `role="dialog"`；按 Esc 关闭；选「指定代理」时出现代理下拉；保存时 `saveAiProvider` 收到 `proxy`。
- 测试桩里 `listAiProviders` 返回的站点补 `proxy: { mode: "inherit" }`；`saveAiProvider` 桩补回传 `proxy`。

Run: `npm run test -- run src/__tests__/ai.test.tsx`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/AiProviderDialog.tsx src/AiPanel.tsx src/api.ts src/index.css src/__tests__/ai.test.tsx
git commit -m "界面：AI 站点改弹窗并支持站点级代理"
```

---

### Task 6: 更新项目记忆

**Files:**
- Modify: `.ai-memory/handoff.md`
- Modify: `.ai-memory/project_memory.md`
- Modify: `.ai-memory/20261012/daily.md`

- [ ] **Step 1: 追加当日日志**

写清：标签组默认收起 + 去掉「开启这一组」文字；AI 站点改弹窗；站点级代理三档；四关结果与提交哈希。

- [ ] **Step 2: 更新交接与项目记忆**

`handoff.md`「已完成」追加本次；`project_memory.md` 增加一条决策：AI 站点支持站点级代理（跟随全局 / 直连 / 指定代理），默认跟随全局；代理被删退回跟随全局。

- [ ] **Step 3: 提交**

```bash
git add .ai-memory
git commit -m "记忆：记录 AI 站点弹窗与代理策略"
```

> 注意：`.ai-memory/` 可能被 `.gitignore` 忽略，若 `git add` 无内容则跳过提交，只更新文件。

---

### Task 7: 四关验证与收尾

- [ ] **Step 1: 跑四关**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run build
```

Expected: 四条全部成功退出。

- [ ] **Step 2: 如有修复则提交**

```bash
git add -A
git commit -m "验证：AI 站点弹窗与代理四关全绿"
```

- [ ] **Step 3: 确认工作区干净并汇报**

Run: `git status --short`
Expected: 无输出。汇报四件事：做完的（带提交哈希与四关证据）、相关文件、没做完的、卡在哪。

---

## 自查

- **规格覆盖**：AI 站点规格 R1–R12 全部落到 Task 2–5；标签组规格 v1.3 R35 落到 Task 1。
- **占位符扫描**：无 TBD/TODO；每个改动步骤都给了具体代码或明确的替换目标。
- **类型一致**：`proxy` 一路用 `AccountProxyMode`（Rust）/ `AccountProxy`（TS），前后端与命令 DTO 一致；默认都是 `inherit`。
- **安全**：代理密码仍只走保险箱；本次不新增外发路径。