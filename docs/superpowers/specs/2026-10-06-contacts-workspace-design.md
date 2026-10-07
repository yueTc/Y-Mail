# em-master 通讯录 — 设计规格（Spec）

| 项 | 值 |
|---|---|
| 文档版本 | v1.2 |
| 日期 | 2026-10-06 |
| 状态 | 已实现（2026-10-07，五关全绿） |
| 依据 | `docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md` v1.2；`docs/superpowers/specs/2026-10-05-inbox-ui-optimization-design.md` v1.2 |
| 范围 | 通讯录入口与三栏页面（v1.0 / v1.2）+ 联系人增删改、分组、备注、导入导出、批量操作（v1.1 新增） |
| 变更记录 | v1.0 → v1.1（2026-10-06）：通讯录从「只读展示」升级为「可管理的地址簿」。用户拍板四条：删除用「隐藏 + 可恢复」；导入导出 CSV 与 vCard 都要；导入导出只放通讯录页；不做导出前的隐私提醒。v1.0 里「不做增删改 / 不做分组备注 / 不做导入导出」三条作废，见第 1.2 节。 v1.1 → v1.2（2026-10-06）：通讯录页从单栏改成三栏（分组栏 / 列表栏 / 详情栏），三栏之间可拖动并记忆宽度；列表按拼音首字母与数字排序并分区。v1.0 / v1.1 里「通讯录页仍是单栏页面」的表述作废。 用户 2026-10-06 已拍板：首字母分区不引拼音库，按 D10 方案甲实现。 |

---

## 0. 需求溯源

1. **谁要的**：项目所有者本人。
2. **解决什么问题**：v1.0 的通讯录只能看不能动——想把某个发件人改个好记的名字、想按「同事 / 客户」分堆、想给某个人留一句备注、想把手机通讯录搬进来或导出去，都做不到。
3. **不做什么会怎样**：自动收集进来的地址会越来越多且没法整理；换电脑或换手机时通讯录搬不动；想删的人还得忍着（而且删了下次同步又回来）。
4. **怎么算做成**：见第 6 节验收标准，共 19 条。
5. **为什么不是只改样式**：要改数据库结构（联系人按邮箱归并、加分组 / 备注 / 来源 / 隐藏）、加十二条命令、加两个文件格式的解析与生成，全是数据层的活。
6. **伪需求甄别**：本条不是伪需求。「点人名看详情、按分组分堆、备注、导入导出」都是用户明确点名的动作，且每一件都有一个可测的完成判据（见第 6 节）。反过来说，「头像 / 生日 / 公司地址」这些 vCard 全字段用户没要，本规格不做。

---

## 1. 目标与边界

### 1.1 本规格包含

- 通讯录页从只读列表升级为可管理的地址簿：新建、编辑、删除、分组、备注。
- 联系人按邮箱归并：一个邮箱一条记录。
- 删除一律先隐藏，进「已隐藏」；可恢复，也可彻底删除。
- 分组可新建、改名、删除；分组名唯一；删分组后成员回到「未分组」。
- 备注是仅本地的多行纯文本。
- 导出 CSV 与 vCard(.vcf) 到用户用系统对话框选定的位置。
- 导入 CSV 与 vCard：选文件 → 列映射 → 预览 → 确认落库。
- 批量选中后批量隐藏、批量改分组。
- 通讯录页是三栏：左「分组栏」（搜索 + 分组列表）、中「列表栏」（分区排序的联系人）、右「详情栏」（查看与就地编辑），两个分隔条可拖动并记忆宽度。
- 列表虚拟滚动，支撑上千条。
- 设置页增加「通讯录」分组，放「清空自动收集的联系人」这个破坏性操作。
- v1.0 已有的内容继续有效：三入口布局、本地搜索、一键写邮件、收件人预填去重。

### 1.2 本规格不包含

- 不接 CardDAV / LDAP / 服务器端通讯录同步，不做多设备同步。
- 不做头像、生日、公司、地址、电话等 vCard 全字段；只认显示名、邮箱、备注、分组。
- 不做 GBK / GB18030 编码的 CSV，只认 UTF-8（可带 BOM）。
- 不做导出前的隐私提醒（用户 2026-10-06 明确不要）。
- 不引入第二个导航列（最左设置栏仍是三个入口）；三栏是通讯录页内部布局，不是新的导航栏。
- 第三栏不做往来邮件时间线；要看往来邮件就点「写邮件」或切回收件箱。
- 不改账号认证、代理、人工智能、外部接入的任何规则。
- 不做联系人合并 / 拆分界面；同邮箱归并在迁移时一次性完成。

---

## 2. 架构决策（含反选理由）

### Decision D1：一个邮箱 = 一个联系人，`contact` 表按邮箱重建

- **Reason**：分组、备注、隐藏这三个状态是「人对人」的属性，不是「账号对人」的属性。一个邮箱在多行时，备注写在哪一行没有答案。
- **Alternatives Considered**：保留 `account_id` 分行存储，界面按邮箱去重展示。
- **Rejected Because**：去重逻辑要在 SQL 和界面各写一遍；编辑名字要往同一个人的多行做扇出写；同一个人可能被显示成两行且状态不一致。
- **Trade-offs Accepted**：存量数据要合并（本机实测 142 行 → 136 行，6 组重复）；写信收件人补全从「按账号隔离」变成「跨账号共用」；`account_id` 这一列被删掉，`upsert_contact` / `search_contacts` 等函数签名随之变更。
- **Revisit When**：用户提出「同一个邮箱在不同账号下要分开管」。

### Decision D2：删除 = 隐藏（软删），彻底删除只放在「已隐藏」里

