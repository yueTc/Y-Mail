//! Wave 9 端到端验收测试（集成测试）：只用 `mail-core` 的公开接口，
//! 用假 IMAP / SMTP / AI 站点把关键验收场景整条跑通。
//!
//! 覆盖：账号连接自检、同步与统一收件箱、搜索、读信（安全渲染与远程图片拦截）、
//! 写信 / 发件队列、OAuth2 不退回明文、AI 默认关闭与逐次授权（译文只取一次）、
//! MCP 默认关闭 / 只读 / 审计 / 一键关闭。
//!
//! 真实邮箱、真实 AI 站点、真实 Ollama / DeepL、托盘通知点击、安装器界面这类
//! 没法在这里自动化的，见 docs/manual-acceptance-checklist.md。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use mail_core::{
    AiFunction, AiProviderInput, AiProviderKind, AiThinkingLevel, InboxQuery, MailEngine, McpDraftInput,
    McpError, McpRecipient, MemorySecretStore, NewOutbox, OutboxKind, OutboxState, SearchQuery,
};
use mail_domain::account::{AccountDraft, AccountProxyMode, AuthType, OAuthProvider, Security, ServerConfig};
use mail_domain::proxy::Secret;
use mail_domain::FolderKind;
use mail_store::NewMessage;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// 假服务器记下的 APPEND 内容：目标文件夹 + 原始 MIME。
type AppendedLog = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

const MAIL_DATE: &str = "Sat, 04 Oct 2026 08:00:00 +0800";
const INTERNAL_DATE: &str = "04-Oct-2026 08:00:00 +0800";

fn draft(email: &str, imap_port: u16, smtp_port: u16) -> AccountDraft {
    let server = |port: u16| ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        security: Security::Plain,
    };
    AccountDraft {
        display_name: email.to_string(),
        email: email.to_string(),
        auth_type: AuthType::Password,
        username: email.to_string(),
        imap: server(imap_port),
        smtp: server(smtp_port),
        proxy: AccountProxyMode::Direct,
        color: String::new(),
        enabled: true,
        oauth_provider: None,
        oauth_client_id: String::new(),
    }
}

/// 一条待同步的假邮件：信封元数据 + 原始 MIME 字节。
#[derive(Clone)]
struct Mail {
    uid: u32,
    subject: String,
    from_name: String,
    from_addr: String,
    message_id: String,
    internal_date: String,
    seen: bool,
    raw: Vec<u8>,
}

fn mail(uid: u32, subject: &str, html: &str) -> Mail {
    let raw = format!(
        "From: Alice <alice@example.com>\r\nTo: <me@example.com>\r\nSubject: {subject}\r\n\
         Message-ID: <m{uid}@example.com>\r\nDate: {MAIL_DATE}\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n"
    )
    .into_bytes();
    Mail {
        uid,
        subject: subject.to_string(),
        from_name: "Alice".to_string(),
        from_addr: "alice@example.com".to_string(),
        message_id: format!("m{uid}@example.com"),
        internal_date: INTERNAL_DATE.to_string(),
        seen: false,
        raw,
    }
}

#[derive(Clone)]
struct ImapConfig {
    uidvalidity: u32,
    mails: Vec<Mail>,
    accept_login: bool,
    accept_xoauth2: bool,
    advertise_idle: bool,
}

struct FakeImap {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
    appended: AppendedLog,
}

struct FakeSmtp {
    port: u16,
    received: Arc<Mutex<Vec<Vec<u8>>>>,
}

async fn start_imap(config: ImapConfig) -> FakeImap {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定假 IMAP");
    let port = listener.local_addr().expect("取地址").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let appended = Arc::new(Mutex::new(Vec::new()));
    let seen_server = seen.clone();
    let appended_server = appended.clone();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            let config = config.clone();
            let seen = seen_server.clone();
            let appended = appended_server.clone();
            tokio::spawn(async move {
                let _ = handle_imap(socket, config, seen, appended).await;
            });
        }
    });
    FakeImap { port, seen, appended }
}

async fn start_smtp(xoauth2_only: bool) -> FakeSmtp {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定假 SMTP");
    let port = listener.local_addr().expect("取地址").port();
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_server = received.clone();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            let received = received_server.clone();
            tokio::spawn(async move {
                let _ = handle_smtp(socket, xoauth2_only, received).await;
            });
        }
    });
    FakeSmtp { port, received }
}

async fn reply(reader: &mut BufReader<TcpStream>, text: &str) -> std::io::Result<()> {
    reader.get_mut().write_all(text.as_bytes()).await
}

