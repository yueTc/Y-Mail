# 批量清除未读 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在邮箱账号和文件夹右键菜单增加批量标为已读，账号范围覆盖全部文件夹，文件夹范围只覆盖自己，全程只改本机。

**Architecture:** `mail-store` 在一个事务里批量更新 `message.is_read` 并按实际邮件重算 `folder.unread_count`；`mail-core` 做范围和存在性校验；Tauri 暴露一个批量命令；React 只负责菜单、确认、调用和刷新，不逐封发请求。

**Tech Stack:** Rust 2021、rusqlite、Tauri、React、TypeScript、Vitest、SQLite WAL。

**Spec:** `docs/superpowers/specs/2026-10-20-bulk-clear-unread-design.md`

## Global Constraints

- 面向用户一律用中文大白话；代码、命令、文件名除外。
- 邮件正文是不可信内容，本功能不得读取正文、不得联网、不得回写服务器。
- 密码、授权码、令牌、AI 密钥只许存进系统保险箱；本功能不接触任何凭据。
- 只改本机数据库；不得生成服务器待回写任务。
- 每批代码改动必须通过：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`npm run build`。
- 写文件使用 `[System.IO.File]::WriteAllText($path, $content, (New-Object System.Text.UTF8Encoding $false))`；UTF-8 无 BOM、LF。
- 提交信息用中文；禁止提交 `*.db`、`.env`、密钥文件和构建产物。
- 当前分支：`codex/bulk-clear-unread`；不得碰 `D:\projects` 父目录仓库。
- 每批只暂存本批文件，禁止 `git add .`。

---

## 文件结构

- 数据层：`crates/mail-store/src/inbox.rs`。
- 编排层：`crates/mail-core/src/inbox.rs`。
- 外壳：`src-tauri/src/commands.rs`、`src-tauri/src/lib.rs`。
- 前端：`src/api.ts`、`src/InboxPanel.tsx`、`src/i18n.en.ts`、`src/__tests__/folderManagement.test.tsx`。
- 文档：`docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md`、`docs/user-guide.md`、`.ai-memory/`。

---

### Task 1: 数据层与核心层批量接口

**Files:**
- Modify: `crates/mail-store/src/inbox.rs`
- Modify: `crates/mail-core/src/inbox.rs`
- Test: `crates/mail-store/src/inbox.rs`
- Test: `crates/mail-core/src/inbox.rs`

**Interfaces:**
- Consumes: 现有 `Store`、`InboxQuery`、`MailEngine::get_account`。
- Produces: `Store::mark_all_read(account_id: i64, folder_id: Option<i64>) -> Result<usize, StoreError>`。
- Produces: `MailEngine::mark_all_read(account_id: i64, folder_id: Option<i64>) -> Result<usize, EngineError>`。

- [ ] **Step 1: 写数据层失败测试**

在 `crates/mail-store/src/inbox.rs` 测试模块加入三个场景：