- **Reason**：自动收集的联系人是同步写进来的，真删之后下次同步会重新收回来，用户会以为删除失效。
- **Alternatives Considered**：直接物理删除；加墓碑表记录被删邮箱。
- **Rejected Because**：直接物理删除会被同步复活；墓碑表要在同步写入路径上加一层「这个邮箱被永久拉黑」的查询，为一个边缘需求增加常态开销。
- **Trade-offs Accepted**：已隐藏的人仍占一行存储；「彻底删除」后若再收到该地址的来信，会重新出现在通讯录里，界面必须明说。
- **Revisit When**：用户反馈「删了又回来」不可接受 ⇒ 升级为墓碑表方案。

### Decision D3：`source` 区分 `auto` / `manual`，用户编辑过的自动联系人升级为手动

- **Reason**：用户手工改过的显示名不能被下次同步的信件署名覆盖掉。
- **Alternatives Considered**：不记录来源，同步永远覆盖名字。
- **Rejected Because**：用户改完名字第二天又变回邮箱前缀，属于明显的数据丢失体感。
- **Trade-offs Accepted**：多一个字段与一条升级规则；同步的 `upsert` 要写成「manual 行不改名」。
- **Revisit When**：用户希望「同步永远以邮件署名为准」。

### Decision D4：CSV 与 vCard 的解析与生成全部手写，不引新依赖

- **Reason**：项目依赖树里没有 `csv` crate，引入要联网拉取并过依赖审查；本规格只需要 RFC 4180 子集与 vCard 3.0 的五个字段。
- **Alternatives Considered**：引入 `csv` crate；引入 `vcard` 系列 crate。
- **Rejected Because**：`csv` 要连带 `csv-core`、`serde`；vCard 的第三方 crate 质量参差且字段支持口径不一，为了五个字段背一个依赖不划算。
- **Trade-offs Accepted**：解析器要自己写、自己测，覆盖引号 / 逗号 / 换行 / BOM / 折行 / 转义 / 多值。
- **Revisit When**：要支持 vCard 全字段（电话 / 地址 / 单位）或 GBK 编码的 CSV。

### Decision D5：导入导出走系统文件对话框 + Rust 读写文件

- **Reason**：项目既有模式就是「前端选路径、Rust 读写」（见 `readInlineImage`、附件下载）；`dialog:default` 已含 `allow-save`，不用加权限。
- **Alternatives Considered**：前端用 File System Access 或下载链接直接读写。
- **Rejected Because**：与既有模式不一致，且 WebView 里拿不到稳定的本地路径语义。
- **Trade-offs Accepted**：导入导出必须走一次命令调用；路径从系统对话框来，用户不选文件就不会有读写。
- **Revisit When**：不做（本决策与项目既有模式绑定）。

### Decision D6：导入时前端把解析结果回传给 Rust 落库

- **Reason**：所见即所写——预览里显示的条目就是落库的条目。
- **Alternatives Considered**：落库时 Rust 重新读一次文件。
- **Rejected Because**：预览与落库之间文件被改动时，预览数字与落库结果会对不上。
- **Trade-offs Accepted**：IPC 要传一份条目数组（上限 5000 条，约 1.5 MB）。
- **Revisit When**：导入条目上限被提到数万条。

### Decision D7：列表用虚拟滚动（`@tanstack/react-virtual`，项目已在用）

- **Reason**：导入后条数可能上千，普通渲染会卡。
- **Alternatives Considered**：普通渲染 + 500 条上限；服务端分页 + 加载更多。
- **Rejected Because**：前者在导入场景下等于功能残缺；后者要为 136 条量级的数据引入分页状态机，属于过度设计。
- **Trade-offs Accepted**：行高要固定或被准确估算，与收件箱列表同样的约束。
- **Revisit When**：出现变高行（比如展开备注）。

### Decision D8：通讯录页改成三栏（分组栏 / 列表栏 / 详情栏）

- **Reason**：联系人多起来之后，单栏「一列到底」既放不下分组浏览，也放不下详情阅读；三栏和收件箱是同一套心智模型，用户不用重新学。
- **Alternatives Considered**：保持单栏 + 弹窗编辑；两栏（列表 + 详情），分组做成顶部下拉。
- **Rejected Because**：单栏放不下分组树；两栏把分组塞进下拉后，分组一多就找不到人，也做不了「重命名 / 删除」的就地操作。
- **Trade-offs Accepted**：窄窗口下三栏会挤，必须沿用收件箱那套「先保详情栏最小宽度、再挤列表、最后挤分组栏」的收缩顺序。
- **Revisit When**：用户反馈窄窗口下三栏不好用，或需要折叠其中一栏。

### Decision D9：三栏宽度复用 `PaneResizer`，把 `usePaneWidths` 参数化

- **Reason**：收件箱已经有一整套「拖动 / 方向键 / 双击复位 / 本地记忆 / 窗口变窄自动收缩」的实现，通讯录再写一套就是复制代码。
- **Alternatives Considered**：给通讯录单写一个 `useContactsPaneWidths`。
- **Rejected Because**：两套同构逻辑各自演化，往后改收缩规则要改两处，属于「半迁移」。
- **Trade-offs Accepted**：`usePaneWidths` 要加「档位」参数（存储键 + 默认值 + 上下限），`InboxPanel` 与它的测试要跟着改一处。
- **Revisit When**：出现第三种三栏页面时，再考虑抽成通用布局组件。

### Decision D10：列表按拼音首字母与数字排序并分区，不引拼音库

- **Reason**：`Intl.Collator("zh-Hans-CN", { numeric: true })` 在 Chromium 里本来就把中文按拼音排序，排序这件事零依赖就对；分区标题再用一组「首字母参照字」二分求出，常见姓名够用。
- **Alternatives Considered**：引入 `pinyin-pro`（前端包体已经 875 KB，再加约 150 KB）；只对 ASCII 名字分区、中文全进「#」区。
- **Rejected Because**：为一个分区标题再加一个前端库不划算；全进「#」区等于中文联系人的分区名存实亡。
- **Trade-offs Accepted**：多音字与生僻字可能分错桶（**排序不会错，只是桶的字母可能不对**）；用一批常见姓名写单测兜底，不准就退化成「#」区。
- **Revisit When**：用户反馈分区字母常错，或需要按拼音做搜索（不只是排序）。

