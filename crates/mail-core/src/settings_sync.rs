//! 设置同步的编排层（规格 3.6 / 3.9）。
//!
//! 这一层管四件事：
//! - 同步状态落在 `setting` 表，缺键按默认值读，老用户升级不受影响。
//! - 版本与冲突判定只看出网包的公开头部，不解密就能判断。
//! - 开启 / 关闭 / 重设密码 / 新设备加入四个动作。
//! - 写操作成功后的「待同步」标记与去抖合并。
//!
//! 密钥规矩：同步密码只进系统保险箱（键 `sync/password`），
//! 数据库里只出现状态，不出现密码、令牌或任何明文密钥。

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mail_domain::account::AccountProxyMode;
use mail_domain::proxy::{ProxyRoute, Secret};
use mail_store::{
    NotifyNewMailWhen, Store, SETTING_BLOCK_REMOTE_IMAGES, SETTING_MINIMIZE_TO_TRAY,
    SETTING_NOTIFY_AI_ENABLED, SETTING_NOTIFY_NEW_MAIL, SETTING_NOTIFY_NEW_MAIL_WHEN,
    SETTING_START_MINIMIZED,
};
use mail_sync::crypto::{decrypt, derive_key, encrypt, random_bytes, KdfParams, NONCE_LEN, SALT_LEN};
use mail_sync::envelope::{
    SyncEnvelope, FORMAT_NAME, KDF_ARGON2ID, SCHEMA_VERSION as ENVELOPE_SCHEMA_VERSION,
};
use mail_sync::gist::{self, GistEndpoints, GistSummary};
use serde::{Deserialize, Serialize};

use crate::engine::{EngineError, MailEngine};
use crate::secrets::SecretStore;
use crate::sync_settings::{account_logical_key, proxy_logical_key, SettingsSnapshot};

/// 同步密码在保险箱里的键（规格 3.8 固定，不许改）。
pub const SYNC_PASSWORD_KEY: &str = "sync/password";

/// 是否已开启设置同步。
pub const SYNC_ENABLED_KEY: &str = "sync.settings-enabled";
/// 本机设备标识。
pub const SYNC_DEVICE_ID_KEY: &str = "sync.device-id";
/// 本机设备名。
pub const SYNC_DEVICE_LABEL_KEY: &str = "sync.device-label";
/// 云端 Gist 编号。
pub const SYNC_GIST_ID_KEY: &str = "sync.gist-id";
/// 本机当前版本号。
pub const SYNC_REVISION_KEY: &str = "sync.revision";
/// 上次见到的远端版本号。
pub const SYNC_REMOTE_REVISION_KEY: &str = "sync.remote-revision";
/// 上次同步时间（RFC 3339 UTC 文本）。
pub const SYNC_LAST_SYNC_AT_KEY: &str = "sync.last-sync-at";
/// 上次同步错误（只存可读文案，不存底层细节）。
pub const SYNC_LAST_ERROR_KEY: &str = "sync.last-error";
/// 待处理的冲突（一段 JSON，只含公开头部字段）。
pub const SYNC_CONFLICT_KEY: &str = "sync.conflict";

/// 写操作去抖窗口：静默等 5 秒，把期间多次改动并成一次上传。
pub const DEBOUNCE_DELAY: Duration = Duration::from_secs(5);

/// 单次 Gist 请求超时。
const GITHUB_TIMEOUT: Duration = Duration::from_secs(30);

/// 同步密码最短长度。
const MIN_PASSWORD_CHARS: usize = 8;

/// 设备名最长长度。
const MAX_DEVICE_LABEL_CHARS: usize = 64;

/// 会参与设置同步的界面开关：外壳的 `settings.json` 与同步包之间的桥。
///
/// 运行值在外壳，同步包从 `setting` 表导出；两份必须保持一致，
/// 否则导出的是默认值、导入也落不到界面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncedToggles {
    /// 新邮件系统通知。
    pub notify_new_mail: bool,
    /// 通知智能识别。
    pub notify_ai_enabled: bool,
    /// 通知触发时机；`None` 表示表里没有这个值，读回时让外壳保持本机选择。
    pub notify_new_mail_when: Option<NotifyNewMailWhen>,
    /// 默认拦截远程图片。
    pub block_remote_images_by_default: bool,
    /// 关闭键收托盘。
    pub minimize_to_tray_on_close: bool,
    /// 启动时静默进托盘。
    pub start_minimized_to_tray: bool,
}

/// 给界面看的一份同步状态（不含任何密钥）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSyncStatus {
    /// 是否已开启。
    pub enabled: bool,
    /// 本机设备标识。
    pub device_id: Option<String>,
    /// 本机设备名。
    pub device_label: Option<String>,
    /// 云端 Gist 编号。
    pub gist_id: Option<String>,
    /// Gist 的网页地址，给界面显示。
    pub gist_url: Option<String>,
    /// 本机当前版本号。
    pub revision: u64,
    /// 上次见到的远端版本号。
    pub remote_revision: u64,
    /// 上次同步时间。
    pub last_sync_at: Option<String>,
    /// 上次同步错误。
    pub last_error: Option<String>,
    /// 待用户处理的冲突；没有就是空。
    pub conflict: Option<SettingsSyncConflict>,
}

/// 冲突的公开信息：哪台设备、什么时间、哪个版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSyncConflict {
    /// 对方设备标识。
    pub remote_device_id: String,
    /// 对方设备名。
    pub remote_device_label: String,
    /// 对方版本号。
    pub remote_revision: u64,
    /// 对方出网时间。
    pub updated_at: String,
}

/// 冲突处理选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictChoice {
    /// 保留本机配置，重新上传覆盖远端。
    KeepLocal,
    /// 丢掉本机改动，按远端配置导入。
    KeepRemote,
}

/// 拉取时对远端新版本的处置方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteChange {
    /// 远端没有比本机记录更新。
    UpToDate,
    /// 远端是本机自己写的（上传成功后本机状态没来得及更新等）。
    OwnUpload,
    /// 远端更新，本机没有未上传改动，可以直接导入。
    ApplyRemote,
    /// 两边都改过，必须让用户选。
    Conflict,
}

/// 冲突判定只看公开版本号与作者标识，不解密。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSyncVersions {
    /// 本机设备标识。
    pub device_id: String,
    /// 本机当前版本号。
    pub revision: u64,
    /// 上次见到的远端版本号。
    pub remote_revision: u64,
    /// 本机是否有还没上传的改动。
    pub has_local_changes: bool,
}

/// 比对远端包与本机记录，给出处置方式。
pub fn classify_remote_change(local: &LocalSyncVersions, remote: &SyncEnvelope) -> RemoteChange {
    if remote.revision <= local.remote_revision {
        return RemoteChange::UpToDate;
    }
    if remote.authored_by(&local.device_id) {
        return RemoteChange::OwnUpload;
    }
    if local.has_local_changes || local.revision > local.remote_revision {
        RemoteChange::Conflict
    } else {
        RemoteChange::ApplyRemote
    }
}

/// Gist 后端的异步返回类型。
pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, EngineError>> + Send + 'a>>;

/// 设置同步用得到的最小远端能力；生产走 GitHub，测试可以换成内存假实现。
pub trait SettingsSyncBackend: Send + Sync {
    /// 找出这个账号里用于设置同步的那一个私密 Gist。
    fn find_gist<'a>(&'a self, token: &'a str) -> BackendFuture<'a, Option<GistSummary>>;
    /// 新建一个私密 Gist 并写入正文。
    fn create_gist<'a>(&'a self, token: &'a str, content: &'a [u8]) -> BackendFuture<'a, GistSummary>;
    /// 读取指定 Gist 里同步文件的正文。
    fn read_gist<'a>(&'a self, token: &'a str, gist_id: &'a str) -> BackendFuture<'a, Vec<u8>>;
    /// 覆盖指定 Gist 里同步文件的正文。
    fn update_gist<'a>(
        &'a self,
        token: &'a str,
        gist_id: &'a str,
        content: &'a [u8],
    ) -> BackendFuture<'a, ()>;
    /// 删除指定 Gist。
    fn delete_gist<'a>(&'a self, token: &'a str, gist_id: &'a str) -> BackendFuture<'a, ()>;
}

/// 生产后端：每次操作前按当前代理设置决议出网路由，再走 mail-sync 的 Gist 客户端。
pub(crate) struct GitHubGistBackend {
    store: Arc<std::sync::Mutex<Store>>,
    secrets: Arc<dyn SecretStore>,
    endpoints: GistEndpoints,
    timeout: Duration,
}

impl GitHubGistBackend {
    /// 用引擎里的存储与保险箱句柄构造；不持有引擎，避免把网络等待和界面锁绑在一起。
    pub(crate) fn new(store: Arc<std::sync::Mutex<Store>>, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            store,
            secrets,
            endpoints: GistEndpoints::github(),
            timeout: GITHUB_TIMEOUT,
        }
    }

    /// 当前代理设置下的出网路由；只短暂持锁，不跨网络等待。
    fn route(&self) -> Result<Option<ProxyRoute>, EngineError> {
        crate::proxies::resolve_route_with(
            &self.store,
            self.secrets.as_ref(),
            AccountProxyMode::InheritGlobal,
        )
    }
}