fn inbox_uids(config: &ImapConfig) -> Vec<u32> {
    config.mails.iter().map(|mail| mail.uid).collect()
}

fn expand_uid_set(set: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for part in set.split(',') {
        match part.split_once(':') {
            Some((start, end)) => {
                if let (Ok(a), Ok(b)) = (start.parse::<u32>(), end.parse::<u32>()) {
                    for uid in a..=b {
                        out.push(uid);
                    }
                }
            }
            None => {
                if let Ok(uid) = part.parse::<u32>() {
                    out.push(uid);
                }
            }
        }
    }
    out
}

fn search_reply(tag: &str, uids: &[u32]) -> String {
    let list = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
    format!("* SEARCH {list}\r\n{tag} OK completed\r\n")
}

fn meta_line(mail: &Mail) -> String {
    let (mailbox, domain) = mail.from_addr.split_once('@').unwrap_or(("alice", "example.com"));
    let flags = if mail.seen { "\\Seen" } else { "" };
    format!(
        "* 1 FETCH (UID {uid} FLAGS ({flags}) INTERNALDATE \"{date}\" RFC822.SIZE {size} \
         ENVELOPE (\"{MAIL_DATE}\" \"{subject}\" ((\"{name}\" NIL \"{mailbox}\" \"{domain}\")) \
         NIL NIL ((\"Me\" NIL \"me\" \"example.com\")) NIL NIL NIL \"<{mid}>\"))\r\n",
        uid = mail.uid,
        flags = flags,
        date = mail.internal_date,
        size = mail.raw.len(),
        subject = mail.subject,
        name = mail.from_name,
        mailbox = mailbox,
        domain = domain,
        mid = mail.message_id,
    )
}

