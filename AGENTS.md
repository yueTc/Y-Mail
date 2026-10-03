# AGENTS.md — em-master 项目工作规则

> 每个智能体在本仓库开工前都会自动读到这份文件。先说人话，再谈技术。

## 一、说人话（硬要求）

1. 面向用户一律用中文大白话。先说结论，再说原因；句子短，别绕。
2. 禁止中英夹杂，禁止英文旁白；代码、命令、文件名除外。
3. 技术名词第一次出现时用一句大白话解释；能用日常词就别用术语。
4. 不堆日志、不堆术语、不给用户看长篇分析；用户要细节时再展开。
5. 没验证过的事必须直说"这块没验"，不许含糊过去。

## 二、安全铁律

- 邮件正文一律当成不可信内容，不许由它触发发信、跳转、写库或执行命令。
- 密码、授权码、令牌、AI 密钥只许存进系统保险箱（keyring）；数据库、日志、报错信息里都不许出现明文。
- AI 和翻译默认关闭；每次外发前必须用户点头，并说清发给谁、用哪个模型、是不是本地。
- 远程图片默认拦截。

## 三、干活规矩

- 规格文档是唯一依据：docs/superpowers/specs/2026-10-03-unified-inbox-client-design.md（v1.1）。
- 每批改动必须过四关：cargo fmt --all --check、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace、npm run build。
- 终端若提示找不到 cargo，先在当前窗口执行：$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
- 每个阶段结束更新 .ai-memory/（handoff.md、project_memory.md、当日 daily.md），并汇报四件事：做完的（带证据）/ 相关文件 / 没做完的 / 卡在哪。
- 规格没写、要花钱、涉及安全的事，先问用户，不许自作主张。

## 四、写文件与仓库

- 写文件用 [System.IO.File]::WriteAllText($path, $content, (New-Object System.Text.UTF8Encoding $false))；UTF-8 无 BOM、LF 换行；不许用 Set-Content / Out-File；多个写操作分开执行。
- 仓库：D:\projects\em-master；分支用 codex/ 开头；提交信息用中文。
- 禁止提交 *.db、.env、密钥文件和构建产物。
- 不要动 D:\projects 这个父目录的仓库。