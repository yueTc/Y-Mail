//! 同步引擎的端到端测试：用假 IMAP 服务器跑通快照、增量、断点续传、
//! UIDVALIDITY 重建、退避重试、失败隔离与取消。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mail_domain::account::{AccountDraft, AccountProxyMode, AuthType, Security, ServerConfig};
use mail_domain::proxy::Secret;
use mail_domain::FolderKind;
use mail_imap::{ClientConfig, ImapClient};
use mail_store::NewMessage;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::engine::MailEngine;
use crate::secrets::{MemorySecretStore, SecretStore};

use super::fetcher::sync_folder;
use super::state::{AccountSyncStatus, CancelFlag, SyncConfig, SyncService, SyncState};
use super::worker::{run, WorkerContext};

/// 假服务器每连接一次的参数。
struct Script {
    /// SELECT 时报告的 UIDVALIDITY。
    uidvalidity: u32,
    /// SINCE 搜索（快照与补齐下界）命中的 UID。
    snapshot_uids: Vec<u32>,
    /// `UID n:*` 增量搜索命中的 UID。
    incremental_uids: Vec<u32>,
    /// `UID 1:n` 补齐搜索命中的 UID。
    older_uids: Vec<u32>,
}

impl Script {
    /// 只关心快照的最小脚本。
    fn snapshot(uidvalidity: u32, snapshot_uids: Vec<u32>) -> Self {
        Self {
            uidvalidity,
            snapshot_uids,
            incremental_uids: Vec::new(),
            older_uids: Vec::new(),
        }
    }
}

fn draft(port: u16) -> AccountDraft {
    draft_for(port, "tester@example.com")
}

fn draft_for(port: u16, email: &str) -> AccountDraft {
    let server = ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        security: Security::Plain,
    };
    AccountDraft {
        display_name: "测试账号".to_string(),
        email: email.to_string(),
        auth_type: AuthType::Password,
        username: email.to_string(),
        imap: server.clone(),
        smtp: server,
        proxy: AccountProxyMode::Direct,
        color: String::new(),
        enabled: true,
        oauth_provider: None,
        oauth_client_id: String::new(),
    }
}

fn client_config(port: u16) -> ClientConfig {
    ClientConfig {
        host: "127.0.0.1".to_string(),
        port,
        security: Security::Plain,
        username: "tester@example.com".to_string(),
        auth: mail_domain::auth::AuthMaterial::password("pw"),
        timeout: Duration::from_secs(5),
    }
}

async fn read_line(reader: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    let _ = reader.read_line(&mut line).await;
    line.trim_end().to_string()
}

async fn write(reader: &mut BufReader<TcpStream>, text: &str) {
    reader
        .get_mut()
        .write_all(text.as_bytes())
        .await
        .expect("写应答失败");
}

/// 把 `1:3,5` 这种 UID 集合展开成列表。
fn expand(set: &str) -> Vec<u32> {
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

/// 一条 FETCH 应答行（纯 ASCII，避免字面量编码差异）。
fn fetch_line(uid: u32) -> String {
    format!(
        "* 1 FETCH (UID {uid} FLAGS (\\Seen) INTERNALDATE \"04-Oct-2026 08:00:00 +0800\" \
         RFC822.SIZE 100 ENVELOPE (\"Sat, 04 Oct 2026 08:00:00 +0800\" \"subject {uid}\" \
         NIL NIL NIL NIL NIL NIL NIL \"<m{uid}@x>\"))\r\n"
    )
}

/// 假 IMAP 服务器：覆盖登录、列文件夹、选择、搜索与抓取；只接受一条连接。
/// 起一个「能连上但立刻断开」的地址，模拟始终连不上的服务器。
async fn dead_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let port = listener.local_addr().expect("地址").port();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            drop(socket);
        }
    });
    port
}

async fn serve(listener: TcpListener, script: Script) {
    serve_many(listener, vec![script]).await;
}

/// 按顺序接受多条连接，每条连接用一份脚本（模拟断线重连）。
async fn serve_many(listener: TcpListener, scripts: Vec<Script>) {
    for script in scripts {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        handle_connection(socket, script).await;
    }
}

