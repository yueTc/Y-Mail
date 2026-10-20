# 通用决策模型站点接入设计

| 项 | 值 |
|---|---|
| 版本 | v1.0 |
| 日期 | 2026-10-20 |
| 状态 | 待用户审核 |
| 依据 | TypeSafe 官方接口文档；Bamboo 公开模型与价格表；现有邮件分类规格 |
| 取代范围 | `2026-10-11-mail-label-groups-design.md` 中“分类只认 TypeSafe jev”的站点范围约定 |

---

## 0. 背景

“决策模型”是指输入一段内容和若干问题，只返回选项、分数或真假概率，不生成聊天文字的一类模型。JEV 是其中一种，但不是唯一一种。

Bamboo 公开模型表里至少有两种决策模型：

- StartLux：`startlux-decision-27b-autothink`，使用 `POST /v1/decisions` 或 `POST /v1/systemone`。
- TypeSafe JEV：`typesafe/jev-1.14`，使用 `POST /jev/v1/decisions`，兼容 `/jev/v1/systemone`。

现有代码把站点类型 `typesafe` 和 JEV 绑定，且只会拼接 `/systemone`。这会使后续每接一个决策模型都要新增一套厂商专用代码。

本设计新增通用“决策模型站点”，同时保留旧 TypeSafe 站点向后兼容。邮件分类业务仍只允许决策模型站点，不再限定厂商。

## 1. 目标

1. 用户可用一个通用站点类型接入任意兼容 `state + questions + answers` 形状的决策模型。
2. 首版明确支持 JEV 和 StartLux 的实际调用路径。
3. 新站点配置使用完整请求地址，代码不猜接口路径。
4. 保留已有 TypeSafe 站点和分类结果，升级后无需用户重建配置。
5. 继续遵守密钥入系统保险箱、默认关闭、逐次确认、正文上限和远程图片拦截。
6. 用统一测试覆盖请求构造、三类答案解析、地址处理、旧站点兼容和界面筛选。

## 2. 非目标

- 首版不接图片输入，即使上游支持。
- 首版不支持流式输出。
- 首版不允许决策站点承担翻译、摘要、润色、起草或通知识别。
- 首版不自动从模型列表接口发现决策模型。
- 首版不实现厂商专用 SDK。
- 首版不改变标签组、门槛、颜色、审计和分类业务规则。

## 3. 方案比较

### 方案 A：新增通用决策站点，旧 TypeSafe 保留兼容（选定）

- 新增 `decision_api` 类型。
- 新类型把 `base_url` 字段当作完整请求地址。
- 旧 `typesafe` 类型继续按原方式工作。
- 优点：改动集中，旧数据零迁移风险，后续厂商通过模型编号和完整地址接入。
- 代价：短期内同时存在新类型和旧类型。

### 方案 B：把 TypeSafe 类型直接改名为通用类型

- 优点：数据结构最干净。
- 缺点：需要事务迁移全部旧配置和地址，用户升级风险高；当前工作区已有大量未提交改动，不适合扩大迁移面。
- 结论：不选。

### 方案 C：每接一家厂商新增一个站点类型

- 优点：每个厂商可写最专用的格式转换。
- 缺点：界面和代码持续膨胀，不符合本次目的。
- 结论：不选。

## 4. 数据模型

### 4.1 站点类型

新增存储值：

```text
decision_api
```

规则：

- 新建站点时只提供“决策模型站点”入口。
- 旧 `typesafe` 站点仍可读取、编辑、测试和参与分类。
- 旧 `typesafe` 站点不作为新建选项。
- 两种类型都只支持邮件分类。

### 4.2 请求地址

`ai_provider.base_url` 字段继续复用，但含义按类型区分：

- `decision_api`：完整请求地址，例如 `https://api.bamboonode.dev/v1/decisions`。
- `typesafe`：旧根地址，例如 `https://api.typesafe.ai/v1`。

对 `decision_api`：

- 不在客户端追加任何路径。
- 允许地址带查询参数。
- 只接受 `http` 或 `https`；非本机地址仍必须使用 `https`。
- 地址无法解析时在发请求前返回可读中文错误。

界面对新类型显示“完整接口地址”，并给出两个示例：

```text
https://api.bamboonode.dev/v1/decisions
https://api.typesafe.ai/v1/systemone
```

### 4.3 模型编号

`decision_api` 必须填写模型编号。

示例：

```text
startlux-decision-27b-autothink
typesafe/jev-1.14
jev-latest
```

决策站点不调用模型列表接口。模型列表允许手工填写，用于功能级模型下拉提示；测试连接只使用当前选择的测试模型。

## 5. 请求协议

统一发送：

