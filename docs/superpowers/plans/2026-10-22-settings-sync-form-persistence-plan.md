# 设置同步表单默认保留 — 实施计划

> **给执行者：** 按任务顺序做；每步先写失败测试，再实现，再跑测试。不要一次性改完再补测试。

**目标：** 设置页「同步」面板的同步密码、设备名和两条须知，在应用重启后自动恢复；密码只以圆点显示。

**架构：** 密码继续只存系统保险箱；设备名和勾选存本机 `setting` 表。React 打开面板时通过只读命令取回密码，并读取公开状态恢复另外三项；用户修改设备名或勾选时调用本机保存命令。

**规格：** `docs/superpowers/specs/2026-10-22-settings-sync-form-persistence-design.md`

**分支：** `codex/fix-notification-open`

---

## 全局约束

- 同步密码不得写入数据库、日志、报错、浏览器缓存或前端持久存储。
- 密码框必须保持 `type="password"`，默认只显示圆点。
- 本功能不新增远程请求，不改 Gist 格式和加密协议。
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

- 核心逻辑：`crates/mail-core/src/settings_sync.rs`
- 核心导出：`crates/mail-core/src/lib.rs`
- Tauri 命令：`src-tauri/src/sync_commands.rs`
- 命令注册：`src-tauri/src/lib.rs`
- 前端接口：`src/api.ts`
- 前端面板：`src/SettingsSyncPanel.tsx`
- 前端测试：`src/__tests__/settingsSync.test.tsx`
- 记忆文件：`.ai-memory/handoff.md`、`.ai-memory/project_memory.md`、`.ai-memory/20261022/daily.md`

---

### Task 1：核心层增加表单偏好和密码读取

**文件：**

- 修改：`crates/mail-core/src/settings_sync.rs`
- 修改：`crates/mail-core/src/lib.rs`
- 测试：`crates/mail-core/src/settings_sync.rs`

**接口：**

- `SYNC_FORM_DEVICE_LABEL_KEY`
- `SYNC_FORM_AGREE_PRIVATE_KEY`
- `SYNC_FORM_AGREE_SECRETS_KEY`
- `MailEngine::remember_settings_sync_form(&self, device_label: &str, agree_private: bool, agree_secrets: bool) -> Result<(), EngineError>`
- `MailEngine::saved_settings_sync_password(&self) -> Result<Option<String>, EngineError>`
- `SettingsSyncStatus` 增加：
  - `form_device_label: Option<String>`
  - `agree_private: bool`
  - `agree_secrets: bool`

- [ ] **Step 1：写失败测试**

在 `settings_sync.rs` 测试模块加入：

1. `表单设备名和勾选能写回状态`
   - 调用 `remember_settings_sync_form("我的电脑", true, true)`。
   - 断言 `settings_sync_status()` 的三个新字段为保存值。
2. `空设备名不会覆盖已保存设备名`
   - 先保存“我的电脑”，再保存空串。
   - 断言设备名仍为“我的电脑”。
3. `已保存密码只从保险箱读回`
   - 用测试保险箱写入 `sync/password`。
   - 断言 `saved_settings_sync_password()` 返回原值。
   - 断言数据库文件里搜不到该密码明文；复用现有 `data_dir_contains` 测试辅助函数。
4. `没有保存密码时返回空`
   - 新建引擎直接调用，断言返回 `None`。

- [ ] **Step 2：运行测试确认失败**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test -p mail-core 表单设备名和勾选能写回状态 -- --nocapture
```

预期：编译失败，提示新接口不存在。

- [ ] **Step 3：实现核心层**

- 增加三个固定设置键常量和对应注释。
- 给 `SettingsSyncStatus` 增加三个公开字段，并保持 `camelCase` 序列化。
- `settings_sync_status()` 从 `setting` 表读取三个键；缺省时设备名为 `None`，两个勾选为 `false`。
- `remember_settings_sync_form()`：
  - 设备名去首尾空白。
  - 非空才写 `sync.form-device-label`。
  - 两个勾选分别写 `true` / `false`。
  - 不接收密码参数。
- `saved_settings_sync_password()`：
  - 从保险箱键 `sync/password` 读取。
  - 只返回到调用方，不写日志、不写数据库。
- 在 `crates/mail-core/src/lib.rs` 导出三个新设置键。

- [ ] **Step 4：运行核心测试**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test -p mail-core settings_sync -- --nocapture
```

预期：新增测试和原有设置同步测试全部通过。

---

### Task 2：增加 Tauri 命令和前端接口

**文件：**

- 修改：`src-tauri/src/sync_commands.rs`
- 修改：`src-tauri/src/lib.rs`
- 修改：`src/api.ts`
- 测试：`src-tauri/src/sync_commands.rs`（如现有模块适合单测）或依赖 Task 1 核心测试

**接口：**

- `settings_sync_saved_password() -> Option<String>`
- `settings_sync_remember_form(device_label, agree_private, agree_secrets) -> ()`

