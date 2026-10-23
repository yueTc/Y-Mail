# 邮箱右键「立即同步」实施计划

> **给执行者：** 按任务顺序做；每步先写失败测试，再实现，再跑测试。必须按 `superpowers:subagent-driven-development` 或 `superpowers:executing-plans` 执行本计划。

**目标：** 在左侧邮箱账号右键菜单增加「立即同步」，只重启被点中的邮箱，并给出成功短提示和可读失败原因。

**架构：** 复用现有 Tauri 同步命令，前端先停止目标账号再立即启动，随后刷新同步状态。账号菜单、短提示、错误提示和忙碌集合都留在 `InboxPanel` 内，不新增后端接口。

**技术栈：** React 19、TypeScript、Vitest、Tauri 2、Rust。

**规格：** `docs/superpowers/specs/2026-10-23-account-context-sync-now-design.md`

**分支：** `codex/account-context-sync-now`

---

## 全局约束

- 面向用户使用中文大白话；代码、命令和文件名除外。
- 不新增后端接口，不改同步协议、凭据、代理和数据库。
- 只重启被右键点中的账号，不调用其它账号的同步操作。
- 不增加二次确认；点击后菜单关闭并显示「正在同步「邮箱名」」短提示。
- 写文件使用 `[System.IO.File]::WriteAllText($path, $content, (New-Object System.Text.UTF8Encoding $false))`；UTF-8 无 BOM、LF。
- 只暂存本批文件，禁止 `git add .`；提交信息用中文。
- 每批改动必须通过：
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `npm run build`
- 本机找不到 `cargo` 时先执行：`$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"`。

---

## 文件结构

- 邮箱栏和账号菜单：`src/InboxPanel.tsx`
- 现有命令封装：`src/api.ts`（只读确认 `stopSync`、`startSync`、`syncStatus`，不修改）
- 前端测试：`src/__tests__/flaggedView.test.tsx`
- 英文文案：`src/i18n.en.ts`
- 主规格：`docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md`
- 功能规格：`docs/superpowers/specs/2026-10-23-account-context-sync-now-design.md`
- 记忆文件：`.ai-memory/handoff.md`、`.ai-memory/project_memory.md`、`.ai-memory/20261023/daily.md`

---

### Task 1：账号右键菜单增加立即同步

**Files：**

- Modify: `src/InboxPanel.tsx:660-703`
- Modify: `src/InboxPanel.tsx:1671-1683`
- Modify: `src/InboxPanel.tsx:2038-2068`
- Modify: `src/InboxPanel.tsx:2174-2229`
- Modify: `src/__tests__/flaggedView.test.tsx:138-178`
- Modify: `src/__tests__/flaggedView.test.tsx:486-497`
- Test: `src/__tests__/flaggedView.test.tsx` 新增「邮箱右键立即同步」测试组
- Modify: `src/i18n.en.ts:350-360`

**Interfaces：**

- Consumes: `api.stopSync(accountId?: number): Promise<void>`
- Consumes: `api.startSync(accountId?: number): Promise<number>`
- Consumes: `api.syncStatus(): Promise<SyncStatus[]>`
- Produces: `syncAccountNow(account: AccountInboxSummary): Promise<void>`，仅供 `InboxPanel` 内部使用
- Produces: 账号菜单按钮文案 `立即同步`
- Produces: 短提示文案 `正在同步「{0}」`

- [ ] **Step 1：先改测试，让新行为处于失败状态**

在 `src/__tests__/flaggedView.test.tsx` 的 `beforeEach` 中，在 `api.stopSync` 的模拟值后面补一行：

```tsx
vi.mocked(api.startSync).mockResolvedValue(1);
```

把现有账号菜单顺序断言改成：

```tsx
expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
  "全部标为已读", "立即同步", "编辑", "新建文件夹", "删除邮箱",
]);
```

在「邮箱栏右键添加和删除」测试组后新增测试组：