```json
{
  "model": "<模型编号>",
  "state": "<待判断内容>",
  "questions": {
    "g1": {
      "type": "choice",
      "instructions": "<问题说明>",
      "criteria": {
        "<选项名>": null
      }
    },
    "g2": {
      "type": "noul",
      "instructions": "<问题说明>"
    },
    "g3": {
      "type": "score",
      "instructions": "<问题说明>",
      "criteria": ["<档位 1>", "<档位 2>"]
    }
  }
}
```

规则：

- 所有已开启标签组必须放在一次请求里。
- 选择题的 `criteria` 是“选项名到说明”的对象。
- 是否题使用 `noul`。
- 打分题的 `criteria` 是有序数组。
- `state` 仍只放纯文本正文，最多 2000 个字符。
- 请求正文和响应正文都不得写入日志。

## 6. 响应解析

上游响应必须包含 `answers` 对象，键与问题键一致。

### 6.1 选择题

优先读取：

- `choice`
- `confidence`
- `probabilities`

规则：

- `choice` 必须是非空字符串且属于请求候选。
- `confidence` 必须在 0 到 1 之间。
- 没有 `confidence` 但有 `probabilities` 时，取最大值作为把握程度。
- 字段缺失或类型不符只跳过该组，不影响其他组。

### 6.2 是否题

接受以下数值字段之一：

- `noul`
- `probability`

规则：

- 数值必须在 0 到 1 之间。
- 若返回布尔值 `true` 或 `false`，分别换算成 1.0 或 0.0。
- 同时出现多个字段时按上述顺序取第一个合法值。

### 6.3 打分题

优先使用 `probabilities`：

- 键必须是档位下标字符串。
- 值必须在 0 到 1 之间。
- 取概率最高的下标，换算成 `下限 + 下标`。
- 把握程度取最高概率。

没有 `probabilities` 时允许读取 `score`：

- 必须是整数。
- 必须落在组的上下限内。
- 必须同时提供 0 到 1 的 `confidence`；缺失或越界时跳过该组。

小数 `score` 不自动猜测或四舍五入，按协议不完整跳过该组。

### 6.4 类型字段

- 响应带 `type` 时必须与请求题型一致。
- 响应不带 `type` 时允许按字段形状识别，兼容不完整实现。
- `noul` 与 `bool` 视为同一种是否题类型别名。

## 7. 测试连接

决策站点测试连接使用固定探针：

- 一条固定的短文本。
- 一个 `noul` 问题。
- 当前选择的模型编号。
- 当前地址、代理和密钥。

成功标准：

- 返回 HTTP 200。
- 返回 `answers` 对象。
- 至少一个答案能被统一解析器识别为合法是否题答案。

测试不读取真实邮件，不写分类结果，不保存测试响应正文，不覆盖站点模型列表。

## 8. 界面

### 8.1 站点弹窗

站点类型新增：

```text
决策模型站点
```

选择后：

- 地址标签改为“完整接口地址”。
- 预填 `https://api.bamboonode.dev/v1/decisions`。
- 默认模型预填 `startlux-decision-27b-autothink`。
- 模型仍可手工修改。
- 按钮显示“测试连接”，不显示“测试连接并拉取模型”。
- 测试模型选择器继续显示。

旧 `typesafe` 站点编辑时：

- 类型显示“旧版 TypeSafe jev（兼容）”。
- 保留原地址提示和 `/systemone` 兼容逻辑。
- 可继续测试和保存。

### 8.2 分类设置

- 站点下拉列出 `decision_api` 和旧 `typesafe`。
- 无候选时提示“还没有可用的决策模型站点”。
- 分类开关的确认弹窗仍显示站点名、完整域名、模型和是否本机。
- 功能级模型选择保持不变。

## 9. 安全与失败处理

- 密钥继续只存系统保险箱，数据库只存引用键。
- 新建决策站点默认不启用；用户显式启用后才可被选中。
- 外发确认机制不削弱。
- 响应内容视为不可信数据，只解析固定字段，不执行任何命令或跳转。
- 401、403、404、422 返回可读中文说明，不自动重试。
- 429 和 529 使用指数退避，最多额外重试两次。
- 超时或网络断开沿用现有错误分类，并允许用户重新发起。
- 失败不写半套分类结果。

## 10. 兼容与迁移

必须新增数据库迁移，使 `ai_provider.kind` 的约束允许 `decision_api`。

迁移要求：

- 保留旧 `typesafe` 数据和地址，不改写。
- 迁移全程单事务，失败整体回滚。
- 旧库升级后可新增、读取、更新和删除 `decision_api` 站点。
- 非法类型值仍被数据库拒绝。
- 设置导入导出支持 `decision_api`。
- 旧备份没有新类型时仍可导入。