fn brace_size(rest: &str) -> usize {
    rest.rsplit('{')
        .next()
        .and_then(|value| value.trim_end_matches('}').trim().parse().ok())
        .unwrap_or(0)
}
async fn handle_imap(
    socket: TcpStream,
    config: ImapConfig,
    seen: Arc<Mutex<Vec<String>>>,
    appended: AppendedLog,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(socket);
    reply(&mut reader, "* OK ready\r\n").await?;
    let mut selected = "INBOX".to_string();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
        if trimmed.is_empty() {
            continue;
        }
        seen.lock().expect("锁").push(trimmed.clone());
        let (tag, rest) = match trimmed.split_once(' ') {
            Some((tag, rest)) => (tag.to_string(), rest.to_string()),
            None => (trimmed.clone(), String::new()),
        };
        let upper = rest.to_ascii_uppercase();
        if upper.starts_with("LOGIN") {
            if config.accept_login {
                reply(&mut reader, &format!("{tag} OK LOGIN completed\r\n")).await?;
            } else {
                reply(
                    &mut reader,
                    &format!("{tag} NO [AUTHENTICATIONFAILED] 登录被拒绝\r\n"),
                )
                .await?;
            }
        } else if upper.starts_with("AUTHENTICATE XOAUTH2") {
            if config.accept_xoauth2 {
                reply(&mut reader, &format!("{tag} OK AUTHENTICATE completed\r\n")).await?;
            } else {
                reply(
                    &mut reader,
                    &format!("+ \r\n{tag} NO [AUTHENTICATIONFAILED] 令牌被拒\r\n"),
                )
                .await?;
            }
        } else if upper.starts_with("CAPABILITY") {
            let caps = if config.advertise_idle {
                "IMAP4rev1 ID IDLE"
            } else {
                "IMAP4rev1 ID"
            };
            reply(
                &mut reader,
                &format!("* CAPABILITY {caps}\r\n{tag} OK CAPABILITY completed\r\n"),
            )
            .await?;
        } else if upper.starts_with("ID ") || upper == "ID" {
            reply(
                &mut reader,
                &format!("* ID (\"name\" \"EmMaster\")\r\n{tag} OK ID completed\r\n"),
            )
            .await?;
        } else if upper.starts_with("LIST") {
            let out = format!(
                "* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n\
                 * LIST (\\HasNoChildren) \"/\" \"Sent\"\r\n\
                 {tag} OK LIST completed\r\n"
            );
            reply(&mut reader, &out).await?;
        } else if upper.starts_with("SELECT") {
            selected = if upper.contains("SENT") {
                "Sent".to_string()
            } else {
                "INBOX".to_string()
            };
            let count = if selected == "INBOX" {
                config.mails.len()
            } else {
                0
            };
            let out = format!(
                "* {count} EXISTS\r\n* OK [UIDVALIDITY {}] ok\r\n* OK [UIDNEXT 100] ok\r\n\
                 * OK [UNSEEN 1] ok\r\n{tag} OK SELECT completed\r\n",
                config.uidvalidity
            );
            reply(&mut reader, &out).await?;
        } else if upper.starts_with("UID SEARCH SINCE") {
            let uids = if selected == "INBOX" {
                inbox_uids(&config)
            } else {
                Vec::new()
            };
            reply(&mut reader, &search_reply(&tag, &uids)).await?;
        } else if upper.starts_with("UID SEARCH UID") {
            let set = rest.split_whitespace().nth(3).unwrap_or_default();
            let mut uids = inbox_uids(&config);
            if let Some(from) = set.strip_suffix(":*") {
                let from: u32 = from.parse().unwrap_or(1);
                uids.retain(|uid| *uid >= from);
            } else {
                let bound: u32 = set
                    .split(':')
                    .nth(1)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
                uids.retain(|uid| *uid <= bound);
            }
            reply(&mut reader, &search_reply(&tag, &uids)).await?;
        } else if upper.starts_with("UID FETCH") {
            if upper.contains("BODY.PEEK[]") {
                let uid: u32 = rest
                    .split_whitespace()
                    .nth(2)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
                if let Some(mail) = config.mails.iter().find(|mail| mail.uid == uid) {
                    let head = format!("* 1 FETCH (UID {uid} BODY[] {{{}}}\r\n", mail.raw.len());
                    reader.get_mut().write_all(head.as_bytes()).await?;
                    reader.get_mut().write_all(&mail.raw).await?;
                    reader.get_mut().write_all(b")\r\n").await?;
                    reply(&mut reader, &format!("{tag} OK completed\r\n")).await?;
                } else {
                    reply(&mut reader, &format!("{tag} OK completed\r\n")).await?;
                }
            } else {
                let set = rest.split_whitespace().nth(2).unwrap_or_default();
                let mut out = String::new();
                for uid in expand_uid_set(set) {
                    if let Some(mail) = config.mails.iter().find(|mail| mail.uid == uid) {
                        out.push_str(&meta_line(mail));
                    }
                }
                out.push_str(&format!("{tag} OK completed\r\n"));
                reply(&mut reader, &out).await?;
            }
        } else if upper.starts_with("APPEND") {
            let size = brace_size(&rest);
            let folder = rest
                .split_once('"')
                .and_then(|(_, after)| after.split_once('"'))
                .map(|(name, _)| name.to_string())
                .unwrap_or_else(|| selected.clone());
            reply(&mut reader, "+ Ready for literal data\r\n").await?;
            let mut body = vec![0u8; size];
            reader.read_exact(&mut body).await?;
            let mut crlf = [0u8; 2];
            let _ = reader.read_exact(&mut crlf).await;
            appended.lock().expect("锁").push((folder, body));
            reply(&mut reader, &format!("{tag} OK [APPENDUID 1 1] completed\r\n")).await?;
        } else if upper.starts_with("LOGOUT") {
            reply(&mut reader, &format!("* BYE\r\n{tag} OK\r\n")).await?;
            return Ok(());
        } else if upper.starts_with("IDLE") {
            reply(&mut reader, "+ idling\r\n").await?;
            let mut done = String::new();
            let _ = reader.read_line(&mut done).await;
            reply(&mut reader, &format!("{tag} OK completed\r\n")).await?;
        } else {
            reply(&mut reader, &format!("{tag} OK completed\r\n")).await?;
        }
    }
}
async fn handle_smtp(
    socket: TcpStream,
    xoauth2_only: bool,
    received: Arc<Mutex<Vec<Vec<u8>>>>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(socket);
    reply(&mut reader, "220 smtp ready\r\n").await?;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
        let upper = trimmed.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            let caps = if xoauth2_only {
                "250-localhost\r\n250-AUTH XOAUTH2\r\n250 OK\r\n"
            } else {
                "250-localhost\r\n250-AUTH PLAIN LOGIN XOAUTH2\r\n250 OK\r\n"
            };
            reply(&mut reader, caps).await?;
        } else if upper.starts_with("AUTH ") {
            let accepted = if xoauth2_only {
                upper.starts_with("AUTH XOAUTH2")
            } else {
                true
            };
            if accepted {
                reply(&mut reader, "235 2.7.0 authenticated\r\n").await?;
            } else {
                reply(&mut reader, "535 5.7.8 bad credentials\r\n").await?;
            }
        } else if upper.starts_with("DATA") {
            reply(&mut reader, "354 go ahead\r\n").await?;
            let mut body = Vec::new();
            loop {
                let mut part = Vec::new();
                let read = reader.read_until(b'\n', &mut part).await?;
                if read == 0 {
                    break;
                }
                if part == b".\r\n" || part == b".\n" {
                    break;
                }
                body.extend_from_slice(&part);
            }
            received.lock().expect("锁").push(body);
            reply(&mut reader, "250 2.0.0 queued as TEST\r\n").await?;
        } else if upper.starts_with("MAIL FROM")
            || upper.starts_with("RCPT TO")
            || upper.starts_with("RSET")
            || upper.starts_with("NOOP")
        {
            reply(&mut reader, "250 ok\r\n").await?;
        } else if upper.starts_with("QUIT") {
            reply(&mut reader, "221 bye\r\n").await?;
            return Ok(());
        } else {
            reply(&mut reader, "250 ok\r\n").await?;
        }
    }
}

