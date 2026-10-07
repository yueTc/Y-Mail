//! `ymail-mcp` 的端到端测试：真拉起可执行文件，走标准输入输出。
//!
//! 覆盖规格 Scenario 12.1（默认态连接不可用）、12.2（只读搜索可用 + 审计落库）
//! 与 12.3（越权工具被拒），以及协议版本不匹配拒绝服务。

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;

use mail_core::MailEngine;
use mail_domain::account::{AccountDraft, AccountProxyMode, AuthType, Security, ServerConfig};
use mail_domain::FolderKind;
use mail_store::NewMessage;
use serde_json::{json, Value};

/// 测试用账号。
fn draft() -> AccountDraft {
    AccountDraft {
        display_name: "测试账号".to_string(),
        email: "someone@example.com".to_string(),
        auth_type: AuthType::Password,
        username: "someone@example.com".to_string(),
        imap: ServerConfig {
            host: "imap.example.com".to_string(),
            port: 993,
            security: Security::Tls,
        },
        smtp: ServerConfig {
            host: "smtp.example.com".to_string(),
            port: 465,
            security: Security::Tls,
        },
        proxy: AccountProxyMode::InheritGlobal,
        color: "#3366ff".to_string(),
        enabled: true,
        oauth_provider: None,
        oauth_client_id: String::new(),
    }
}

/// 建一份临时数据目录：一个账号 + 一封收件箱邮件；返回（目录, 账号号, 邮件号）。
fn seed_data_dir() -> (tempfile::TempDir, i64, i64) {
    let dir = tempfile::tempdir().expect("临时目录");
    let engine =
        MailEngine::initialize_with_secrets(dir.path(), Arc::new(mail_core::MemorySecretStore::new()))
            .expect("初始化引擎");
    let account_id = engine.store().insert_account(&draft(), None).expect("插账号").0;
    let store = engine.store();
    let folder_id = store
        .upsert_folder(account_id, "INBOX", "/", FolderKind::Inbox)
        .expect("插文件夹");
    store
        .insert_messages(&[NewMessage {
            account_id,
            folder_id,
            uid: 1,
            message_id_header: "<m1@example.com>".to_string(),
            thread_key: "thread-1".to_string(),
            subject: "十月发票报销".to_string(),
            from_name: "张三".to_string(),
            from_addr: "zhangsan@example.com".to_string(),
            to_json: "[]".to_string(),
            cc_json: "[]".to_string(),
            date_utc: "2026-10-05T01:00:00Z".to_string(),
            size: 1234,
            has_attachments: false,
            is_read: false,
            is_flagged: false,
            is_answered: false,
            is_draft: false,
        }])
        .expect("插邮件");
    let message_id: i64 = store
        .list_inbox_messages(&mail_store::InboxQuery {
            account_id: Some(account_id),
            folder_id: Some(folder_id),
            unread_only: false,
            flagged_only: false,
            folder_kind: None,
            offset: 0,
            limit: 10,
        })
        .expect("读回收件箱")
        .into_iter()
        .find(|message| message.uid == 1)
        .expect("应能读回刚插的邮件")
        .id;
    drop(store);
    (dir, account_id, message_id)
}

/// 拉起可执行文件。
fn spawn(data_dir: &std::path::Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_ymail-mcp"))
        .env("YMAIL_DATA_DIR", data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("拉起 ymail-mcp")
}

/// 写一行 JSON-RPC 请求。
fn write_line(child: &mut std::process::Child, value: &Value) {
    let stdin = child.stdin.as_mut().expect("子进程标准输入");
    writeln!(stdin, "{value}").expect("写请求");
    stdin.flush().expect("刷新标准输入");
}

/// 读一行 JSON-RPC 响应。
fn read_line(reader: &mut BufReader<std::process::ChildStdout>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("读响应");
    assert!(!line.trim().is_empty(), "服务端应返回一行 JSON");
    serde_json::from_str(line.trim()).expect("响应应是合法 JSON")
}

#[test]
fn 默认关闭时进程直接拒绝服务() {
    let dir = tempfile::tempdir().expect("临时目录");
    // 先建库但不打开 MCP 开关。
    MailEngine::initialize_with_secrets(dir.path(), Arc::new(mail_core::MemorySecretStore::new()))
        .expect("初始化引擎");

    let output = Command::new(env!("CARGO_BIN_EXE_ymail-mcp"))
        .env("YMAIL_DATA_DIR", dir.path())
        .stdin(Stdio::null())
        .output()
        .expect("运行 ymail-mcp");

    assert!(!output.status.success(), "默认关闭时进程应拒绝服务");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("MCP 未启用"), "应给出可读原因：{stderr}");
}