impl SettingsSyncBackend for GitHubGistBackend {
    fn find_gist<'a>(&'a self, token: &'a str) -> BackendFuture<'a, Option<GistSummary>> {
        Box::pin(async move {
            let route = self.route()?;
            gist::find_sync_gist(route.as_ref(), &self.endpoints, token, self.timeout)
                .await
                .map_err(EngineError::from)
        })
    }

    fn create_gist<'a>(&'a self, token: &'a str, content: &'a [u8]) -> BackendFuture<'a, GistSummary> {
        Box::pin(async move {
            let route = self.route()?;
            gist::create_sync_gist(route.as_ref(), &self.endpoints, token, content, self.timeout)
                .await
                .map_err(EngineError::from)
        })
    }

    fn read_gist<'a>(&'a self, token: &'a str, gist_id: &'a str) -> BackendFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let route = self.route()?;
            gist::read_sync_file(route.as_ref(), &self.endpoints, token, gist_id, self.timeout)
                .await
                .map_err(EngineError::from)
        })
    }

    fn update_gist<'a>(
        &'a self,
        token: &'a str,
        gist_id: &'a str,
        content: &'a [u8],
    ) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            let route = self.route()?;
            gist::update_sync_gist(
                route.as_ref(),
                &self.endpoints,
                token,
                gist_id,
                content,
                self.timeout,
            )
            .await
            .map_err(EngineError::from)
        })
    }

    fn delete_gist<'a>(&'a self, token: &'a str, gist_id: &'a str) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            let route = self.route()?;
            gist::delete_gist(route.as_ref(), &self.endpoints, token, gist_id, self.timeout)
                .await
                .map_err(EngineError::from)
        })
    }
}

/// 待同步标记与唤醒；后台任务用它做去抖合并。
#[derive(Debug, Default)]
pub struct SettingsSyncLive {
    pending: AtomicBool,
    notify: tokio::sync::Notify,
}

impl SettingsSyncLive {
    /// 新建一份标记。
    pub fn new() -> Self {
        Self::default()
    }

    /// 打上待同步标记，并唤醒后台任务。
    pub fn mark_pending(&self) {
        self.pending.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    /// 是否还有待同步标记。
    pub fn has_pending(&self) -> bool {
        self.pending.load(Ordering::SeqCst)
    }

    /// 取走待同步标记；取到一次就够，后续重复改动已经包含在这次导出里。
    pub fn take_pending(&self) -> bool {
        self.pending.swap(false, Ordering::SeqCst)
    }

    /// 等一次待同步标记；被唤醒但标记已被处理时继续等。
    pub async fn wait_for_change(&self) {
        loop {
            self.notify.notified().await;
            if self.has_pending() {
                return;
            }
        }
    }
}

/// 一条完整的同步状态记录（内存里用，不直接给界面）。
#[derive(Debug, Clone)]
struct SyncRecord {
    enabled: bool,
    device_id: String,
    device_label: String,
    gist_id: Option<String>,
    revision: u64,
    remote_revision: u64,
}
impl MailEngine {
    /// 读设置同步状态；缺键按默认值处理。
    pub fn settings_sync_status(&self) -> Result<SettingsSyncStatus, EngineError> {
        let record = self.read_sync_record()?;
        let (last_sync_at, last_error, conflict_text) = {
            let guard = self.store();
            (
                guard.get_setting(SYNC_LAST_SYNC_AT_KEY)?,
                guard.get_setting(SYNC_LAST_ERROR_KEY)?,
                guard.get_setting(SYNC_CONFLICT_KEY)?,
            )
        };
        let conflict = conflict_text
            .filter(|text| !text.trim().is_empty())
            .and_then(|text| serde_json::from_str::<SettingsSyncConflict>(&text).ok());
        Ok(SettingsSyncStatus {
            enabled: record.enabled,
            device_id: non_empty(Some(record.device_id)),
            device_label: non_empty(Some(record.device_label)),
            gist_url: record.gist_id.as_deref().map(gist::gist_html_url),
            gist_id: record.gist_id,
            revision: record.revision,
            remote_revision: record.remote_revision,
            last_sync_at: non_empty(last_sync_at),
            last_error: non_empty(last_error),
            conflict,
        })
    }

    /// 把会同步的界面开关写进 `setting` 表；返回是否有值发生变化。
    ///
    /// 外壳保存设置后调用；有变化时由调用方决定打不打待同步标记。
    pub fn store_synced_toggles(&self, toggles: &SyncedToggles) -> Result<bool, EngineError> {
        let guard = self.store();
        let mut changed = false;
        for (key, value, default) in [
            (SETTING_NOTIFY_NEW_MAIL, toggles.notify_new_mail, true),
            (SETTING_NOTIFY_AI_ENABLED, toggles.notify_ai_enabled, false),
            (
                SETTING_BLOCK_REMOTE_IMAGES,
                toggles.block_remote_images_by_default,
                true,
            ),
            (SETTING_MINIMIZE_TO_TRAY, toggles.minimize_to_tray_on_close, true),
            (SETTING_START_MINIMIZED, toggles.start_minimized_to_tray, false),
        ] {
            if toggle_bool(guard.get_setting(key)?, default) != value {
                guard.set_setting(key, if value { "true" } else { "false" })?;
                changed = true;
            }
        }
        // 提醒时机单独处理：带值才写，`None` 表示这次不碰表里的旧值。
        if let Some(when) = toggles.notify_new_mail_when {
            let current = guard.get_setting(SETTING_NOTIFY_NEW_MAIL_WHEN)?;
            if current.as_deref() != Some(when.as_str()) {
                guard.set_setting(SETTING_NOTIFY_NEW_MAIL_WHEN, when.as_str())?;
                changed = true;
            }
        }
        Ok(changed)
    }

    /// 读回会同步的界面开关；导入远端快照后外壳调用，让界面与行为跟上。
    pub fn read_synced_toggles(&self) -> Result<SyncedToggles, EngineError> {
        let guard = self.store();
        let notify_new_mail_when = guard
            .get_setting(SETTING_NOTIFY_NEW_MAIL_WHEN)?
            .as_deref()
            .map(NotifyNewMailWhen::parse);
        Ok(SyncedToggles {
            notify_new_mail: toggle_bool(guard.get_setting(SETTING_NOTIFY_NEW_MAIL)?, true),
            notify_ai_enabled: toggle_bool(guard.get_setting(SETTING_NOTIFY_AI_ENABLED)?, false),
            notify_new_mail_when,
            block_remote_images_by_default: toggle_bool(
                guard.get_setting(SETTING_BLOCK_REMOTE_IMAGES)?,
                true,
            ),
            minimize_to_tray_on_close: toggle_bool(guard.get_setting(SETTING_MINIMIZE_TO_TRAY)?, true),
            start_minimized_to_tray: toggle_bool(guard.get_setting(SETTING_START_MINIMIZED)?, false),
        })
    }

    /// 写操作成功后打待同步标记；后台任务负责去抖合并成一次上传。
    pub fn mark_settings_changed(&self) {
        self.settings_sync_live_handle().mark_pending();
    }

    /// 后台任务用：取走待同步标记；取到就执行一次上传。
    ///
    /// 调用方要先在锁外等到标记并睡过 [`DEBOUNCE_DELAY`]，再调这里。
    pub async fn flush_pending_settings_sync(&self) -> Result<Option<SettingsSyncStatus>, EngineError> {
        let backend = self.settings_sync_backend();
        self.flush_pending_settings_sync_with(backend.as_ref()).await
    }

    /// 去抖上传的具体实现；测试可以换成内存假 Gist。
    pub(crate) async fn flush_pending_settings_sync_with(
        &self,
        backend: &dyn SettingsSyncBackend,
    ) -> Result<Option<SettingsSyncStatus>, EngineError> {
        if !self.settings_sync_live_handle().take_pending() {
            return Ok(None);
        }
        if !self.settings_sync_status()?.enabled {
            return Ok(None);
        }
        let status = self.upload_settings_sync_with(backend, false).await?;
        Ok(Some(status))
    }

    /// 上传到云端：加密本机配置，已有内容需先确认覆盖。
    pub async fn enable_settings_sync(
        &self,
        password: &str,
        device_label: &str,
        overwrite_remote: bool,
    ) -> Result<SettingsSyncStatus, EngineError> {
        let backend = self.settings_sync_backend();
        self.enable_settings_sync_with(password, device_label, overwrite_remote, backend.as_ref())
            .await
    }

    /// 关闭设置同步；`delete_remote` 为真时连云端那份一起删。
    pub async fn disable_settings_sync(
        &self,
        delete_remote: bool,
    ) -> Result<SettingsSyncStatus, EngineError> {
        let backend = self.settings_sync_backend();
        self.disable_settings_sync_with(delete_remote, backend.as_ref())
            .await
    }