- [ ] **Step 1：增加命令**

在 `sync_commands.rs` 增加：

- `settings_sync_saved_password`：取引擎锁后调用核心方法，返回 `Option<String>`。
- `settings_sync_remember_form`：校验设备名长度不超过 64 个字符；去掉首尾空白后调用核心方法。
- 两个命令都不写密码日志。

在 `lib.rs` 的调用清单注册两个命令。

- [ ] **Step 2：补前端类型和调用**

在 `src/api.ts`：

- `SettingsSyncStatus` 增加：
  - `formDeviceLabel: string | null`
  - `agreePrivate: boolean`
  - `agreeSecrets: boolean`
- 增加：
  - `settingsSyncSavedPassword: () => call<string | null>("settings_sync_saved_password")`
  - `settingsSyncRememberForm: (deviceLabel, agreePrivate, agreeSecrets) => call<void>("settings_sync_remember_form", { deviceLabel, agreePrivate, agreeSecrets })`

- [ ] **Step 3：编译检查**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo check -p mail-core
cargo check -p ymail
npm run build
```

预期：命令注册、序列化字段和 TypeScript 类型都通过；前端测试可能因旧模拟数据缺新字段而失败，留到 Task 3 一次补齐。

---

### Task 3：面板恢复密码、设备名和勾选

**文件：**

- 修改：`src/SettingsSyncPanel.tsx`
- 修改：`src/__tests__/settingsSync.test.tsx`

**行为：**

- 打开面板时读取已保存密码；密码填入现有 `type="password"` 输入框。
- 设备名优先级：本机表单保存值 → 同步记录设备名 → 电脑名称 → “本机”。
- 两条勾选按状态恢复。
- 用户修改设备名后 300 毫秒合并保存一次；失焦时立即补存。
- 勾选变化立即保存。
- 异步回填不得覆盖用户刚输入的内容。
- 密码输入过程中不写保险箱；上传或下载成功后继续沿用现有后端持久化。

- [ ] **Step 1：更新测试模拟数据和失败用例**

在 `src/__tests__/settingsSync.test.tsx`：

- 模拟 API 增加 `settingsSyncSavedPassword`、`settingsSyncRememberForm`。
- `baseStatus` 增加 `formDeviceLabel: null`、`agreePrivate: false`、`agreeSecrets: false`。
- 新增测试：
  1. 保险箱返回 `password123` 时，密码框值为 `password123`，且 `type` 为 `password`。
  2. 状态里有设备名和两个勾选时，三项都恢复。
  3. 首次打开无记录时，密码框为空，两个勾选不勾。
  4. 修改设备名后调用保存命令，参数为新设备名和当前勾选状态。
  5. 勾选任一须知后立即调用保存命令。
- 更新现有设备名默认值用例，覆盖“表单保存值优先于同步记录与电脑名称”。

- [ ] **Step 2：运行前端测试确认失败**

```powershell
npm test -- --run src/__tests__/settingsSync.test.tsx
```

预期：新用例失败，旧模拟数据缺字段或新接口不存在。

- [ ] **Step 3：实现面板回填和保存**

在 `SettingsSyncPanel.tsx`：

- 增加密码读取副作用；使用编辑标记避免异步结果覆盖用户输入。
- 设备名和勾选读取合并到现有状态加载流程；使用编辑标记保护用户输入。
- 设备名输入变化时启动短定时器保存；失焦时清定时器并立即保存。
- 勾选变化时直接保存三项当前值。
- 保存失败显示现有错误区域，不阻塞上传下载。
- 组件卸载时清掉待执行定时器。

- [ ] **Step 4：运行前端回归**

```powershell
npm test -- --run src/__tests__/settingsSync.test.tsx
npm run build
```

预期：设置同步测试全部通过，构建成功。

---

### Task 4：完整四关、文档与记忆更新

- [ ] **Step 1：跑完整检查**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run build
```

任何一关失败都先修，不能跳过。

- [ ] **Step 2：核对安全**

```powershell
git diff --check
git status --short
```

人工确认：

- 没有 `*.db`、`.env`、密钥文件和构建产物进入暂存区。
- 新代码没有把密码写进设置表或日志。
- 前端没有使用 `localStorage` / `sessionStorage` 保存同步密码。

- [ ] **Step 3：更新用户说明**

在 `docs/user-guide.md` 的设置同步段落补一句：

- 同步密码、设备名和两条须知会默认保留；密码只存在系统保险箱，界面始终显示圆点。

- [ ] **Step 4：更新记忆**

更新：

- `.ai-memory/handoff.md`
- `.ai-memory/project_memory.md`
- `.ai-memory/20261022/daily.md`

记录四项：

- 做完的及测试证据。
- 相关文件。
- 没做完的。
- 卡点。

- [ ] **Step 5：提交**

只暂存本功能的源码、测试、用户说明和记忆文件，提交信息示例：

```text
设置同步表单默认保留
```