```tsx
describe("邮箱右键立即同步", () => {
  function twoAccounts() {
    return {
      accounts: [
        {
          accountId: 1,
          email: "a@example.com",
          displayName: "测试账号",
          color: "#3366ff",
          enabled: true,
          messageCount: 3,
          unreadCount: 2,
        },
        {
          accountId: 2,
          email: "b@example.com",
          displayName: "第二个账号",
          color: "#ff6633",
          enabled: true,
          messageCount: 1,
          unreadCount: 1,
        },
      ],
      totalUnread: 3,
      totalMessages: 4,
    };
  }

  it("只重启点中的账号，并显示正在同步的短提示", async () => {
    vi.mocked(api.inboxSummary).mockResolvedValue(twoAccounts());
    render(<InboxPanel />);

    const account = await screen.findByRole("button", { name: /测试账号/ });
    fireEvent.contextMenu(account);
    const menu = screen.getByRole("menu", { name: "邮箱菜单" });
    fireEvent.click(within(menu).getByRole("menuitem", { name: "立即同步" }));

    await waitFor(() => expect(api.startSync).toHaveBeenCalledWith(1));
    expect(api.stopSync).toHaveBeenCalledWith(1);
    expect(api.stopSync).not.toHaveBeenCalledWith(2);
    expect(api.startSync).not.toHaveBeenCalledWith(2);
    expect(vi.mocked(api.stopSync).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(api.startSync).mock.invocationCallOrder[0],
    );
    expect(screen.queryByRole("menu", { name: "邮箱菜单" })).toBeNull();
    expect(screen.getByText("正在同步「测试账号」")).toBeTruthy();
  });

  it("同步重启期间，同一账号再次右键时按钮禁用", async () => {
    let finishStop: (() => void) | undefined;
    vi.mocked(api.stopSync).mockImplementationOnce(
      () => new Promise<void>((resolve) => {
        finishStop = resolve;
      }),
    );
    render(<InboxPanel />);

    const account = await screen.findByRole("button", { name: /测试账号/ });
    fireEvent.contextMenu(account);
    fireEvent.click(screen.getByRole("menuitem", { name: "立即同步" }));
    await waitFor(() => expect(api.stopSync).toHaveBeenCalledWith(1));

    fireEvent.contextMenu(account);
    const busyMenu = screen.getByRole("menu", { name: "邮箱菜单" });
    const syncItem = within(busyMenu).getByRole("menuitem", { name: "立即同步" }) as HTMLButtonElement;
    expect(syncItem.disabled).toBe(true);

    await act(async () => {
      finishStop?.();
      await Promise.resolve();
    });
    await waitFor(() => expect(api.startSync).toHaveBeenCalledWith(1));
  });

  it("启动失败时显示错误，忙碌状态恢复并可再次点击", async () => {
    vi.mocked(api.startSync).mockRejectedValueOnce(new Error("连接失败"));
    render(<InboxPanel />);

    const account = await screen.findByRole("button", { name: /测试账号/ });
    fireEvent.contextMenu(account);
    fireEvent.click(screen.getByRole("menuitem", { name: "立即同步" }));

    expect(await screen.findByText("同步操作失败：连接失败")).toBeTruthy();
    fireEvent.contextMenu(account);
    const menu = screen.getByRole("menu", { name: "邮箱菜单" });
    const syncItem = within(menu).getByRole("menuitem", { name: "立即同步" }) as HTMLButtonElement;
    expect(syncItem.disabled).toBe(false);
  });
});
```

- [ ] **Step 2：运行定向测试，确认先失败**

```powershell
npm test -- --run src/__tests__/flaggedView.test.tsx
```

预期：账号菜单找不到「立即同步」；新增测试失败。若测试文件本身有类型或语法错误，先修测试，直到失败原因确实是功能未实现。

- [ ] **Step 3：实现菜单项和单账号重启**

在 `src/InboxPanel.tsx` 的账号菜单状态旁增加：

```tsx
// 正在执行「停止后立即启动」的账号；按账号分开记，避免互相挡住。
const [syncingAccounts, setSyncingAccounts] = useState<Set<number>>(new Set());
const [accountSyncError, setAccountSyncError] = useState("");
```

在 `retrySync` 附近增加单账号重启函数：

```tsx
/** 账号右键「立即同步」：只重启这一账号，复用同步页的停止后再启动做法。 */
const syncAccountNow = useCallback(async (account: AccountInboxSummary) => {
  setAccountMenu(undefined);
  setAccountSyncError("");
  setSyncingAccounts((current) => new Set(current).add(account.accountId));
  setStatusNote(t("正在同步「{0}」", [account.displayName || account.email]));
  try {
    await api.stopSync(account.accountId);
    await api.startSync(account.accountId);
    setSyncStatuses(await api.syncStatus());
  } catch (caught) {
    setAccountSyncError(describeError(caught));
  } finally {
    setSyncingAccounts((current) => {
      const next = new Set(current);
      next.delete(account.accountId);
      return next;
    });
  }
}, []);
```

在 `inbox-main` 的现有错误提示旁增加：

```tsx
{accountSyncError && (
  <p className="error" role="alert">
    {t("同步操作失败：")}{accountSyncError}
  </p>
)}
```

在账号菜单「全部标为已读」按钮和「编辑」按钮之间增加：

```tsx
<button
  type="button"
  role="menuitem"
  className="context-menu-item"
  disabled={syncingAccounts.has(menuAccount.accountId)}
  onClick={() => void syncAccountNow(menuAccount)}
>
  {t("立即同步")}
</button>
```

把账号菜单的顶部避让高度从 `190` 改为 `240`，容纳新增菜单项：

```tsx
top: Math.min(accountMenu.y, Math.max(8, window.innerHeight - (menuAccount ? 240 : 60))),
```

在 `src/i18n.en.ts` 的同步文案附近增加：

```ts
"正在同步「{0}」": "Syncing “{0}”",
```

说明：`立即同步` 和 `同步操作失败：` 的英文翻译已经存在，不重复添加。

- [ ] **Step 4：运行定向测试和构建**

```powershell
npm test -- --run src/__tests__/flaggedView.test.tsx
npm run build
```