---

## 3. 数据模型（迁移 `0011_contact_address_book.sql`）

### 3.1 新表

```sql
CREATE TABLE IF NOT EXISTS contact_group (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_contact_group_name ON contact_group (name);
```

### 3.2 联系人表重建

```sql
-- 迁移前原样留一份老数据；不自动清理。
CREATE TABLE contact_backup_0011 AS SELECT * FROM contact;

CREATE TABLE contact_v2 (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    email        TEXT NOT NULL,
    name         TEXT NOT NULL DEFAULT '',
    note         TEXT NOT NULL DEFAULT '',
    group_id     INTEGER REFERENCES contact_group(id) ON DELETE SET NULL,
    source       TEXT NOT NULL DEFAULT 'auto' CHECK (source IN ('auto', 'manual')),
    hidden       INTEGER NOT NULL DEFAULT 0 CHECK (hidden IN (0, 1)),
    last_used_at TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
) STRICT;

-- 同一个邮箱只留一条：有名字的优先，其次名字长的，再次最近联系的，最后编号小的。
INSERT INTO contact_v2 (email, name, last_used_at, created_at, updated_at)
SELECT email, name, last_used_at, created_at, created_at FROM (
    SELECT email, name, last_used_at, created_at,
           ROW_NUMBER() OVER (
               PARTITION BY lower(email)
               ORDER BY (trim(name) = '') ASC, length(name) DESC,
                        COALESCE(last_used_at, '') DESC, id ASC
           ) AS rn
    FROM contact
) WHERE rn = 1;

DROP TABLE contact;
DROP INDEX IF EXISTS idx_contact_account_email;
DROP INDEX IF EXISTS idx_contact_global_email;
DROP INDEX IF EXISTS idx_contact_email;
ALTER TABLE contact_v2 RENAME TO contact;

CREATE UNIQUE INDEX idx_contact_email ON contact (lower(email));
CREATE INDEX idx_contact_group ON contact (group_id);
CREATE INDEX idx_contact_hidden ON contact (hidden);
```

### 3.3 字段口径

| 字段 | 类型 | 口径 |
|---|---|---|
| `email` | TEXT NOT NULL | 唯一键，比较时不分大小写 |
| `name` | TEXT NOT NULL DEFAULT `''` | 显示名；空则界面用邮箱顶 |
| `note` | TEXT NOT NULL DEFAULT `''` | 本地备注，纯文本，上限 2000 字 |
| `group_id` | INTEGER NULL | 指向 `contact_group`；NULL 表示「未分组」 |
| `source` | TEXT NOT NULL | `auto`（同步收集）/ `manual`（用户建或改过） |
| `hidden` | INTEGER NOT NULL | 1 表示已隐藏，不出现在列表与收件人补全里 |
| `last_used_at` | TEXT NULL | 最近一次收发信时间 |
| `created_at` / `updated_at` | TEXT NOT NULL | UTC ISO-8601 |

### 3.4 迁移事实（读本机真实库只取计数，未导出任何联系人内容）

- 迁移前：总行数 142、不同邮箱 136、空名字 24、`account_id` 非空 142、全局 0、重复邮箱 6 组（每组 2 条）。
- 迁移后预期：136 行，字段齐全，`contact_backup_0011` 保留 142 行原始数据。
- **回滚**：整条迁移在一个事务里，失败自动回滚。迁移成功后发现异常，可从 `contact_backup_0011` 手工恢复（恢复 SQL 写进交付文档）。

---

## 4. 功能需求

> 关键词口径：MUST = 必须；SHOULD = 应当（有正当理由可偏离）；MAY = 可选。

### R1 入口与三栏布局

系统 MUST 在最左设置栏提供三个入口，顺序固定为：收件箱、通讯录、设置。

- 通讯录图标 SHOULD 用「人 / 联系人」语义的线性图标，鼠标悬停与窄栏下都能看到中文名。
- 当前入口 MUST 有明确选中态（`aria-current="page"`）。
- 三个页面组件 MUST 同时挂载、靠 `hidden` 切换，来回切不丢收件箱里已选的账号、文件夹、邮件与面板宽度。
- 通讯录页 MUST 是三栏：左「分组栏」、中「列表栏」、右「详情栏」；两个分隔条 MUST 可拖动。
- 三栏宽度 MUST 本地记忆并在下次打开时还原；窗口变窄时的收缩顺序 MUST 是「先挤列表栏，再挤分组栏」，详情栏 MUST 不低于 320 像素。

**Scenario 1.1**：GIVEN 用户在收件箱；WHEN 点最左「通讯录」；THEN 右侧切换成通讯录三栏页，设置栏仍在最左，收件箱状态不变。

**Scenario 1.2**：GIVEN 用户在通讯录；WHEN 点「设置」；THEN 显示单栏设置页，通讯录的选中分组、选中联系人与搜索词保留。

**Scenario 1.3**：GIVEN 用户把分组栏拖宽到 300 像素；WHEN 切到收件箱再切回通讯录；THEN 分组栏宽度仍是 300 像素。

### R2 新建与编辑联系人

系统 MUST 允许新建与编辑联系人，字段为：显示名、邮箱、分组、备注。

- 邮箱 MUST 做格式校验（本地部分 + `@` + 域名部分，不含空格）。
- 邮箱 MUST 大小写不敏感地去重：新建时与已有记录冲突 MUST 拒绝并给出可读提示。
- 显示名 MAY 为空，空则界面用邮箱地址顶上。
- 备注 SHALL 支持多行，长度上限 2000 字，超出 MUST 拒绝并提示。
- 编辑一个 `source = 'auto'` 的联系人时，保存后 `source` MUST 升级为 `manual`。
- 保存成功 MUST 让通讯录列表立刻反映最新内容（不要求用户手动刷新）。

