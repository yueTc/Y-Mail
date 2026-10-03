//! 同步服务的共享状态：取消旗标、配置、状态快照与服务句柄。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mail_domain::dates::{format_iso8601_utc, unix_now};
use mail_domain::HistoryRange;
use mail_store::Store;

use crate::secrets::SecretStore;

/// 取消旗标：既支持随时查看，也能把正在睡觉的工作线程立刻叫醒。
pub struct CancelFlag {
    flag: AtomicBool,
    notify: tokio::sync::Notify,
}

impl CancelFlag {
    /// 新建一个未取消的旗标。
    pub fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }

    /// 标记取消并唤醒等待者。
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// 是否已被取消。
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// 睡 `duration`；期间被取消就立刻返回 `true`。
    pub async fn sleep(&self, duration: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + duration;
        loop {
            if self.is_cancelled() {
                return true;
            }
            let notified = self.notify.notified();
            tokio::pin!(notified);
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return self.is_cancelled();
            }
            tokio::select! {
                _ = &mut notified => {}
                _ = tokio::time::sleep(remaining) => return self.is_cancelled(),
            }
        }
    }
}

impl Default for CancelFlag {
    fn default() -> Self {
        Self::new()
    }
}

/// 同步节流参数，全部可调（测试里会改小）。
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// 首次快照回看的自然天数。
    pub snapshot_days: u32,
    /// 首次快照最多拉多少封（取最新的）。
    pub snapshot_limit: usize,
    /// 后台补齐每批多少封。
    pub backfill_batch: usize,
    /// 默认补齐范围（可被设置项覆盖）。
    pub history_range: HistoryRange,
    /// 没有 IDLE 时的轮询间隔。
    pub poll_interval: Duration,
    /// 一次 IDLE 最长等待时间。
    pub idle_timeout: Duration,
    /// 重试退避的基数。
    pub backoff_base: Duration,
    /// 重试退避的上限。
    pub backoff_cap: Duration,
    /// 临时错误最多重试几次。
    pub max_retries: u32,
    /// 连续失败后转入慢速轮询的间隔。
    pub slow_poll_interval: Duration,
    /// 单账号邮件条数上限。
    pub max_messages_per_account: i64,
    /// 单账号占用上限（字节，按 RFC822.SIZE 估算）。
    pub max_bytes_per_account: i64,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            snapshot_days: 30,
            snapshot_limit: 500,
            backfill_batch: 300,
            history_range: HistoryRange::LastYear,
            poll_interval: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(240),
            backoff_base: Duration::from_secs(5),
            backoff_cap: Duration::from_secs(300),
            max_retries: 2,
            slow_poll_interval: Duration::from_secs(600),
            max_messages_per_account: 100_000,
            max_bytes_per_account: 4096 * 1024 * 1024,
        }
    }
}

/// 账号同步状态（面向界面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    /// 空闲（还没启动）。
    Idle,
    /// 正在连接服务器。
    Connecting,
    /// 正在拉取邮件。
    Syncing,
    /// 正在补齐历史。
    Backfilling,
    /// 已连上，等待新邮件。
    IdleWaiting,
    /// 出错后等待重试。
    Error,
    /// 需要用户重新填写授权码。
    NeedsReauth,
    /// 已停止。
    Stopped,
}

impl SyncState {
    /// 传给界面的稳定小写标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Connecting => "connecting",
            Self::Syncing => "syncing",
            Self::Backfilling => "backfilling",
            Self::IdleWaiting => "idle_waiting",
            Self::Error => "error",
            Self::NeedsReauth => "needs_reauth",
            Self::Stopped => "stopped",
        }
    }

    /// 界面显示的中文名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "未启动",
            Self::Connecting => "正在连接",
            Self::Syncing => "正在同步",
            Self::Backfilling => "正在补齐历史",
            Self::IdleWaiting => "等待新邮件",
            Self::Error => "同步出错",
            Self::NeedsReauth => "需要重新授权",
            Self::Stopped => "已停止",
        }
    }

    /// 是否是一个正在跑的状态。
    pub fn is_running(self) -> bool {
        matches!(
            self,
            Self::Connecting | Self::Syncing | Self::Backfilling | Self::IdleWaiting
        )
    }
}

/// 一个账号的同步状态快照。
#[derive(Debug, Clone)]
pub struct AccountSyncStatus {
    /// 账号编号。
    pub account_id: i64,
    /// 邮箱地址。
    pub email: String,
    /// 当前状态。
    pub state: SyncState,
    /// 已处理条数。
    pub progress: i64,
    /// 本轮总条数（未知时为 0）。
    pub total: i64,
    /// 给用户看的一句话说明。
    pub message: String,
    /// 是否需要重新授权。
    pub needs_reauth: bool,
    /// 更新时间（UTC ISO-8601）。
    pub updated_at: String,
}

impl AccountSyncStatus {
    /// 新建一份初始状态。
    pub fn new(account_id: i64, email: impl Into<String>) -> Self {
        Self {
            account_id,
            email: email.into(),
            state: SyncState::Idle,
            progress: 0,
            total: 0,
            message: String::new(),
            needs_reauth: false,
            updated_at: format_iso8601_utc(unix_now()),
        }
    }
}

/// 一个正在跑的工作线程。
pub(crate) struct WorkerHandle {
    /// 取消旗标。
    pub cancel: Arc<CancelFlag>,
    /// 线程句柄。
    pub join: tokio::task::JoinHandle<()>,
}

/// 同步服务：持有存储与保险箱句柄，按账号管理后台线程。
pub struct SyncService {
    /// 存储句柄（与引擎共用同一把锁）。
    pub(crate) store: Arc<Mutex<Store>>,
    /// 凭据保险箱。
    pub(crate) secrets: Arc<dyn SecretStore>,
    /// 节流参数。
    pub(crate) config: SyncConfig,
    /// 正在运行的工作线程。
    pub(crate) workers: Mutex<HashMap<i64, WorkerHandle>>,
    /// 各账号的最新状态。
    pub(crate) statuses: Mutex<HashMap<i64, Arc<Mutex<AccountSyncStatus>>>>,
}

impl std::fmt::Debug for SyncService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let workers = self.workers.lock().map(|map| map.len()).unwrap_or(0);
        f.debug_struct("SyncService")
            .field("workers", &workers)
            .finish_non_exhaustive()
    }
}
