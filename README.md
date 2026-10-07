# Y-Mail

本地优先的**多邮箱统一收件箱客户端**（Windows 优先）。邮件和密码都留在你自己的电脑上。

- 技术路线：Tauri 2 + Rust 引擎（crate 分层）+ React/TS + SQLite(FTS5)
- 邮箱支持：国内邮箱（IMAP/SMTP + 授权码）、Gmail / Outlook（OAuth2，PKCE 本机回环回调）
- 安全底线：凭据只进 Windows 凭据管理器；AI / 翻译默认关闭，每次外发都要你点头

> 当前版本 `0.1.0`。产品名 `Y-Mail`，机器标识 `com.ymail.desktop`。
> 使用说明见 [用户手册](docs/user-guide.md)，安全结论见 [安全审计报告](docs/security-audit.md)。

## 能干什么

### 账号与同步

- 多账号并存：国内邮箱用「授权码」登录；Gmail / Outlook 走 OAuth2（PKCE + 本机回环回调，令牌只进系统保险箱，OAuth2 绝不退回明文认证）。
- 代理分层：账号级 > 全局自定义 > 跟随系统 > 直连，按账号单独配。
- 同步：文件夹映射、快照 + 后台补齐 + 按需深拉、增量拉取、IDLE 实时收信（不支持就轮询）、断点续传、UIDVALIDITY 变化自动重建、退避重试；一个账号连不上不影响其它账号。

### 收件箱与读信

- 统一收件箱：跨账号合并、账号色标、未读合计、账号 / 文件夹树、虚拟滚动分页；默认平铺，可切「按会话聚合」。
- 旗标：单封红旗 / 已标记，带「已标记」视图。
- 读信：正文懒加载并本地缓存；HTML 白名单清洗 + 沙箱渲染；远程图片默认拦截，单封放行才加载；内嵌图片（cid:）安全显示；正文链接点一下用系统默认浏览器打开；附件按需下载、文件名消毒；正文框与附件栏可拖动。
- 外观：浅色 / 深色 / 跟随系统，全站生效。

### 搜索与写信

- 全文检索：SQLite FTS5（中文按子串命中）；支持 `from:` / `has:attachment` / `is:unread` / `before:` 四种语法；本地搜不满一页时按需联网补一批历史。
- 写信 / 回复 / 转发：富文本编辑器（加粗、颜色、字号、对齐、缩进、行距、列表、表格、图片、链接，另有格式刷与清除格式）；草稿、附件、每账号签名。
- 发件队列：发送前原子认领防重复投递，失败自动重试，发成功后尽力追加到「已发送」文件夹。

### 通讯录

- 本地联系人页，联系人来自收发件人自动登记，不联网、不接 CardDAV / LDAP。
- 点某位联系人的「写邮件」，直接带着收件人打开写信窗格。

### AI 与翻译（默认关闭）

- 站点支持 OpenAI 兼容中转站 / 本机 Ollama / DeepL，密钥只进系统保险箱。
- 翻译支持**对照 / 行内 / 直接**三种模式，三种模式共用同一份段落对齐译文，切换不重复调用模型。
- 另支持摘要、润色与 AI 起草；思考程度四档，站点不支持时自动降级。
- 每次外发都会弹窗写清「目标域名 / 模型 / 是否本地」，确认后才发出去；设置页可「一键关闭 AI」并「清空缓存」。

### MCP 外部接入（默认关闭）

- 默认关闭、默认只读，只走本地标准输入输出，不监听任何端口。
- 打开总开关后，设置页会生成给 Codex / Claude Desktop / Cursor 用的配置示例（`command` 指向安装目录里的 `ymail-mcp.exe`，`env.YMAIL_DATA_DIR` 指向应用数据目录）。
- 只读工具：`list_accounts` / `list_folders` / `search_messages` / `get_message` / `get_thread`；写工具仅 `create_draft`（默认关闭，只能建草稿，不能发送）。
- 每次调用都写本地审计（工具名 / 账号范围 / 参数哈希 / 状态，不含正文）；「关闭 MCP」一键生效。

### 桌面集成

- 托盘常驻：关窗收进托盘，新邮件弹系统通知。
- 单实例：重复启动只唤起已有窗口，不会两个实例抢同一个数据库。
- 开机自动启动：勾上后开机静默进托盘，后台照常收信；开关以系统真实状态为准。
- 数据目录：首次启动向导让你选「用默认位置」还是「自己选文件夹」；之后也能在设置里改，改完自动重启，可选顺手清理旧目录里的数据（保留设置文件）。