```rust
#[test]
fn 账号批量已读覆盖全部文件夹且不影响其它账号() {
    let store = migrated();
    let a = account(&store, "a@example.com", "#111111");
    let b = account(&store, "b@example.com", "#222222");
    let a_inbox = inbox(&store, a);
    let a_custom = store
        .upsert_folder(a, "工作", "/", FolderKind::Custom)
        .expect("插入自定义文件夹");
    let b_inbox = inbox(&store, b);
    store.insert_messages(&[
        message(a, a_inbox, 1, "a1", "<a1@x>", "2026-10-01T00:00:00Z", false),
        message(a, a_inbox, 2, "a2", "<a2@x>", "2026-10-01T01:00:00Z", false),
        message(a, a_custom, 1, "a3", "<a3@x>", "2026-10-01T02:00:00Z", false),
        message(b, b_inbox, 1, "b1", "<b1@x>", "2026-10-01T03:00:00Z", false),
    ]).expect("插入邮件");

    assert_eq!(store.mark_all_read(a, None).expect("账号批量已读"), 3);
    let folders = store.list_inbox_folders().expect("列文件夹");
    assert!(folders.iter().filter(|f| f.account_id == a).all(|f| f.unread_count == 0));
    assert_eq!(folders.iter().find(|f| f.account_id == b).expect("b 收件箱").unread_count, 1);
}

#[test]
fn 文件夹批量已读只处理当前文件夹() {
    let store = migrated();
    let a = account(&store, "a@example.com", "#111111");
    let first = inbox(&store, a);
    let second = store
        .upsert_folder(a, "工作", "/", FolderKind::Custom)
        .expect("插入自定义文件夹");
    store.insert_messages(&[
        message(a, first, 1, "t1", "<m1@x>", "2026-10-01T00:00:00Z", false),
        message(a, second, 1, "t2", "<m2@x>", "2026-10-01T01:00:00Z", false),
    ]).expect("插入邮件");

    assert_eq!(store.mark_all_read(a, Some(second)).expect("文件夹批量已读"), 1);
    let folders = store.list_inbox_folders().expect("列文件夹");
    assert_eq!(folders.iter().find(|f| f.folder_id == first).expect("收件箱").unread_count, 1);
    assert_eq!(folders.iter().find(|f| f.folder_id == second).expect("工作").unread_count, 0);
}

#[test]
fn 没有未读时批量已读返回零() {
    let store = migrated();
    let a = account(&store, "a@example.com", "#111111");
    let fa = inbox(&store, a);
    store.insert_messages(&[
        message(a, fa, 1, "t1", "<m1@x>", "2026-10-01T00:00:00Z", true),
    ]).expect("插入邮件");

    assert_eq!(store.mark_all_read(a, None).expect("批量已读"), 0);
}
```

- [ ] **Step 2: 运行测试确认失败**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test -p mail-store 批量已读 -- --nocapture
```

Expected: 编译失败，提示 `mark_all_read` 不存在。

- [ ] **Step 3: 实现数据层批量接口**

在 `impl Store` 里加入：

```rust
/// 把范围内未读邮件一次改为已读，并按实际邮件重算受影响文件夹未读数。
pub fn mark_all_read(
    &self,
    account_id: i64,
    folder_id: Option<i64>,
) -> Result<usize, StoreError> {
    let tx = self.conn().unchecked_transaction()?;
    let changed = match folder_id {
        Some(folder_id) => tx.execute(
            "UPDATE message SET is_read = 1, \
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE account_id = ?1 AND folder_id = ?2 AND is_read = 0",
            rusqlite::params![account_id, folder_id],
        )?,
        None => tx.execute(
            "UPDATE message SET is_read = 1, \
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE account_id = ?1 AND is_read = 0",
            [account_id],
        )?,
    };

    match folder_id {
        Some(folder_id) => {
            tx.execute(
                "UPDATE folder SET unread_count = \
                 (SELECT COUNT(*) FROM message m WHERE m.folder_id = folder.id AND m.is_read = 0), \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
                 WHERE account_id = ?1 AND id = ?2",
                rusqlite::params![account_id, folder_id],
            )?;
        }
        None => {
            tx.execute(
                "UPDATE folder SET unread_count = \
                 (SELECT COUNT(*) FROM message m WHERE m.folder_id = folder.id AND m.is_read = 0), \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
                 WHERE account_id = ?1",
                [account_id],
            )?;
        }
    }

    tx.commit()?;
    Ok(changed)
}
```

- [ ] **Step 4: 运行数据层测试确认通过**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test -p mail-store 批量已读 -- --nocapture
```

Expected: 3 条通过。

- [ ] **Step 5: 写核心层失败测试**

在 `crates/mail-core/src/inbox.rs` 测试模块加入：

```rust
#[test]
fn 批量已读会拒绝不存在的账号() {
    let dir = tempfile::tempdir().expect("创建临时目录");
    let engine = MailEngine::initialize_with_secrets(dir.path(), Arc::new(MemorySecretStore::new()))
        .expect("初始化引擎");
    let error = engine.mark_all_read(999, None).expect_err("不存在的账号应拒绝");
    assert!(error.to_string().contains("账号不存在"));
}
```