**Scenario 2.1**：GIVEN 通讯录里没有 `bob@example.com`；WHEN 新建并填写显示名「鲍勃」、邮箱 `bob@example.com`、分组「客户」、备注「上次报价 3 万」；THEN 该联系人出现在「客户」分组下，详情里显示名、邮箱、备注与填写一致。

**Scenario 2.2**：GIVEN 已有 `bob@example.com`；WHEN 新建时再填一次（大小写任意）；THEN 保存被拒绝，提示该地址已存在，列表不新增记录。

**Scenario 2.3**：GIVEN 一个由同步收集的联系人；WHEN 用户改掉它的显示名并保存；THEN 该记录 `source` 变成 `manual`；再跑一次同步后显示名仍是用户改的那个。

### R3 删除（隐藏）与已隐藏

删除操作 MUST 是软删除：把 `hidden` 置 1，不出现在通讯录列表，也不出现在写信的收件人补全里。

- 通讯录页 MUST 提供进入「已隐藏」的入口，并显示已隐藏条数。
- 已隐藏列表 MUST 支持「恢复」与「彻底删除」两个操作。
- 「彻底删除」MUST 二次确认，且提示「以后收到这个地址的来信，它会重新出现在通讯录里」。
- 同步写入 MUST NOT 把已隐藏的联系人重新置为可见。
- 同步写入 MUST NOT 更新已隐藏联系人的名字。

**Scenario 3.1**：GIVEN 通讯录里有「张三」；WHEN 点删除；THEN 它从列表消失，「已隐藏」计数 +1。

**Scenario 3.2**：GIVEN 「张三」已被隐藏，且用户此时收到张三发来的新邮件并完成一次同步；WHEN 回到通讯录；THEN 张三仍然不在正常列表里，仍在「已隐藏」里。

**Scenario 3.3**：GIVEN 张三在「已隐藏」里；WHEN 点恢复；THEN 它回到原来的分组（分组已删则回到「未分组」）。

**Scenario 3.4**：GIVEN 张三在「已隐藏」里；WHEN 点彻底删除并确认；THEN 该行从库里消失，「已隐藏」列表不再包含它。

### R4 分组

系统 MUST 允许新建、改名、删除分组，并把联系人归入分组。

- 分组名 MUST 非空、去掉首尾空格后不超过 20 字。
- 分组名 MUST 唯一（大小写不敏感），重名 MUST 拒绝并提示。
- 删除分组 MUST 二次确认；确认后组内联系人 MUST 回到「未分组」，联系人本身 MUST NOT 被删除。
- 通讯录页 SHOULD 提供分组筛选（全部 / 未分组 / 各分组），并显示每个分组的条数。
- 一个联系人 SHALL 最多属于一个分组。

**Scenario 4.1**：GIVEN 用户新建分组「客户」；WHEN 把「鲍勃」的分组改成「客户」；THEN 分组筛选里的「客户」条数 +1，「未分组」条数 -1。

**Scenario 4.2**：GIVEN 分组「客户」下有三个联系人；WHEN 删除分组「客户」并确认；THEN 三个联系人出现在「未分组」下，通讯录总条数不变。

**Scenario 4.3**：GIVEN 已有分组「客户」；WHEN 再建一个「客户」；THEN 被拒绝并提示重名。

### R5 备注

系统 MUST 把备注作为仅本地的纯文本保存与展示。

- 备注 MUST NOT 参与任何外发动作，不进入邮件正文、不进入任何联网请求。
- 备注渲染 MUST 按纯文本处理，不做 HTML 注入。
- 列表里 MUST 只显示摘要（取第一行前 60 字），完整内容在详情里看。

**Scenario 5.1**：GIVEN 备注里含换行、引号、`<script>` 字样；WHEN 保存后重新打开详情；THEN 内容原样显示，`<script>` 不执行、不当作标签渲染。

### R6 导出

系统 MUST 支持把当前全部可见联系人（不含已隐藏）导出为 CSV 或 vCard。

- 导出 MUST 走系统保存对话框由用户选路径；用户取消时 MUST NOT 写任何文件。
- CSV MUST 带 UTF-8 BOM，列顺序固定为：显示名、邮箱、分组、备注。
- CSV MUST 按 RFC 4180 转义：含逗号 / 引号 / 换行的字段用双引号包裹，内部双引号写成两个。
- vCard MUST 输出 3.0，字段为 `FN` / `EMAIL` / `NOTE` / `CATEGORIES`（分组）。
- 导出成功后 MUST 提示实际写出的条数与文件路径。

**Scenario 6.1**：GIVEN 通讯录有 3 个可见联系人和 1 个已隐藏联系人；WHEN 导出 CSV 并确认保存；THEN 文件里只有 3 条联系人数据（不含表头），不含已隐藏的那位；用 Excel 打开中文不乱码。

**Scenario 6.2**：GIVEN 某位联系人的备注里带逗号与换行；WHEN 导出 CSV；THEN 该字段被双引号包裹，重新导入后内容与导出前一致。

**Scenario 6.3**：GIVEN 用户在保存对话框里点取消；THEN 不产生任何文件，界面不报错。

### R7 导入

系统 MUST 支持从 CSV 或 vCard 文件导入联系人，流程为：选文件 → 列映射 → 预览 → 确认落库。

- 文件 MUST 由用户在系统打开对话框里选择；系统 MUST NOT 自动读取任何路径。
- 文件 MUST 不超过 5 MB，解析出的条目 MUST 不超过 5000 条；超出 MUST 拒绝并提示。
- 编码 MUST 按 UTF-8 处理（允许 BOM）；按其他编码解析出非法字符时 MUST 记为坏行而不是静默替换。
- CSV MUST 让用户指认哪一列是邮箱；邮箱列缺失或全为空 MUST 拒绝导入。
- 预览 MUST 显示：读到几条、将新增几条、重复几条、无效几条，并列出最多 20 条无效行的行号与原因。
- 落库策略 MUST 由用户选：跳过重复（默认）或用文件里的内容覆盖重复项的名字与备注。
- 导入 MUST 只增不删：MUST NOT 删除或隐藏库里已有的联系人。
- 无效行 MUST NOT 写库（拒绝档），有效行照常写入（放行档）；两者 MUST 在结果里分别报数。
- 落库 MUST 在单个事务里完成；任一步失败 MUST 整批回滚。
- 导入进来的联系人与分组，其 `source` MUST 记为 `manual`。