async fn handle_connection(socket: TcpStream, script: Script) {
    let mut reader = BufReader::new(socket);
    write(&mut reader, "* OK ready\r\n").await;
    loop {
        let line = read_line(&mut reader).await;
        if line.is_empty() {
            return;
        }
        let (tag, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        let rest = rest.to_string();
        let upper = rest.to_ascii_uppercase();
        if upper.starts_with("LOGIN") {
            write(&mut reader, &format!("{tag} OK LOGIN completed\r\n")).await;
        } else if upper.starts_with("CAPABILITY") {
            write(
                &mut reader,
                &format!("* CAPABILITY IMAP4rev1 ID\r\n{tag} OK completed\r\n"),
            )
            .await;
        } else if upper.starts_with("ID ") {
            write(&mut reader, &format!("{tag} OK\r\n")).await;
        } else if upper.starts_with("LIST") {
            write(
                &mut reader,
                &format!("* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n{tag} OK completed\r\n"),
            )
            .await;
        } else if upper.starts_with("SELECT") {
            let exists = script.snapshot_uids.len();
            write(
                &mut reader,
                &format!(
                    "* {exists} EXISTS\r\n* OK [UIDVALIDITY {}] ok\r\n* OK [UIDNEXT 100] ok\r\n\
                     * OK [UNSEEN 1] ok\r\n{tag} OK completed\r\n",
                    script.uidvalidity
                ),
            )
            .await;
        } else if upper.starts_with("UID SEARCH SINCE") {
            let uids: Vec<String> = script.snapshot_uids.iter().map(u32::to_string).collect();
            write(
                &mut reader,
                &format!("* SEARCH {}\r\n{tag} OK completed\r\n", uids.join(" ")),
            )
            .await;
        } else if upper.starts_with("UID SEARCH UID") {
            let set = rest.split_whitespace().nth(3).unwrap_or_default();
            let uids: Vec<u32> = if let Some(from) = set.strip_suffix(":*") {
                let from = from.parse::<u32>().unwrap_or(1);
                script
                    .incremental_uids
                    .iter()
                    .copied()
                    .filter(|uid| *uid >= from)
                    .collect()
            } else {
                let bound = set
                    .split(':')
                    .nth(1)
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(0);
                script
                    .older_uids
                    .iter()
                    .copied()
                    .filter(|uid| *uid <= bound)
                    .collect()
            };
            let text: Vec<String> = uids.iter().map(u32::to_string).collect();
            write(
                &mut reader,
                &format!("* SEARCH {}\r\n{tag} OK completed\r\n", text.join(" ")),
            )
            .await;
        } else if upper.starts_with("UID FETCH") {
            let set = rest.split_whitespace().nth(2).unwrap_or_default().to_string();
            let mut out = String::new();
            for uid in expand(&set) {
                out.push_str(&fetch_line(uid));
            }
            out.push_str(&format!("{tag} OK completed\r\n"));
            write(&mut reader, &out).await;
        } else {
            write(&mut reader, &format!("{tag} OK completed\r\n")).await;
        }
    }
}

/// 直接调 `sync_folder` 用的最小上下文（不带真实凭据）。
fn context(engine: &MailEngine, account_id: i64) -> Arc<WorkerContext> {
    Arc::new(WorkerContext {
        account_id,
        store: engine.store.clone(),
        secrets: Arc::new(MemorySecretStore::new()),
        config: SyncConfig::default(),
        status: Arc::new(Mutex::new(AccountSyncStatus::new(
            account_id,
            "tester@example.com",
        ))),
        cancel: Arc::new(CancelFlag::new()),
    })
}

/// 跑工作线程用的上下文：可注入凭据与节流参数。
fn worker_ctx(
    engine: &MailEngine,
    account_id: i64,
    secrets: Arc<dyn SecretStore>,
    config: SyncConfig,
) -> Arc<WorkerContext> {
    Arc::new(WorkerContext {
        account_id,
        store: engine.store.clone(),
        secrets,
        config,
        status: Arc::new(Mutex::new(AccountSyncStatus::new(
            account_id,
            "tester@example.com",
        ))),
        cancel: Arc::new(CancelFlag::new()),
    })
}

/// 起一个临时引擎，并给账号塞一条内存凭据。
fn engine_with_account(
    email: &str,
    port: u16,
    key: Option<&str>,
) -> (MailEngine, Arc<MemorySecretStore>, i64) {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.keep();
    let secrets = Arc::new(MemorySecretStore::new());
    if let Some(key) = key {
        secrets.set(key, &Secret::new("pw")).expect("写凭据");
    }
    let engine = MailEngine::initialize_with_secrets(&path, secrets.clone()).expect("初始化引擎");
    let account_id = {
        let store = engine.store();
        store
            .insert_account(&draft_for(port, email), key)
            .expect("建账号")
            .0
    };
    (engine, secrets, account_id)
}

#[tokio::test]
async fn 快照把邮件写进本地库并留下断点() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    tokio::spawn(serve(listener, Script::snapshot(42, vec![1, 2, 3])));

    let dir = tempfile::tempdir().expect("临时目录");
    let engine = MailEngine::initialize_with_secrets(dir.path(), Arc::new(MemorySecretStore::new()))
        .expect("初始化引擎");
    let account_id = {
        let store = engine.store();
        store.insert_account(&draft(addr.port()), None).expect("建账号").0
    };

    let mut client = ImapClient::connect(&client_config(addr.port()), None)
        .await
        .expect("连接");
    let folders = client.list_folders().await.expect("列文件夹");
    let ctx = context(&engine, account_id);

    sync_folder(&ctx, &mut client, &folders[0])
        .await
        .expect("同步收件箱");

    let store = engine.store();
    let folder = store
        .get_folder(account_id, "INBOX")
        .expect("查询")
        .expect("文件夹应存在");
    assert_eq!(folder.kind, FolderKind::Inbox);
    assert_eq!(folder.uidvalidity, Some(42));
    assert_eq!(store.count_folder_messages(folder.id).expect("计数"), 3);
    assert_eq!(folder.synced_min_uid, Some(1), "断点应落在快照最小 UID");
}