- [ ] **Step 6: 运行核心层测试确认失败**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test -p mail-core 批量已读会拒绝不存在的账号 -- --nocapture
```

Expected: 编译失败，提示 `MailEngine::mark_all_read` 不存在。

- [ ] **Step 7: 实现核心层校验和调用**

在 `crates/mail-core/src/inbox.rs` 的方法区加入：

```rust
/// 把当前账号或指定文件夹里的未读邮件一次改为已读。
///
/// 只写本机库，不访问网络，也不生成服务器回写任务。
pub fn mark_all_read(
    &self,
    account_id: i64,
    folder_id: Option<i64>,
) -> Result<usize, EngineError> {
    self.get_account(mail_domain::account::AccountId(account_id))?;
    if let Some(folder_id) = folder_id {
        let folder = self
            .store()
            .get_folder_by_id(folder_id)?
            .ok_or_else(|| EngineError::BadRequest("文件夹不存在或已被删除".to_string()))?;
        if folder.account_id != account_id {
            return Err(EngineError::BadRequest(
                "文件夹不属于这个邮箱".to_string(),
            ));
        }
    }
    Ok(self.store().mark_all_read(account_id, folder_id)?)
}
```

- [ ] **Step 8: 跑四关并确认本任务通过**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run build
```

Expected: 四条命令退出码均为 0。

- [ ] **Step 9: 提交本任务**

```powershell
git add -- crates/mail-store/src/inbox.rs crates/mail-core/src/inbox.rs
git commit -m "功能：增加本机批量已读"
```

---

### Task 2: Tauri 命令与前端接口

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/api.ts`

**Interfaces:**
- Consumes: `MailEngine::mark_all_read(account_id, folder_id)`。
- Produces: Tauri 命令 `mark_all_read(account_id, folder_id?) -> { changed }`。
- Produces: `api.markAllRead(accountId, folderId?) -> Promise<{ changed: number }>`。

- [ ] **Step 1: 加外壳命令**

在 `src-tauri/src/commands.rs` 的单封已读命令附近加入：

```rust
/// 批量标为已读的结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkAllReadDto {
    /// 本次由未读变为已读的邮件数量。
    pub changed: usize,
}

/// 把当前账号或指定文件夹里的未读邮件一次改为已读，只写本机库。
#[tauri::command]
pub async fn mark_all_read(
    state: tauri::State<'_, AppState>,
    account_id: i64,
    folder_id: Option<i64>,
) -> Result<MarkAllReadDto, String> {
    if account_id <= 0 {
        return Err("邮箱编号不合法".to_string());
    }
    if folder_id.is_some_and(|id| id <= 0) {
        return Err("文件夹编号不合法".to_string());
    }
    let engine = state.engine().await;
    let changed = engine
        .mark_all_read(account_id, folder_id)
        .map_err(|err| err.to_string())?;
    Ok(MarkAllReadDto { changed })
}
```

在 `src-tauri/src/lib.rs` 的命令列表里，紧跟 `commands::set_message_read` 加入：

```rust
            commands::mark_all_read,
```

- [ ] **Step 2: 加前端接口**

在 `src/api.ts` 的单封已读接口旁边加入：

```ts
/** 批量把当前账号或指定文件夹里的未读邮件标为已读；只改本机。 */
markAllRead: (accountId: number, folderId?: number) =>
  call<{ changed: number }>(
    "mark_all_read",
    folderId === undefined ? { accountId } : { accountId, folderId },
  ),
```

- [ ] **Step 3: 跑四关并确认本任务通过**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run build
```

Expected: 四条命令退出码均为 0。

- [ ] **Step 4: 提交本任务**

```powershell
git add -- src-tauri/src/commands.rs src-tauri/src/lib.rs src/api.ts
git commit -m "接口：接通批量已读命令"
```

---

### Task 3: 右键菜单、确认与界面刷新

**Files:**
- Modify: `src/InboxPanel.tsx`
- Modify: `src/i18n.en.ts`
- Test: `src/__tests__/folderManagement.test.tsx`

**Interfaces:**
- Consumes: `api.markAllRead(accountId, folderId?)`。
- Produces: 账号菜单项「全部标为已读」，带确认。
- Produces: 文件夹菜单项「全部标为已读」，直接执行。

- [ ] **Step 1: 扩展前端测试桩并写失败测试**

在 `src/__tests__/folderManagement.test.tsx` 的 `api` 模拟对象里加入：

```ts
    markAllRead: vi.fn(),
```

在 `describe("文件夹右键菜单")` 与账号菜单测试附近加入：