不迁移旧 TypeSafe 根地址到新类型；用户需要时自行新建通用决策站点。

## 11. 业务编排

`mail-core` 仍是唯一编排层：

- 功能映射不变，`AiFunction::Classify` 对应决策站点。
- 站点能力判断允许 `decision_api` 和旧 `typesafe`。
- 分类预览、一次性授权、审计、门槛、兜底和手动锁定逻辑不变。
- `mail-ai` 只负责请求构造、发送和响应归一化。

## 12. 需求清单

- **R1**：系统 MUST 支持 `decision_api` 站点。
- **R2**：旧 `typesafe` 站点 MUST 继续可用。
- **R3**：`decision_api` MUST 使用完整请求地址，代码 MUST NOT 自动拼接接口路径。
- **R4**：`decision_api` 模型编号为空时 MUST 在发请求前拒绝。
- **R5**：分类请求 MUST 一次携带所有已开启标签组的问题。
- **R6**：请求 MUST 支持 `choice`、`noul`、`score` 三种题型。
- **R7**：响应解析 MUST 校验选项、分数、概率和把握程度范围。
- **R8**：单个答案不合法 MUST 只跳过该组，不中断整批请求。
- **R9**：测试连接 MUST 使用固定探针和当前选择的模型，不读取真实邮件。
- **R10**：界面 MUST 将新类型显示为“决策模型站点”，并接受完整接口地址。
- **R11**：分类站点筛选 MUST 同时支持新类型和旧类型。
- **R12**：密钥、确认、正文上限、图片拦截和审计规则 MUST 保持不变。
- **R13**：旧种类站点在升级后 MUST 可读、可测、可分类。
- **R14**：数据库和设置导入导出 MUST 支持新类型。

## 13. 验证策略

### 13.1 后端

- 新增类型序列化、反序列化和旧别名测试。
- 完整地址不追加 `/decisions` 或 `/systemone`。
- 旧 TypeSafe 地址仍追加 `/systemone`。
- 请求体包含 `model`、`state`、`questions`。
- 三种题型请求形状正确。
- 选择题、是否题、打分题合法与非法响应。
- `noul`、`probability`、布尔值兼容。
- 缺 `type` 时按字段识别。
- 429 和 529 退避重试，其他错误不重试。
- 测试探针不读取邮件。

### 13.2 数据层

- 新库允许 `decision_api`。
- 旧库升级后旧 `typesafe` 站点完整保留。
- 新类型的新增、读取、更新、删除。
- 设置导入导出往返。
- 非法类型仍被拒绝。

### 13.3 前端

- 新类型显示“决策模型站点”。
- 选择新类型预填完整地址和模型。
- 新类型按钮为“测试连接”。
- 空模型拦截且不发请求。
- 分类站点下拉包含新类型和旧类型。
- 无候选提示使用“决策模型站点”措辞。
- 旧 TypeSafe 站点编辑提示保持兼容。

### 13.4 四关

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `npm run build`

## 14. 真实接口验证边界

已确认：

- 无密钥调用 `https://api.bamboonode.dev/v1/decisions` 返回 401。
- Bamboo 公开表列出 StartLux 和 JEV 的路径及模型编号。
- TypeSafe 官方文档确认 `POST /systemone`、`model + state + questions` 和三题型结构。

尚未确认：

- StartLux 成功响应的完整字段。
- StartLux 是否强制要求 `model` 字段。
- 两家站点对多问题请求的真实延迟和上限。

因此实现先用假服务端覆盖协议；真实成功调用需要用户提供可用密钥后单独联调，不在本规格内假装已验证。

## 15. 受影响文件

预计涉及：

- `crates/mail-ai/src/provider.rs`
- `crates/mail-ai/src/client.rs`
- `crates/mail-ai/src/lib.rs`
- `crates/mail-store/src/ai.rs`
- `crates/mail-store/src/migrations/`
- `crates/mail-store/src/settings_import.rs`
- `crates/mail-core/src/ai.rs`
- `src-tauri/src/commands.rs`
- `src/api.ts`
- `src/AiProviderDialog.tsx`
- `src/AiPanel.tsx`
- `src/i18n.en.ts`
- 对应 Rust 与前端测试
- `docs/superpowers/specs/2026-10-11-mail-label-groups-design.md`

## 16. 规格取代关系

本规格生效后：

- 邮件分类不再限定 TypeSafe jev。
- 分类范围改为“通用决策模型站点或旧 TypeSafe jev 兼容站点”。
- 新站点地址按完整请求地址保存，不再沿用“填到 `/v1`”的统一提示。
- 旧 TypeSafe 站点的路径兼容保持不变。