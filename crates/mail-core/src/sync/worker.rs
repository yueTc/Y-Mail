//! 每账号一个同步工作线程：连接、跑一遍各文件夹，再守着收件箱等新邮件。

use std::sync::{Arc, Mutex, MutexGuard};

use mail_domain::auth::AuthMaterial;
use mail_domain::dates::{format_iso8601_utc, unix_now};
use mail_domain::proxy::Secret;
use mail_domain::{
    Account, AccountId, ConnectionError, ConnectionErrorKind, FolderKind, SyncJobKind, SyncJobState,
};
use mail_imap::{ClientConfig, FolderInfo, IdleOutcome, ImapClient};
use mail_store::{Store, StoreError};

use crate::proxies::resolve_route_with;
use crate::secrets::SecretStore;

use super::fetcher::{align_folders, sync_folder};
use super::state::{AccountSyncStatus, CancelFlag, PollInterval, SyncConfig, SyncState};

/// 一条命令里最多抓几封，避免单次应答太大。
pub(super) const FETCH_BATCH: usize = 200;

/// 工作线程要用到的全部共享句柄。
pub(super) struct WorkerContext {
    /// 账号编号。
    pub account_id: i64,
    /// 存储句柄（与引擎共用同一把锁）。
    pub store: Arc<Mutex<Store>>,
    /// 凭据保险箱。
    pub secrets: Arc<dyn SecretStore>,
    /// 节流参数（不含会变的轮询间隔）。
    pub config: SyncConfig,
    /// 不支持推送时的轮询间隔；共享句柄，改值立刻生效。
    pub poll_interval: Arc<PollInterval>,
    /// 状态快照。
    pub status: Arc<Mutex<AccountSyncStatus>>,
    /// 取消旗标。
    pub cancel: Arc<CancelFlag>,
}

/// 内部失败原因：保留分类，供上层决定重试还是停。
#[derive(Debug)]
pub(super) struct Failure {
    /// 错误大类。
    pub kind: ConnectionErrorKind,
    /// 面向用户的一句话。
    pub message: String,
    /// 是否必须用户重新授权。
    pub needs_reauth: bool,
}

impl Failure {
    /// 由协议层错误转换。
    pub fn connection(error: ConnectionError) -> Self {
        Self {
            kind: error.kind,
            needs_reauth: matches!(error.kind, ConnectionErrorKind::AuthFailed),
            message: error.message,
        }
    }

    /// 由存储错误转换。
    pub fn store(error: StoreError) -> Self {
        tracing::warn!(error = %error, "同步过程中本地存储出错");
        Self {
            kind: ConnectionErrorKind::Unknown,
            message: "本地数据库写入失败，请稍后重试".to_string(),
            needs_reauth: false,
        }
    }

    /// 本地状态不一致（正常不该发生）。
    pub fn internal(message: &str) -> Self {
        Self {
            kind: ConnectionErrorKind::Unknown,
            message: message.to_string(),
            needs_reauth: false,
        }
    }

    /// 需要用户重新填写授权码。
    pub fn reauth(message: &str) -> Self {
        Self {
            kind: ConnectionErrorKind::AuthFailed,
            message: message.to_string(),
            needs_reauth: true,
        }
    }
}

/// 线程入口：连不上就按策略退避重试，直到被取消。
pub(super) async fn run(ctx: Arc<WorkerContext>) {
    let mut attempt = 0u32;
    loop {
        if ctx.cancel.is_cancelled() {
            break;
        }
        set_status(&ctx, SyncState::Connecting, 0, 0, "正在连接服务器");
        match session(&ctx).await {
            Ok(()) => break,
            Err(failure) => {
                if failure.needs_reauth {
                    set_status(&ctx, SyncState::NeedsReauth, 0, 0, &failure.message);
                    return;
                }
                let retryable = matches!(
                    failure.kind,
                    ConnectionErrorKind::NetworkUnreachable
                        | ConnectionErrorKind::Timeout
                        | ConnectionErrorKind::ProxyFailure
                        | ConnectionErrorKind::TlsFailure
                        | ConnectionErrorKind::Unknown
                );
                if retryable && attempt < ctx.config.max_retries {
                    let delay =
                        mail_domain::backoff_delay(attempt, ctx.config.backoff_base, ctx.config.backoff_cap);
                    attempt += 1;
                    tracing::warn!(account = ctx.account_id, kind = %failure.kind.label(), "同步失败将重试");
                    set_status(
                        &ctx,
                        SyncState::Error,
                        0,
                        0,
                        &format!("{}；{} 秒后重试", failure.message, delay.as_secs().max(1)),
                    );
                    if ctx.cancel.sleep(delay).await {
                        break;
                    }
                } else {
                    attempt = 0;
                    tracing::warn!(account = ctx.account_id, kind = %failure.kind.label(), "同步连续失败，转入慢速轮询");
                    set_status(
                        &ctx,
                        SyncState::Error,
                        0,
                        0,
                        &format!("{}；已转慢速轮询", failure.message),
                    );
                    if ctx.cancel.sleep(ctx.config.slow_poll_interval).await {
                        break;
                    }
                }
            }
        }
    }
    set_status(&ctx, SyncState::Stopped, 0, 0, "同步已停止");
}