**Scenario 7.1**：GIVEN 一个含 100 条联系人的 vCard；WHEN 选择它；THEN 预览显示读到 100 条；WHEN 确认；THEN 新增数与预览一致，通讯录总数按预期增加。

**Scenario 7.2**：GIVEN CSV 里 10 条数据，其中 3 条的邮箱为空；WHEN 预览；THEN 显示 7 条有效、3 条无效并列出这 3 条的行号；WHEN 确认导入；THEN 只写入 7 条。

**Scenario 7.3**：GIVEN 文件里有 `bob@example.com`，库里已有同名地址且已隐藏；WHEN 按默认「跳过重复」导入；THEN 该条不写入，已隐藏的那位保持隐藏。

**Scenario 7.4**：GIVEN 用户选了一个 6 MB 的文件；THEN 导入被拒绝，提示文件过大，库里没有任何变化。

### R8 批量操作

系统 MUST 支持多选通讯录条目并批量操作。

- 批量操作 MUST 至少包含：批量隐藏、批量改分组。
- 批量隐藏 MUST 走与单条删除相同的软删语义。
- 批量操作前 MUST 二次确认，并显示将影响的条数。
- 全部取消选择时，批量操作按钮 MUST 禁用。

**Scenario 8.1**：GIVEN 列表里勾选 5 个联系人；WHEN 点批量隐藏并确认；THEN 这 5 个从列表消失，「已隐藏」计数 +5。

**Scenario 8.2**：GIVEN 勾选 5 个联系人；WHEN 选「移到分组 / 客户」并确认；THEN 这 5 个的 `group_id` 都指向「客户」，「未分组」条数相应减少。

### R9 设置页「通讯录」分组

系统 SHOULD 在设置页增加「通讯录」分组，放「清空自动收集的联系人」。

- 该操作 MUST 二次确认，并说明它只清 `source = 'auto'` 且未被隐藏的记录。
- `source = 'manual'` 的记录 MUST NOT 被这条操作影响。
- 已隐藏的记录 MUST NOT 被这条操作影响（那是用户主动藏的）。

**Scenario 9.1**：GIVEN 库里有 10 条 `auto`（其中 1 条已隐藏）与 3 条 `manual`；WHEN 执行「清空自动收集的联系人」并确认；THEN 剩下 1 条已隐藏的 `auto` 与 3 条 `manual`，共 4 条。

### R10 写信补全口径（MODIFIED）

**Previously**：`search_contacts(account_id, keyword, limit)` 只在指定账号的联系人与全局联系人里搜。

**Now**：系统 MUST 在一份共用的通讯录里搜联系人，且 MUST 排除已隐藏的联系人。

**Scenario 10.1**：GIVEN 通讯录里有 `bob@example.com`；WHEN 用户在写信窗格的收件人输入框里敲 `bob`；THEN 补全列表里出现鲍勃，点一下填入收件人。

**Scenario 10.2**：GIVEN `bob@example.com` 已被隐藏；WHEN 在收件人输入框里敲 `bob`；THEN 补全列表里不出现他，但用户手敲完整地址仍然可以正常发送。

### R11 第一栏：搜索与分组

第一栏 MUST 由「搜索框」和「分组列表」两部分组成，搜索框在上、分组列表在下。

- 搜索框 MUST 按名字或邮箱过滤联系人，输入即时生效，过滤范围是当前选中的分组。
- 分组列表 MUST 含一个固定条目「所有联系人」，它 MUST NOT 被重命名或删除。
- 用户自建分组 MUST 支持新建、重命名、删除三个操作，全部在第一栏内就地完成（不做弹窗跳转）：新建走底部按钮，**重命名与删除走分组上的右键菜单**。
- 每个条目 MUST 同行显示名称与条数（例如「客户 12」）。
- 存在未分组联系人时 SHOULD 显示固定条目「未分组」；没有未分组联系人时 MAY 隐藏。
- 分组列表 MUST 含固定条目「已隐藏」，并在后面显示条数。
- 第一栏底部 SHOULD 放「导入」「导出」两个入口。
- 删除分组 MUST 二次确认，并说明「组内联系人会回到未分组，联系人本身不会被删」。

**Scenario 11.1**：GIVEN 用户还没建过任何分组；WHEN 打开通讯录；THEN 第一栏只有「所有联系人」（有需要时还有「未分组」「已隐藏」），没有任何自建分组。

**Scenario 11.2**：GIVEN 用户在第一栏新建分组「客户」；WHEN 选中它；THEN 第二栏只显示属于「客户」的联系人，「所有联系人」的条数不变。

**Scenario 11.3**：GIVEN 分组「客户」里有三个联系人；WHEN 在第一栏把它删掉并确认；THEN 三个联系人出现在「未分组」下，「所有联系人」条数不变。

**Scenario 11.4**：GIVEN 第一栏选中「所有联系人」；WHEN 在搜索框输入 `zhang`；THEN 第二栏只剩名字或邮箱含 `zhang` 的人。

**Scenario 11.5**：GIVEN 用户把分组「客户」重命名为「老客户」；THEN 条目名变了，组内联系人一条不动。

### R12 第二栏：联系人列表

第二栏 MUST 显示当前分组（或搜索结果、或已隐藏列表）里的联系人。