#[test]
fn 协议版本不匹配时拒绝服务() {
    let (dir, _account_id, _message_id) = seed_data_dir();
    {
        let engine =
            MailEngine::initialize_with_secrets(dir.path(), Arc::new(mail_core::MemorySecretStore::new()))
                .expect("初始化引擎");
        engine.mcp_set_enabled(true).expect("打开 MCP");
    }

    let mut child = spawn(dir.path());
    let mut reader = BufReader::new(child.stdout.take().expect("子进程标准输出"));
    write_line(
        &mut child,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "1999-01-01", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"} }
        }),
    );
    let response = read_line(&mut reader);
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("协议版本不匹配"),
        "应明确拒绝不认识的版本：{response}"
    );

    // 拒绝之后不允许再试探别的方法。
    write_line(
        &mut child,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );
    let response = read_line(&mut reader);
    assert!(
        response.get("error").is_some(),
        "被拒会话不应再提供工具：{response}"
    );

    drop(reader);
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn 工具清单没有发送类工具且搜索能返回审计落库() {
    let (dir, account_id, message_id) = seed_data_dir();
    {
        let engine =
            MailEngine::initialize_with_secrets(dir.path(), Arc::new(mail_core::MemorySecretStore::new()))
                .expect("初始化引擎");
        engine.mcp_set_enabled(true).expect("打开 MCP");
    }

    let mut child = spawn(dir.path());
    let mut reader = BufReader::new(child.stdout.take().expect("子进程标准输出"));

    write_line(
        &mut child,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"} }
        }),
    );
    let response = read_line(&mut reader);
    assert_eq!(response["result"]["protocolVersion"], "2025-06-18");

    write_line(
        &mut child,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );
    let response = read_line(&mut reader);
    let tools = response["result"]["tools"].as_array().expect("工具数组");
    let names: Vec<&str> = tools.iter().filter_map(|tool| tool["name"].as_str()).collect();
    assert!(names.contains(&"search_messages"));
    assert!(
        !names
            .iter()
            .any(|name| name.contains("send") || name.contains("export")),
        "不能提供发送 / 导出类工具：{names:?}"
    );
    assert!(
        !names.contains(&"create_draft"),
        "写工具开关没开，清单里不应出现 create_draft：{names:?}"
    );

    write_line(
        &mut child,
        &json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "search_messages", "arguments": { "query": "发票", "accountId": account_id } }
        }),
    );
    let response = read_line(&mut reader);
    let messages = response["result"]["structuredContent"]["messages"]
        .as_array()
        .expect("搜索结果数组");
    assert_eq!(messages.len(), 1, "应命中本机已同步邮件：{response}");
    assert_eq!(messages[0]["id"], json!(message_id), "要能拿到邮件编号");
    assert_eq!(messages[0]["subject"], json!("十月发票报销"));

    // 发送类工具不在清单里，直接调用也必须被拒。
    write_line(
        &mut child,
        &json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "send_email", "arguments": { "to": "x@example.com" } }
        }),
    );
    let response = read_line(&mut reader);
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("不在允许清单"), "发送类工具必须被拒：{response}");

    drop(reader);
    let _ = child.kill();
    let _ = child.wait();

    // 审计应落库：成功的 search_messages 与两次被拒调用都有记录，且不含正文。
    let engine =
        MailEngine::initialize_with_secrets(dir.path(), Arc::new(mail_core::MemorySecretStore::new()))
            .expect("初始化引擎");
    let audit = engine.mcp_audit(50).expect("审计");
    assert!(audit
        .iter()
        .any(|row| row.tool == "search_messages" && row.status == "ok"));
    assert!(audit
        .iter()
        .any(|row| row.tool == "send_email" && row.status == "rejected"));
    for row in &audit {
        assert_eq!(row.args_digest.len(), 64, "参数摘要应是 SHA-256");
        assert!(!row.args_digest.contains("发票"), "审计不能落参数原文");
    }
}

#[test]
fn 一键关闭后已握手的会话立刻被拒() {
    let (dir, account_id, _message_id) = seed_data_dir();
    {
        let engine =
            MailEngine::initialize_with_secrets(dir.path(), Arc::new(mail_core::MemorySecretStore::new()))
                .expect("初始化引擎");
        engine.mcp_set_enabled(true).expect("打开 MCP");
    }

    let mut child = spawn(dir.path());
    let mut reader = BufReader::new(child.stdout.take().expect("子进程标准输出"));
    write_line(
        &mut child,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"} }
        }),
    );
    let response = read_line(&mut reader);
    assert_eq!(response["result"]["protocolVersion"], "2025-03-26");

    // 应用侧一键关闭（另一条连接写同一个库）。
    {
        let engine =
            MailEngine::initialize_with_secrets(dir.path(), Arc::new(mail_core::MemorySecretStore::new()))
                .expect("初始化引擎");
        engine.mcp_set_enabled(false).expect("关闭 MCP");
    }

    write_line(
        &mut child,
        &json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "list_accounts", "arguments": { "accountId": account_id } }
        }),
    );
    let response = read_line(&mut reader);
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("已关闭"), "关闭后调用必须立刻被拒：{response}");

    drop(reader);
    let _ = child.kill();
    let _ = child.wait();
}
