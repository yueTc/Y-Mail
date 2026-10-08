# em-master 设置页「关于」与自动更新 — 设计规格（Spec）

| 项 | 值 |
|---|---|
| 文档版本 | v1.0 |
| 日期 | 2026-10-08 |
| 状态 | 已实现（2026-10-08）；密钥已生成，真实更新链路待发版实测 |
| 依据 | `docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md`；`docs/superpowers/specs/2026-10-08-settings-two-column-design.md` |
| 范围 | 设置页新增「关于」分组：看当前版本、跳转 GitHub、检查更新、下载安装并重启 |
| 变更记录 | v1.0（2026-10-08）：新增。用户 2026-10-08 拍板要做「看版本 / 跳 GitHub / 检查更新 / 热更新」四项。同日用户同意生成签名密钥；密钥已生成、公钥已填入配置，带签名打包已实测通过。 |

---

## 0. 需求溯源

1. **谁要的**：项目所有者本人。
2. **解决什么问题**：现在用户要升级只能自己去 GitHub 找安装包手动下载重装，不知道装的是哪个版本。
3. **不做什么会怎样**：每次升级都要人工找包、重装；版本号也无处可查。
4. **怎么算做成**：见第 5 节验收标准（R1–R7）。
5. **为什么不是只改样式**：要引入 Tauri 官方更新插件、签名校验与一条发版流水线，属于外壳与发布链路改动。

---

## 1. 目标与边界

### 1.1 本规格包含

- 设置页新增「关于」分组。
- 显示当前正在运行的版本号（取打包时写进程序里的版本，不写死常量）。
- 一键用系统浏览器打开项目主页与最新发布页；网址写死在后端与界面常量里。
- 一键检查更新：没新版提示已是最新，有新版显示版本号与发布说明。
- 一键下载并安装：下载完先验发布方签名再安装，装完重启进新版本；下载过程显示进度。
- 一条标签触发的发版流水线：构建 Windows 安装包、签名、生成更新清单 `latest.json` 并上传到 Release。

### 1.2 本规格不包含

- 不做静默后台自动更新（不弹提示就自动装）；更新一律由用户点按钮触发。
- 不做增量 / 差分包，只做整包替换。
- 不做回滚与多版本并存。
- 不做 macOS / Linux 的更新适配（本项目只发 Windows 包）。
- 不改现有通知、同步、托盘、单实例逻辑。
- 不在仓库里保存任何私钥或密钥文件。

---

## 2. 架构决策（含反选理由）

### Decision D1：用官方插件 `tauri-plugin-updater` + `tauri-plugin-process`

- **Reason**：官方维护、Tauri 2 适配；更新包用 minisign 签名，应用里只放公钥，装之前先验签，能挡住被篡改的更新包；重启用配套的 `tauri-plugin-process`。
- **Alternatives Considered**：自己写「下载 exe 再执行」。
- **Rejected Because**：自己写要处理验签、覆盖安装、更新失败回滚，任何一环漏了都是安全问题。
- **Trade-offs Accepted**：多两个依赖；发版必须带签名密钥。

### Decision D2：更新源用 GitHub Releases，走固定地址 `latest.json`

- **Reason**：项目本来就发在 GitHub；官方约定 `https://github.com/<owner>/<repo>/releases/latest/download/latest.json`，不用自建服务器。
- **Alternatives Considered**：自建更新服务器。
- **Rejected Because**：多一台要维护、要花钱的机器，与「不花钱」原则冲突。
- **Trade-offs Accepted**：依赖 GitHub 可达。

### Decision D3：界面只显示版本信息，更新句柄留在 `api.ts` 内部

- **Reason**：`check()` 拿到的更新句柄在 Rust 侧占资源，装完或重查前必须关掉；把它关在 `api.ts` 里，界面只拿到可显示的字段，不会误用。
- **Alternatives Considered**：把 `Update` 对象直接交给组件。
- **Rejected Because**：组件卸载后句柄可能泄着，也容易让界面接触到不该碰的能力。
- **Trade-offs Accepted**：多一层封装。

### Decision D4：发版落成草稿，人工检查后再发布

- **Reason**：更新包一旦正式发布，所有在用客户端都能查到。按 `AGENTS.md`「每次外发前必须用户点头」，先落草稿由人确认。
- **Alternatives Considered**：标签一推就直接发正式 Release。
- **Rejected Because**：构建出错或版本写错会直接推给用户。
- **Trade-offs Accepted**：发版多一步人工点「发布」。

---

## 3. 接口与数据

### 后端

- 注册 `tauri_plugin_updater`、`tauri_plugin_process` 插件；更新源与公钥写在 `src-tauri/tauri.conf.json` 的 `plugins.updater`。
- 权限只加 `updater:default`、`process:default`；不放开 shell / fs / http。
- 打开外链复用现有 `open_external_url` 命令：只认绝对 `http` / `https` / `mailto`。

### 前端封装（`src/api.ts`）

- `api.appVersion() -> Promise<string>`：读当前版本。
- `api.checkForUpdate() -> Promise<UpdateInfo | null>`：没新版返回 `null`。
- `api.installPendingUpdate(onProgress)`：下载并安装，回调带百分比（读不到总长时为 `null`）。
- `api.relaunchApp()`：装完重启。
- `UpdateInfo`：`version` / `currentVersion` / `notes` / `date`，只带界面要用的字段。

### 界面

- `src/AboutPanel.tsx`：版本、检查更新、安装进度、打开主页 / 发布页。
- `SettingsWorkspace` 分类数组新增 `{ id: "about", label: "关于" }`，排在「通讯录」之后。
- 中英文案双份（`src/i18n.ts` 中文原文为键，`src/i18n.en.ts` 英文表）。