#[tokio::test]
async fn uidvalidity变化会清空重建() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    tokio::spawn(serve(listener, Script::snapshot(99, vec![7, 8])));

    let dir = tempfile::tempdir().expect("临时目录");
    let engine = MailEngine::initialize_with_secrets(dir.path(), Arc::new(MemorySecretStore::new()))
        .expect("初始化引擎");
    let account_id = {
        let store = engine.store();
        let id = store.insert_account(&draft(addr.port()), None).expect("建账号").0;
        let folder_id = store
            .upsert_folder(id, "INBOX", "/", FolderKind::Inbox)
            .expect("建文件夹");
        store
            .update_folder_select(folder_id, Some(42), Some(9), 0)
            .expect("旧选择");
        store
            .set_folder_synced_min_uid(folder_id, Some(1))
            .expect("旧断点");
        let messages = vec![NewMessage {
            account_id: id,
            folder_id,
            uid: 1,
            message_id_header: "<old@x>".to_string(),
            thread_key: "旧主题".to_string(),
            subject: "旧主题".to_string(),
            from_name: String::new(),
            from_addr: "old@example.com".to_string(),
            to_json: "[]".to_string(),
            cc_json: "[]".to_string(),
            date_utc: "2026-10-04T00:00:00Z".to_string(),
            size: 10,
            has_attachments: false,
            is_read: false,
            is_flagged: false,
            is_answered: false,
            is_draft: false,
        }];
        store.insert_messages(&messages).expect("写旧邮件");
        id
    };

    let mut client = ImapClient::connect(&client_config(addr.port()), None)
        .await
        .expect("连接");
    let folders = client.list_folders().await.expect("列文件夹");
    let ctx = context(&engine, account_id);

    sync_folder(&ctx, &mut client, &folders[0])
        .await
        .expect("同步收件箱");

    let store = engine.store();
    let folder = store
        .get_folder(account_id, "INBOX")
        .expect("查询")
        .expect("文件夹应存在");
    assert_eq!(folder.uidvalidity, Some(99), "UIDVALIDITY 应更新");
    assert_eq!(store.count_folder_messages(folder.id).expect("计数"), 2);
    assert_eq!(store.max_message_uid(folder.id).expect("最大"), Some(8));
    assert_eq!(store.min_message_uid(folder.id).expect("最小"), Some(7));
}