    /// 立即同步：先拉远端比对，没有冲突就把本机改动传上去。
    pub async fn sync_now(&self) -> Result<SettingsSyncStatus, EngineError> {
        let backend = self.settings_sync_backend();
        self.upload_settings_sync_with(backend.as_ref(), false).await
    }

    /// 启动拉取：远端有新版本且本机没有未上传改动时自动导入。
    pub async fn pull_settings_sync(&self) -> Result<SettingsSyncStatus, EngineError> {
        let backend = self.settings_sync_backend();
        self.pull_settings_sync_with(backend.as_ref(), false).await
    }

    /// 重设同步密码：建新 Gist，再把旧 Gist 删掉。
    pub async fn reset_settings_sync_password(
        &self,
        new_password: &str,
        confirm: &str,
    ) -> Result<SettingsSyncStatus, EngineError> {
        let backend = self.settings_sync_backend();
        self.reset_settings_sync_password_with(new_password, confirm, backend.as_ref())
            .await
    }

    /// 从云端下载：找同步 Gist，用同一个密码解密后导入。
    pub async fn download_settings_sync(
        &self,
        password: &str,
        device_label: &str,
        confirm_overwrite: bool,
    ) -> Result<SettingsSyncStatus, EngineError> {
        let backend = self.settings_sync_backend();
        self.download_settings_sync_with(password, device_label, confirm_overwrite, backend.as_ref())
            .await
    }

    /// 解决冲突：保留本机就重新上传，保留远端就强制导入。
    pub async fn resolve_settings_sync_conflict(
        &self,
        choice: ConflictChoice,
    ) -> Result<SettingsSyncStatus, EngineError> {
        let backend = self.settings_sync_backend();
        match choice {
            ConflictChoice::KeepLocal => self.upload_settings_sync_with(backend.as_ref(), true).await,
            ConflictChoice::KeepRemote => self.pull_settings_sync_with(backend.as_ref(), true).await,
        }
    }

    /// 生产后端：每次操作拿一份独立句柄，不把网络等待绑在引擎锁上。
    pub(crate) fn settings_sync_backend(&self) -> Arc<dyn SettingsSyncBackend> {
        Arc::new(GitHubGistBackend::new(self.store_handle(), self.secrets_handle()))
    }

    /// 开启的具体实现；测试可以换成内存假 Gist。
    pub(crate) async fn enable_settings_sync_with(
        &self,
        password: &str,
        device_label: &str,
        overwrite_remote: bool,
        backend: &dyn SettingsSyncBackend,
    ) -> Result<SettingsSyncStatus, EngineError> {
        validate_password(password)?;
        let previous_record = self.read_sync_record()?;
        let token = self
            .github_token()?
            .ok_or_else(|| EngineError::BadRequest("请先登录 GitHub".to_string()))?;
        let existing = backend.find_gist(token.expose()).await?;
        if existing.is_some() && !overwrite_remote {
            return Err(EngineError::BadRequest(
                "云端已有同步内容，请确认覆盖后重试".to_string(),
            ));
        }

        // 覆盖沿用原 Gist，版本继续递增，其他设备才能识别这次上传。
        let previous_content = match existing.as_ref() {
            Some(gist) => Some(backend.read_gist(token.expose(), &gist.id).await?),
            None => None,
        };
        let revision = previous_content
            .as_deref()
            .and_then(|bytes| SyncEnvelope::parse(bytes).ok())
            .map(|envelope| envelope.revision)
            .unwrap_or(0)
            .max(previous_record.revision)
            .checked_add(1)
            .ok_or_else(|| EngineError::BadRequest("同步版本号已达上限".to_string()))?;
        let previous_password = self.secrets().get(SYNC_PASSWORD_KEY)?;
        let device_id = if previous_record.device_id.is_empty() {
            generate_device_id()?
        } else {
            previous_record.device_id.clone()
        };
        let label = clean_device_label(device_label);
        let snapshot = self.export_settings_snapshot(&device_id, &label, revision)?;
        let (bytes, _envelope) = encrypt_snapshot(&snapshot, password)?;
        let created = match existing {
            Some(existing) => {
                backend.update_gist(token.expose(), &existing.id, &bytes).await?;
                existing
            }
            None => backend.create_gist(token.expose(), &bytes).await?,
        };

        let record = SyncRecord {
            enabled: true,
            device_id,
            device_label: label,
            gist_id: Some(created.id.clone()),
            revision,
            remote_revision: revision,
        };
        let saved = self
            .secrets()
            .set(SYNC_PASSWORD_KEY, &Secret::new(password))
            .map_err(EngineError::from)
            .and_then(|()| self.write_sync_record(&record));
        if let Err(err) = saved {
            // 覆盖失败时还原原密文；只有本次新建的 Gist 才能删除。
            if let Some(previous) = previous_content {
                let _ = backend.update_gist(token.expose(), &created.id, &previous).await;
            } else {
                let _ = backend.delete_gist(token.expose(), &created.id).await;
            }
            restore_password(self, previous_password);
            let _ = self.write_sync_record(&previous_record);
            return Err(err);
        }
        self.clear_conflict()?;
        self.record_success_time()?;
        self.settings_sync_status()
    }
}

fn validate_password(password: &str) -> Result<(), EngineError> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(EngineError::BadRequest("同步密码至少 8 位".to_string()));
    }
    Ok(())
}

/// 空串（或只有空白）当作没设。
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

/// 把 setting 表里的 "true"/"1" 之类解析成布尔；缺省或看不懂就用默认值。
fn toggle_bool(value: Option<String>, default: bool) -> bool {
    match value.as_deref().map(str::trim) {
        Some("true") | Some("1") => true,
        Some("false") | Some("0") => false,
        _ => default,
    }
}

/// 把存库的版本号文本读成数字；缺失或坏值一律当 0。
fn parse_revision(raw: Option<String>) -> u64 {
    raw.and_then(|text| text.trim().parse::<u64>().ok()).unwrap_or(0)
}

/// 校验新密码：两次一致，且不少于最少位数。
fn validate_new_password(password: &str, confirm: &str) -> Result<(), EngineError> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(EngineError::BadRequest(format!(
            "同步密码至少 {MIN_PASSWORD_CHARS} 位"
        )));
    }
    if password != confirm {
        return Err(EngineError::BadRequest("两次输入的同步密码不一致".to_string()));
    }
    Ok(())
}

/// 设备名清洗：去首尾空白、空名给默认值、超长截断。
fn clean_device_label(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "本机".to_string();
    }
    trimmed.chars().take(MAX_DEVICE_LABEL_CHARS).collect()
}