### 发版

- `.github/workflows/release.yml`：推 `v*` 标签触发，用 `tauri-apps/tauri-action` 构建、签名、上传安装包与 `latest.json`。
- 仓库 Secrets：`TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。
- `bundle.createUpdaterArtifacts = true`：产出更新包与 `.sig` 签名文件。

---

## 4. 行为

- 进「关于」分组：读一次版本号并显示 `Y-Mail <版本>`；读失败显示错误。
- 点「检查更新」：查询中按钮转忙；没新版显示「已经是最新版本。」；有新版显示版本号、当前版本、发布说明与「下载并安装」按钮；失败显示可读中文原因。
- 点「下载并安装」：显示进度；装完调用重启。Windows 上安装程序起来后主程序可能已被结束，走不到重启那一步属正常。
- 安装失败：显示原因，不重启，按钮恢复可点。
- 点「打开 GitHub 页面」/「打开发布页」：调系统浏览器；失败只提示，不影响别处。
- 更新包验签不过：安装阶段直接失败，不把来路不明的包装上去。

---

## 5. 验收标准

- **R1**（MUST）设置页左栏出现「关于」，点开能看到当前版本号。
- **R2**（MUST）版本号来自后端读取，不写死常量。
- **R3**（MUST）「打开 GitHub 页面」用系统浏览器打开仓库主页，「打开发布页」打开最新发布页；地址写死，界面传不进任意链接。
- **R4**（MUST）「检查更新」按钮查完给出三态之一：有新版 / 已是最新 / 失败并给出原因。
- **R5**（MUST）有新版时可「下载并安装」，过程中显示进度，装完重启。
- **R6**（MUST）安装失败时显示原因且不重启。
- **R7**（MUST）推 `v*` 标签能产出带签名的更新包与 `latest.json`（需先配好签名密钥）。

---

## 6. 安全与合规

- 只放公钥进仓库；私钥只进本机与 GitHub Secrets，绝不入库（`AGENTS.md` 安全铁律）。
- 更新包必须验签；公钥对不上就拒绝安装。
- 外链地址写死在代码常量里，界面无法传任意地址；后端仍按 `http` / `https` / `mailto` 白名单再挡一道。
- 权限只加 `updater:default`、`process:default`，不放开 shell / fs / http。
- 发版先落草稿，人工确认后再发布（对外发布需用户点头）。
- 不做任何由邮件内容触发的行为。

---

## 7. Tasks

- **T1**（后端）：加 `tauri-plugin-updater` / `tauri-plugin-process` 依赖，注册插件，补权限。
- **T2**（后端）：`tauri.conf.json` 配更新源、`createUpdaterArtifacts` 与公钥。
- **T3**（前端）：`api.ts` 封装版本、查更新、安装、重启。
- **T4**（前端）：新增 `AboutPanel.tsx`，并接入设置页「关于」分组。
- **T5**（文案）：补齐中英文词条与样式。
- **T6**（测试）：前端回归（版本显示、三态、安装后重启、安装失败不重启、外链地址、分类可点开）。
- **T7**（发布）：新增 `.github/workflows/release.yml`。
- **T8**：四关 + `npm test` 全绿。
- **T9**：更新 `.ai-memory`（handoff / project_memory / daily）。

---

## 8. 未决与未验证

| 编号 | 类型 | 内容 | 处理 |
|---|---|---|---|
| N1 | 已完成 | 更新签名密钥 | 2026-10-08 用户点头后已生成：私钥在 `%USERPROFILE%\.tauri\ymail.key`，公钥已填入 `plugins.updater.pubkey`；私钥内容需由用户放进 GitHub Secrets |
| N2 | 未验证 | 真实的「查更新 → 下载 → 验签 → 安装 → 重启」全链路 | 本环境没有正式发布的新版本，必须发一版新标签实测 |
| N3 | 已验证 | 缺私钥时本地打包会失败 | 已实测：报 `A public key has been found, but no private key`；本机打包加 `--no-sign`，或先设 `TAURI_SIGNING_PRIVATE_KEY`。设好密钥后已实测出 `.exe` 与 `.exe.sig` |
| N4 | 未验证 | GitHub Actions 上的签名构建与 `latest.json` 生成 | 需要先配好 Secrets，推一个测试标签验证 |

---

## 9. 发布步骤（给维护者）

1. 先把 `src-tauri/tauri.conf.json` 与 `package.json` 的 `version` 一起改到新版本（例如 `0.1.3`），提交。
2. 在本机打包（可选，用来先自测）：

   ```powershell
   $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
   $env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.tauri\ymail.key"
   npm run tauri build
   ```

   只想要不带签名的安装包时，加 `--no-sign`。
3. 在 GitHub 仓库的 `Settings → Secrets and variables → Actions` 里加两个仓库密钥：
   - `TAURI_SIGNING_PRIVATE_KEY`：`ymail.key` 文件的内容（或它在构建机上的路径）；
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`：生成密钥时设的密码；本次生成的密钥没有密码，这一项留空字符串。
4. 打标签并推送：`git tag v0.1.3` 然后 `git push origin v0.1.3`。
5. 去 `Actions` 看发布任务跑完 → 到 `Releases` 里检查草稿 → 没问题再点发布。只有正式发布后，客户端才查得到这一版。

> 私钥一旦丢失就再也签不出能被老版本接受的更新包，请务必自行备份；私钥和密码都不要写进仓库、日志或聊天记录。