#[tokio::test]
async fn 断点续传不会重拉已入库邮件并接着补齐() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    tokio::spawn(serve_many(
        listener,
        vec![
            Script::snapshot(42, vec![10, 11, 12]),
            Script {
                uidvalidity: 42,
                snapshot_uids: (1..=12).collect(),
                incremental_uids: vec![13],
                older_uids: (1..=9).collect(),
            },
        ],
    ));

    let dir = tempfile::tempdir().expect("临时目录");
    let engine = MailEngine::initialize_with_secrets(dir.path(), Arc::new(MemorySecretStore::new()))
        .expect("初始化引擎");
    let account_id = {
        let store = engine.store();
        store.insert_account(&draft(addr.port()), None).expect("建账号").0
    };
    let ctx = context(&engine, account_id);

    // 第一次连接：只建最近快照 10..12，断点停在 10。
    let mut first = ImapClient::connect(&client_config(addr.port()), None)
        .await
        .expect("连接");
    let folders = first.list_folders().await.expect("列文件夹");
    sync_folder(&ctx, &mut first, &folders[0])
        .await
        .expect("首次同步");
    drop(first);
    {
        let store = engine.store();
        let folder = store
            .get_folder(account_id, "INBOX")
            .expect("查询")
            .expect("文件夹应存在");
        assert_eq!(store.count_folder_messages(folder.id).expect("计数"), 3);
        assert_eq!(folder.synced_min_uid, Some(10), "断点应停在快照最小 UID");
    }

    // 第二次连接：增量只拉 13，再从断点 10 往下补齐到 1。
    let mut second = ImapClient::connect(&client_config(addr.port()), None)
        .await
        .expect("重连");
    let folders = second.list_folders().await.expect("列文件夹");
    sync_folder(&ctx, &mut second, &folders[0])
        .await
        .expect("断点续传");

    let store = engine.store();
    let folder = store
        .get_folder(account_id, "INBOX")
        .expect("查询")
        .expect("文件夹应存在");
    assert_eq!(
        store.count_folder_messages(folder.id).expect("计数"),
        13,
        "已入库的 10..12 不应重复拉取"
    );
    assert_eq!(store.min_message_uid(folder.id).expect("最小"), Some(1));
    assert_eq!(store.max_message_uid(folder.id).expect("最大"), Some(13));
    assert_eq!(folder.synced_min_uid, Some(1), "补齐应把断点推进到范围下界");
}