/// 生成设备标识：128 位随机数的十六进制串，不含任何本机信息。
fn generate_device_id() -> Result<String, EngineError> {
    let bytes = random_bytes(16)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// 把明文快照加密成出网包字节；每次都用新的盐与随机数，绝不复用密文。
fn encrypt_snapshot(
    snapshot: &SettingsSnapshot,
    password: &str,
) -> Result<(Vec<u8>, SyncEnvelope), EngineError> {
    let params = KdfParams::default();
    let salt = random_bytes(SALT_LEN)?;
    let nonce = random_bytes(NONCE_LEN)?;
    let key = derive_key(password, &salt, params)?;
    let plaintext =
        serde_json::to_vec(snapshot).map_err(|_| EngineError::BadRequest("同步包无法序列化".to_string()))?;
    let ciphertext = encrypt(&key, &nonce, &plaintext)?;
    let envelope = SyncEnvelope {
        format: FORMAT_NAME.to_string(),
        schema_version: ENVELOPE_SCHEMA_VERSION,
        kdf: KDF_ARGON2ID.to_string(),
        kdf_params: params.into(),
        salt,
        nonce,
        ciphertext,
        device_id: snapshot.device_id.clone(),
        device_label: snapshot.device_label.clone(),
        revision: snapshot.revision,
        updated_at: snapshot.updated_at.clone(),
    };
    let bytes = envelope.to_bytes()?;
    Ok((bytes, envelope))
}

/// 解出网包：格式不认识、密码不对、内容被改过都在这层拦住。
fn decrypt_snapshot(bytes: &[u8], password: &str) -> Result<(SettingsSnapshot, SyncEnvelope), EngineError> {
    let envelope = SyncEnvelope::parse(bytes)?;
    let params: KdfParams = envelope.kdf_params.into();
    let key = derive_key(password, &envelope.salt, params)?;
    let plaintext = decrypt(&key, &envelope.nonce, &envelope.ciphertext)?;
    let snapshot: SettingsSnapshot = serde_json::from_slice(&plaintext)
        .map_err(|_| EngineError::BadRequest("同步包内容无法识别".to_string()))?;
    Ok((snapshot, envelope))
}

/// 回滚保险箱里的同步密码：原来有就还原，没有就删掉。
fn restore_password(engine: &MailEngine, previous: Option<Secret>) {
    match previous {
        Some(old) => {
            let _ = engine.secrets().set(SYNC_PASSWORD_KEY, &old);
        }
        None => {
            let _ = engine.secrets().delete(SYNC_PASSWORD_KEY);
        }
    }
}

impl MailEngine {
    /// 读同步状态；缺键按默认值处理，老用户升级不受影响。
    fn read_sync_record(&self) -> Result<SyncRecord, EngineError> {
        let guard = self.store();
        let enabled = guard
            .get_setting(SYNC_ENABLED_KEY)?
            .map(|text| text.trim() == "true")
            .unwrap_or(false);
        Ok(SyncRecord {
            enabled,
            device_id: guard.get_setting(SYNC_DEVICE_ID_KEY)?.unwrap_or_default(),
            device_label: guard.get_setting(SYNC_DEVICE_LABEL_KEY)?.unwrap_or_default(),
            gist_id: non_empty(guard.get_setting(SYNC_GIST_ID_KEY)?),
            revision: parse_revision(guard.get_setting(SYNC_REVISION_KEY)?),
            remote_revision: parse_revision(guard.get_setting(SYNC_REMOTE_REVISION_KEY)?),
        })
    }

    /// 覆盖式写同步状态；状态键里绝不放密码或令牌。
    fn write_sync_record(&self, record: &SyncRecord) -> Result<(), EngineError> {
        let guard = self.store();
        guard.set_setting(SYNC_ENABLED_KEY, if record.enabled { "true" } else { "false" })?;
        guard.set_setting(SYNC_DEVICE_ID_KEY, &record.device_id)?;
        guard.set_setting(SYNC_DEVICE_LABEL_KEY, &record.device_label)?;
        guard.set_setting(SYNC_GIST_ID_KEY, record.gist_id.as_deref().unwrap_or(""))?;
        guard.set_setting(SYNC_REVISION_KEY, &record.revision.to_string())?;
        guard.set_setting(SYNC_REMOTE_REVISION_KEY, &record.remote_revision.to_string())?;
        Ok(())
    }

    /// 取已开启的状态；没开启就给可读提示。
    fn require_enabled_record(&self) -> Result<SyncRecord, EngineError> {
        let record = self.read_sync_record()?;
        if !record.enabled {
            return Err(EngineError::BadRequest("设置同步还没开启".to_string()));
        }
        Ok(record)
    }

    /// 取 GitHub 令牌；没登录就给可读提示。
    fn require_github_token(&self) -> Result<Secret, EngineError> {
        self.github_token()?
            .ok_or_else(|| EngineError::BadRequest("请先登录 GitHub".to_string()))
    }

    /// 取本机记录的云端 Gist 编号。
    fn require_gist_id(&self, record: &SyncRecord) -> Result<String, EngineError> {
        record
            .gist_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| EngineError::BadRequest("本机还没记录云端位置，请重新开启同步".to_string()))
    }

    /// 取保险箱里的同步密码。
    fn require_sync_password(&self) -> Result<Secret, EngineError> {
        self.secrets()
            .get(SYNC_PASSWORD_KEY)?
            .ok_or_else(|| EngineError::BadRequest("本机没有同步密码，请重设密码或重新加入".to_string()))
    }

    /// 记下这次同步成功：刷新时间、清掉旧错误。
    fn record_success_time(&self) -> Result<(), EngineError> {
        let guard = self.store();
        let now = guard.now_utc()?;
        guard.set_setting(SYNC_LAST_SYNC_AT_KEY, &now)?;
        guard.set_setting(SYNC_LAST_ERROR_KEY, "")?;
        Ok(())
    }

    /// 只记一条可读错误；写状态失败也不往上报，避免把网络问题伪装成存储问题。
    fn record_last_error(&self, message: &str) {
        let guard = self.store();
        let _ = guard.set_setting(SYNC_LAST_ERROR_KEY, message);
    }

    /// 清掉全部同步状态；不碰邮箱、代理等本机业务数据。
    fn clear_sync_state(&self) -> Result<(), EngineError> {
        let guard = self.store();
        guard.set_setting(SYNC_ENABLED_KEY, "false")?;
        guard.set_setting(SYNC_DEVICE_ID_KEY, "")?;
        guard.set_setting(SYNC_DEVICE_LABEL_KEY, "")?;
        guard.set_setting(SYNC_GIST_ID_KEY, "")?;
        guard.set_setting(SYNC_REVISION_KEY, "0")?;
        guard.set_setting(SYNC_REMOTE_REVISION_KEY, "0")?;
        guard.set_setting(SYNC_CONFLICT_KEY, "")?;
        guard.set_setting(SYNC_LAST_ERROR_KEY, "")?;
        guard.set_setting(SYNC_LAST_SYNC_AT_KEY, "")?;
        Ok(())
    }

    /// 读当前待处理的冲突；没有就是空。
    fn pending_conflict(&self) -> Result<Option<SettingsSyncConflict>, EngineError> {
        let text = {
            let guard = self.store();
            guard.get_setting(SYNC_CONFLICT_KEY)?
        };
        Ok(text
            .filter(|value| !value.trim().is_empty())
            .and_then(|value| serde_json::from_str::<SettingsSyncConflict>(&value).ok()))
    }

    /// 记一条冲突；冲突期间不自动上传。
    fn write_conflict(&self, conflict: &SettingsSyncConflict) -> Result<(), EngineError> {
        let text = serde_json::to_string(conflict)
            .map_err(|_| EngineError::BadRequest("冲突信息无法记录".to_string()))?;
        let guard = self.store();
        guard.set_setting(SYNC_CONFLICT_KEY, &text)?;
        Ok(())
    }

    /// 清掉待处理冲突。
    fn clear_conflict(&self) -> Result<(), EngineError> {
        let guard = self.store();
        guard.set_setting(SYNC_CONFLICT_KEY, "")?;
        Ok(())
    }

    /// 待同步标记的共享句柄；外壳后台任务与写操作共用同一份。
    pub fn settings_sync_live_handle(&self) -> Arc<SettingsSyncLive> {
        self.settings_sync_live.clone()
    }

    /// 解密远端出网包并导入；解密失败不覆盖本机数据。
    fn apply_remote_snapshot(&self, bytes: &[u8]) -> Result<SettingsSnapshot, EngineError> {
        let password = self.require_sync_password()?;
        let (snapshot, _envelope) = decrypt_snapshot(bytes, password.expose())?;
        self.import_settings_snapshot(&snapshot)?;
        Ok(snapshot)
    }
}

impl MailEngine {
    /// 关闭同步的具体实现；`delete_remote` 为真时连云端那份一起删。
    ///
    /// 只清同步状态与同步用的凭据，绝不碰本机邮箱、代理等业务数据。
    pub(crate) async fn disable_settings_sync_with(
        &self,
        delete_remote: bool,
        backend: &dyn SettingsSyncBackend,
    ) -> Result<SettingsSyncStatus, EngineError> {
        let record = self.require_enabled_record()?;
        let mut warning: Option<String> = None;

        if delete_remote {
            if let Some(gist_id) = record.gist_id.as_deref() {
                match self.github_token()? {
                    Some(token) => {
                        if backend.delete_gist(token.expose(), gist_id).await.is_err() {
                            warning = Some(format!(
                                "云端那份没能删掉，可能还在，请到 {} 手动删除",
                                gist::gist_html_url(gist_id)
                            ));
                        }
                    }
                    None => {
                        warning = Some(format!(
                            "没有可用的 GitHub 令牌，云端那份可能还在，请到 {} 手动删除",
                            gist::gist_html_url(gist_id)
                        ));
                    }
                }
            }
        }

        self.clear_sync_state()?;
        self.secrets().delete(SYNC_PASSWORD_KEY)?;
        // 关掉同步就一并退出登录：状态与凭据保持一致，不留半登录态。
        self.sign_out_github()?;
        if let Some(message) = warning {
            self.record_last_error(&message);
        }
        self.settings_sync_status()
    }

    /// 重设同步密码的具体实现：用新密码建新 Gist，再把旧 Gist 删掉。
    pub(crate) async fn reset_settings_sync_password_with(
        &self,
        new_password: &str,
        confirm: &str,
        backend: &dyn SettingsSyncBackend,
    ) -> Result<SettingsSyncStatus, EngineError> {
        validate_new_password(new_password, confirm)?;
        let record = self.require_enabled_record()?;
        let token = self.require_github_token()?;

        let revision = record.revision + 1;
        let snapshot = self.export_settings_snapshot(&record.device_id, &record.device_label, revision)?;
        let (bytes, _envelope) = encrypt_snapshot(&snapshot, new_password)?;
        let created = backend.create_gist(token.expose(), &bytes).await?;

        // 先换密码、再改状态；任一步写失败就把新 Gist 与密码一起回滚。
        let previous_password = self.secrets().get(SYNC_PASSWORD_KEY)?;
        self.secrets()
            .set(SYNC_PASSWORD_KEY, &Secret::new(new_password))?;
        let mut updated = record.clone();
        updated.gist_id = Some(created.id.clone());
        updated.revision = revision;
        updated.remote_revision = revision;
        if let Err(err) = self.write_sync_record(&updated) {
            let _ = backend.delete_gist(token.expose(), &created.id).await;
            restore_password(self, previous_password);
            return Err(err);
        }

        let mut warning: Option<String> = None;
        if let Some(old_id) = record.gist_id.as_deref() {
            if backend.delete_gist(token.expose(), old_id).await.is_err() {
                warning = Some(format!(
                    "旧 Gist 可能还在，请到 {} 手动删除",
                    gist::gist_html_url(old_id)
                ));
            }
        }
        self.record_success_time()?;
        if let Some(message) = warning {
            self.record_last_error(&message);
        }
        self.settings_sync_status()
    }