- 排序 MUST 按显示名（没有显示名就用邮箱）的拼音 / 字母顺序；以数字开头的条目 MUST 按数值顺序排在字母之前。
- 列表 MUST 分区显示，区标题为：数字区、`A`–`Z`、其他区。
- 区标题 MUST 在滚动时保持可辨认（吸顶）。
- 每行 MUST 显示显示名与邮箱；该联系人有备注时 SHOULD 显示备注第一行的摘要。
- 在联系人行上右键 MUST 给出「添加到」，鼠标浮上去 MUST 展开全部可选分组（含「未分组」），选一个就把这位联系人挪过去。
- 一行的显示名 MUST 用邮箱兜底（没有显示名时不显示空标题）。
- 第二栏顶部 MUST 有「新建联系人」入口；当前分组不是「所有联系人」时，新建的联系人 MUST 默认落进当前分组。
- 第二栏顶部 MUST 提供批量选择开关；进入批量后每行前出现勾选框，顶部出现「批量隐藏」「移到分组」与已选条数。
- 列表 MUST 用虚拟滚动，导入 3000 条后仍可流畅滚动。
- 第二栏在没有任何联系人时 MUST 显示提示文案，不留空白。

**Scenario 12.1**：GIVEN 通讯录里有「阿里」「Bob」「3M」「张三」；WHEN 选中「所有联系人」；THEN 列表按「数字区 → 字母区」排列：`3M` 落在数字区且在最前，其余三人按各自首字母落在对应区。

**Scenario 12.2**：GIVEN 第一栏选中「客户」；WHEN 点第二栏「新建联系人」并保存；THEN 新联系人的分组是「客户」，并出现在第二栏。

**Scenario 12.3**：GIVEN 第二栏勾选 3 个人；WHEN 点「移到分组 / 客户」并确认；THEN 这 3 个人进入「客户」，「未分组」条数相应减少。

**Scenario 12.4**：GIVEN 某位联系人现在在「未分组」；WHEN 在他那一行上右键、鼠标浮到「添加到」、点「客户」；THEN 这位联系人进入「客户」，「未分组」条数 -1。

**Scenario 12.5**：GIVEN 用户在第一栏某个自建分组上右键；WHEN 点「重命名」并改名；THEN 分组名跟着变，组内联系人一条不动。

### R13 第三栏：联系人详情与就地编辑

第三栏 MUST 显示第二栏里选中的那个联系人的详情，并在同一栏内完成编辑。

- 详情 MUST 显示：显示名、邮箱、分组、备注、最近联系时间、来源（自动收集 / 手动添加）。
- 详情 MUST 提供「写邮件」「复制地址」「编辑」「删除」四个操作。
- 点「编辑」后 MUST 在同一栏内切换成表单（显示名 / 邮箱 / 分组 / 备注），并给出「保存」「取消」。
- 保存失败时 MUST 留在表单里显示可读原因，MUST NOT 静默丢弃用户已经输入的内容。
- 详情栏没选中任何人时 MUST 显示空态提示（例如「从中间选一个人」）。
- 选中的联系人被隐藏或被彻底删除后，详情栏 MUST 回到空态。

**Scenario 13.1**：GIVEN 用户在第二栏点中「鲍勃」；WHEN 详情栏显示；THEN 能看到他的邮箱、分组、备注、最近联系时间与来源。

**Scenario 13.2**：GIVEN 详情栏已打开鲍勃；WHEN 点「编辑」把显示名改成「鲍勃·李」并保存；THEN 详情栏显示新名字，第二栏那一行同步更新。

**Scenario 13.3**：GIVEN 详情栏打开的是张三；WHEN 用户在第二栏把张三删除；THEN 详情栏回到空态。

### R14 三栏拖动与宽度记忆

三栏之间 MUST 有两个可拖动的分隔条。

- 分隔条 MUST 支持鼠标拖动、左右方向键调整、双击复位（与收件箱同一套交互）。
- 三个宽度 MUST 各有上下限，且任何情况下详情栏 MUST 不低于 320 像素。
- 宽度 MUST 存本地，存储键 MUST 与收件箱的分开存，互不影响。
- 窗口由窄变宽时 MUST 恢复到用户设定的宽度。

**Scenario 14.1**：GIVEN 用户把分组栏拖到 300 像素；WHEN 关掉应用再打开并进入通讯录；THEN 分组栏还是 300 像素。

**Scenario 14.2**：GIVEN 窗口被拖窄到放不下三栏；WHEN 观察收缩结果；THEN 先挤列表栏，再挤分组栏，详情栏不低于 320 像素。

---

## 5. 命令与契约

### 5.1 新增命令

`create_contact`、`update_contact`、`hide_contact`、`restore_contact`、`purge_contact`、`list_contact_groups`、`create_contact_group`、`rename_contact_group`、`delete_contact_group`、`export_contacts`、`preview_contact_import`、`apply_contact_import`。

### 5.2 修改命令

| 命令 | 现状 | 改成 |
|---|---|---|
| `list_contacts` | `keyword?`, `limit?` | 追加 `scope`（`active` / `hidden`），返回值加 `groupId` / `groupName` / `note` / `source` / `hidden` |
| `search_contacts` | `account_id`, `keyword`, `limit?` | 去掉 `account_id`；结果加 `hidden = 0` 过滤 |

### 5.3 Rust 内部签名变更（已 grep 核实，共 6 处）

| 位置 | 改成 |
|---|---|
| `crates/mail-store/src/compose.rs` `upsert_contact` | 去掉 `account_id` 参数；命中 `manual` 行不改名 |
| `crates/mail-store/src/compose.rs` `upsert_contacts` | 去掉 `account_id` 参数；跳过已隐藏行 |
| `crates/mail-store/src/compose.rs` `search_contacts` | 去掉 `account_id`；加 `hidden = 0` |
| `crates/mail-store/src/compose.rs` `list_contacts` | 加 `scope` 与分组 / 备注字段 |
| `crates/mail-core/src/compose.rs` `search_contacts` | 去掉 `account_id` |
| `crates/mail-core/src/sync/fetcher.rs` | 调用点去掉 `ctx.account_id` |