/// 假 OpenAI 兼容站点：回一个固定两段译文，并统计真实收到的请求数。
async fn start_ai_server() -> (u16, Arc<Mutex<usize>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定假 AI");
    let port = listener.local_addr().expect("取地址").port();
    let hits = Arc::new(Mutex::new(0usize));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            *counter.lock().expect("锁") += 1;
            let mut buf = vec![0u8; 16384];
            let _ = socket.read(&mut buf).await;
            let body = r#"{"choices":[{"message":{"role":"assistant","content":"[\"译一\",\"译二\"]"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    (port, hits)
}

/// 往本地库里直接塞一条邮件 + 正文，供不依赖网络的 AI / MCP 验收用。
fn seed_local_message(engine: &MailEngine, email: &str) -> i64 {
    let account_id = {
        let store = engine.store();
        let account_id = store.insert_account(&draft(email, 1, 1), None).expect("插账号").0;
        let folder_id = store
            .upsert_folder(account_id, "INBOX", "/", FolderKind::Inbox)
            .expect("插文件夹");
        let message = NewMessage {
            account_id,
            folder_id,
            uid: 1,
            message_id_header: "<seed@example.com>".to_string(),
            thread_key: "<seed@example.com>".to_string(),
            subject: "项目进度报告".to_string(),
            from_name: "Alice".to_string(),
            from_addr: "alice@example.com".to_string(),
            to_json: "[]".to_string(),
            cc_json: "[]".to_string(),
            date_utc: "2026-10-04T08:00:00Z".to_string(),
            size: 128,
            has_attachments: false,
            is_read: false,
            is_flagged: false,
            is_answered: false,
            is_draft: false,
        };
        store.insert_messages(&[message]).expect("插邮件");
        account_id
    };
    let page = engine.inbox_messages(&InboxQuery::default()).expect("查收件箱");
    let message_id = page
        .items
        .iter()
        .find(|item| item.account_id == account_id)
        .expect("应能查到刚插入的邮件")
        .id;
    engine
        .store()
        .save_message_body(
            message_id,
            Some("第一段内容\n第二段内容"),
            Some("<p>第一段内容</p><p>第二段内容</p>"),
        )
        .expect("写正文");
    message_id
}

async fn wait_for_inbox(engine: &MailEngine, expected: i64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(page) = engine.inbox_messages(&InboxQuery::default()) {
            if page.total >= expected {
                return;
            }
        }
        if tokio::time::Instant::now() > deadline {
            panic!("等待同步写入收件箱超时");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn data_dir_contains(dir: &std::path::Path, needle: &[u8]) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            if bytes.windows(needle.len()).any(|window| window == needle) {
                return true;
            }
        }
    }
    false
}

