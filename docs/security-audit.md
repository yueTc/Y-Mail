# Y-Mail 安全审计报告（Wave 9）

| 项 | 值 |
|---|---|
| 日期 | 2026-10-05 |
| 范围 | Wave 0–Wave 8 已合入 main 的全部代码；重点核对规格 N1–N5 与 AGENTS.md 安全铁律 |
| 方法 | 读代码取证 + 跑自动化测试取证 + 端到端验收测试取证；不采信口头或注释承诺 |
| 结论 | 已核对的条目全部通过；**真实网络与服务（真邮箱 / 真 AI 站点 / 真 Ollama / 真 DeepL / 真 OAuth 服务商）与安装器界面未验**，见第四节 |

> 说明：本报告只写「查了什么、证据在哪、有没有验」。没验的条目一律标注为「未验」，不当作通过。

---

## 一、逐条核对

### 1. 凭据只进系统保险箱，数据库不存明文（N1、AGENTS.md）

**结论：通过。**

- 保险箱实现：`crates/mail-core/src/secrets.rs` 的 `KeyringSecretStore`，底层用 `keyring` 的 `windows-native`（即 Windows 凭据管理器）；引擎默认走它（`crates/mail-core/src/engine.rs`：`KEYRING_SERVICE = "com.ymail.desktop"`）。
- 数据库只存「引用键」：账号、代理、AI 站点、OAuth 应用都只落 `credential_key` / `api_key_ref` 这类键名，键名由邮箱或名称加哈希生成（`crates/mail-core/src/proxies.rs::new_credential_key`），不含密码原文。
- 数据库结构里没有密码 / 令牌列：`crates/mail-store/src/migrations/sql/*.sql` 里 `account`、`proxy`、`ai_provider`、`oauth_app` 均无明文凭据列。
- 自动化证据：
  - `crates/mail-core/src/accounts.rs`：`数据库文件里查不到明文授权码`（保存后扫描数据目录所有文件，查不到授权码字节）。
  - `crates/mail-core/src/ai.rs`：`密钥明文不会落到任何数据文件`、`站点密钥只进保险箱且出参不带明文`。
  - Wave 9 端到端：`crates/mail-core/tests/wave9_e2e.rs` 的 `验收_账号自检同步统一收件箱搜索与安全读信` 与 `验收_oauth2账号只走xoauth2不退回明文` 都断言数据目录里搜不到明文授权码 / 访问令牌；`验收_ai默认关闭逐次授权且译文只取一次` 断言搜不到 CDKey。

### 2. 日志与报错里不出现明文密钥（N1、AGENTS.md）

**结论：通过。**

- 敏感值类型 `Secret` 的 `Debug` 固定输出 `Secret(***)`（`crates/mail-domain/src/proxy.rs`），不会顺手被 `{:?}` 带进日志。
- 协议层错误先脱敏：`crates/mail-net/src/error.rs::redact` 把密钥原文替换成 `***`；IMAP / SMTP / AI 的错误文案都过这道脱敏（`crates/mail-imap/src/probe.rs`、`crates/mail-smtp/src/send.rs`、`crates/mail-ai/src/error.rs`）。
- 业务日志只记编号、阶段、错误大类；全量核对 `crates/mail-core` 与 `crates/mail-ai` 里的 `tracing::*` 调用，没有把正文原文或凭据原文写进日志。
- 自动化证据：
  - `crates/mail-ai/src/error.rs`：`错误正文里的正文片段不会被带出来`、`回显的密钥前缀形态会被洗掉`、`非json错误正文里的密钥也不会漏出来`。
  - `crates/mail-smtp/src/send.rs`：`认证被拒归类为认证失败且错误不含密码`、`令牌认证被拒时错误不泄露令牌与回显内容`。
  - `crates/mail-imap` 假服务器用例断言登录失败文案不回显授权码。

### 3. 邮件正文一律当不可信内容（AGENTS.md）

**结论：通过。**