    /// 从云端下载的具体实现：找 Gist、输密码解密、导入。
    ///
    /// `confirm_overwrite` 为假时，本机已有同邮箱账号或同标签代理会先拒绝，
    /// 让界面弹一次确认再重来——规格要求不静默覆盖。
    pub(crate) async fn download_settings_sync_with(
        &self,
        password: &str,
        device_label: &str,
        confirm_overwrite: bool,
        backend: &dyn SettingsSyncBackend,
    ) -> Result<SettingsSyncStatus, EngineError> {
        if password.is_empty() {
            return Err(EngineError::BadRequest("请输入同步密码".to_string()));
        }
        let previous_record = self.read_sync_record()?;
        let token = self.require_github_token()?;
        let existing = backend
            .find_gist(token.expose())
            .await?
            .ok_or_else(|| EngineError::BadRequest("没有找到同步数据".to_string()))?;
        let bytes = backend.read_gist(token.expose(), &existing.id).await?;
        let (snapshot, _envelope) = decrypt_snapshot(&bytes, password)?;

        if !confirm_overwrite {
            let overlap = self.local_overlap_with(&snapshot)?;
            if !overlap.is_empty() {
                return Err(EngineError::BadRequest(format!(
                    "云端配置会覆盖本机这些条目：{}。请确认是否覆盖",
                    overlap.join("、")
                )));
            }
        }

        let device_id = if previous_record.device_id.is_empty() {
            generate_device_id()?
        } else {
            previous_record.device_id.clone()
        };
        let label = clean_device_label(device_label);
        let previous_password = self.secrets().get(SYNC_PASSWORD_KEY)?;
        self.import_settings_snapshot(&snapshot)?;
        self.secrets().set(SYNC_PASSWORD_KEY, &Secret::new(password))?;
        let record = SyncRecord {
            enabled: true,
            device_id,
            device_label: label,
            gist_id: Some(existing.id.clone()),
            revision: snapshot.revision,
            remote_revision: snapshot.revision,
        };
        if let Err(err) = self.write_sync_record(&record) {
            restore_password(self, previous_password);
            let _ = self.write_sync_record(&previous_record);
            return Err(err);
        }
        self.clear_conflict()?;
        self.record_success_time()?;
        self.settings_sync_status()
    }

    /// 找出本机与云端快照同键的条目（同邮箱账号、同标签代理），覆盖前先确认。
    fn local_overlap_with(&self, snapshot: &SettingsSnapshot) -> Result<Vec<String>, EngineError> {
        let (local_emails, local_proxies) = {
            let guard = self.store();
            let emails = guard
                .list_accounts()?
                .into_iter()
                .map(|account| account_logical_key(&account.email))
                .collect::<Vec<_>>();
            let proxies = guard
                .list_proxies()?
                .into_iter()
                .map(|stored| {
                    proxy_logical_key(&stored.config.label, &stored.config.host, stored.config.port)
                })
                .collect::<Vec<_>>();
            (emails, proxies)
        };

        let mut overlap = Vec::new();
        for account in &snapshot.accounts {
            if local_emails.contains(&account_logical_key(&account.email)) {
                overlap.push(format!("邮箱 {}", account.email));
            }
        }
        for proxy in &snapshot.proxies {
            if local_proxies.contains(&proxy.key) {
                overlap.push(format!("代理 {}", proxy.label));
            }
        }
        Ok(overlap)
    }

    /// 上传本机改动；先拉远端比对版本，遇到冲突就只记状态、不覆盖。
    pub(crate) async fn upload_settings_sync_with(
        &self,
        backend: &dyn SettingsSyncBackend,
        force: bool,
    ) -> Result<SettingsSyncStatus, EngineError> {
        let record = self.require_enabled_record()?;
        if !force && self.pending_conflict()?.is_some() {
            return Err(EngineError::BadRequest(
                "还有没处理的冲突，请先选保留哪一边".to_string(),
            ));
        }
        let token = self.require_github_token()?;
        let gist_id = self.require_gist_id(&record)?;

        // 上传前一律先拉一次远端，避免盲目覆盖别人的改动。
        let remote = match backend.read_gist(token.expose(), &gist_id).await {
            Ok(bytes) => SyncEnvelope::parse(&bytes).ok(),
            Err(err) => {
                self.record_last_error(&err.to_string());
                return Err(err);
            }
        };

        if let Some(envelope) = &remote {
            let change = classify_remote_change(
                &LocalSyncVersions {
                    device_id: record.device_id.clone(),
                    revision: record.revision,
                    remote_revision: record.remote_revision,
                    has_local_changes: true,
                },
                envelope,
            );
            if !force && change == RemoteChange::Conflict {
                let conflict = SettingsSyncConflict {
                    remote_device_id: envelope.device_id.clone(),
                    remote_device_label: envelope.device_label.clone(),
                    remote_revision: envelope.revision,
                    updated_at: envelope.updated_at.clone(),
                };
                self.write_conflict(&conflict)?;
                return self.settings_sync_status();
            }
        }

        let revision = record
            .revision
            .max(remote.as_ref().map(|env| env.revision).unwrap_or(0))
            + 1;
        let snapshot = self.export_settings_snapshot(&record.device_id, &record.device_label, revision)?;
        let password = self.require_sync_password()?;
        let (bytes, _envelope) = encrypt_snapshot(&snapshot, password.expose())?;

        if let Err(err) = backend.update_gist(token.expose(), &gist_id, &bytes).await {
            self.record_last_error(&err.to_string());
            return Err(err);
        }

        let mut updated = record.clone();
        updated.revision = revision;
        updated.remote_revision = revision;
        if let Err(err) = self.write_sync_record(&updated) {
            self.record_last_error(&err.to_string());
            return Err(err);
        }
        self.clear_conflict()?;
        self.record_success_time()?;
        self.settings_sync_status()
    }

