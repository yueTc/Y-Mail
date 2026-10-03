//! 后台同步服务：按账号起一个工作线程，线程与界面之间只交换状态快照。
//!
//! 关键约束：存储锁绝不跨 `.await` 持有；每个账号只允许一个工作线程；
//! 取消是「先标记、再唤醒」的协作式退出，不做强制杀线程。

pub(crate) mod fetcher;
pub(crate) mod state;
pub(crate) mod worker;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mail_store::Store;

use crate::engine::{EngineError, MailEngine};
use crate::secrets::SecretStore;

pub use state::{AccountSyncStatus, SyncConfig, SyncService, SyncState};

use state::{CancelFlag, WorkerHandle};
use worker::WorkerContext;

impl SyncService {
    /// 组装服务；存储与保险箱句柄由引擎传入，保证只有一条写库路径。
    pub fn new(store: Arc<Mutex<Store>>, secrets: Arc<dyn SecretStore>, config: SyncConfig) -> Self {
        Self {
            store,
            secrets,
            config,
            workers: Mutex::new(HashMap::new()),
            statuses: Mutex::new(HashMap::new()),
        }
    }

    /// 启动一个账号的同步；已在跑就返回 false，不重复起线程。
    pub fn start(self: &Arc<Self>, account_id: i64) -> Result<bool, EngineError> {
        let mut workers = self.lock_workers();
        if workers.contains_key(&account_id) {
            return Ok(false);
        }
        let status = self.status_slot(account_id)?;
        let cancel = Arc::new(CancelFlag::new());
        let ctx = Arc::new(WorkerContext {
            account_id,
            store: self.store.clone(),
            secrets: self.secrets.clone(),
            config: self.config.clone(),
            status: status.clone(),
            cancel: cancel.clone(),
        });
        let service = self.clone();
        let join = tokio::spawn(async move {
            worker::run(ctx).await;
            service.finish_work(account_id);
        });
        workers.insert(account_id, WorkerHandle { cancel, join });
        Ok(true)
    }

    /// 启动全部启用账号的同步；返回实际启动的数量。
    pub fn start_all(self: &Arc<Self>) -> Result<usize, EngineError> {
        let accounts = {
            let store = self.lock_store();
            store.list_accounts()?
        };
        let mut started = 0usize;
        for account in accounts {
            if !account.enabled {
                continue;
            }
            if self.start(account.id.0)? {
                started += 1;
            }
        }
        Ok(started)
    }

    /// 停止一个账号；传 None 表示全部停止。先标记取消、释放锁，再等线程退出。
    pub async fn stop(&self, account_id: Option<i64>) {
        let handles: Vec<WorkerHandle> = {
            let mut workers = self.lock_workers();
            match account_id {
                Some(id) => workers.remove(&id).into_iter().collect(),
                None => workers.drain().map(|(_, handle)| handle).collect(),
            }
        };
        for handle in &handles {
            handle.cancel.cancel();
        }
        for handle in handles {
            let _ = handle.join.await;
        }
    }

    /// 面向界面的全部账号状态（按编号排序）。
    pub fn statuses(&self) -> Vec<AccountSyncStatus> {
        let mut list: Vec<AccountSyncStatus> = {
            let slots = self.lock_status_slots();
            slots
                .values()
                .map(|slot| {
                    slot.lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone()
                })
                .collect()
        };
        list.sort_by_key(|item| item.account_id);
        list
    }

    /// 单个账号的状态快照。
    pub fn status_of(&self, account_id: i64) -> Option<AccountSyncStatus> {
        let slot = {
            let slots = self.lock_status_slots();
            slots.get(&account_id).cloned()
        }?;
        let snapshot = slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Some(snapshot)
    }

    /// 线程收尾：摘掉句柄并留下「已停止」状态。
    fn finish_work(&self, account_id: i64) {
        let mut workers = self.lock_workers();
        workers.remove(&account_id);
    }

    /// 取（或建）一个账号的状态槽。
    fn status_slot(&self, account_id: i64) -> Result<Arc<Mutex<AccountSyncStatus>>, EngineError> {
        let mut slots = self.lock_status_slots();
        if let Some(slot) = slots.get(&account_id) {
            return Ok(slot.clone());
        }
        let account = {
            let store = self.lock_store();
            store.get_account(mail_domain::AccountId(account_id))?
        };
        let account = account.ok_or(EngineError::AccountNotFound(account_id))?;
        let slot = Arc::new(Mutex::new(AccountSyncStatus::new(account_id, account.email)));
        slots.insert(account_id, slot.clone());
        Ok(slot)
    }

    fn lock_workers(&self) -> std::sync::MutexGuard<'_, HashMap<i64, WorkerHandle>> {
        self.workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_status_slots(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Arc<Mutex<AccountSyncStatus>>>> {
        self.statuses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl MailEngine {
    /// 启动同步：传账号编号就只起这一个，传 None 就起全部启用账号。
    ///
    /// 返回实际启动的账号数量；已经在跑的账号会被跳过，不重复起线程。
    pub fn start_sync(&self, account_id: Option<i64>) -> Result<usize, EngineError> {
        match account_id {
            Some(id) => Ok(usize::from(self.sync.start(id)?)),
            None => self.sync.start_all(),
        }
    }

    /// 停止同步：传账号编号就只停这一个，传 None 就全停。
    pub async fn stop_sync(&self, account_id: Option<i64>) {
        self.sync.stop(account_id).await;
    }

    /// 当前各账号同步状态。
    pub fn sync_statuses(&self) -> Vec<AccountSyncStatus> {
        self.sync.statuses()
    }
}

#[cfg(test)]
mod tests;