## 第一次使用（简版）

1. 第一次打开会弹向导，选邮件数据放哪：直接用默认位置，或自己挑一个文件夹（挑完要重启生效）。
2. 进「设置 → 账号」添加账号：国内邮箱填邮箱与授权码（授权码要先去网页版邮箱生成）；Gmail / Outlook 走 OAuth2 一键授权。填完点「连接自检」，通过再保存。
3. 打开同步面板同步一次，等邮件进统一收件箱。
4. Gmail / Outlook 若要走 OAuth2，需要在服务商后台注册桌面应用、登记本机回环回调地址；内置客户端编号可直接用，也可以填自己的。详见 [用户手册第 4 节](docs/user-guide.md)。
5. 需要代理的账号，在「设置 → 代理」按账号单独配置。

## 隐私说明

- 密钥只进 Windows 凭据管理器，数据库 / 日志 / 报错里没有明文。
- 同步、搜索、读信、附件都在本机完成；应用无遥测、不上报。
- HTML 邮件里的远程图片默认拦截，单封放行才加载。
- AI / 翻译默认关闭，逐次授权；模型输出只作纯文本渲染。
- AI 与 MCP 调用都在本地留审计，不记正文原文。

## 文档

- [用户手册](docs/user-guide.md)：安装、首次配置、账号与代理、OAuth2、AI、MCP、故障排查
- [安全审计报告](docs/security-audit.md)：安全铁律逐条结论、证据、未验部分
- [人工验收清单](docs/manual-acceptance-checklist.md)：真邮箱 / 真 AI / 托盘 / 安装器等自动化测不了的场景
- [发布检查清单与回滚方案](docs/release-checklist.md)：版本号、产物清单、数据库备份与升级、卸载与回退
- 设计规格（`docs/superpowers/specs/`）：统一收件箱总规格（v1.2），以及界面优化、文件夹与旗标、通讯录、开机启动、改名与数据目录清理等子规格

## 开发环境

- Node.js 24+ 与 npm 11+
- Rust stable（`rust-toolchain.toml` 已固定 channel 与组件，`rustup` 会自动按需安装）
- Windows 需 WebView2 运行时
- MSVC 生成工具（`x86_64-pc-windows-msvc` 目标）

## 常用命令

```powershell
npm ci                      # 安装前端依赖
npm run build               # 前端类型检查 + 构建
npm run tauri dev           # 启动桌面应用（开发模式）
npm run tauri build         # 打包（含 MSI / NSIS；会自动生成 MCP sidecar）
npm run build:mcp-sidecar   # 单独生成 MCP sidecar
npm test                    # 前端测试

cargo fmt --all --check             # 格式检查
cargo clippy --workspace --all-targets -- -D warnings   # 静态检查（警告视为错误）
cargo test --workspace              # 全部 Rust 测试（含 Wave 9 端到端验收）
```

## 目录结构

```
crates/
  mail-domain/   纯类型与业务模型（最底层，不依赖任何其它 crate）
  mail-net/      IMAP / SMTP 共用的连接、代理选路与 TLS 包装
  mail-store/    SQLite 连接与迁移；唯一写库者
  mail-mime/     邮件解析与 HTML 白名单清洗
  mail-imap/     IMAP 客户端（含连接自检）
  mail-smtp/     SMTP 客户端
  mail-oauth/    OAuth2 / XOAUTH2
  mail-ai/       AI 与翻译旁路（默认关闭）
  mail-core/     引擎门面，唯一对外接口
  mail-mcp/      MCP stdio 服务端（可执行入口 ymail-mcp）
src-tauri/       桌面外壳：窗口、生命周期、命令转发
src/             React/TS 前端
scripts/         仓库内辅助脚本（图标生成、MCP sidecar 生成等）
docs/            用户手册、安全审计、人工验收、发布与回滚、设计规格
.ai-memory/      项目级记忆（交接与项目备忘；按日期的会话日志不入库）
```

依赖方向单向、无环：`mail-domain` ← 其余全部；`mail-store` ← `mail-core`；协议层 ← `mail-core`；`mail-core` ← `src-tauri` ← 前端。