    /// 拉取远端并按判定导入；`force` 为真时忽略冲突直接覆盖（用户选了「用对方的」）。
    pub(crate) async fn pull_settings_sync_with(
        &self,
        backend: &dyn SettingsSyncBackend,
        force: bool,
    ) -> Result<SettingsSyncStatus, EngineError> {
        let record = self.require_enabled_record()?;
        let token = self.require_github_token()?;
        let gist_id = self.require_gist_id(&record)?;

        let bytes = match backend.read_gist(token.expose(), &gist_id).await {
            Ok(bytes) => bytes,
            Err(err) => {
                self.record_last_error(&err.to_string());
                return Err(err);
            }
        };
        let envelope = SyncEnvelope::parse(&bytes)?;

        let change = if force {
            RemoteChange::ApplyRemote
        } else {
            classify_remote_change(
                &LocalSyncVersions {
                    device_id: record.device_id.clone(),
                    revision: record.revision,
                    remote_revision: record.remote_revision,
                    has_local_changes: self.settings_sync_live.has_pending(),
                },
                &envelope,
            )
        };

        match change {
            RemoteChange::UpToDate | RemoteChange::OwnUpload => {
                let mut updated = record.clone();
                updated.remote_revision = updated.remote_revision.max(envelope.revision);
                if change == RemoteChange::OwnUpload {
                    updated.revision = updated.revision.max(envelope.revision);
                }
                self.write_sync_record(&updated)?;
                if force {
                    self.clear_conflict()?;
                }
                self.record_success_time()?;
                self.settings_sync_status()
            }
            RemoteChange::ApplyRemote => {
                if let Err(err) = self.apply_remote_snapshot(&bytes) {
                    self.record_last_error(&err.to_string());
                    return Err(err);
                }
                let mut updated = record.clone();
                updated.revision = envelope.revision;
                updated.remote_revision = envelope.revision;
                self.write_sync_record(&updated)?;
                self.clear_conflict()?;
                self.record_success_time()?;
                self.settings_sync_status()
            }
            RemoteChange::Conflict => {
                let conflict = SettingsSyncConflict {
                    remote_device_id: envelope.device_id.clone(),
                    remote_device_label: envelope.device_label.clone(),
                    remote_revision: envelope.revision,
                    updated_at: envelope.updated_at.clone(),
                };
                self.write_conflict(&conflict)?;
                self.settings_sync_status()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mail_domain::account::{AccountDraft, AccountProxyMode, AuthType, Security, ServerConfig};
    use mail_domain::proxy::{ProxyConfig, ProxyKind, Secret};
    use mail_store::{NotifyNewMailWhen, SETTING_NOTIFY_NEW_MAIL};
    use mail_sync::envelope::{EnvelopeKdfParams, SyncEnvelope, FORMAT_NAME, KDF_ARGON2ID, SCHEMA_VERSION};
    use mail_sync::gist::{self, GistSummary};
    use mail_sync::github::GitHubProfile;

    use crate::engine::{EngineError, MailEngine};
    use crate::secrets::{MemorySecretStore, SecretStore, SecretStoreError};

    use super::{
        classify_remote_change, BackendFuture, LocalSyncVersions, RemoteChange, SettingsSyncBackend,
        SyncedToggles, SYNC_PASSWORD_KEY,
    };

    fn engine(dir: &std::path::Path) -> (MailEngine, Arc<MemorySecretStore>) {
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = MailEngine::initialize_with_secrets(dir, secrets.clone()).expect("初始化引擎");
        (engine, secrets)
    }

    fn profile() -> GitHubProfile {
        GitHubProfile {
            login: "octocat".to_string(),
            name: Some("八爪猫".to_string()),
            avatar_url: Some("https://avatars.githubusercontent.com/u/1".to_string()),
        }
    }

    fn envelope(device_id: &str, revision: u64) -> SyncEnvelope {
        SyncEnvelope {
            format: FORMAT_NAME.to_string(),
            schema_version: SCHEMA_VERSION,
            kdf: KDF_ARGON2ID.to_string(),
            kdf_params: EnvelopeKdfParams { m: 65536, t: 3, p: 1 },
            salt: vec![1, 2, 3, 4],
            nonce: vec![0_u8; 12],
            ciphertext: vec![7, 8, 9],
            device_id: device_id.to_string(),
            device_label: "对方".to_string(),
            revision,
            updated_at: "2026-10-09T00:00:00Z".to_string(),
        }
    }

    /// 内存假 Gist：记录每个 Gist 的正文和调用次数，方便断言「只传了一次」。
    #[derive(Default)]
    struct FakeGist {
        inner: std::sync::Mutex<FakeGistState>,
    }

    #[derive(Default)]
    struct FakeGistState {
        gists: Vec<FakeGistEntry>,
        next_id: usize,
        creates: usize,
        updates: usize,
        deletes: usize,
        fail_delete: Option<String>,
    }

    struct FakeGistEntry {
        id: String,
        content: Vec<u8>,
    }

    impl FakeGist {
        fn new() -> Self {
            Self::default()
        }

        fn stored(&self) -> Option<Vec<u8>> {
            self.inner
                .lock()
                .unwrap()
                .gists
                .last()
                .map(|entry| entry.content.clone())
        }

        fn creates(&self) -> usize {
            self.inner.lock().unwrap().creates
        }

        fn updates(&self) -> usize {
            self.inner.lock().unwrap().updates
        }

        fn deletes(&self) -> usize {
            self.inner.lock().unwrap().deletes
        }

        fn fail_delete(&self, id: &str) {
            self.inner.lock().unwrap().fail_delete = Some(id.to_string());
        }
    }

    fn summary(id: String) -> GistSummary {
        GistSummary {
            id,
            description: gist::GIST_DESCRIPTION.to_string(),
            updated_at: "2026-10-09T00:00:00Z".to_string(),
            public: false,
        }
    }

    impl SettingsSyncBackend for FakeGist {
        fn find_gist<'a>(&'a self, _token: &'a str) -> BackendFuture<'a, Option<GistSummary>> {
            Box::pin(async move {
                let guard = self.inner.lock().unwrap();
                Ok(guard.gists.last().map(|entry| summary(entry.id.clone())))
            })
        }

        fn create_gist<'a>(&'a self, _token: &'a str, content: &'a [u8]) -> BackendFuture<'a, GistSummary> {
            Box::pin(async move {
                let mut guard = self.inner.lock().unwrap();
                guard.next_id += 1;
                let id = format!("gist-{}", guard.next_id);
                guard.gists.push(FakeGistEntry {
                    id: id.clone(),
                    content: content.to_vec(),
                });
                guard.creates += 1;
                Ok(summary(id))
            })
        }

        fn read_gist<'a>(&'a self, _token: &'a str, gist_id: &'a str) -> BackendFuture<'a, Vec<u8>> {
            Box::pin(async move {
                let guard = self.inner.lock().unwrap();
                guard
                    .gists
                    .iter()
                    .find(|entry| entry.id == gist_id)
                    .map(|entry| entry.content.clone())
                    .ok_or_else(|| EngineError::BadRequest("没有这个 Gist".to_string()))
            })
        }