/// 一次完整连接：登录、列文件夹、跑同步，最后守着收件箱。
async fn session(ctx: &Arc<WorkerContext>) -> Result<(), Failure> {
    let account = load_account(ctx)?;
    let secret = load_secret(ctx, &account).await?;
    let route = resolve_route_with(&ctx.store, ctx.secrets.as_ref(), account.proxy)
        .map_err(|error| Failure::internal(&format!("选择代理失败：{error}")))?;

    let client_config = ClientConfig {
        host: account.imap.host.clone(),
        port: account.imap.port,
        security: account.imap.security,
        username: account.username.clone(),
        auth: AuthMaterial::for_account(account.auth_type, secret),
        timeout: mail_net::DEFAULT_TIMEOUT,
    };

    let mut client = ImapClient::connect(&client_config, route.as_ref())
        .await
        .map_err(Failure::connection)?;
    let folders = client.list_folders().await.map_err(Failure::connection)?;
    align_folders(ctx, &folders)?;
    flush_pending(ctx, &mut client).await?;

    let syncable = syncable_folders(&folders);
    count_server_totals(ctx, &mut client, &syncable).await;
    let job_id = start_job(ctx);
    let outcome = run_folders(ctx, &mut client, &syncable).await;
    if let Some(job_id) = job_id {
        match &outcome {
            Ok(()) => finish_job(ctx, job_id, true, 0, None),
            Err(failure) => finish_job(ctx, job_id, false, 0, Some(&failure.message)),
        }
    }
    if outcome.is_ok() {
        client.logout().await;
    }
    outcome
}

/// 一次性会话：连上 → 逐文件夹同步（含一次有界历史补齐）→ 退出。
///
/// 搜索的「按需深拉」用它：不挂 IDLE、不进常驻循环，跑完就把连接放掉。
pub(super) async fn sync_once(ctx: &Arc<WorkerContext>) -> Result<(), Failure> {
    let account = load_account(ctx)?;
    let secret = load_secret(ctx, &account).await?;
    let route = resolve_route_with(&ctx.store, ctx.secrets.as_ref(), account.proxy)
        .map_err(|error| Failure::internal(&format!("选择代理失败：{error}")))?;

    let client_config = ClientConfig {
        host: account.imap.host.clone(),
        port: account.imap.port,
        security: account.imap.security,
        username: account.username.clone(),
        auth: AuthMaterial::for_account(account.auth_type, secret),
        timeout: mail_net::DEFAULT_TIMEOUT,
    };

    let mut client = ImapClient::connect(&client_config, route.as_ref())
        .await
        .map_err(Failure::connection)?;
    let folders = client.list_folders().await.map_err(Failure::connection)?;
    align_folders(ctx, &folders)?;
    flush_pending(ctx, &mut client).await?;
    let syncable = syncable_folders(&folders);
    for info in &syncable {
        if ctx.cancel.is_cancelled() {
            break;
        }
        sync_folder(ctx, &mut client, info).await?;
    }
    client.logout().await;
    Ok(())
}

/// 把当前账号所有待同步的红旗变更回写服务器。
///
/// 复用已经连上的客户端：按文件夹分组 SELECT，再逐条 UID STORE。单条失败只记
/// 日志、保留待同步状态，不打断整体同步；下次同步会自动重试。
pub(super) async fn flush_pending(ctx: &Arc<WorkerContext>, client: &mut ImapClient) -> Result<(), Failure> {
    let pending = {
        let store = lock_store(&ctx.store);
        store.list_pending_flags(ctx.account_id).map_err(Failure::store)?
    };
    if pending.is_empty() {
        return Ok(());
    }

    let mut current: Option<String> = None;
    let mut selected = false;
    for item in &pending {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        if current.as_deref() != Some(item.server_path.as_str()) {
            current = Some(item.server_path.clone());
            selected = client.select(&item.server_path).await.is_ok();
            if !selected {
                tracing::warn!(
                    account = ctx.account_id,
                    folder = %item.server_path,
                    "打开文件夹失败，红旗回写稍后重试"
                );
            }
        }
        if !selected {
            continue;
        }
        match client.uid_store_flags(item.uid, item.flagged).await {
            Ok(()) => {
                let store = lock_store(&ctx.store);
                store
                    .clear_message_flag_pending(item.message_id)
                    .map_err(Failure::store)?;
            }
            Err(error) => {
                tracing::warn!(
                    account = ctx.account_id,
                    message = item.message_id,
                    "红旗回写失败，稍后自动重试：{}",
                    error.message
                );
            }
        }
    }
    Ok(())
}

