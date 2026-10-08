//! 应用状态。
//!
//! 关键约束：
//! 1. `rusqlite::Connection` 可以跨线程移动（`Send`），但不能被多个线程共享（不是 `Sync`）。
//!    Tauri 的应用状态要求 `Send + Sync`，所以引擎句柄必须套一层锁。
//! 2. 连接自检最长可跑十几秒，不能阻塞界面线程，所以锁用 `tokio::sync::Mutex`：
//!    命令是 `async` 的，自检期间会让出执行权，界面照常响应。
//! 3. 状态里同时保存不可变的初始化摘要，避免每次查询都加锁。
//! 4. 存储目录与通知开关存在默认应用数据目录的 `settings.json`，界面改完立刻写盘。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{Mutex, MutexGuard};

use mail_core::{EngineInit, MailEngine};

use crate::logging::Logging;
use crate::settings::AppSettings;

/// 外壳状态：引擎句柄（加锁）+ 只读摘要 + 日志句柄 + 应用设置。
pub struct AppState {
    engine: Mutex<MailEngine>,
    init: EngineInit,
    /// 日志句柄：字段本身不再被读，但必须留着——它一被丢弃，后台写日志的线程就停了。
    #[allow(dead_code)]
    logging: Logging,
    /// 默认应用数据目录；`settings.json` 固定放这里。
    default_data_dir: PathBuf,
    /// 当前设置（与磁盘上的 `settings.json` 保持同步）。
    settings: StdMutex<AppSettings>,
    /// 新邮件通知开关；后台轮询线程直接读它，改了立刻生效。
    notify_enabled: Arc<AtomicBool>,
    /// 读信是否默认拦截远程图片；与引擎共用同一个原子开关，改设置立刻生效。
    block_remote_images: Arc<AtomicBool>,
    /// 数据目录迁移是否正在跑；防止界面重复触发。
    migrating_data_dir: AtomicBool,
    /// 本次启动是不是全新安装后的第一次；界面据此弹「数据放哪」向导。
    first_run: bool,
    /// 迁移成功后暂存的旧数据目录；重启命令直接用它，不接受界面传路径。
    pending_cleanup_dir: StdMutex<Option<PathBuf>>,
}

impl AppState {
    /// 构造状态。
    pub fn new(
        engine: MailEngine,
        logging: Logging,
        default_data_dir: PathBuf,
        settings: AppSettings,
        notify_enabled: Arc<AtomicBool>,
        first_run: bool,
    ) -> Self {
        let init = engine.init_summary();
        // 引擎自己持有这个原子开关，外壳复用同一个句柄，改设置就能让读信立刻换规则。
        let block_remote_images = engine.block_remote_images_handle();
        block_remote_images.store(settings.block_remote_images_by_default, Ordering::Relaxed);
        Self {
            engine: Mutex::new(engine),
            init,
            logging,
            default_data_dir,
            settings: StdMutex::new(settings),
            notify_enabled,
            block_remote_images,
            migrating_data_dir: AtomicBool::new(false),
            first_run,
            pending_cleanup_dir: StdMutex::new(None),
        }
    }

    /// 取得引擎句柄；调用方持有返回值期间独占引擎（自检、写库都经它）。
    pub(crate) async fn engine(&self) -> MutexGuard<'_, MailEngine> {
        self.engine.lock().await
    }

    /// 默认应用数据目录。
    pub fn default_data_dir(&self) -> &Path {
        &self.default_data_dir
    }

    /// 本次启动是不是全新安装后的第一次。
    pub fn first_run(&self) -> bool {
        self.first_run
    }

    /// 当前设置的快照。
    pub fn settings_snapshot(&self) -> AppSettings {
        self.settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 覆盖内存里的设置，并写回磁盘。
    ///
    /// 先写盘再改内存：写失败就整体不动，避免界面显示的和磁盘上的不一致。
    pub fn save_settings(&self, settings: AppSettings) -> std::io::Result<()> {
        settings.save(&self.default_data_dir)?;
        self.notify_enabled
            .store(settings.notify_new_mail, Ordering::Relaxed);
        self.block_remote_images
            .store(settings.block_remote_images_by_default, Ordering::Relaxed);
        let mut guard = self
            .settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = settings;
        Ok(())
    }

    /// 尝试开始一次数据目录迁移；已经在跑时返回 `false`。
    pub fn try_begin_data_dir_migration(&self) -> bool {
        self.migrating_data_dir
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// 结束数据目录迁移标记。
    pub fn finish_data_dir_migration(&self) {
        self.migrating_data_dir.store(false, Ordering::Release);
    }

    /// 记住本次迁移前的旧数据目录，等重启命令来决定清不清。
    pub fn set_pending_cleanup_dir(&self, dir: Option<PathBuf>) {
        let mut guard = self
            .pending_cleanup_dir
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = dir;
    }

    /// 读当前暂存的旧数据目录；只有后端重启命令会用，界面拿不到路径。
    pub fn pending_cleanup_dir(&self) -> Option<PathBuf> {
        self.pending_cleanup_dir
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 清掉暂存的旧数据目录。
    pub fn clear_pending_cleanup_dir(&self) {
        self.set_pending_cleanup_dir(None);
    }

    /// 当前引擎实际在用的邮件数据目录。
    pub fn active_data_dir(&self) -> &str {
        &self.init.root_dir
    }

    /// 当前引擎实际在用的附件目录。
    pub fn active_attachment_dir(&self) -> &str {
        &self.init.attachment_dir
    }
}
