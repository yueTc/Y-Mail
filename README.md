# em-master

本地优先的**多邮箱统一收件箱客户端**（Windows 优先）。

- 技术路线：Tauri 2 + Rust 引擎（crate 分层）+ React/TS + SQLite(FTS5)
- 支持：国内邮箱（IMAP/SMTP + 授权码）与 Gmail / Outlook（OAuth2）
- 原则：邮件与密钥不出本机；AI / 翻译外发须逐次授权

## 文档

- 设计规格（v1.0）：`docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md`

## 状态

需求与设计已定稿，待用户审核 spec；尚未开始编码。