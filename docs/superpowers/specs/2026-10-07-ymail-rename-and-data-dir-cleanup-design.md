# Y-Mail 改名与迁移后重启清理 — 设计规格（Spec）

| 项 | 值 |
|---|---|
| 文档版本 | v1.0 |
| 日期 | 2026-10-07 |
| 状态 | 用户已确认方向，待实现 |
| 依据 | `docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md` v1.2；`docs/superpowers/specs/2026-10-06-mail-folders-flags-storage-reader-design.md` v1.0 |
| 范围 | 应用全面改名 Y-Mail；迁移完成后的自动重启与旧目录清理；迁移校验误判修复 |

---

## 0. 需求溯源

1. **谁要的**：项目所有者本人。
2. **解决什么问题**：
   - 应用已经对外叫 Y-Mail，但机器标识符、数据库、日志、MCP 程序里还叫 em-master，里外不一致。
   - 改完数据目录只提示「重启后生效」，要用户自己重启，流程走不完。
   - 旧数据目录留在原地没人管：用户想清又怕删错，尤其设置文件也在里面，删了就回向导。
   - 迁移时日志正在写，会被误判成「复制校验失败」，设置根本改不了。
3. **不做什么会怎样**：名字乱、用户困惑；用户忘了重启，以为换目录没生效；手动删旧目录可能连设置一起删掉，下次启动又回向导；迁移成功率被日志写入干扰。
4. **怎么算做成**：见第 5 节验收标准。

---

## 1. 目标与边界

### 1.1 本规格包含

- 把面向机器和面向用户的全部 em-master 名称改成 ymail / Y-Mail。
- 数据目录迁移成功后自动进入重启流程；重启前弹窗问要不要清理旧目录。
- 用户选「清理」时，下次启动清理旧目录里的应用数据，保留设置文件。
- 修复迁移时「日志变大被误判失败」的问题。

### 1.2 本规格不包含

- 不把 `%APPDATA%\com.emmaster.desktop` 的旧数据迁到新标识符目录（应用未发布、无真实用户，不写兼容层）。
- 不改 IMAP / SMTP / OAuth2 协议层，也不改系统保险箱里凭据的存储格式。
- 不改仓库文件夹名、Git 远端和历史提交。
- 不改旧的 superpowers 规格文档（历史记录，原文保留）。
- 不自动清理用户选了「直接重启（不清理旧文件）」的旧目录。

---

## 2. 应用改名：em-master → Y-Mail

### 2.1 改名对照

| 项目 | 现在 | 改成 | 所在位置 |
|---|---|---|---|
| 机器标识符 | `com.emmaster.desktop` | `com.ymail.desktop` | `src-tauri/tauri.conf.json` 的 `identifier` |
| 系统保险箱服务名 | `com.emmaster.desktop` | `com.ymail.desktop` | `crates/mail-core/src/engine.rs` 的 `KEYRING_SERVICE` |
| 数据库文件 | `em-master.db`（含 `-wal` / `-shm`） | `ymail.db` | `crates/mail-core/src/paths.rs`、`src-tauri/src/storage_dir.rs` 的 `DATABASE_NAME` |
| 日志文件 | `em-master.log.<日期>` | `ymail.log.<日期>` | `src-tauri/src/logging.rs` |
| MCP 可执行文件 | `em-master-mcp` | `ymail-mcp` | `src-tauri/src/mcp_commands.rs` 的 `MCP_BINARY_NAME`、`crates/mail-mcp/Cargo.toml` 的 `[[bin]]`、`src-tauri/tauri.conf.json` 的 `externalBin`、`scripts/build-mcp-sidecar.mjs` |
| MCP 二进制产物名 | `em-master-mcp-x86_64-pc-windows-msvc.exe` | `ymail-mcp-x86_64-pc-windows-msvc.exe` | `src-tauri/binaries/` |
| MCP 数据目录环境变量 | `EM_MASTER_DATA_DIR` | `YMAIL_DATA_DIR` | `src-tauri/src/mcp_commands.rs`、`crates/mail-mcp/src/*` |
| MCP 配置示例里的服务名 | `"em-master"` | `"ymail"` | `src-tauri/src/mcp_commands.rs` 的 `config_example` |
| Rust 主程序包名 | `em-master` | `ymail` | `src-tauri/Cargo.toml` 的 `[package] name` |
| Rust 内部库名 | `em_master_lib` | `ymail_lib` | `src-tauri/Cargo.toml` 的 `[lib] name`、`src-tauri/src/main.rs` |
| 临时探针文件名 | `.em-master-write-test-<进程号>` | `.ymail-write-test-<进程号>` | `src-tauri/src/storage_dir.rs` |
| 测试用临时目录名 | `em-master-settings-test-*` | `ymail-settings-test-*` | `src-tauri/src/settings.rs` |

