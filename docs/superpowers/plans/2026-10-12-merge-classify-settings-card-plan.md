# 邮件自动分类设置卡片合并实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把设置页里独立的「邮件自动分类」卡片与「标签组管理」卡片合并成一张卡片：顶部放总开关与手动分类免确认，删除全局把握程度门槛，门槛只留在每个标签组内部。

**Architecture:** 只动设置页界面与一个后端命令签名，不碰分类请求格式、搜索、列表胶囊显示。前端 `AiPanel.tsx` 去掉全局门槛状态与输入框，把原两张卡片合并为一张 `section`；后端 `set_classify_settings` 去掉已废弃的 `confidence_percent` 参数，只改开关，老字段 `classify_confidence_percent` 保留在 `settings.json` 里只读兼容。

**Tech Stack:** Tauri 2 + Rust（`src-tauri/src/commands.rs`）+ React/TS（`src/AiPanel.tsx`、`src/api.ts`）+ Vitest。

**Spec:** `docs/superpowers/specs/2026-10-11-mail-label-groups-design.md`（v1.2，R1–R36）

## Global Constraints

- 版本号保持 0.3.0，7 个版本文件保持一致（本计划不改版本号）。
- 面向用户的文案一律中文；不新增英文旁白。
- 邮件正文当不可信内容；本计划不新增任何外发路径。
- 老字段 `classify_confidence_percent` / `classify_confidence_migrated` 保留，只读兼容；分类只按每组 `confidence_threshold`。
- 默认关闭保持不变。
- 收尾四关必须全绿：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`npm run build`。
- 终端若提示找不到 cargo，先执行：`$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"`。
- 写文件用 UTF-8 无 BOM、LF；提交信息用中文；分支保持 `codex/mail-auto-classification`。

---

### Task 1: 前端合并卡片并删除全局门槛

**Files:**
- Modify: `src/AiPanel.tsx`（状态约 272–275、读取设置约 326–328、开关逻辑约 567–657、卡片渲染约 1401–1443 与 1566–1616）
- Test: `src/__tests__/ai.test.tsx`（约 103–109、856–862、952–989、966–968）

**Interfaces:**
- Consumes: `api.setClassifySettings(enabled: boolean)`（Task 1 同时把 `src/api.ts` 和测试桩改成单参数，后端在 Task 2 对齐）。
- Produces: 一张 `h3`「邮件自动分类 · 标签组管理」+ 一个 `section.ai-feature-card.ai-label-manager`；不再有 `aria-label="把握程度门槛"` 的输入框；每组 `aria-label="{组名}门槛"` 的输入框保留。

- [ ] **Step 1: 改测试，锁住新界面（先红）**

把 `src/__tests__/ai.test.tsx` 里 `setClassifySettings` 的桩改成单参数：

```ts
    setClassifySettings: vi.fn(async (enabled: boolean) => ({
      ...baseSettings,
      classifyEnabled: enabled,
    })),
```

把「自动分类开关默认关闭，门槛默认 75」这个用例整段替换成：

```tsx
  it("自动分类开关默认关闭，全局门槛不再显示且仍有一张合并卡片", async () => {
    prepare();
    const toggle = await screen.findByLabelText("开启邮件自动分类");
    expect((toggle as HTMLInputElement).checked).toBe(false);
    expect(screen.queryByLabelText("把握程度门槛")).toBeNull();
    expect(await screen.findByText("邮件自动分类 · 标签组管理")).toBeTruthy();
  });
```

把「总开关关闭时提示组开关打开也不外发」用例里的文案断言替换成：

```tsx
  it("总开关关闭时提示组开关打开也不外发", async () => {
    prepare();
    expect(await screen.findByText("总开关没开，组开关打开也不外发。")).toBeTruthy();
  });
```

把它余下 4 处 `screen.findByText("标签组管理")` 改成 `screen.findByText("邮件自动分类 · 标签组管理")`（约 973、989 两处，以及 952、1093 两处）。注意用「·」全角中点，前后各一个空格。

- [ ] **Step 2: 跑测试确认红**

Run: `npm run test -- run src/__tests__/ai.test.tsx`
Expected: FAIL，报「把握程度门槛」仍存在或找不到新标题「邮件自动分类 · 标签组管理」。

- [ ] **Step 3: 改前端状态与逻辑**

在 `src/AiPanel.tsx`：

删除这两个状态声明（约 272–275）：

```tsx
  // 「邮件自动分类」开关与把握程度门槛；只存本机，不参与账号同步。
  const [classifyEnabled, setClassifyEnabled] = useState(false);
  const [classifyPercent, setClassifyPercent] = useState(75);
  const [classifySkipConfirm, setClassifySkipConfirm] = useState(false);
```

