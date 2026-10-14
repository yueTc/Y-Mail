# 导航、账户入口与设备名调整实施计划

| 项 | 值 |
|---|---|
| 日期 | 2026-10-15 |
| 分支 | codex/ui-polish-inbox |
| 规格 | docs/superpowers/specs/2026-10-15-navigation-account-device-name-design.md |
| 目标 | 调整主导航和设置分类，并让设置同步默认使用电脑名称 |

## 实施约束

- 保留主导航三个工作区模式，不新增路由。
- 文字只从视觉上隐藏，保留 `aria-label`、`title` 和键盘操作。
- “账户”挂载原“账号与同步”里的 `SyncPollSettingsPanel`、`SyncPanel`、`AccountPanel`；“设置同步”只挂载 `SettingsSyncPanel`。
- 设备名由后端只读命令读取；前端加载失败时用“本机”兜底。
- 不触碰密码、令牌、邮件正文和远端同步协议。
- 每批改动结束运行四项检查：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`npm run build`。

## 步骤 1：主导航改为图标栏

**文件：**

- 修改 `src/WorkspaceShell.tsx`
- 修改 `src/index.css`（按现有样式结构调整）
- 测试 `src/__tests__/inbox.test.tsx` 或新增 `src/__tests__/workspaceShell.test.tsx`

**做法：**

- 将入口渲染分为上方入口和底部设置入口，使用外层布局把设置推到底部。
- 删除视觉文字节点或改成仅供辅助工具使用的隐藏文本，保留按钮的 `aria-label` 和 `title`。
- 保持现有激活态、图标和模式切换回调。
- 测试三个按钮仍可找到，设置按钮仍能切换到设置工作区，视觉文字不再占用导航宽度。

## 步骤 2：拆分“账户”和“设置同步”分类

**文件：**

- 修改 `src/SettingsWorkspace.tsx`
- 修改相关设置页测试

**做法：**

- 把分类编号从 `accounts` 改成 `account` 和 `settingsSync`。
- 分类文字改为“账户”和“设置同步”。
- “账户”渲染 `SyncPollSettingsPanel`、`SyncPanel`、`AccountPanel`；“设置同步”只渲染 `SettingsSyncPanel`。
- 把原来“账号与同步”分类拆成两个区块，保持每个面板在切换时继续挂载。
- 更新默认分类与所有面板显示判断，避免旧编号残留。

## 步骤 3：增加读取电脑名称的后端命令

**文件：**

- 修改 `src-tauri/src/commands.rs`
- 修改 `src-tauri/src/lib.rs`
- 修改 `src/api.ts`
- 测试 `src-tauri/src/commands.rs` 或命令所在模块的单元测试

**做法：**

- 添加只读命令，Windows 优先读 `COMPUTERNAME`，再读 `HOSTNAME`，都为空时返回“本机”。
- 对返回值做首尾空白清理；空值不得返回空字符串。
- 在 `api.ts` 添加类型安全的命令封装。
- 在 `lib.rs` 注册命令。

## 步骤 4：设备名默认值接入设置同步面板

**文件：**

- 修改 `src/SettingsSyncPanel.tsx`
- 修改设置同步测试

**做法：**

- 面板首次加载时读取电脑名称。
- 同时填充开启同步和加入同步两个输入框；加载失败时填“本机”。
- 只在输入框尚未被用户编辑时写入默认值，避免异步返回覆盖用户输入。
- 保持已有空值校验、提交接口、同步状态显示和密码清空行为。

## 步骤 5：验证与记忆更新

- 先运行受影响的前端和 Rust 测试，修复类型、布局或命令注册问题。
- 再运行项目要求的四项完整检查。
- 更新 `.ai-memory/handoff.md`、`.ai-memory/project_memory.md`、`.ai-memory/2026-10-15.md`，记录完成证据、相关文件、未完成项和阻塞点。
- 提交实现，提交信息使用中文；确认未加入 `.db`、`.env`、密钥和构建产物。