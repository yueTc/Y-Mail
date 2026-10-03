# em-master

本地优先的**多邮箱统一收件箱客户端**（Windows 优先）。

- 技术路线：Tauri 2 + Rust 引擎（crate 分层）+ React/TS + SQLite(FTS5)
- 支持：国内邮箱（IMAP/SMTP + 授权码）与 Gmail / Outlook（OAuth2）
- 原则：邮件与密钥不出本机；AI / 翻译外发须逐次授权

## 文档

- 设计规格（v1.1）：`docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md`
- Wave 0 任务拆解：`.ai-memory/20261003/wave0-plan.md`（按日期的记忆目录不入库）

## 状态

**Wave 0（工程骨架）已完成**：Tauri 2 空窗口可启动，初始化时建库并写入迁移登记记录；格式、静态检查、测试与前端构建四项本地验证全绿。

尚未实现：账号接入、邮件同步、界面功能（自 Wave 1 起）。

## 开发环境

- Node.js 24+ 与 npm 11+
- Rust stable（`rust-toolchain.toml` 已固定 channel 与组件，`rustup` 会自动按需安装）
- Windows 需 WebView2 运行时（Windows 10/11 通常已预装）
- MSVC 生成工具（`x86_64-pc-windows-msvc` 目标）

## 常用命令

```powershell
npm ci                      # 安装前端依赖
npm run build               # 前端类型检查 + 构建（tsc --noEmit && vite build）
npm run tauri dev           # 启动桌面应用（开发模式）
npm run tauri build         # 打包（Wave 6 起才启用 bundle）

cargo fmt --all --check             # 格式检查
cargo clippy --workspace --all-targets -- -D warnings   # 静态检查（警告视为错误）
cargo test --workspace              # 全部 Rust 测试
```

## 目录结构

```
crates/
  mail-domain/   纯类型与业务模型（最底层，不依赖任何其它 crate）
  mail-store/    SQLite 连接与迁移；唯一写库者
  mail-mime/     邮件解析（Wave 2 起）
  mail-imap/     IMAP 客户端（Wave 1 起）
  mail-smtp/     SMTP 客户端（Wave 1 起）
  mail-oauth/    OAuth2 / XOAUTH2（Wave 1 起）
  mail-ai/       AI 与翻译旁路（Wave 7 起，默认关闭）
  mail-core/     引擎门面，唯一对外接口
src-tauri/       桌面外壳：窗口、生命周期、命令转发
src/             React/TS 前端
scripts/         仓库内辅助脚本（图标生成等）
```

依赖方向单向、无环：`mail-domain` ← 其余全部；`mail-store` ← `mail-core`；协议层 ← `mail-core`；`mail-core` ← `src-tauri` ← 前端。

## 数据位置

Windows 下运行数据写在 `%APPDATA%\com.emmaster.desktop\`：

- `em-master.db`（SQLite，WAL 模式）
- `logs\em-master.log.<日期>`（本地日志，不外传）

## 应用图标

图标由 `scripts/gen-icon.mjs` 程序化生成占位源图（纯 Node，无第三方依赖）：

```powershell
node scripts/gen-icon.mjs src-tauri/icons/icon-source.png
npm run tauri -- icon src-tauri/icons/icon-source.png
```

换正式品牌图标时，准备好 1024×1024 源图后直接跑第二条命令即可。