替换成：

```tsx
  // 「邮件自动分类」总开关与手动分类免确认；只存本机，不参与账号同步。
  const [classifyEnabled, setClassifyEnabled] = useState(false);
  const [classifySkipConfirm, setClassifySkipConfirm] = useState(false);
```

删除读取设置里的 `setClassifyPercent(nextSettings.classifyConfidencePercent);`（约 327）。

`toggleClassify` 关闭分支改成：

```tsx
          const saved = await api.setClassifySettings(false);
```

并把依赖数组从

```tsx
    [classifyDraft.model, classifyDraft.providerId, classifyPercent, classifyProvider],
```

改成

```tsx
    [classifyDraft.model, classifyDraft.providerId, classifyProvider],
```

`confirmEnableClassify` 里的保存改成：

```tsx
      const saved = await api.setClassifySettings(true);
```

并把依赖数组从

```tsx
  }, [classifyDraft.model, classifyDraft.providerId, classifyDraft.thinkingLevel, classifyPercent]);
```

改成

```tsx
  }, [classifyDraft.model, classifyDraft.providerId, classifyDraft.thinkingLevel]);
```

整段删除 `saveClassifyPercent`（约 627–637，含注释「门槛输入失焦时落盘」）。

- [ ] **Step 4: 改前端渲染，合并成一张卡片**

删除原「邮件自动分类」标题、说明和独立 `section`（约 1401–1443），即从 `<h3>{t("邮件自动分类")}</h3>` 到对应 `</section>`。

把原「标签组管理」标题、说明（约 1566–1569）替换成：

```tsx
      <h3>{t("邮件自动分类 · 标签组管理")}</h3>
      <p className="hint">
        {t("打开后，新邮件正文会发给所选 TypeSafe jev 站点；模型只返回标签名和把握程度，标签和颜色只存本机。默认关闭。")}
      </p>
      <p className="hint">
        {t("组就是判断维度，一封邮件每组各出一个结果；把握门槛按组设置。标签和颜色只存在本机，不发给模型、不参与账号同步。")}
      </p>
```

把开头的 `<section className="ai-feature-card ai-label-manager">`（约 1570）替换成：

```tsx
      <section className="ai-feature-card ai-label-manager ai-label-manager-merged">
        <div className="ai-classify-global">
          <label className="checkbox">
            <input
              type="checkbox"
              aria-label={t("开启邮件自动分类")}
              checked={classifyEnabled}
              onChange={(event) => void toggleClassify(event.target.checked)}
            />
            <span>{t("开启邮件自动分类")}</span>
          </label>
          <label className="checkbox">
            <input
              type="checkbox"
              aria-label={t("手动分类免确认")}
              checked={classifySkipConfirm}
              onChange={(event) => void toggleClassifySkipConfirm(event.target.checked)}
            />
            <span>{t("手动分类免确认")}</span>
          </label>
          <p className="hint">
            {t("勾选手动分类免确认后，右键「立即分类」会直接外发、不再逐封确认；随时可在这里关掉。默认关闭。")}
          </p>
          {!classifyEnabled && (
            <p className="hint">{t("总开关没开，组开关打开也不外发。")}</p>
          )}
          {classifyNotice && <p className="notice" role="status">{classifyNotice}</p>}
          {classifyError && (
            <p className="error" role="alert">
              {t("操作失败：")}{classifyError}
            </p>
          )}
        </div>
```

把紧跟组列表前面的重复提示（约 1614–1616）整段删除：

```tsx
        {!classifyEnabled && (
          <p className="hint">{t("总开关没开，组开关打开也不外发。")}</p>
        )}
```

`src/api.ts` 里把封装改成单参数：

```ts
  /** 保存「邮件自动分类」总开关；只写本机设置。 */
  setClassifySettings: (enabled: boolean) =>
    call<AppSettings>("set_classify_settings", { enabled }),
```

- [ ] **Step 5: 加合并卡片的样式**

在 `src/index.css` 的 `.ai-label-manager {` 规则后面追加：

```css
.ai-label-manager-merged {
  gap: 12px;
}
.ai-classify-global {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding-bottom: 12px;
  border-bottom: 1px solid var(--line);
}
```

- [ ] **Step 6: 跑测试确认绿**

Run: `npm run test -- run src/__tests__/ai.test.tsx`
Expected: PASS，无「把握程度门槛」相关失败。

- [ ] **Step 7: 提交**

```bash
git add src/AiPanel.tsx src/api.ts src/index.css src/__tests__/ai.test.tsx
git commit -m "界面：合并邮件自动分类与标签组管理卡片"
```

---

### Task 2: 后端 `set_classify_settings` 变只写开关