/// 单独开一次连接把待同步红旗写回服务器（界面点红旗后立刻用）。
pub(super) async fn flush_flags(ctx: &Arc<WorkerContext>) -> Result<(), Failure> {
    let pending = {
        let store = lock_store(&ctx.store);
        store.list_pending_flags(ctx.account_id).map_err(Failure::store)?
    };
    if pending.is_empty() {
        return Ok(());
    }

    let account = load_account(ctx)?;
    let secret = load_secret(ctx, &account).await?;
    let route = resolve_route_with(&ctx.store, ctx.secrets.as_ref(), account.proxy)
        .map_err(|error| Failure::internal(&format!("选择代理失败：{error}")))?;
    let client_config = ClientConfig {
        host: account.imap.host.clone(),
        port: account.imap.port,
        security: account.imap.security,
        username: account.username.clone(),
        auth: AuthMaterial::for_account(account.auth_type, secret),
        timeout: mail_net::DEFAULT_TIMEOUT,
    };
    let mut client = ImapClient::connect(&client_config, route.as_ref())
        .await
        .map_err(Failure::connection)?;
    let result = flush_pending(ctx, &mut client).await;
    client.logout().await;
    result
}

/// 数一遍服务器上该账号所有可同步文件夹的邮件总数，写进状态快照。
///
/// 只统计一次；单个文件夹打不开或搜索失败按 0 计，不让统计拖垮整轮同步。
async fn count_server_totals(ctx: &Arc<WorkerContext>, client: &mut ImapClient, folders: &[FolderInfo]) {
    set_status(ctx, SyncState::Syncing, 0, 0, "正在统计邮件总数……");
    let mut total: i64 = 0;
    for info in folders {
        if ctx.cancel.is_cancelled() {
            return;
        }
        if let Err(error) = client.select(&info.server_path).await {
            tracing::warn!(
                account = ctx.account_id,
                folder = %info.full_path,
                error = %error,
                "统计邮件总数时打开文件夹失败，按 0 计"
            );
            continue;
        }
        match client.uid_search_after(1).await {
            Ok(uids) => total = total.saturating_add(uids.len() as i64),
            Err(error) => tracing::warn!(
                account = ctx.account_id,
                folder = %info.full_path,
                error = %error,
                "统计邮件总数失败，按 0 计"
            ),
        }
    }
    let count = {
        let store = lock_store(&ctx.store);
        store.count_account_messages(ctx.account_id).unwrap_or(0)
    };
    let synced = if total > 0 { count.min(total) } else { count };
    set_status(ctx, SyncState::Syncing, synced, total, "");
}

/// 先逐文件夹落一遍数据，再进入守着收件箱的循环。
async fn run_folders(
    ctx: &Arc<WorkerContext>,
    client: &mut ImapClient,
    folders: &[FolderInfo],
) -> Result<(), Failure> {
    for info in folders {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        sync_folder(ctx, client, info).await?;
    }
    live_loop(ctx, client, folders).await
}

/// 挑出真正能打开的文件夹。
///
/// 带 `\NoSelect` 的只是「装子文件夹的空壳」，里面不会有邮件；部分服务器（如腾讯
/// 企业邮）还会对它假装 SELECT 成功却不真正选中，导致随后的搜索报「先选文件夹」。
/// 所以同步前先把这类文件夹滤掉，免得一个空壳拖垮整个账号。
pub(super) fn syncable_folders(folders: &[FolderInfo]) -> Vec<FolderInfo> {
    let mut kept = Vec::with_capacity(folders.len());
    for info in folders {
        if info.is_selectable() {
            kept.push(info.clone());
        } else {
            tracing::debug!(folder = %info.full_path, "跳过不可选容器文件夹");
        }
    }
    kept
}