- 解析层只解析、不清洗之外不做任何动作：`crates/mail-mime/src/parse.rs` + `sanitize.rs`；模块注释明确「不得据此触发任何动作」。
- AI 输入与指令隔离：`crates/mail-ai/src/prompt.rs` 的系统提示词固定带抗注入声明，正文包在显式标记里；自动化证据 `系统提示词都带抗注入声明`、`摘要提示词把正文包进标记里`。
- 模型输出只作纯文本渲染：`crates/mail-core/src/ai.rs` 只返回文本；前端 `src/AiPanel.tsx` 的注释与实现都明确不使用 `dangerouslySetInnerHTML`（全仓 `src/` 目录 grep 只在注释里出现该词，没有实际调用）。
- 外部 Agent 侧：`crates/mail-core/src/mcp.rs` 的工具返回值一律附 `MCP_UNTRUSTED_NOTICE`，正文按字符截断；`crates/mail-mcp/tests/stdio.rs` 有端到端断言。
- 读信路径不因正文触发动作：`crates/mail-core/src/reading.rs` 只做解析、清洗、附件元数据，不发送、不跳转、不执行命令。

### 4. 远程图片默认拦截 + XSS 对抗进 CI（N1、R6、AGENTS.md）

**结论：通过（自动化范围）；真实邮件观感未验。**

- 清洗器是白名单（ammonia）：`crates/mail-mime/src/sanitize.rs` 只留允许的标签/属性，`script/style/iframe/object/embed/form/noscript` 连内容一起丢掉，`on*` 事件属性被过滤。
- 远程图片默认改写：http/https/协议相对地址改写成 `data-em-original-src`，只有用户对单封放行时才还原 `src`；`crates/mail-core/src/reading.rs` 返回被拦数量。
- 前端沙箱：`src/MessageReader.tsx` 用 `sandbox=""`（不含 `allow-scripts`）的 iframe，文档内再上一条严格 CSP。
- 自动化证据：
  - `crates/mail-mime` 清洗用例（XSS 对抗集）。
  - 前端 `src/__tests__/reader.test.tsx`：`iframe 带 sandbox 且不包含 allow-scripts`、`默认拦截远程图片，放行后才把地址还给界面`、`文档 CSP 默认不放行远程图片，放行后才加 http/https`。
  - Wave 9 端到端 `验收_账号自检同步统一收件箱搜索与安全读信`：真实走「同步 → 读正文」链路，断言脚本标签与事件属性被清掉、`<img src=` 不出现真实地址、放行后才还原。
- CI：`.github/workflows/ci.yml` 在 Windows 上跑 `cargo test --workspace` 与 `npm test`，这些对抗用例失败即阻断合并。

### 5. AI 默认关闭且逐次授权（R10、AGENTS.md）

**结论：通过。**

- 默认态没有任何启用站点，翻译 / 摘要入口直接报「AI 功能还没开启」（`crates/mail-core/src/ai.rs::resolve_ai_target`）。
- 每次外发前先出「预览 + 一次性授权令牌」：`ai_authorization_preview` 返回外发域名、模型、是否本地；真正调用时必须带令牌（`consume_authorization`），令牌一次性消费、5 分钟过期、内容对不上即失效。
- CDKey 只进保险箱，非 localhost 站点必须是 HTTPS（`crates/mail-ai/src/provider.rs`）。
- 一键关闭：`disable_all_ai` 停用全部站点并作废已发出的授权（`clear_ai_authorizations`）。
- 自动化证据：
  - `crates/mail-core/src/ai.rs`：`默认关闭时拒绝预览且不给令牌`、`授权令牌只能消费一次`、`密钥明文不会落到任何数据文件`、`非本机明文地址保存时就拒绝`。
  - 前端 `src/__tests__/ai.test.tsx`：`没有启用站点时按钮显示需启用，并且不调用模型`、`弹窗写清域名、模型和是否本地，确认后才真正调用`、`缓存命中不弹授权框，也不消耗令牌`。
  - Wave 9 端到端 `验收_ai默认关闭逐次授权且译文只取一次`：默认关闭拒绝 → 预览拿令牌 → 不带令牌被拒 → 带令牌成功 → 再翻译命中缓存、模型只被调用一次。

### 6. MCP 默认关闭、stdio、只读、全量审计、一键关闭不可绕过（N5、R12）

**结论：通过。**