### 2.2 连带变化

- 数据目录从 `%APPDATA%\com.emmaster.desktop` 变成 `%APPDATA%\com.ymail.desktop`。
- 系统通知显示的应用名、任务管理器里的进程名、安装包信息随标识符与包名一起变。
- 面向用户的文档（`docs/user-guide.md`、`docs/release-checklist.md`、`docs/manual-acceptance-checklist.md`）里出现的产品名与程序名一并改成 Y-Mail / ymail。
- 旧的 superpowers 规格文档、以及文档正文里出现的仓库本地路径 `D:\projects\em-master`，都保持原样。

### 2.3 兼容性说明（明确不做兼容）

改标识符后，程序不再读取 `%APPDATA%\com.emmaster.desktop`。该目录里的旧数据不会被自动搬运，也不会被自动删除，需要清理时由用户在「更改目录 / 清理旧目录」流程或手动处理。应用尚未发布、没有真实用户数据，因此本规格明确不写迁移兼容层。

---

## 3. 迁移完成后的重启与旧目录清理

### 3.1 完整流程

1. 用户在设置页选新目录，点「更改目录」。
2. 后端规划迁移：确认目标可写、空间够、无同名覆盖；目标已有内容时先让界面确认。
3. 后端复制文件、由 SQLite 自身做一致性快照写数据库、校验数据库完整性。
4. 校验通过后把新目录写进设置。此时新目录开始生效，但引擎仍在旧目录上运行。
5. 界面弹窗，只有两个按钮。
   - **清理并重启**：在设置里记一笔「下次启动清理旧目录」，然后重启。
   - **直接重启（不清理旧文件）**：不记录，直接重启。
6. 重启后：读设置 → 在新目录启动引擎、建库、校验通过之后，才执行待清理；清理结束抹掉记录。

### 3.2 弹窗交互约定

- 弹窗不可关闭：没有关闭按钮，按 Esc 和点遮罩都无效，只能在「清理并重启」和「直接重启（不清理旧文件）」之间二选一。
- 说明文字必须点明「会重启应用，当前所有窗口会关闭」。
- 不做「取消 / 稍后」按钮，避免停在「目录已改但没重启」的中间状态。
- 弹窗是前端界面元素，与现有设置页、首次向导的风格保持一致。

### 3.3 待清理记录

- 设置文件新增字段 `pendingCleanupDir`（可选字符串，写入时为 `camelCase`）。
- 值为空表示没有待清理目录；有值表示下次启动要清理这个目录。
- 旧目录路径由后端在迁移成功时暂存在应用内存里，重启命令直接使用；**不接受界面传入的待删路径**，避免界面传错路径导致误删。

### 3.4 清理范围

只处理旧目录下这些已知项：

| 目标 | 类型 |
|---|---|
| `ymail.db`、`ymail.db-wal`、`ymail.db-shm` | 文件 |
| `logs` | 目录 |
| `downloads` | 目录 |
| `compose-images` | 目录 |
| `backup-*`（按前缀匹配） | 目录 |

- **不删 `settings.json`**，也不删上面清单以外的任何文件或目录。
- 清单里不存在的项直接跳过，不算失败。

### 3.5 清理前的安全闸

清理前逐条检查，任一条不满足就不删、只写日志、并抹掉记录：