#[tokio::test]
async fn 连不上会退避重试并最终连上() {
    // 先占一个端口再释放，保证一开始连不上。
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let port = probe.local_addr().expect("地址").port();
    drop(probe);

    // 稍后在同一端口起假服务器，模拟「短暂不可达后恢复」。
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        let listener = loop {
            match TcpListener::bind(("127.0.0.1", port)).await {
                Ok(listener) => break listener,
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        };
        serve(listener, Script::snapshot(7, vec![1, 2])).await;
    });

    let (_engine, secrets, account_id) = engine_with_account("retry@example.com", port, Some("acct"));
    let engine = _engine;
    let ctx = worker_ctx(
        &engine,
        account_id,
        secrets,
        SyncConfig {
            backoff_base: Duration::from_millis(10),
            backoff_cap: Duration::from_millis(20),
            max_retries: 20,
            poll_interval: Duration::from_millis(20),
            slow_poll_interval: Duration::from_millis(20),
            ..SyncConfig::default()
        },
    );
    let handle = tokio::spawn(run(ctx.clone()));

    let mut synced = false;
    for _ in 0..150 {
        let count = {
            let store = engine.store();
            store.count_account_messages(account_id).expect("计数")
        };
        if count >= 2 {
            synced = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(synced, "退避重试之后应能连上并同步");

    ctx.cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("取消后应退出")
        .expect("工作线程");
    assert_eq!(
        ctx.status.lock().expect("状态").state,
        SyncState::Stopped,
        "线程收尾应落到「已停止」"
    );
}

#[tokio::test]
async fn 一个账号连不上不影响另一个账号() {
    let good = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let good_port = good.local_addr().expect("地址").port();
    tokio::spawn(serve(good, Script::snapshot(5, vec![1, 2])));

    // 坏账号指向一个「能连上但立刻断开」的端口，避免端口复用带来的偶发。
    let bad_port = dead_port().await;

    let dir = tempfile::tempdir().expect("临时目录");
    let secrets = Arc::new(MemorySecretStore::new());
    secrets.set("good", &Secret::new("pw")).expect("写凭据");
    secrets.set("bad", &Secret::new("pw")).expect("写凭据");
    let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("初始化引擎");
    let (good_id, bad_id) = {
        let store = engine.store();
        let good_id = store
            .insert_account(&draft_for(good_port, "good@example.com"), Some("good"))
            .expect("建好账号")
            .0;
        let bad_id = store
            .insert_account(&draft_for(bad_port, "bad@example.com"), Some("bad"))
            .expect("建坏账号")
            .0;
        (good_id, bad_id)
    };

    let service = Arc::new(SyncService::new(
        engine.store.clone(),
        secrets.clone(),
        SyncConfig {
            backoff_base: Duration::from_millis(10),
            backoff_cap: Duration::from_millis(10),
            max_retries: 1,
            poll_interval: Duration::from_millis(20),
            slow_poll_interval: Duration::from_millis(20),
            ..SyncConfig::default()
        },
    ));
    assert!(service.start(good_id).expect("启动好账号"));
    assert!(service.start(bad_id).expect("启动坏账号"));

    let mut good_ok = false;
    let mut bad_error = false;
    for _ in 0..200 {
        if !good_ok {
            let count = {
                let store = engine.store();
                store.count_account_messages(good_id).expect("计数")
            };
            good_ok = count >= 2;
        }
        if !bad_error {
            bad_error = service
                .status_of(bad_id)
                .map(|status| status.state == SyncState::Error)
                .unwrap_or(false);
        }
        if good_ok && bad_error {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(good_ok, "好账号应正常同步");
    assert!(bad_error, "坏账号应进入出错状态");

    service.stop(None).await;
    let store = engine.store();
    assert_eq!(
        store.count_account_messages(good_id).expect("计数"),
        2,
        "坏账号不应影响好账号已入库的数据"
    );
}

#[tokio::test]
async fn 取消让等待立刻返回并留下已停止状态() {
    // 等待被取消后必须马上醒，而不是睡满整段时间。
    let cancel = Arc::new(CancelFlag::new());
    let waiter = cancel.clone();
    let sleeping = tokio::spawn(async move { waiter.sleep(Duration::from_secs(60)).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();
    let woke = tokio::time::timeout(Duration::from_secs(1), sleeping)
        .await
        .expect("应立刻被唤醒")
        .expect("等待任务");
    assert!(woke, "取消后 sleep 应返回 true");

    // 工作线程被取消后应落成「已停止」。
    // 用一个「能连上但立刻断开」的端口，保证很快进入出错状态，不受端口复用影响。
    let port = dead_port().await;

    let (engine, secrets, account_id) = engine_with_account("stop@example.com", port, Some("acct"));
    let ctx = worker_ctx(
        &engine,
        account_id,
        secrets,
        SyncConfig {
            backoff_base: Duration::from_millis(5),
            backoff_cap: Duration::from_millis(5),
            max_retries: 0,
            slow_poll_interval: Duration::from_secs(60),
            ..SyncConfig::default()
        },
    );
    let handle = tokio::spawn(run(ctx.clone()));

    let mut errored = false;
    for _ in 0..300 {
        if ctx.status.lock().expect("状态").state == SyncState::Error {
            errored = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(errored, "连不上应先进入出错状态");

    ctx.cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("取消后应退出")
        .expect("工作线程");
    assert_eq!(ctx.status.lock().expect("状态").state, SyncState::Stopped);
}