### 5.4 前端

| 位置 | 改动 |
|---|---|
| `src/api.ts` | `Contact` 加 `groupId` / `groupName` / `note` / `source` / `hidden`；新增 `ContactGroup`、导入预览与结果类型；`searchContacts` 去掉 `accountId` |
| `src/ComposePanel.tsx` | 补全调用点去掉 `accountId` |
| `src/ContactsWorkspace.tsx` | 从只读页升级为可管理页 |
| `src/SettingsWorkspace.tsx` | 加「通讯录」分组 |
| `src/index.css` | 通讯录相关样式 |

---

## 6. 验收标准

> 前 7 条是 v1.0 的既有验收，继续有效；V8–V19 是本版新增。

| 编号 | 验收 | 对应需求 |
|---|---|---|
| V1 | 设置栏三个入口按「收件箱 / 通讯录 / 设置」排列，点谁谁亮 | R1 |
| V2 | 通讯录页列出全部未隐藏联系人，按最近联系倒序 | R1 |
| V3 | 搜索框输入名字或邮箱片段，列表当场只剩匹配项并显示条数 | R1 |
| V4 | 点某行「写邮件」切回收件箱、打开写信窗格、收件人已填好 | R10 |
| V5 | 收件人里已有同一地址（不分大小写）时不重复添加 | R10 |
| V6 | 没有显示名的联系人用邮箱顶，不留空行 | R2 |
| V7 | 读取失败时给出可读错误提示，页面不崩 | R1 |
| V8 | 新建联系人后列表立刻出现；重复邮箱被拒绝并提示 | R2 |
| V9 | 编辑改完邮箱、分组、备注后，列表与详情同步刷新 | R2 |
| V10 | 删除后列表消失、进入「已隐藏」；恢复后回到列表 | R3 |
| V11 | 删掉一个自动收集的联系人后再同步一次，它不会回到列表 | R3 |
| V12 | 分组改名成员跟着走；删分组后成员回到「未分组」，总条数不变 | R4 |
| V13 | 备注含换行 / 引号 / `<script>` 字样，保存后原样读回且不执行 | R5 |
| V14 | 导出 CSV 用 Excel 打开中文不乱码、列不串位，且不含已隐藏联系人 | R6 |
| V15 | 导出 vCard 后重新导入，内容与导出前一致（往返） | R6 |
| V16 | 导入预览的数字与落库结果一致；坏行被列出且不写库 | R7 |
| V17 | 同一个邮箱导入两次，第二次按「跳过」不产生重复行 | R7 |
| V18 | 批量隐藏 5 条后，「已隐藏」计数正好 +5 | R8 |
| V19 | 导入 3000 条后列表滚动不卡；已隐藏的人不出现在收件人补全里 | R3 / R7 |
| V20 | 通讯录页是三栏，两个分隔条都能拖动，宽度能记住 | R1 / R14 |
| V21 | 第一栏默认只有「所有联系人」，自建分组能新建 / 重命名 / 删除 | R11 |
| V22 | 列表按「数字区 + 字母区」排列，中文按拼音落在对应字母区 | R12 |
| V23 | 点第二栏一行，第三栏显示详情；点「编辑」能在同一栏改完并保存 | R13 |
| V24 | 窗口拖到最窄时先挤列表栏、再挤分组栏，详情栏不低于 320 像素 | R14 |
| V25 | 把详情栏里那个人删掉后，详情栏回到空态 | R13 |
| V26 | 右键分组出现「重命名 / 删除分组」，删除前有二次确认 | R11 |
| V27 | 右键联系人出现「添加到」，浮出分组列表，点一个就把人挪过去 | R12 |

---

## 7. 影响文件

| 文件 | 改动 |
|---|---|
| `crates/mail-store/src/migrations/sql/0011_contact_address_book.sql` | 新增：分组表、联系人表重建、备份表 |
| `crates/mail-store/src/migrations/mod.rs` | 登记迁移 0011 |
| `crates/mail-store/src/compose.rs` | 联系人 CRUD、分组 CRUD、同步写入口径、签名变更、单测 |
| `crates/mail-store/src/lib.rs` | 导出新类型 |
| `crates/mail-core/src/compose.rs` | 编排与校验、CSV / vCard 解析生成、单测 |
| `crates/mail-core/src/sync/fetcher.rs` | 同步登记口径调整 |
| `src-tauri/src/commands.rs` | 12 个新命令 + 2 个改签名命令 |
| `src-tauri/src/lib.rs` | 注册新命令 |
| `src/api.ts` | 类型与调用封装 |
| `src/usePaneWidths.ts` | 参数化：支持第二套存储键与宽度档位（通讯录三栏），收件箱默认行为不变 |
| `src/PaneResizer.tsx` | 不改，直接复用 |
| `src/ContactsWorkspace.tsx` | 从单栏页升级为三栏页（分组栏 / 列表栏 / 详情栏） |
| `src/ComposePanel.tsx` | 补全调用点调整 |
| `src/SettingsWorkspace.tsx` | 通讯录分组 |
| `src/index.css` | 样式 |
| `src/__tests__/contactsWorkspace.test.tsx` | 回归更新 |
| `src/__tests__/contactsImportExport.test.tsx` | 新增 |
| `src/__tests__/compose.test.tsx` | 补全调用点调整 |

---

## 8. 安全与规格约束

- 邮件正文、联系人备注一律当不可信内容；备注只按纯文本渲染，不注入 HTML。
- 导入导出 MUST 只使用用户在系统对话框里亲自选定的文件；MUST NOT 自动读写任何路径，MUST NOT 联网。
- 联系人数据只有显示名、邮箱、备注、分组，MUST NOT 携带密码、授权码、令牌、密钥；这些凭据不进本规格的任何数据通路。
- 导入文件不做任何解释执行（不解析脚本、不打开链接、不触发外发）。
- 发信仍然只由写信窗格里的「发送」按钮触发，逐次确认规则不变；通讯录的任何操作都不发信。
- 远程图片默认拦截、人工智能默认关闭等规则不受影响。
- 本规格与 `2026-10-05-inbox-ui-optimization-design.md` 里「设置栏只允许两个图标」冲突时，以本规格为准（该文档已升到 v1.2 并加注）。