```tsx
it("账号右键全部已读先确认，取消时不调用接口", async () => {
  const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
  await openAccount();
  fireEvent.contextMenu(screen.getByRole("button", { name: /测试账号/ }));
  fireEvent.click(await screen.findByRole("menuitem", { name: "全部标为已读" }));
  expect(confirm).toHaveBeenCalled();
  expect(api.markAllRead).not.toHaveBeenCalled();
});

it("账号右键全部已读确认后覆盖整个账号并提示数量", async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  vi.mocked(api.markAllRead).mockResolvedValue({ changed: 4 });
  await openAccount();
  fireEvent.contextMenu(screen.getByRole("button", { name: /测试账号/ }));
  fireEvent.click(await screen.findByRole("menuitem", { name: "全部标为已读" }));
  await waitFor(() => expect(api.markAllRead).toHaveBeenCalledWith(1, undefined));
  expect(await screen.findByText("已将 4 封邮件标为已读")).toBeTruthy();
});

it("文件夹右键全部已读直接处理当前文件夹", async () => {
  vi.mocked(api.listInboxFolders).mockResolvedValue([
    folder({ folderId: 10, fullPath: "工作" }),
  ]);
  vi.mocked(api.markAllRead).mockResolvedValue({ changed: 2 });
  const confirm = vi.spyOn(window, "confirm");
  await openAccount();
  fireEvent.contextMenu(screen.getByText("工作"));
  fireEvent.click(await screen.findByRole("menuitem", { name: "全部标为已读" }));
  await waitFor(() => expect(api.markAllRead).toHaveBeenCalledWith(1, 10));
  expect(confirm).not.toHaveBeenCalled();
  expect(await screen.findByText("已将 2 封邮件标为已读")).toBeTruthy();
});

it("没有未读时提示没有未读邮件", async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  vi.mocked(api.markAllRead).mockResolvedValue({ changed: 0 });
  await openAccount();
  fireEvent.contextMenu(screen.getByRole("button", { name: /测试账号/ }));
  fireEvent.click(await screen.findByRole("menuitem", { name: "全部标为已读" }));
  expect(await screen.findByText("没有未读邮件")).toBeTruthy();
});
```

- [ ] **Step 2: 运行前端测试确认失败**

```powershell
npx vitest run src/__tests__/folderManagement.test.tsx
```

Expected: 找不到「全部标为已读」菜单项，测试失败。

- [ ] **Step 3: 实现批量已读处理器**

在 `InboxPanel` 状态区加入：

```tsx
const [bulkReadBusy, setBulkReadBusy] = useState(false);
```

在 `refreshAll` 附近加入：

```tsx
/** 批量已读后刷新侧栏；搜索中保留关键词重新搜索。 */
const refreshAfterBulkRead = useCallback(async () => {
  await refreshSidebar();
  if (searchRaw !== "") {
    await runSearch({ raw: searchRaw });
  } else {
    await load(true);
  }
}, [refreshSidebar, searchRaw, runSearch, load]);

/** 批量标为已读；folderId 为空表示当前账号所有文件夹。 */
const markAllRead = useCallback(async (accountId: number, folderId?: number) => {
  if (bulkReadBusy) return;
  setAccountMenu(undefined);
  setFolderMenu(undefined);
  setStatusNote("");
  setError(undefined);
  setBulkReadBusy(true);
  try {
    const result = await api.markAllRead(accountId, folderId);
    setStatusNote(
      result.changed > 0
        ? t("已将 {0} 封邮件标为已读", [result.changed])
        : t("没有未读邮件"),
    );
    await refreshAfterBulkRead();
  } catch (caught) {
    setError(t("批量标为已读失败：{0}", [describeError(caught)]));
  } finally {
    setBulkReadBusy(false);
  }
}, [bulkReadBusy, refreshAfterBulkRead]);
```

- [ ] **Step 4: 加入菜单项并调整菜单高度**

账号菜单在「编辑」前加入：

```tsx
<button
  type="button"
  role="menuitem"
  className="context-menu-item"
  disabled={bulkReadBusy}
  onClick={() => {
    const name = menuAccount.displayName || menuAccount.email;
    setAccountMenu(undefined);
    if (!window.confirm(t("确定把「{0}」所有文件夹里的未读邮件都标为已读吗？只改本机，不回写服务器。", [name]))) {
      return;
    }
    void markAllRead(menuAccount.accountId);
  }}
>
  {t("全部标为已读")}
</button>
```