**Files:**
- Modify: `src-tauri/src/commands.rs:674-693`（`set_classify_settings` 命令）
- Modify: `src-tauri/src/commands.rs:29`（如删除后常量不再被使用，则同时收窄 import）
- Modify: `src-tauri/src/commands.rs:585-586`（DTO 注释标明只读兼容）

**Interfaces:**
- Consumes: 前端 Task 1 已改成 `set_classify_settings` 只传 `enabled`。
- Produces: `set_classify_settings(state, enabled) -> Result<AppSettingsDto, CommandError>`；`settings.json` 里的 `classify_confidence_percent` 原值不动。

- [ ] **Step 1: 改命令签名与实现**

把命令头和实现（约 674–693）替换成：

```rust
/// 保存「邮件自动分类」总开关。
///
/// 只存本机、不参与账号同步；把握门槛按标签组各存各的，全局门槛字段只读兼容、不再写入。
#[tauri::command]
pub async fn set_classify_settings(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<AppSettingsDto, CommandError> {
    let mut settings = state.settings_snapshot();
    settings.classify_enabled = enabled;
    state
        .save_settings(settings)
        .map_err(|error| CommandError::new(format!("保存设置失败：{error}")))?;
    Ok(build_settings_dto(state.inner()))
}
```

- [ ] **Step 2: 收窄 import，避免未使用告警**

如果 `CLASSIFY_CONFIDENCE_MAX_PERCENT` / `CLASSIFY_CONFIDENCE_MIN_PERCENT` 在 `commands.rs` 里只剩这一处使用，就把它们从 `commands.rs:29` 的 import 列表删掉；若还有其它使用处，保持不动。用下面的命令确认：

Run: `rg -n "CLASSIFY_CONFIDENCE_(MIN|MAX)_PERCENT" src-tauri/src/commands.rs`
Expected: 只剩 import 或全无；再据此决定是否删 import。目标是编译时 `-D warnings` 不报未使用。

把 DTO 字段注释（约 585–586）改成：

```rust
    /// 分类把握程度门槛（百分数，50–99）；只读兼容字段，分类不再使用，界面不展示。
    pub classify_confidence_percent: u8,
```

- [ ] **Step 3: 跑后端测试确认绿**

Run（先补 PATH）:
```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test -p em-master --lib classify
```
Expected: PASS；`migrate_classify_confidence` 相关用例仍通过（迁移逻辑不动）。

- [ ] **Step 4: 提交**

```bash
git add src-tauri/src/commands.rs
git commit -m "后端：分类总开关命令不再写全局门槛"
```

---

### Task 3: 更新项目记忆

**Files:**
- Modify: `.ai-memory/handoff.md`
- Modify: `.ai-memory/project_memory.md`
- Modify: `.ai-memory/20261012/daily.md`

**Interfaces:**
- Consumes: Task 1、Task 2 的提交与验证结果。
- Produces: 记录合并卡片、全局门槛废弃、四关证据。

- [ ] **Step 1: 追加当日日志**

在 `.ai-memory/20261012/daily.md` 末尾追加一节，写清：合并卡片、删除全局门槛、`set_classify_settings` 单参数、四关结果与提交哈希。

- [ ] **Step 2: 更新交接与项目记忆**

在 `.ai-memory/handoff.md` 的「已完成」末尾追加本次合并；在 `.ai-memory/project_memory.md` 增加一条决策记录：全局把握门槛废弃、只读兼容，门槛只按组设置。

- [ ] **Step 3: 提交**

```bash
git add .ai-memory
git commit -m "记忆：记录合并分类设置卡片"
```

---

### Task 4: 四关验证与收尾

**Files:**
- Modify: 仅在验证失败时回改前面涉及的文件。

**Interfaces:**
- Consumes: Task 1–3 全部改动。
- Produces: 四关全绿的证据；工作区干净。

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
git commit -m "验证：合并分类设置卡片四关全绿"
```

- [ ] **Step 3: 确认工作区干净并汇报**

Run: `git status --short`
Expected: 无输出。汇报四件事：做完的（带提交哈希与四关证据）、相关文件、没做完的、卡在哪。

---

## 自查

- **规格覆盖**：R1–R35 不受影响；本计划对应 R36（一张合并卡片、无全局门槛）、R35（组折叠与操作顺序保持不变）、R8（组级门槛保留）。R7 总开关语义不变。
- **占位符扫描**：无 TBD/TODO；每个改动步骤都给了具体代码。
- **类型一致**：`setClassifySettings(enabled: boolean)` 三处（`api.ts`、`AiPanel.tsx`、`ai.test.tsx` 桩）一致；后端命令 `set_classify_settings(enabled)` 与之一致。