---

## 9. Tasks

### Wave A1：数据层（无依赖）

- [x] Task A1.1：写迁移 `0011`，含分组表、备份表、按邮箱重建、索引
- [x] Task A1.2：`migrations/mod.rs` 登记 0011
- [x] Task A1.3：迁移单测（老结构 + 重复邮箱 → 新结构）
- [x] Task A1.4：`mail-store` 联系人 CRUD（建 / 改 / 隐藏 / 恢复 / 彻底删）
- [x] Task A1.5：`mail-store` 分组 CRUD（建 / 改名 / 删，删后成员回未分组）
- [x] Task A1.6：`mail-store` 同步写入新口径（`upsert_contact` / `upsert_contacts` / `search_contacts` / `list_contacts` 签名调整）

### Wave A2：引擎与外壳（依赖 A1）

- [x] Task A2.1：`mail-core` 编排与校验（邮箱格式、备注长度、分组名长度与重名）
- [x] Task A2.2：`mail-core` 单测
- [x] Task A2.3：`src-tauri` 新增 8 个联系人 / 分组命令并注册
- [x] Task A2.4：`fetcher.rs` 调用点调整

### Wave A3：界面（依赖 A2）

- [x] Task A3.1：`src/api.ts` 类型与封装
- [x] Task A3.2：`usePaneWidths` 参数化（档位 = 存储键 + 默认值 + 上下限），`InboxPanel` 与 `paneWidths.test.ts` 跟着改
- [x] Task A3.3：三栏骨架 + 两个 `PaneResizer` + 宽度记忆
- [x] Task A3.4：第一栏：搜索框 + 分组列表（所有联系人 / 未分组 / 已隐藏 / 自建分组）+ 分组新建 / 重命名 / 删除
- [x] Task A3.5：第二栏：分区排序列表（数字区 / A–Z / 其他）+ 虚拟滚动 + 批量选择
- [x] Task A3.6：第三栏：详情 + 就地编辑表单 + 写邮件 / 复制 / 编辑 / 删除四个操作
- [x] Task A3.7：删除与「已隐藏」视图（恢复 / 彻底删除）
- [x] Task A3.8：`ComposePanel` 补全调用点调整
- [x] Task A3.9：前端测试（V8–V13、V18、V20–V25）

### Wave B1：导入导出（依赖 A2）

- [x] Task B1.1：手写 CSV 解析与生成（RFC 4180 子集）+ 单测
- [x] Task B1.2：手写 vCard 3.0 解析与生成 + 单测
- [x] Task B1.3：`export_contacts` 命令（写文件）
- [x] Task B1.4：`preview_contact_import` 与 `apply_contact_import` 命令
- [x] Task B1.5：前端导出按钮 + 导入向导（选文件 / 列映射 / 预览 / 确认）
- [x] Task B1.6：前端测试（V14–V17、V19）

### Wave C：收尾（依赖 A3、B1）

- [x] Task C1.1：设置页「通讯录」分组 + 清空自动收集（二次确认）
- [x] Task C1.2：设置页测试
- [x] Task C1.3：四关 + `npm test` 全绿
- [x] Task C1.4：更新 `.ai-memory`（handoff / project_memory / daily）
- [x] Task C1.5：本规格状态改为「已实现」

---

## 10. 编码前 Spec 验证

- [x] 每个 Requirement 至少一个 Scenario（R1–R14 全部有）
- [x] 三栏布局的排序 / 分区 / 拖动 / 空态都有可测场景（R1、R11–R14）
- [x] 每个 Scenario 可测（有明确 Given / When / Then）
- [x] 成功标准明确（用 MUST / SHOULD，不用「可能 / 尽量」）
- [x] 变更范围聚焦（不做头像、生日、CardDAV、GBK、隐私提醒）
- [x] 模糊词已转成判据（「不卡」→ 3000 条滚动；「只增不删」→ 不得删除或隐藏已有记录）
- [x] 架构决策写了反选理由与接受的代价（D1–D10）
- [x] 数据模型给了可直接执行的 SQL
- [x] 回滚路径明确（单事务 + `contact_backup_0011` + 手工恢复 SQL）
- [x] 未决项与未验证项已列（见第 11 节）

---

## 11. 未决与未验证

| 编号 | 类型 | 内容 | 处理 |
|---|---|---|---|
| U1 | 未决 | `contact_backup_0011` 要不要到期清理 | 先不清理；用户嫌占地方再做 |
| U2 | 未决 | 彻底删除是否升级为墓碑表 | 先不做；用户反馈「删了又回来」再升级 |
| U3 | 已定 | 中文名分区的首字母不引拼音库（方案乙否掉） | 用户 2026-10-06 拍板：按 D10 方案甲实现；只在单测发现分区字母常错时回头找他 |
| N1 | 未验证 | 真实窗口里的观感与系统保存 / 打开对话框 | 本环境没有桌面窗口，必须由用户在本机点一遍 |
| N2 | 未验证 | 别人家导出的 vCard（苹果 / 安卓 / Outlook）字段写法差异 | 只保证自家往返一致；需要用户提供样本再补兼容 |
| N3 | 未验证 | Excel 打开导出 CSV 的实际表现 | 保证生成的是带 BOM 的 UTF-8，最终观感由用户验 |
| N4 | 未验证 | `Intl.Collator` + 首字母参照字表在 WebView2 里对中文姓名的分桶准确率 | 用一批常见姓名写单测兜底；不准就退化成「#」区，或改走方案乙 |