文件夹菜单在新建入口前加入：

```tsx
<button
  type="button"
  role="menuitem"
  className="context-menu-item"
  disabled={bulkReadBusy}
  onClick={() => {
    const target = folderMenu.folder;
    setFolderMenu(undefined);
    void markAllRead(target.accountId, target.folderId);
  }}
>
  {t("全部标为已读")}
</button>
```

账号菜单可用高度从 140 调到 190，文件夹菜单从 170 调到 210。

- [ ] **Step 5: 补英文文案**

在 `src/i18n.en.ts` 加入：

```ts
  "全部标为已读": "Mark all as read",
  "已将 {0} 封邮件标为已读": "Marked {0} messages as read",
  "没有未读邮件": "No unread messages",
  "批量标为已读失败：{0}": "Failed to mark messages as read: {0}",
  "确定把「{0}」所有文件夹里的未读邮件都标为已读吗？只改本机，不回写服务器。": "Mark every unread message in all folders of “{0}” as read? This changes local data only and does not write back to the server.",
```

- [ ] **Step 6: 跑前端定向测试和四关**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
npx vitest run src/__tests__/folderManagement.test.tsx
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run build
```

Expected: 全部通过，所有命令退出码为 0。

- [ ] **Step 7: 提交本任务**

```powershell
git add -- src/InboxPanel.tsx src/i18n.en.ts src/__tests__/folderManagement.test.tsx
git commit -m "界面：增加账号和文件夹批量已读"
```

---

### Task 4: 文档、记忆与总验证

**Files:**
- Modify: `docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md`
- Modify: `docs/user-guide.md`
- Modify: `.ai-memory/handoff.md`
- Modify: `.ai-memory/project_memory.md`
- Modify: `.ai-memory/20261020/daily.md`

- [ ] **Step 1: 主规格补入口**

在邮箱栏或已读状态相关章节加入：

```markdown
- **批量清除未读（2026-10-20，用户确认）**：账号右键「全部标为已读」在确认后清除该账号所有文件夹的未读；文件夹右键同一入口只清除当前文件夹。两者只改本机数据库，不联网、不回写服务器。后端必须在单个事务内批量更新，并按邮件实际状态重算文件夹未读数。
```

- [ ] **Step 2: 用户手册补操作说明**

在收件箱或文件夹操作章节写清：

- 账号右键可清全部文件夹未读，会先确认。
- 文件夹右键只清当前文件夹。
- 只改本机，服务器仍可能显示未读。

- [ ] **Step 3: 更新记忆**

每个记忆文件追加：

- 做完的：账号级与文件夹级批量已读已完成。
- 证据：接口测试、前端测试、四关和真实界面验收结果。
- 相关文件：数据层、核心层、外壳、API、界面、测试和文档。
- 没做完：真实服务器 Seen 状态仍不回写。
- 卡在哪：如实填写；没有就写无。

- [ ] **Step 4: 跑四关**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run build
```

Expected: 四条命令退出码均为 0。

- [ ] **Step 5: 检查工作区与文件格式**

```powershell
git diff --check
git status --short
```

确认没有数据库、密钥、构建产物进入暂存区；本批文件为 UTF-8 无 BOM、LF。

- [ ] **Step 6: 提交文档与记忆说明**

`.ai-memory/` 当前被 `.gitignore` 忽略。若仍被忽略，只记录工作区文件，不强行 `git add -f`；其余文档正常提交：

```powershell
git add -- docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md docs/user-guide.md
git commit -m "文档：补充批量已读说明"
```

---

## 验收对照

| 规格要求 | 对应任务 |
|---|---|
| 账号右键先确认，覆盖全部文件夹 | Task 1、Task 3 |
| 文件夹右键只清当前文件夹 | Task 1、Task 3 |
| 只改本机，不联网 | Task 1、Task 2、Task 3 |
| 单事务、失败回滚、重算未读数 | Task 1 |
| 刷新侧栏、列表与搜索结果 | Task 3 |
| 提示实际数量或没有未读 | Task 3 |
| 四关与文件格式 | Task 4 |