        fn update_gist<'a>(
            &'a self,
            _token: &'a str,
            gist_id: &'a str,
            content: &'a [u8],
        ) -> BackendFuture<'a, ()> {
            Box::pin(async move {
                let mut guard = self.inner.lock().unwrap();
                let entry = guard
                    .gists
                    .iter_mut()
                    .find(|entry| entry.id == gist_id)
                    .ok_or_else(|| EngineError::BadRequest("没有这个 Gist".to_string()))?;
                entry.content = content.to_vec();
                guard.updates += 1;
                Ok(())
            })
        }

        fn delete_gist<'a>(&'a self, _token: &'a str, gist_id: &'a str) -> BackendFuture<'a, ()> {
            Box::pin(async move {
                let mut guard = self.inner.lock().unwrap();
                if guard.fail_delete.as_deref() == Some(gist_id) {
                    return Err(EngineError::BadRequest("删不掉".to_string()));
                }
                let before = guard.gists.len();
                guard.gists.retain(|entry| entry.id != gist_id);
                if guard.gists.len() != before {
                    guard.deletes += 1;
                }
                Ok(())
            })
        }
    }

    #[test]
    fn 界面开关能写进setting表再读回来() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _) = engine(dir.path());
        let toggles = SyncedToggles {
            notify_new_mail: false,
            notify_ai_enabled: true,
            notify_new_mail_when: Some(NotifyNewMailWhen::Background),
            block_remote_images_by_default: false,
            minimize_to_tray_on_close: false,
            start_minimized_to_tray: true,
        };

        assert!(
            engine.store_synced_toggles(&toggles).expect("写入"),
            "首次该算有变化"
        );
        assert_eq!(engine.read_synced_toggles().expect("读回"), toggles);
        assert!(
            !engine.store_synced_toggles(&toggles).expect("再次写入"),
            "值没变不该算变化"
        );

        // `None` 表示这次不带这个值：表里旧值原样保留。
        let untouched = SyncedToggles {
            notify_new_mail_when: None,
            ..toggles
        };
        assert!(
            !engine.store_synced_toggles(&untouched).expect("带空值写入"),
            "空值不动表里旧值，不该算变化"
        );
        assert_eq!(
            engine.read_synced_toggles().expect("读回").notify_new_mail_when,
            Some(NotifyNewMailWhen::Background),
            "本机选择要保住"
        );
    }

    #[test]
    fn 冲突判定覆盖四类情形() {
        let local = LocalSyncVersions {
            device_id: "me".to_string(),
            revision: 2,
            remote_revision: 2,
            has_local_changes: false,
        };

        assert_eq!(
            classify_remote_change(&local, &envelope("other", 2)),
            RemoteChange::UpToDate,
            "版本没涨就是没变化"
        );
        assert_eq!(
            classify_remote_change(&local, &envelope("me", 3)),
            RemoteChange::OwnUpload,
            "远端是本机写的"
        );
        assert_eq!(
            classify_remote_change(&local, &envelope("other", 3)),
            RemoteChange::ApplyRemote,
            "别的设备写的、本机没改动，直接导入"
        );

        let dirty = LocalSyncVersions {
            has_local_changes: true,
            ..local
        };
        assert_eq!(
            classify_remote_change(&dirty, &envelope("other", 3)),
            RemoteChange::Conflict,
            "两边都改过必须让用户选"
        );
    }

    #[tokio::test]
    async fn 开启同步会建私密gist并记住状态() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());
        engine
            .save_github_login(&Secret::new("token"), &profile())
            .expect("登录");
        let backend = FakeGist::new();

        let status = engine
            .enable_settings_sync_with("password-123", "我的电脑", false, &backend)
            .await
            .expect("开启同步");

        assert!(status.enabled, "开启后状态该是已开启");
        assert_eq!(status.revision, 1);
        assert_eq!(status.gist_id.as_deref(), Some("gist-1"));
        assert!(status
            .gist_url
            .as_deref()
            .unwrap()
            .starts_with("https://gist.github.com/"));
        assert!(backend.stored().is_some(), "云端该有内容");
        assert!(secrets.contains(SYNC_PASSWORD_KEY), "同步密码该进保险箱");
    }

    #[tokio::test]
    async fn 单次密码太短时不能上传() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());
        let backend = FakeGist::new();
        let result = engine
            .enable_settings_sync_with("short", "本机", false, &backend)
            .await;
        assert!(result.unwrap_err().to_string().contains("至少 8 位"));
        assert_eq!(backend.creates(), 0);
        assert!(!secrets.contains(SYNC_PASSWORD_KEY));
        assert!(!engine.settings_sync_status().expect("状态").enabled);
    }

    #[tokio::test]
    async fn 云端已有内容必须先确认且覆盖后版本继续递增() {
        let dir_a = tempfile::tempdir().expect("临时目录");
        let (engine_a, _) = engine(dir_a.path());
        engine_a
            .save_github_login(&Secret::new("token-a"), &profile())
            .expect("登录");
        let backend = FakeGist::new();
        let initial = engine_a
            .enable_settings_sync_with("old-password", "甲机", false, &backend)
            .await
            .expect("首次上传");
        engine_a
            .upload_settings_sync_with(&backend, true)
            .await
            .expect("版本增加");
        let original = backend.stored().expect("原内容");

        let dir_b = tempfile::tempdir().expect("临时目录");
        let (engine_b, secrets_b) = engine(dir_b.path());
        engine_b
            .save_github_login(&Secret::new("token-b"), &profile())
            .expect("登录");
        engine_b
            .store()
            .set_setting(SETTING_NOTIFY_NEW_MAIL, "false")
            .expect("本机配置");
        let blocked = engine_b
            .enable_settings_sync_with("new-password", "乙机", false, &backend)
            .await
            .unwrap_err();
        assert!(blocked.to_string().contains("云端已有同步内容"));
        assert_eq!(backend.stored().expect("未覆盖"), original);
        assert_eq!(backend.updates(), 1);
        assert!(!secrets_b.contains(SYNC_PASSWORD_KEY));
        assert!(!engine_b.settings_sync_status().expect("状态").enabled);

        let uploaded = engine_b
            .enable_settings_sync_with("new-password", "乙机", true, &backend)
            .await
            .expect("确认覆盖");
        assert_eq!(uploaded.gist_id, initial.gist_id);
        assert_eq!(uploaded.revision, 3);
        assert_eq!(uploaded.remote_revision, 3);
        assert_eq!(backend.creates(), 1, "覆盖应沿用原 Gist");
        assert_eq!(backend.updates(), 2);
        let (snapshot, _) = super::decrypt_snapshot(&backend.stored().unwrap(), "new-password")
            .expect("用上面输入的密码加密");
        assert!(!snapshot.settings.notify_new_mail);
        assert_eq!(snapshot.device_label, "乙机");
        assert_eq!(
            secrets_b.plain(SYNC_PASSWORD_KEY).as_deref(),
            Some("new-password")
        );
    }

    /// 只让同步密码的一次写入失败，验证上传后本机落盘失败能恢复云端原内容。
    #[derive(Default)]
    struct FailSyncPasswordOnce {
        inner: MemorySecretStore,
        fail: std::sync::atomic::AtomicBool,
    }

    impl SecretStore for FailSyncPasswordOnce {
        fn get(&self, key: &str) -> Result<Option<Secret>, SecretStoreError> {
            self.inner.get(key)
        }

        fn set(&self, key: &str, value: &Secret) -> Result<(), SecretStoreError> {
            if key == SYNC_PASSWORD_KEY && self.fail.swap(false, std::sync::atomic::Ordering::SeqCst) {
                return Err(SecretStoreError::Backend);
            }
            self.inner.set(key, value)
        }

        fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
            self.inner.delete(key)
        }
    }

    #[tokio::test]
    async fn 本机保存密码失败时恢复云端且不误删已有内容() {
        for has_remote in [false, true] {
            let dir = tempfile::tempdir().expect("临时目录");
            let secrets = Arc::new(FailSyncPasswordOnce::default());
            let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("初始化");
            engine
                .save_github_login(&Secret::new("token"), &profile())
                .expect("登录");
            let backend = FakeGist::new();
            let original = b"original encrypted content";
            if has_remote {
                backend.create_gist("token", original).await.expect("已有云端");
            }
            secrets
                .inner
                .set(SYNC_PASSWORD_KEY, &Secret::new("previous-password"))
                .expect("原密码");
            secrets.fail.store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(engine
                .enable_settings_sync_with("new-password", "本机", true, &backend)
                .await
                .is_err());
            assert!(!engine.settings_sync_status().expect("状态").enabled);
            assert_eq!(
                secrets.inner.plain(SYNC_PASSWORD_KEY).as_deref(),
                Some("previous-password")
            );
            if has_remote {
                assert_eq!(backend.stored().as_deref(), Some(original.as_slice()));
                assert_eq!(backend.deletes(), 0, "不得删除原有 Gist");
            } else {
                assert!(backend.stored().is_none(), "回滚本次新建 Gist");
                assert_eq!(backend.deletes(), 1);
            }
        }
    }

    #[tokio::test]
    async fn 下载覆盖邮箱与代理前必须确认且取消不写本机() {
        let dir_a = tempfile::tempdir().expect("临时目录");
        let (engine_a, _) = engine(dir_a.path());
        engine_a
            .save_github_login(&Secret::new("token-a"), &profile())
            .expect("登录");
        let remote_account = AccountDraft {
            display_name: "云端邮箱".to_string(),
            email: "test@example.com".to_string(),
            auth_type: AuthType::Password,
            username: "test@example.com".to_string(),
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
            proxy: AccountProxyMode::Direct,
            color: String::new(),
            enabled: true,
            oauth_provider: None,
            oauth_client_id: String::new(),
        };
        let remote_proxy = ProxyConfig {
            id: None,
            label: "公司代理".to_string(),
            kind: ProxyKind::Socks5,
            host: "proxy.example.com".to_string(),
            port: 1080,
            username: "remote-user".to_string(),
        };
        engine_a
            .store()
            .insert_account(&remote_account, None)
            .expect("云端邮箱");
        engine_a
            .store()
            .insert_proxy(&remote_proxy, None)
            .expect("云端代理");
        let backend = FakeGist::new();
        engine_a
            .enable_settings_sync_with("password-123", "甲机", false, &backend)
            .await
            .expect("上传");

        let dir_b = tempfile::tempdir().expect("临时目录");
        let (engine_b, secrets_b) = engine(dir_b.path());
        engine_b
            .save_github_login(&Secret::new("token-b"), &profile())
            .expect("登录");
        let local_account = AccountDraft {
            display_name: "本机邮箱".to_string(),
            ..remote_account
        };
        let local_proxy = ProxyConfig {
            username: "local-user".to_string(),
            ..remote_proxy
        };
        engine_b
            .store()
            .insert_account(&local_account, None)
            .expect("本机邮箱");
        engine_b
            .store()
            .insert_proxy(&local_proxy, None)
            .expect("本机代理");
        let blocked = engine_b
            .download_settings_sync_with("password-123", "乙机", false, &backend)
            .await
            .unwrap_err()
            .to_string();
        assert!(blocked.contains("邮箱 test@example.com"));
        assert!(blocked.contains("代理 公司代理"));
        assert_eq!(engine_b.list_accounts().unwrap()[0].display_name, "本机邮箱");
        assert_eq!(
            engine_b.store().list_proxies().unwrap()[0].config.username,
            "local-user"
        );
        assert!(!secrets_b.contains(SYNC_PASSWORD_KEY));
        assert!(!engine_b.settings_sync_status().unwrap().enabled);

        let downloaded = engine_b
            .download_settings_sync_with("password-123", "乙机", true, &backend)
            .await
            .expect("确认后下载");
        assert!(downloaded.enabled);
        assert_eq!(engine_b.list_accounts().unwrap()[0].display_name, "云端邮箱");
        assert_eq!(
            engine_b.store().list_proxies().unwrap()[0].config.username,
            "remote-user"
        );
        assert_eq!(
            secrets_b.plain(SYNC_PASSWORD_KEY).as_deref(),
            Some("password-123")
        );
        assert_eq!(backend.updates(), 0, "下载不能改云端");
    }

    #[tokio::test]
    async fn 同步成功后可以重复上传下载并保留设备标识() {
        let dir_a = tempfile::tempdir().expect("临时目录");
        let (engine_a, _) = engine(dir_a.path());
        engine_a
            .save_github_login(&Secret::new("token-a"), &profile())
            .expect("登录");
        let backend = FakeGist::new();
        let first = engine_a
            .enable_settings_sync_with("password-123", "甲机", false, &backend)
            .await
            .expect("首次上传");
        engine_a
            .store()
            .set_setting(SETTING_NOTIFY_NEW_MAIL, "false")
            .expect("修改本机");
        engine_a
            .write_conflict(&super::SettingsSyncConflict {
                remote_device_id: "other".to_string(),
                remote_device_label: "对方".to_string(),
                remote_revision: 1,
                updated_at: "2026-10-18T00:00:00Z".to_string(),
            })
            .expect("待处理冲突");
        let blocked = engine_a
            .enable_settings_sync_with("password-123", "甲机", false, &backend)
            .await
            .unwrap_err()
            .to_string();
        assert!(blocked.contains("云端已有同步内容"), "应继续走覆盖确认");
        let second = engine_a
            .enable_settings_sync_with("password-123", "甲机", true, &backend)
            .await
            .expect("再次上传");
        assert_eq!(second.device_id, first.device_id);
        assert_eq!(second.gist_id, first.gist_id);
        assert_eq!(second.revision, 2);
        assert!(second.conflict.is_none(), "手动上传已选择本机");
        assert_eq!(backend.creates(), 1);

        let dir_b = tempfile::tempdir().expect("临时目录");
        let (engine_b, _) = engine(dir_b.path());
        engine_b
            .save_github_login(&Secret::new("token-b"), &profile())
            .expect("登录");
        let joined = engine_b
            .download_settings_sync_with("password-123", "乙机", false, &backend)
            .await
            .expect("下载");
        engine_b
            .store()
            .set_setting(SETTING_NOTIFY_NEW_MAIL, "true")
            .expect("修改乙机");
        let uploaded = engine_b
            .enable_settings_sync_with("password-123", "乙机", true, &backend)
            .await
            .expect("下载后上传");
        assert_eq!(uploaded.device_id, joined.device_id);
        assert_eq!(uploaded.revision, 3);

        let downloaded = engine_a
            .download_settings_sync_with("password-123", "甲机", false, &backend)
            .await
            .expect("上传后下载");
        assert_eq!(downloaded.device_id, first.device_id);
        assert!(engine_a.read_synced_toggles().unwrap().notify_new_mail);
        let repeated = engine_a
            .download_settings_sync_with("password-123", "甲机", false, &backend)
            .await
            .expect("再次下载");
        assert_eq!(repeated.device_id, first.device_id);
        assert_eq!(repeated.revision, 3);
        assert_eq!(backend.updates(), 2, "下载不应改动云端");
    }

    #[tokio::test]
    async fn 云端没有内容时下载不写本机() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());
        engine
            .save_github_login(&Secret::new("token"), &profile())
            .expect("登录");
        let backend = FakeGist::new();
        let result = engine
            .download_settings_sync_with("password-123", "本机", false, &backend)
            .await;
        assert!(result.unwrap_err().to_string().contains("没有找到同步数据"));
        assert!(!engine.settings_sync_status().unwrap().enabled);
        assert!(!secrets.contains(SYNC_PASSWORD_KEY));
        assert_eq!(backend.creates(), 0);
    }

    #[tokio::test]
    async fn 上传到云端的正文只有密文且改一字节解不开() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _) = engine(dir.path());
        engine
            .save_github_login(&Secret::new("gho-token-9527"), &profile())
            .expect("登录");
        let backend = FakeGist::new();
        engine
            .enable_settings_sync_with("password-123", "我的电脑", false, &backend)
            .await
            .expect("开启同步");

        let stored = backend.stored().expect("云端该有内容");
        let surface = String::from_utf8_lossy(&stored);
        assert!(
            !surface.contains("password-123"),
            "上传到云端的正文不该出现同步密码明文"
        );

        // 用对的密码能原样解回来。
        let (snapshot, _) = super::decrypt_snapshot(&stored, "password-123").expect("正常解密");
        assert_eq!(snapshot.device_label, "我的电脑");

        // 改一个字节就解不开（靠完整性校验挡住）。
        let mut tampered = stored.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert!(
            super::decrypt_snapshot(&tampered, "password-123").is_err(),
            "被改过的云端正文必须解不开"
        );
    }
    #[tokio::test]
    async fn 关闭同步会清状态并按需删远端() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());
        engine
            .save_github_login(&Secret::new("token"), &profile())
            .expect("登录");
        let backend = FakeGist::new();
        engine
            .enable_settings_sync_with("password-123", "我的电脑", false, &backend)
            .await
            .expect("开启同步");

        let status = engine
            .disable_settings_sync_with(true, &backend)
            .await
            .expect("关闭同步");

        assert!(!status.enabled);
        assert!(backend.stored().is_none(), "云端那份该删掉");
        assert!(!secrets.contains(SYNC_PASSWORD_KEY), "同步密码该清掉");
        assert!(
            engine.github_token().expect("读令牌").is_none(),
            "关掉同步要顺带退出登录"
        );
    }

    #[tokio::test]
    async fn 重设密码后旧密码解不开() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine_a, _) = engine(dir.path());
        engine_a
            .save_github_login(&Secret::new("token"), &profile())
            .expect("登录");
        let backend = FakeGist::new();
        engine_a
            .enable_settings_sync_with("old-password-1", "我的电脑", false, &backend)
            .await
            .expect("开启同步");

        let status = engine_a
            .reset_settings_sync_password_with("new-password-2", "new-password-2", &backend)
            .await
            .expect("重设密码");
        assert_eq!(status.gist_id.as_deref(), Some("gist-2"), "该换成新 Gist");
        assert_eq!(backend.creates(), 2);
        assert_eq!(backend.deletes(), 1, "旧 Gist 该被删掉");
        assert!(status.last_error.is_none(), "都成功时不该有残留提示");

        let dir_b = tempfile::tempdir().expect("临时目录");
        let (engine_b, _) = engine(dir_b.path());
        engine_b
            .save_github_login(&Secret::new("token-b"), &profile())
            .expect("登录");
        let old = engine_b
            .download_settings_sync_with("old-password-1", "乙机", false, &backend)
            .await;
        assert!(old.is_err(), "旧密码该解不开新 Gist");
        assert!(!engine_b.settings_sync_status().expect("状态").enabled);

        let joined = engine_b
            .download_settings_sync_with("new-password-2", "乙机", false, &backend)
            .await
            .expect("新密码加入");
        assert!(joined.enabled);
    }

    #[tokio::test]
    async fn 从云端下载能拉到并导入配置() {
        let dir_a = tempfile::tempdir().expect("临时目录");
        let (engine_a, _) = engine(dir_a.path());
        engine_a
            .save_github_login(&Secret::new("token-a"), &profile())
            .expect("登录");
        let backend = FakeGist::new();
        engine_a
            .enable_settings_sync_with("password-123", "甲机", false, &backend)
            .await
            .expect("开启同步");
        {
            let store = engine_a.store();
            store
                .set_setting(SETTING_NOTIFY_NEW_MAIL, "false")
                .expect("改开关");
        }
        engine_a
            .upload_settings_sync_with(&backend, true)
            .await
            .expect("上传改动");

        let dir_b = tempfile::tempdir().expect("临时目录");
        let (engine_b, _) = engine(dir_b.path());
        engine_b
            .save_github_login(&Secret::new("token-b"), &profile())
            .expect("登录");
        let status = engine_b
            .download_settings_sync_with("password-123", "乙机", false, &backend)
            .await
            .expect("加入同步");

        assert!(status.enabled);
        assert!(status.device_id.is_some(), "乙机该有自己的设备号");
        assert!(
            !engine_b.read_synced_toggles().expect("读开关").notify_new_mail,
            "该把开关导过来"
        );
    }

    #[tokio::test]
    async fn 连续改动合并成一次上传() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _) = engine(dir.path());
        engine
            .save_github_login(&Secret::new("token"), &profile())
            .expect("登录");
        let backend = FakeGist::new();
        engine
            .enable_settings_sync_with("password-123", "我的电脑", false, &backend)
            .await
            .expect("开启同步");

        engine.mark_settings_changed();
        engine.mark_settings_changed();
        engine.mark_settings_changed();
        let first = engine
            .flush_pending_settings_sync_with(&backend)
            .await
            .expect("去抖上传");
        assert!(first.is_some(), "有待同步标记时该真上传");
        assert_eq!(backend.updates(), 1, "三次改动该并成一次上传");

        let second = engine
            .flush_pending_settings_sync_with(&backend)
            .await
            .expect("再冲一次");
        assert!(second.is_none(), "标记取走后不该再传");
        assert_eq!(backend.updates(), 1);
    }

    #[tokio::test]
    async fn 旧gist删不掉会留下明确提示() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _) = engine(dir.path());
        engine
            .save_github_login(&Secret::new("token"), &profile())
            .expect("登录");
        let backend = FakeGist::new();
        engine
            .enable_settings_sync_with("password-123", "我的电脑", false, &backend)
            .await
            .expect("开启同步");
        backend.fail_delete("gist-1");

        let status = engine
            .reset_settings_sync_password_with("new-password-2", "new-password-2", &backend)
            .await
            .expect("重设密码");

        let message = status.last_error.unwrap_or_default();
        assert!(message.contains("手动删除"), "该提示用户手动删：{message}");
        assert!(message.contains("gist-1"), "提示里该带旧地址：{message}");
    }
}
