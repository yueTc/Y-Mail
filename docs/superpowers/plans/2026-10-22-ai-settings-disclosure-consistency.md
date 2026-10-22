# AI 设置折叠交互统一实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 AI 设置里的功能卡片和标签组都支持点击名称展开，同时保留标签组名称输入框只负责改名。

**Architecture:** 复用现有 `expandedFeatures`、`expandedGroups` 和切换函数，不新增持久化状态。功能卡片把标题放进展开按钮；标签组把“箭头 + 组名文字”放进展开按钮，把组名输入框移到展开区。三处统一使用 `data-expanded`、`aria-expanded` 和 `aria-controls`。

**Tech Stack:** React 19、TypeScript、Vitest、Testing Library、CSS。

**Spec:** `docs/superpowers/specs/2026-10-22-ai-settings-disclosure-consistency-design.md`

## Global Constraints

- 默认全部收起，多个区域可以同时展开，不保存折叠状态。
- 只改前端界面和测试，不改后端接口、数据库、配置、密钥、代理和外发确认。
- 标签组收起时仍显示组名、类型、排序、删除和启用开关。
- 标签组名称输入框只负责改名，不触发折叠。
- 界面文字使用中文；文件保持 UTF-8 无 BOM、LF 换行。
- 不碰当前工作区里与本次任务无关的未提交改动。

## Review Focus

- 组名或功能名很长时，标题按钮不能把右侧操作挤出容器；窄窗口要能换行。
- 点击类型、排序、删除或启用开关时不能误触折叠。
- 标签组展开后，改名输入框必须仍能失焦保存，且只出现在展开区。
- 读屏辅助名称必须随状态在“展开某名称”和“折叠某名称”之间切换。
- 同时展开多个功能或标签组时，不能把先前展开的项目自动收起。

---

### Task 1: 功能卡片名称可点击

**Files:**
- Modify: `src/AiPanel.tsx:1090-1201`
- Modify: `src/wave7.css:166-198`
- Modify: `src/index.css:3913-3931`
- Test: `src/__tests__/ai.test.tsx:1429-1452`

**Interfaces:**
- Consumes: `toggleFeatureExpanded(fn: AiFunction)`, `expandedFeatures: Record<string, boolean>`
- Produces: 功能卡片按钮包含箭头和功能名称；详情区使用 `id={`ai-feature-details-${fn}`}`

- [ ] **Step 1: 写失败测试**

在 `src/__tests__/ai.test.tsx` 增加“点功能名称也能展开，别的功能保持折叠”：
- 取第一张功能卡片里的“翻译”文字。
- 点击这段文字。
- 断言卡片 `data-expanded="true"`，并出现“翻译站点”。
- 断言“摘要站点”仍不存在。
- 断言按钮具备 `aria-controls`，且指向详情区编号。

Run: `npm test -- src/__tests__/ai.test.tsx`
Expected: FAIL，因为当前标题不在展开按钮里。

- [ ] **Step 2: 改功能卡片标题和详情关联**

在 `src/AiPanel.tsx`：
- 把功能名称从按钮外的 `h4` 移到 `ai-feature-toggle` 按钮内部。
- 按钮内依次放箭头和 `<span>` 形式的功能名称，保留现有中文辅助名称。
- 给详情区增加 `id={`ai-feature-details-${fn}`}`，按钮增加同名 `aria-controls`。
- 卡片状态属性改为 `data-expanded={expanded ? "true" : "false"}`。
- 点击统一调用 `toggleFeatureExpanded(fn)`。

- [ ] **Step 3: 调整样式**

在 `src/wave7.css` 和 `src/index.css`：
- `ai-feature-toggle` 改为横向撑满剩余宽度、左对齐、允许文字换行。
- 移除只针对按钮外 `h4` 的间距依赖，保留紧凑标题字号。
- 焦点样式与 AI 站点按钮一致。

- [ ] **Step 4: 跑定向测试**

Run: `npm test -- src/__tests__/ai.test.tsx`
Expected: PASS，65 项以上全过。