- 待清理目录必须存在；不存在就跳过。
- 待清理目录不能等于当前生效数据目录（比较规范化后的绝对路径）。
- 待清理目录不能是盘符根目录或文件系统根（例如 `E:\`）。
- 当前生效数据目录不能落在待清理目录内部（防止往上删到父目录）。

### 3.6 时机与顺序

启动顺序固定为：读设置 → 确定生效数据目录 → 起日志 → 开引擎、建库、跑迁移校验 → **以上全部成功之后**才执行待清理 → 抹掉记录。

- 引擎初始化失败时**不执行清理**，记录保留，留到下次启动再试。
- 清理结果写进日志（删了哪些、哪些没删掉、为什么跳过）。

### 3.7 清理失败处理

- 单个文件删不掉（占用、只读、权限不足）：写日志、跳过，继续处理其余项。
- 全部处理完后**无论如何都抹掉记录**，避免每次启动都重复失败、日志刷屏；没删掉的由用户手动处理，日志里写清是哪些。

---

## 4. 迁移校验误判修复（已在改，纳入本规格验收）

### 4.1 现状与根因

迁移规划时记录每个文件的字节数，真正复制后再比对。旧逻辑要求「实际字节数必须等于计划字节数」，于是正在追加写入的日志（规划时 0 字节、复制时已有 2054 字节）被判成失败，设置不改。

### 4.2 设计

- 校验条件从「实际不等于计划」改成「实际小于计划」：只有复制到的比规划时还少，才算没复制全。
- 失败文案改成「复制校验失败：<路径> 计划 <N> 字节，实际只有 <M> 字节；设置未改变」。
- 测试拆成两个用例：变大放行并断言内容完整、变小拦截且设置不变。

---

## 5. 验收标准

1. **改名**：启动应用后，任务管理器显示 `ymail.exe`；数据目录为 `%APPDATA%\com.ymail.desktop`；目录里是 `ymail.db` 和 `logs\ymail.log.<日期>`。
2. **改名**：设置页 MCP 配置示例里显示 `ymail-mcp` 与 `YMAIL_DATA_DIR`；安装目录里存在 `ymail-mcp.exe`。
3. **改名**：仓库、代码、面向用户文档里不再出现作为产品名的 em-master（旧规格文档与本地路径除外）。
4. **迁移**：更改目录成功后会弹窗，且弹窗只能选两个按钮，不能关闭。
5. **迁移**：选「清理并重启」后重启，旧目录里的数据库、日志、下载、图片、备份都被清掉、`settings.json` 还在，无关文件不受影响；设置里不再有待清理记录。
6. **迁移**：选「直接重启（不清理旧文件）」后重启，旧目录原样不动，设置里没有待清理记录。
7. **安全闸**：把待清理目录构造为当前数据目录、盘符根目录或当前目录的父目录时，清理一律不执行。
8. **校验修复**：日志正在写入的情况下迁移仍然成功，设置正常切换。
9. **质量关**：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`npm run build`、`npm test` 全部通过。

---

## 6. 影响文件（主要）

- `src-tauri/tauri.conf.json`：标识符、externalBin。
- `src-tauri/Cargo.toml`、`src-tauri/src/main.rs`：包名与内部库名。
- `src-tauri/src/lib.rs`、`src-tauri/src/state.rs`、`src-tauri/src/settings.rs`：设置字段、待清理记录、启动时序。
- `src-tauri/src/commands.rs`：迁移命令、重启命令、设置快照。
- `src-tauri/src/storage_dir.rs`：数据库名、校验条件、清理实现。
- `src-tauri/src/logging.rs`：日志文件名。
- `src-tauri/src/mcp_commands.rs`：MCP 程序名、环境变量、配置示例。
- `crates/mail-core/src/engine.rs`、`crates/mail-core/src/paths.rs`：保险箱服务名、数据库路径。
- `crates/mail-mcp/Cargo.toml`、`crates/mail-mcp/src/*`：可执行文件名与环境变量。
- `scripts/build-mcp-sidecar.mjs`、`package.json`、`src-tauri/binaries/`：MCP 产物名。
- `src/SettingsWorkspace.tsx`、`src/api.ts`、`src/FirstRunWizard.tsx`：迁移后的弹窗与重启调用。
- `src/__tests__/`：受改名影响的断言。
- `docs/user-guide.md`、`docs/release-checklist.md`、`docs/manual-acceptance-checklist.md`：产品名。