预期：`flaggedView.test.tsx` 全部通过，现有邮箱菜单、删除、编辑和文件夹测试不回归；TypeScript 与 Vite 构建成功。

- [ ] **Step 5：只提交本任务文件**

```powershell
git add -- src/InboxPanel.tsx src/__tests__/flaggedView.test.tsx src/i18n.en.ts
git diff --cached --check
git commit -m "邮箱右键增加立即同步"
```

---

### Task 2：四关、规格、记忆和交付检查

**Files：**

- Modify: `docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md`
- Modify: `.ai-memory/handoff.md`
- Modify: `.ai-memory/project_memory.md`
- Create: `.ai-memory/20261023/daily.md`

**Interfaces：**

- Consumes: Task 1 已落地的菜单、单账号重启和测试。
- Produces: 主规格 v1.6 的已确认变更记录。
- Produces: 本次阶段交接、项目记忆和当日记录。

- [ ] **Step 1：跑完整四关**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run build
```

任何一关失败都先修，不能跳过。记录实际命令结果，不写“大概通过”。

- [ ] **Step 2：检查改动和安全边界**

```powershell
git diff --check
git status --short
git diff -- src/InboxPanel.tsx src/__tests__/flaggedView.test.tsx src/i18n.en.ts
```

人工确认：

- 只对目标账号调用 `stopSync` 和 `startSync`。
- 没有新增网络请求、保险箱读取、日志或数据库写入。
- 没有提交 `*.db`、`.env`、密钥文件和构建产物。
- 空白处右键仍只有「添加邮箱」。

- [ ] **Step 3：更新主规格**

在 `docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md`：

1. 顶部版本从 `v1.5` 改为 `v1.6`。
2. 状态行追加：`v1.6 为 2026-10-23 的邮箱右键立即同步补充`。
3. 在 R1「邮箱栏右键入口」条目追加：

```markdown
- **账号右键立即同步（2026-10-23，用户确认）**：账号右键菜单在「全部标为已读」与「编辑」之间提供「立即同步」。点击后 MUST 只停止并重新启动该账号的同步，不得影响其它账号；操作期间同一账号入口 MUST 禁用；成功后显示包含邮箱名称的短提示，失败 MUST 在收件箱显示可读原因。空白处、文件夹和统一视图的右键行为不变。
```

4. 变更记录表增加：

```markdown
| v1.6 | 2026-10-23 | 新增邮箱账号右键「立即同步」：只重启目标账号，失败隔离并在收件箱提示 |
```

- [ ] **Step 4：更新记忆**

在 `.ai-memory/handoff.md` 顶部「已完成」区域增加本次记录：

```markdown
- **邮箱右键立即同步（2026-10-23）**：左侧邮箱账号右键菜单在「全部标为已读」与「编辑」之间新增「立即同步」；只对目标账号执行停止后启动，期间同一账号入口禁用，失败在收件箱显示可读原因。四关与前端定向测试全绿；未做真实服务器上的桌面手工复验。
```

在 `.ai-memory/project_memory.md` 增加决策：

```markdown
## 2026-10-23：邮箱右键立即同步

- 账号右键「立即同步」复用设置同步页的「先停止、再启动」做法，确保不等原来的空闲等待结束。
- 只重启点中的账号；其它账号不受影响。
- 短提示写明邮箱名称；失败原因放在收件箱现有错误提示区。
- 该入口不新增后端命令，也不增加二次确认。
```

创建 `.ai-memory/20261023/daily.md`，内容按实际结果填写：

```markdown
# 2026-10-23

## 邮箱右键立即同步

- 做完的：账号右键新增「立即同步」；只重启目标账号；补充菜单顺序、调用顺序、忙碌恢复和失败提示测试。
- 验证证据：填写本次 `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`npm run build` 的实际结果，以及前端定向测试结果。
- 相关文件：`src/InboxPanel.tsx`、`src/__tests__/flaggedView.test.tsx`、`src/i18n.en.ts`、主规格和功能规格。
- 没做完的：真实服务器上的桌面手工复验。
- 卡在哪：如果四关或手工复验有问题，在这里写具体命令、错误和下一步；没有就写「无」。
```

- [ ] **Step 5：复查文档并提交**

```powershell
git diff --check
git add -- docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md .ai-memory/handoff.md .ai-memory/project_memory.md .ai-memory/20261023/daily.md
git diff --cached --check
git commit -m "记录邮箱右键立即同步"
```

最后向用户汇报四件事：做完的（带证据）、相关文件、没做完的、卡在哪。

---

## 完成标准

- 账号右键菜单顺序为：全部标为已读、立即同步、编辑、新建文件夹、删除邮箱。
- 点击后仅对该账号按顺序执行停止、启动，并刷新同步状态。
- 同一账号正在重启时入口禁用；失败后恢复可点。
- 成功短提示包含邮箱名称；失败错误在收件箱显示。
- 主规格升级到 v1.6，交接、项目记忆和当日记录已更新。
- 四关全部通过。