/// 收件箱挂 IDLE；不支持 IDLE 就按固定间隔轮询。每次醒来都重跑一轮同步。
async fn live_loop(
    ctx: &Arc<WorkerContext>,
    client: &mut ImapClient,
    folders: &[FolderInfo],
) -> Result<(), Failure> {
    let inbox = folders
        .iter()
        .find(|info| FolderKind::classify(&info.full_path, &info.attributes).is_inbox());
    let idle_capable = client.supports("IDLE") && inbox.is_some();

    loop {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        refresh_progress(ctx, SyncState::IdleWaiting, false);
        if idle_capable {
            let path = inbox.map_or("INBOX", |info| info.server_path.as_str());
            client.select(path).await.map_err(Failure::connection)?;
            match client
                .idle_wait_interruptible(ctx.config.idle_timeout, ctx.cancel.idle_interrupt())
                .await
                .map_err(Failure::connection)?
            {
                // 收到停止信号：这次 IDLE 已经发过 DONE 收尾，可以干净退出。
                IdleOutcome::Cancelled => return Ok(()),
                IdleOutcome::Changed | IdleOutcome::Timeout => {}
            }
        } else if ctx.poll_interval.sleep(&ctx.cancel).await {
            return Ok(());
        }
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        flush_pending(ctx, client).await?;
        for info in folders {
            if ctx.cancel.is_cancelled() {
                return Ok(());
            }
            sync_folder(ctx, client, info).await?;
        }
    }
}

fn load_account(ctx: &WorkerContext) -> Result<Account, Failure> {
    let store = lock_store(&ctx.store);
    store
        .get_account(AccountId(ctx.account_id))
        .map_err(Failure::store)?
        .ok_or_else(|| Failure::internal("账号不存在或已被删除"))
}

async fn load_secret(ctx: &WorkerContext, account: &Account) -> Result<Secret, Failure> {
    crate::oauth::active_secret(&ctx.store, ctx.secrets.as_ref(), account)
        .await
        .map_err(|error| match error {
            crate::oauth::ResolveError::Missing => {
                Failure::reauth("该账号还没有可用的凭据，请到账号设置里填写授权码或重新授权")
            }
            crate::oauth::ResolveError::Backend => Failure::internal("读取系统凭据失败，请稍后重试"),
            crate::oauth::ResolveError::ReauthRequired => {
                Failure::reauth("授权已失效，请到账号设置里重新授权")
            }
            crate::oauth::ResolveError::Failed(message) => Failure::internal(&message),
        })
}

fn start_job(ctx: &WorkerContext) -> Option<i64> {
    let store = lock_store(&ctx.store);
    store
        .create_sync_job(ctx.account_id, None, SyncJobKind::Initial)
        .ok()
}

fn finish_job(ctx: &WorkerContext, job_id: i64, ok: bool, progress: i64, error: Option<&str>) {
    let store = lock_store(&ctx.store);
    let state = if ok {
        SyncJobState::Completed
    } else {
        SyncJobState::Failed
    };
    let _ = store.update_sync_job(job_id, state, progress, error);
}

/// 更新状态快照；失败原因只在内存与日志里，不写库。
pub(super) fn set_status(ctx: &WorkerContext, state: SyncState, progress: i64, total: i64, message: &str) {
    let mut status = ctx.status.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    status.state = state;
    status.progress = progress;
    status.total = total;
    status.message = message.to_string();
    status.needs_reauth = matches!(state, SyncState::NeedsReauth);
    status.updated_at = format_iso8601_utc(unix_now());
}

/// 用「本地已同步条数 / 服务器总数」刷新状态快照。
///
/// `total` 是本次连接开始时统计好的服务器邮件总数；这里只把本地条数刷上去。
/// 总数未知（统计失败）时退回原来的「等待新邮件」，不硬编数字。
pub(super) fn refresh_progress(ctx: &WorkerContext, state: SyncState, capped: bool) {
    let total = {
        let status = ctx.status.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        status.total
    };
    let count = {
        let store = lock_store(&ctx.store);
        store.count_account_messages(ctx.account_id).unwrap_or(0)
    };
    let synced = if total > 0 { count.min(total) } else { count };
    let message = if total == 0 {
        "等待新邮件".to_string()
    } else if capped && synced < total {
        format!("已达本机上限（已同步 {synced} 封 / 共 {total} 封）")
    } else {
        String::new()
    };
    set_status(ctx, state, synced, total, &message);
}
/// 取存储锁；锁中毒时取回内部值继续用（单条 SQL 失败已由错误表达）。
pub(super) fn lock_store(store: &Mutex<Store>) -> MutexGuard<'_, Store> {
    store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