fn tracking_html() -> &'static str {
    "<html><body><p>第一段内容</p><p>项目进度报告正文</p>\
     <img src=\"https://track.example.com/pixel.gif\" onerror=\"alert(1)\">\
     <script>alert('x')</script></body></html>"
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 验收_账号自检同步统一收件箱搜索与安全读信() {
    let imap_a = start_imap(ImapConfig {
        uidvalidity: 11,
        mails: vec![mail(1, "项目进度报告", tracking_html())],
        accept_login: true,
        accept_xoauth2: true,
        advertise_idle: false,
    })
    .await;
    let smtp_a = start_smtp(false).await;
    let imap_b = start_imap(ImapConfig {
        uidvalidity: 22,
        mails: vec![mail(10, "发票通知", "<p>发票已开具</p>")],
        accept_login: true,
        accept_xoauth2: true,
        advertise_idle: false,
    })
    .await;
    let smtp_b = start_smtp(false).await;

    let dir = tempfile::tempdir().expect("临时目录");
    let secrets = Arc::new(MemorySecretStore::new());
    let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("初始化引擎");

    // 账号连接自检：不通过不许落库。
    let account_a = engine
        .create_account(
            &draft("alice@example.com", imap_a.port, smtp_a.port),
            &Secret::new("pw-a-123"),
        )
        .await
        .expect("账号 A 自检并保存");
    engine
        .create_account(
            &draft("bob@example.com", imap_b.port, smtp_b.port),
            &Secret::new("pw-b-456"),
        )
        .await
        .expect("账号 B 自检并保存");
    let report = engine
        .test_saved_account(account_a.id)
        .await
        .expect("已存账号再自检");
    assert_eq!(report.imap_folder_count, 2, "应看到 INBOX 与 Sent 两个文件夹");
    assert_eq!(report.smtp_mechanism, "PLAIN");

    // 同步：两个账号各拉一封，统一收件箱聚合。
    assert_eq!(engine.start_sync(None).expect("启动同步"), 2);
    wait_for_inbox(&engine, 2).await;
    engine.stop_sync(None).await;

    let page = engine.inbox_messages(&InboxQuery::default()).expect("统一收件箱");
    assert_eq!(page.total, 2);
    let subjects: Vec<&str> = page.items.iter().map(|item| item.subject.as_str()).collect();
    assert!(subjects.contains(&"项目进度报告"));
    assert!(subjects.contains(&"发票通知"));

    let summary = engine.inbox_account_summary().expect("账号汇总");
    assert_eq!(summary.len(), 2, "两个账号都要汇总");
    let unread_total: i64 = summary.iter().map(|item| item.unread_count).sum();
    assert_eq!(unread_total, 2, "两个账号各一条未读");

    // 搜索：命中已同步邮件。
    let hits = engine
        .search_messages(&SearchQuery::new("项目进度报告", None, 0, 50))
        .expect("本地搜索");
    assert!(hits.total >= 1, "应能搜到项目进度报告");
    let target = hits
        .items
        .iter()
        .find(|hit| hit.message.subject == "项目进度报告")
        .expect("命中目标邮件")
        .message
        .clone();

    // 安全渲染：脚本与事件属性清掉，远程图片默认拦下。
    let body = engine
        .get_message_body(target.id, false)
        .await
        .expect("联网读正文");
    assert_eq!(body.blocked_remote_images, 1, "应拦下一张跟踪像素");
    let html = body.html.expect("应有 HTML 正文");
    assert!(!html.contains("<script"), "脚本标签应被清掉：{html}");
    assert!(!html.contains("onerror"), "事件属性应被清掉：{html}");
    assert!(
        !html.contains("<img src="),
        "默认不放行远程图片（不给真实 src）：{html}"
    );
    assert!(
        html.contains("data-em-original-src=\"https://track.example.com/pixel.gif\""),
        "应留下被拦占位：{html}"
    );

    // 用户放行后，本次返回才还原真实地址。
    let allowed = engine
        .get_message_body(target.id, true)
        .await
        .expect("放行读正文");
    let allowed_html = allowed.html.expect("应有 HTML");
    assert!(
        allowed_html.contains("https://track.example.com/pixel.gif"),
        "放行后应还原真实地址"
    );

    // 凭据只进保险箱，任何数据文件都不许出现明文授权码。
    assert!(secrets.contains(&account_a.credential_key.clone().expect("凭据键")));
    drop(engine);
    assert!(!data_dir_contains(dir.path(), b"pw-a-123"), "授权码不得明文落盘");
    assert!(!data_dir_contains(dir.path(), b"pw-b-456"), "授权码不得明文落盘");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 验收_重启后断点续传不重复拉取已入库邮件() {
    let imap = start_imap(ImapConfig {
        uidvalidity: 55,
        mails: vec![
            mail(1, "最早的邮件", "<p>一</p>"),
            mail(2, "稍早的邮件", "<p>二</p>"),
            mail(3, "最近的邮件", "<p>三</p>"),
        ],
        accept_login: true,
        accept_xoauth2: true,
        advertise_idle: false,
    })
    .await;
    let smtp = start_smtp(false).await;
    let dir = tempfile::tempdir().expect("临时目录");
    // 同一份保险箱跨两次启动，模拟应用重启（凭据仍在系统保险箱里）。
    let secrets = Arc::new(MemorySecretStore::new());

    let account_id = {
        let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("首次启动");
        let account = engine
            .create_account(
                &draft("resume@example.com", imap.port, smtp.port),
                &Secret::new("pw-resume"),
            )
            .await
            .expect("建账号");
        assert_eq!(engine.start_sync(Some(account.id.0)).expect("启动同步"), 1);
        wait_for_inbox(&engine, 3).await;
        engine.stop_sync(Some(account.id.0)).await;

        // 快照把断点落在最小 UID 上，重启后能从它接着走。
        let folder_id = {
            let store = engine.store();
            let folder = store
                .list_folders(account.id.0)
                .expect("列文件夹")
                .into_iter()
                .find(|folder| folder.full_path == "INBOX")
                .expect("应有 INBOX");
            assert_eq!(folder.synced_min_uid, Some(1), "断点应落在最小 UID");
            folder.id
        };
        assert!(folder_id > 0);
        account.id.0
    };

    // 模拟应用关闭后重启：换一个引擎实例，指向同一个数据目录与保险箱。
    let fetched_before = imap.seen.lock().expect("锁").len();
    let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("重启引擎");
    assert_eq!(engine.list_accounts().expect("账号应还在").len(), 1);
    assert_eq!(
        engine
            .inbox_messages(&InboxQuery::default())
            .expect("重启后可离线读")
            .total,
        3,
        "重启后本地邮件应还在，不依赖网络"
    );

    assert_eq!(engine.start_sync(Some(account_id)).expect("重启后继续同步"), 1);
    tokio::time::sleep(Duration::from_millis(400)).await;
    engine.stop_sync(Some(account_id)).await;

    let page = engine.inbox_messages(&InboxQuery::default()).expect("收件箱");
    assert_eq!(page.total, 3, "重启同步不得重复入库");
    let uids: Vec<u32> = {
        let mut list: Vec<u32> = page.items.iter().map(|item| item.uid).collect();
        list.sort_unstable();
        list
    };
    assert_eq!(uids, vec![1, 2, 3], "UID 应保持唯一且完整");

    // 第二轮没有新邮件：不应再抓任何已入库的邮件元数据。
    let after: Vec<String> = imap
        .seen
        .lock()
        .expect("锁")
        .iter()
        .skip(fetched_before)
        .cloned()
        .collect();
    assert!(
        !after
            .iter()
            .any(|line| line.to_ascii_uppercase().contains("UID FETCH")),
        "重启后不该重拉已入库 UID：{after:?}"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 验收_写信入队投递成功并追加已发送() {
    let imap = start_imap(ImapConfig {
        uidvalidity: 33,
        mails: vec![mail(1, "收到的信", "<p>hello</p>")],
        accept_login: true,
        accept_xoauth2: true,
        advertise_idle: false,
    })
    .await;
    let smtp = start_smtp(false).await;
    let dir = tempfile::tempdir().expect("临时目录");
    let engine = MailEngine::initialize_with_secrets(dir.path(), Arc::new(MemorySecretStore::new()))
        .expect("初始化引擎");
    let account = engine
        .create_account(
            &draft("sender@example.com", imap.port, smtp.port),
            &Secret::new("pw-send"),
        )
        .await
        .expect("建账号");

    // 先同步一次，把 INBOX / Sent 文件夹落到本地（追加已发送要用到）。
    assert_eq!(engine.start_sync(Some(account.id.0)).expect("启动同步"), 1);
    wait_for_inbox(&engine, 1).await;
    engine.stop_sync(Some(account.id.0)).await;

    let draft_id = engine
        .save_draft(
            None,
            &NewOutbox {
                account_id: account.id.0,
                kind: OutboxKind::New,
                to_json: r#"[{"name":"Bob","address":"bob@example.com"}]"#.to_string(),
                cc_json: "[]".to_string(),
                bcc_json: "[]".to_string(),
                subject: "来自验收测试的信".to_string(),
                body_html: "<p>正文</p>".to_string(),
                body_text: "正文".to_string(),
                in_reply_to: None,
                references_json: "[]".to_string(),
                attachments_json: "[]".to_string(),
            },
        )
        .expect("存草稿");
    assert!(engine.enqueue_outbox(draft_id).expect("入队"));

    let outcome = engine.send_outbox().await.expect("发送队列");
    assert_eq!(outcome.attempted, 1);
    assert_eq!(outcome.sent, 1, "应投递成功：{:?}", outcome.errors);

    let stored = engine.get_outbox(draft_id).expect("查队列").expect("记录应还在");
    assert_eq!(stored.state, OutboxState::Sent, "发送成功后应标已发送");

    let delivered = smtp.received.lock().expect("锁").clone();
    assert!(!delivered.is_empty(), "SMTP 应收到投递内容");
    let delivered_text = String::from_utf8_lossy(&delivered[0]).to_string();
    assert!(
        delivered_text.contains("Subject: =?UTF-8?B?"),
        "投递内容应带主题（中文主题按 RFC 2047 编码）"
    );
    let subject_line = delivered_text
        .lines()
        .find(|line| line.starts_with("Subject: "))
        .expect("应有主题行");
    let encoded = subject_line
        .trim_start_matches("Subject: =?UTF-8?B?")
        .trim_end_matches("?=");
    let decoded = STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .expect("主题应能解回 UTF-8");
    assert_eq!(decoded, "来自验收测试的信");

    let appended = imap.appended.lock().expect("锁").clone();
    assert!(!appended.is_empty(), "应追加一份到已发送文件夹");
    assert_eq!(appended[0].0, "Sent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 验收_oauth2账号只走xoauth2不退回明文() {
    let imap = start_imap(ImapConfig {
        uidvalidity: 44,
        mails: Vec::new(),
        accept_login: false,
        accept_xoauth2: true,
        advertise_idle: false,
    })
    .await;
    let smtp = start_smtp(true).await;
    let dir = tempfile::tempdir().expect("临时目录");
    let secrets = Arc::new(MemorySecretStore::new());
    let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("初始化引擎");

    let mut oauth = draft("oauth.user@gmail.com", imap.port, smtp.port);
    oauth.auth_type = AuthType::OAuth2;
    oauth.oauth_provider = Some(OAuthProvider::Gmail);
    oauth.oauth_client_id = "client-id-123".to_string();
    let token = "oauth-access-token-abc";
    let account = engine
        .create_account(&oauth, &Secret::new(token))
        .await
        .expect("OAuth2 自检应走 XOAUTH2 通过");

    let seen = imap.seen.lock().expect("锁").clone();
    assert!(
        seen.iter().any(|line| line.contains("AUTHENTICATE XOAUTH2")),
        "IMAP 应使用 XOAUTH2：{seen:?}"
    );
    assert!(
        !seen
            .iter()
            .any(|line| line.to_ascii_uppercase().contains("LOGIN")),
        "不得退回明文登录：{seen:?}"
    );

    assert!(
        secrets
            .plain(&account.credential_key.clone().expect("凭据键"))
            .is_some(),
        "令牌应写进保险箱"
    );
    drop(engine);
    assert!(
        !data_dir_contains(dir.path(), token.as_bytes()),
        "访问令牌不得明文落盘"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 验收_ai默认关闭逐次授权且译文只取一次() {
    let dir = tempfile::tempdir().expect("临时目录");
    let secrets = Arc::new(MemorySecretStore::new());
    let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("初始化引擎");
    let message_id = seed_local_message(&engine, "ai@example.com");

    // 默认关闭：连预览都不给令牌。
    let err = engine
        .ai_authorization_preview(AiFunction::Translate, Some(message_id), "英语", None)
        .expect_err("AI 默认关闭应拒绝");
    assert!(err.to_string().contains("AI 功能还没开启"), "{err}");

    let (port, hits) = start_ai_server().await;
    let cdkey = "cdkey-secret-7788";
    engine
        .save_ai_provider(
            &AiProviderInput {
                id: None,
                label: "本地兼容站点".to_string(),
                kind: AiProviderKind::OpenAiCompatible,
                base_url: format!("http://127.0.0.1:{port}/v1"),
                default_model: "m1".to_string(),
                models: vec!["m1".to_string()],
                thinking_level: AiThinkingLevel::Off,
                enabled: true,
            },
            Some(&Secret::new(cdkey)),
        )
        .expect("保存并启用站点");

    let preview = engine
        .ai_authorization_preview(AiFunction::Translate, Some(message_id), "英语", None)
        .expect("外发前预览");
    assert!(!preview.from_cache);
    assert!(preview.local, "本机站点应标成本地");
    assert_eq!(preview.host, "127.0.0.1");
    assert_eq!(preview.model, "m1");
    assert!(
        !preview.authorization_token.is_empty(),
        "外发前必须拿到一次性授权令牌"
    );

    // 没有令牌不许外发。
    let err = engine
        .translate_message(message_id, "英语", "")
        .await
        .expect_err("没有授权令牌应被拒");
    assert!(
        matches!(err, mail_core::EngineError::AiAuthorizationRequired),
        "{err}"
    );

    let translated = engine
        .translate_message(message_id, "英语", &preview.authorization_token)
        .await
        .expect("带令牌应成功");
    assert!(!translated.from_cache);
    assert_eq!(translated.original.len(), 2);
    assert_eq!(
        translated.translated,
        vec!["译一".to_string(), "译二".to_string()]
    );
    assert_eq!(*hits.lock().expect("锁"), 1, "只应真的调用一次模型");

    // 换显示模式（对照 / 行内 / 直接）共用这份段落对齐译文，不再调用模型。
    let cached = engine
        .translate_message(message_id, "英语", "")
        .await
        .expect("缓存命中不需要新令牌");
    assert!(cached.from_cache, "第二次应命中本地缓存");
    assert_eq!(cached.translated, translated.translated);
    assert_eq!(*hits.lock().expect("锁"), 1, "切换模式不得重复调用模型");

    let again = engine
        .ai_authorization_preview(AiFunction::Translate, Some(message_id), "英语", None)
        .expect("缓存后预览");
    assert!(again.from_cache);
    assert!(again.authorization_token.is_empty(), "缓存命中不发令牌");

    let audit = engine.list_ai_audit(20).expect("AI 审计");
    assert!(!audit.is_empty(), "外发应留审计");
    let dump = format!("{audit:?}");
    assert!(!dump.contains("第一段内容"), "审计不得带正文");

    drop(engine);
    assert!(
        !data_dir_contains(dir.path(), cdkey.as_bytes()),
        "CDKey 不得明文落盘"
    );
}

#[test]
fn 验收_mcp默认关闭只读审计一键关闭() {
    let dir = tempfile::tempdir().expect("临时目录");
    let engine = MailEngine::initialize_with_secrets(dir.path(), Arc::new(MemorySecretStore::new()))
        .expect("初始化引擎");
    let _message_id = seed_local_message(&engine, "mcp@example.com");

    // 默认关闭，连接不可用，而且被拒也留审计。
    let status = engine.mcp_status().expect("MCP 状态");
    assert!(!status.enabled, "MCP 默认关闭");
    assert!(!status.write_tools_enabled, "写工具默认关闭");
    let err = engine
        .mcp_search_messages("项目", None, 0, 10)
        .expect_err("默认关闭应拒绝");
    assert!(matches!(err, McpError::Disabled));
    assert_eq!(engine.mcp_audit_count().expect("审计条数"), 1, "被拒也留审计");

    // 打开只读：能搜到本机邮件，且带不可信标注。
    let status = engine.mcp_set_enabled(true).expect("打开总开关");
    assert!(
        status.enabled && !status.write_tools_enabled,
        "总开关不等于写开关"
    );
    let page = engine
        .mcp_search_messages("项目进度报告", None, 0, 10)
        .expect("只读搜索");
    assert!(page.total >= 1);
    assert_eq!(page.untrusted_notice, mail_core::MCP_UNTRUSTED_NOTICE);
    assert_eq!(engine.mcp_audit_count().expect("审计条数"), 2);

    // 工具清单里没有发送类工具。
    let tools = engine.mcp_tools().expect("工具清单");
    assert!(
        tools
            .iter()
            .any(|tool| tool.name == "search_messages" && tool.read_only),
        "应有只读搜索工具"
    );
    assert!(
        !tools
            .iter()
            .any(|tool| tool.name == "send_email" || tool.name == "export_all"),
        "v1 不得有发送 / 批量导出工具"
    );

    // 写工具（建草稿）默认关闭。
    let err = engine
        .mcp_create_draft(&McpDraftInput {
            account_id: 1,
            to: vec![McpRecipient {
                name: "Bob".to_string(),
                address: "bob@example.com".to_string(),
            }],
            cc: Vec::new(),
            subject: "草稿".to_string(),
            body_text: "内容".to_string(),
            in_reply_to: None,
        })
        .expect_err("写工具默认关闭");
    assert!(matches!(err, McpError::WriteToolsDisabled));

    // 一键关闭立即生效，不能绕过。
    let status = engine.mcp_set_enabled(false).expect("关闭总开关");
    assert!(!status.enabled);
    let err = engine
        .mcp_search_messages("项目", None, 0, 10)
        .expect_err("关闭后应立刻拒绝");
    assert!(matches!(err, McpError::Disabled));
}
