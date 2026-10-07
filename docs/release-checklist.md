# Y-Mail 发布检查清单与回滚方案（0.1.0）

发布 = 版本号冻结 → 五关全绿 → 打安装包 → 核对产物 → 标记版本 → 交付。任何一步不过，就不发。

---

## 一、版本号

三个地方必须一致（本次都是 `0.1.0`）：

| 文件 | 位置 |
|---|---|
| `Cargo.toml` | `[workspace.package] version` |
| `package.json` | `"version"` |
| `src-tauri/tauri.conf.json` | `"version"` |

发新版本时改这三个，并在「四、打标签」记录提交号。

---

## 二、发布前必须过的五关

在仓库根目录（`D:\projects\em-master`）执行；终端找不到 cargo 时先跑：

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
```

| # | 命令 | 通过标准 |
|---|---|---|
| 1 | `cargo fmt --all --check` | 退出码 0 |
| 2 | `cargo clippy --workspace --all-targets -- -D warnings` | 退出码 0 |
| 3 | `cargo test --workspace` | 全绿；含 `crates/mail-core/tests/wave9_e2e.rs` 的 6 条验收用例 |
| 4 | `npm run build` | 退出码 0；产出 `dist/` |
| 5 | `npm test` | 全绿 |

日志存到 `.ai-memory/<日期>/wave9/`（按日期的记忆目录不入库）。

---

## 三、构建产物清单

执行 `npm run tauri build`（会自动先跑 `npm run build`，再跑 `npm run build:mcp-sidecar` 生成 sidecar）。

产物在 `src-tauri/target/release/bundle/`：

| 产物 | 路径（相对 bundle 目录） | 说明 |
|---|---|---|
| NSIS 安装包 | `nsis/ymail_0.1.0_x64-setup.exe` | 当前用户安装 |
| MSI 安装包 | `msi/ymail_0.1.0_x64_en-US.msi` | 标准 Windows 安装程序 |
| 主程序 | `../ymail.exe` | 由安装包装进安装目录 |
| **MCP sidecar** | `../ymail-mcp.exe` | 由 `src-tauri/binaries/ymail-mcp-x86_64-pc-windows-msvc.exe` 改名装入；两个安装包都必须包含它 |

核对方法（照做一遍，别只看日志）：

```powershell
# 1) sidecar 是否被生成
Test-Path src-tauri\binaries\ymail-mcp-x86_64-pc-windows-msvc.exe

# 2) 两个安装包是否都存在
Get-ChildItem src-tauri\target\release\bundle\nsis,src-tauri\target\release\bundle\msi

# 3) sidecar 能跑（未启用 MCP 时应打印「MCP 未启用」并退出）
& src-tauri\binaries\ymail-mcp-x86_64-pc-windows-msvc.exe
```

**不许入库的产物**：`target/`、`dist/`、`src-tauri/binaries/`、`src-tauri/target/`、`src-tauri/gen/`、`*.db`、`.env`、密钥文件。发布前跑一次：

```powershell
git status --short
git check-ignore -v src-tauri\binaries\ymail-mcp-x86_64-pc-windows-msvc.exe
```

第二条有输出 = 已被忽略，正确。

---

## 四、打标签

```powershell
git tag -a v0.1.0 -m "Y-Mail 0.1.0"
git show --stat v0.1.0
```

**不推送远端**（本项目当前约定：本地发布，不推远端）。

---

## 五、数据库备份与升级

### 5.1 升级前备份（必须做）

数据库在 `%APPDATA%\com.ymail.desktop\`。升级前先退出应用，再备份：

```powershell
$data = "$env:APPDATA\com.ymail.desktop"
$backup = "$env:USERPROFILE\Documents\ymail-backup-$(Get-Date -Format yyyyMMdd-HHmmss)"
New-Item -ItemType Directory -Force -Path $backup | Out-Null
Copy-Item "$data\ymail.db" $backup
Copy-Item "$data\logs" $backup -Recurse -ErrorAction SilentlyContinue
Write-Output "已备份到 $backup"
```

> 数据库开了 WAL 模式，建议先退出应用再拷；如果没退出，把 `ymail.db-wal` 和 `ymail.db-shm` 一起拷走。

### 5.2 升级过程

1. 退出应用（托盘菜单退出）。
2. 按 5.1 备份。
3. 装新版本（直接覆盖安装即可）。
4. 首次启动会自动跑数据库迁移；界面「数据库状态」里能看到「本次新应用」的迁移条数与结构版本。
5. 迁移后抽查：账号是否还在、最近邮件是否能读、搜索是否有结果。

**凭据不在数据库里**，所以在 Windows 凭据管理器里，不随数据库一起备份 / 迁移；换机器需要重新填授权码或重新授权。

### 5.3 降级（新版本出问题时）

迁移是向前的，**降级前必须用备份把数据库换回去**：

1. 卸载新版本。
2. 删掉 `%APPDATA%\com.ymail.desktop\ymail.db*`（先确认备份可用）。
3. 把 5.1 的备份拷回去。
4. 安装旧版本并启动，确认账号、邮件、搜索正常。

---

## 六、卸载与版本回退

| 操作 | 步骤 |
|---|---|
| 卸载（NSIS / MSI） | 「设置 → 应用 → 已安装的应用」里找到 Y-Mail，点卸载 |
| 卸载后会留下什么 | 程序目录清空；**数据目录默认保留**（便于回滚与重装保留数据） |
| 彻底清理 | 卸载后再手工删 `%APPDATA%\com.ymail.desktop`；凭据要到「Windows 凭据管理器 → Windows 凭据」里删掉 `com.ymail.desktop` 前缀的条目 |
| 版本回退 | 见 5.3：先恢复数据库备份，再装旧版本 |

---

## 七、交付前最后确认

- [ ] 三个版本号一致
- [ ] 五关全绿，日志已存档
- [ ] `npm run tauri build` 退出码 0
- [ ] NSIS 与 MSI 都存在，且安装目录里有 `ymail-mcp.exe`
- [ ] `git status --short` 没有数据库、密钥、构建产物
- [ ] 已打版本标签
- [ ] 已写升级前备份说明
- [ ] `docs/manual-acceptance-checklist.md` 的结果已如实填写（没跑的写「没做」）
- [ ] **未推送远端**（本约定）