- [ ] **Step 5: 提交**

```bash
git add src/AiPanel.tsx src/index.css src/wave7.css src/__tests__/ai.test.tsx
git commit -m "统一 AI 功能卡片展开交互"
```

### Task 2: 标签组名称可点击且输入框只改名

**Files:**
- Modify: `src/AiPanel.tsx:1404-1505`
- Modify: `src/index.css:3858-3931`
- Test: `src/__tests__/ai.test.tsx:1399-1427`

**Interfaces:**
- Consumes: `toggleGroupExpanded(groupId: number)`, `expandedGroups: Record<number, boolean>`
- Produces: 标签组按钮包含箭头和组名文字；详情区使用 `id={`ai-label-group-details-${group.id}`}`

- [ ] **Step 1: 写失败测试**

在 `src/__tests__/ai.test.tsx` 增加标签组断言：
- 默认收起时，“展开内容分类”按钮的文字包含“内容分类”。
- 点击该按钮后，组名输入框位于 `.ai-label-group-body` 内。
- 点击输入框只进入编辑，不改变 `aria-expanded="true"`。
- 点击上移、下移、删除和启用开关不触发折叠；沿用现有行为断言。
- 按钮具备 `aria-controls`，且指向标签组详情区编号。
- 用超长组名渲染时，名称按钮和右侧操作仍是并列控件，不互相嵌套。

Run: `npm test -- src/__tests__/ai.test.tsx`
Expected: FAIL，因为当前按钮文字只有箭头，输入框仍在标题行。

- [ ] **Step 2: 改标签组标题和详情结构**

在 `src/AiPanel.tsx`：
- 展开按钮内放箭头和组名文字，保留 `aria-label` 为“展开某组”或“折叠某组”。
- 组类型、操作按钮和启用开关继续留在按钮外。
- 把组名输入框移到展开区，使用带“组名”文字的 `label`，保留现有 `aria-label` 和失焦保存。
- 给详情区增加 `id={`ai-label-group-details-${group.id}`}`，按钮增加同名 `aria-controls`。
- 标签项状态属性改为 `data-expanded={expanded ? "true" : "false"}`。

- [ ] **Step 3: 调整样式**

在 `src/index.css`：
- 标题按钮横向排列箭头和名称，左对齐，名称可换行。
- 给组名输入框在新详情位置保留可读宽度。
- 标题行在窄窗口允许换行，右侧操作仍靠右。
- 标题按钮和 AI 站点按钮使用一致的焦点样式。

- [ ] **Step 4: 跑定向测试**

Run: `npm test -- src/__tests__/ai.test.tsx`
Expected: PASS，所有 AI 设置测试通过。

- [ ] **Step 5: 提交**

```bash
git add src/AiPanel.tsx src/index.css src/__tests__/ai.test.tsx
git commit -m "统一 AI 标签组展开交互"
```

### Task 3: 完整验证和项目记忆

**Files:**
- Modify: `.ai-memory/handoff.md`
- Modify: `.ai-memory/project_memory.md`
- Modify: `.ai-memory/20261022/daily.md`

**Interfaces:**
- Consumes: Task 1 和 Task 2 的最终界面与测试。
- Produces: 可交付的验证结果和后续接手记录。

- [ ] **Step 1: 跑四关**

Run:
```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run build
```
Expected: 四条命令退出码均为 0。

- [ ] **Step 2: 检查改动范围**

Run: `git status --short`
Expected: 与开始实现前保存的状态对比，本次新增改动只出现在 `src/AiPanel.tsx`、`src/index.css`、`src/wave7.css`、`src/__tests__/ai.test.tsx`；开始时已有的未提交改动保持原样。

- [ ] **Step 3: 更新项目记忆**

按仓库规则写 `.ai-memory/handoff.md`、`.ai-memory/project_memory.md` 和 `.ai-memory/20261022/daily.md`，记录做完的、相关文件、没做完的和阻塞点。

- [ ] **Step 4: 汇报**

向用户报告四关结果、相关文件和仍需人工复看的部分；不提交任何无关改动。