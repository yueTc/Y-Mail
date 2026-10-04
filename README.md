# em-master

本地优先的**多邮箱统一收件箱客户端**（Windows 优先）。

- 技术路线：Tauri 2 + Rust 引擎（crate 分层）+ React/TS + SQLite(FTS5)
- 支持：国内邮箱（IMAP/SMTP + 授权码）与 Gmail / Outlook（OAuth2，PKCE 回环回调）
- 原则：邮件与密钥不出本机；AI / 翻译默认关闭，每次外发逐次授权

> 当前版本 `0.1.0`。使用说明见 [用户手册](docs/user-guide.md)，安全结论见 [安全审计报告](docs/security-audit.md)。

## 安装（msi / nsis）

从发布产物里任选一个：

| 安装包 | 说明 |
|---|---|
| `em-master_0.1.0_x64-setup.exe`（NSIS） | 默认装到当前用户目录，不需要管理员权限 |
| `em-master_0.1.0_x64_en-US.msi`（MSI） | 标准 Windows 安装程序，适合统一部署 |

安装前确认系统有 WebView2 运行时（Windows 10/11 一般自带）。安装目录里会自带 `em-master-mcp.exe`（MCP 外部 Agent 接入用，默认关闭）。

数据与日志位置：

- 数据库：`%APPDATA%\com.emmaster.desktop\em-master.db`
- 日志：`%APPDATA%\com.emmaster.desktop\logs\`
- 凭据：Windows 凭据管理器（不是文件）

## 首次使用（简版）

1. 「账号与代理」页签 →「添加账号」→ 填邮箱与授权码（国内邮箱要先用网页版生成授权码）。
2. 点「连接自检」，通过后保存。
3. 「同步」面板点同步，等邮件进统一收件箱。
4. Gmail / Outlook 要走 OAuth2：在服务商后台注册桌面应用、登记本机回环回调地址、把客户端编号填进应用。详见 [用户手册第 4 节](docs/user-guide.md)。
5. 需要代理的账号在「代理」面板单独配置（账号级 > 全局自定义 > 跟随系统 > 直连）。

## AI 与翻译

- **默认关闭**：不配置站点就不会有任何外发。
- 在「AI 与翻译」里添加站点（OpenAI 兼容中转站 / 本机 Ollama / DeepL），填 `base_url` 与 CDKey；CDKey 只进 Windows 凭据管理器。
- 每次翻译 / 摘要 / 润色都会弹窗写清「目标域名 / 模型 / 是否本地」，确认后才外发。
- 翻译支持**对照 / 行内 / 直接**三种模式，三种模式共用同一份段落对齐译文，切换不重复调用模型。
- 设置页可「一键关闭 AI」并「清空缓存」。

## MCP 外部接入

- 默认关闭、默认只读，只走本地标准输入输出，不监听任何端口。
- 打开总开关后，设置页会生成给 Codex / Claude Desktop / Cursor 用的配置示例（`command` 指向安装目录里的 `em-master-mcp.exe`，`env.EM_MASTER_DATA_DIR` 指向应用数据目录）。
- 只读工具：`list_accounts` / `list_folders` / `search_messages` / `get_message` / `get_thread`；写工具仅 `create_draft`（默认关闭，只能建草稿，不能发送）。
- 每次调用都写本地审计（工具名 / 账号范围 / 参数哈希 / 状态，不含正文）；「关闭 MCP」一键生效。

## 隐私说明

- 密钥只进 Windows 凭据管理器，数据库 / 日志 / 报错里没有明文。
- 同步、搜索、读信、附件都在本机完成；应用无遥测、不上报。
- HTML 邮件里的远程图片默认拦截，单封放行才加载。
- AI / 翻译默认关闭，逐次授权；模型输出只作纯文本渲染。
- AI 与 MCP 调用都在本地留审计，不记正文原文。

## 文档

- [用户手册](docs/user-guide.md)：安装、首次配置、账号与代理、OAuth2、AI、MCP、故障排查
- [安全审计报告](docs/security-audit.md)：N1–N5 与安全铁律逐条结论、证据、未验部分
- [人工验收清单](docs/manual-acceptance-checklist.md)：真邮箱 / 真 AI / 托盘 / 安装器等自动化测不了的场景
- [发布检查清单与回滚方案](docs/release-checklist.md)：版本号、产物清单、数据库备份与升级、卸载与回退
- [设计规格](docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md)（v1.2）

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
  mail-mcp/      MCP stdio 服务端（可执行入口 em-master-mcp）
src-tauri/       桌面外壳：窗口、生命周期、命令转发
src/             React/TS 前端
scripts/         仓库内辅助脚本（图标生成、MCP sidecar 生成等）
docs/            用户手册、安全审计、人工验收、发布与回滚
```

依赖方向单向、无环：`mail-domain` ← 其余全部；`mail-store` ← `mail-core`；协议层 ← `mail-core`；`mail-core` ← `src-tauri` ← 前端。

## 应用图标

图标由 `scripts/gen-icon.mjs` 程序化生成占位源图（纯 Node，无第三方依赖）：

```powershell
node scripts/gen-icon.mjs src-tauri/icons/icon-source.png
npm run tauri -- icon src-tauri/icons/icon-source.png
```

换正式品牌图标时，准备好 1024×1024 源图后直接跑第二条命令即可。