- 默认关闭：迁移 `0009_mcp.sql` 写入 `mcp.enabled = 0`、`mcp.write_tools_enabled = 0`；工具调用前查开关，关闭即拒绝并记审计。
- 只走 stdio，不监听端口：`crates/mail-mcp` 的入口只读写标准输入输出；该 crate 的 src 里 grep 不到 `TcpListener` / 端口监听（测试只做手写 stdio，不联网）。
- 默认只读：工具集 `MCP_READ_TOOLS`（账号 / 文件夹 / 搜索 / 读信 / 线程）与写工具 `MCP_WRITE_TOOLS`（仅 `create_draft`）分离；**没有发送类工具**，`send_email` / `export_all` 直接调用会被拒。
- 全量审计：`mcp_audit` 表只记工具名、账号范围、参数哈希、状态与时间，不含正文与凭据；成功 / 被拒 / 出错都记一条。
- 一键关闭：`mcp_set_enabled(false)` 后立即拒绝后续调用，已握手的会话也立刻被拒。
- 不暴露凭据 / 代理 / 原始 MIME：`McpAccountView` 只有编号、邮箱、显示名、启用状态、色标；读信只回纯文本并截断。
- 自动化证据：
  - `crates/mail-core/src/mcp.rs`：默认关闭拒绝并审计、只读搜索成功并审计、一键关闭立即生效、审计不含正文与凭据、写工具默认关闭。
  - `crates/mail-mcp/tests/stdio.rs`：真拉起可执行文件的端到端（默认关闭直接拒绝服务、协议版本不匹配拒不服务、工具清单无发送类、一键关闭后会话立刻被拒）。
  - Wave 9 端到端 `验收_mcp默认关闭只读审计一键关闭`。

### 7. N2 性能 / N3 可靠性 / N4 可观测性

- **N3 可靠性：通过（自动化）。** 单账号故障隔离、UID 去重幂等、崩溃重启恢复断点均有测试（`crates/mail-core/src/sync/tests.rs`、Wave 9 `验收_重启后断点续传不重复拉取已入库邮件`）。真实网络抖动下的长稳未验。
- **N4 可观测性：通过。** 结构化日志只写本地文件（`src-tauri/src/logging.rs`），同步进度落 `sync_job` 表并在界面展示；全仓无遥测 / 上报依赖，前端无对外 `fetch`。
- **N2 性能：部分通过。**
  - 搜索：`crates/mail-store/src/search.rs` 有「一万封搜索低于一秒」测试，通过；规格写的是「10 万封 P95 ≤1s」，**10 万封规模与 P95 未实测**。
  - 虚拟滚动：前端用 `@tanstack/react-virtual`；**10 万封流畅滚动未实测**。
  - **冷启动 ≤3s 未实测。**

---

## 二、发现问题与处理

本轮审计**没有发现需要修的安全缺陷**。以下为已知限制，已在文档中如实标注，不属于本轮修复范围：

1. 内嵌图片（`cid:`）在沙箱里不会换成字节，可能显示不出来（Wave 4 已知限制，规格未要求）。
2. MCP 的 `get_message` 在本地没有正文缓存时会复用 Wave 4 路径联网补一次（属「复用现有只读路径」的取舍，未额外隔离网络）。
3. 真实 AI 站点对「思考程度」参数的兼容性不同，自动降级逻辑有自动化覆盖，真实站点差异未验。

---

## 三、没验的部分（不计入通过）

- 真实邮箱连通：QQ / 163 / 企业邮箱 / Gmail / Outlook 的真实登录、同步、发信。
- 真实 OAuth2 授权页与令牌刷新（Gmail / Outlook 服务商后台）。
- 真实 AI 站点（OpenAI 兼容中转站）、真实 Ollama、真实 DeepL 的调用。
- 托盘通知弹出与点击直达。
- MSI / NSIS 安装器界面与干净 Windows 机器上的安装。
- 10 万封滚动性能、冷启动计时。
- 上述项的人工步骤见 `docs/manual-acceptance-checklist.md`。

---

## 四、原始证据位置

- 五关日志：`.ai-memory/20261005/wave9/`（按日期的记忆目录不入库）。
- Wave 9 端到端测试：`crates/mail-core/tests/wave9_e2e.rs`。
- 交付文档：`README.md`、`docs/user-guide.md`、`docs/manual-acceptance-checklist.md`、`docs/release-checklist.md`。