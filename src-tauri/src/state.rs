//! 应用状态。
//!
//! 关键约束：
//! 1. `rusqlite::Connection` 可以跨线程移动（`Send`），但不能被多个线程共享（不是 `Sync`）。
//!    这个约束由引擎内部的短锁解决，外壳不再给整个引擎套全局锁。
//! 2. 分类、自检等长任务可以各自持有共享引擎句柄；读信等其它命令不会被长任务挡住。
//! 3. 状态里同时保存不可变的初始化摘要，避免每次查询都加锁。
//! 4. 存储目录与通知开关存在默认应用数据目录的 `settings.json`，界面改完立刻写盘。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use mail_core::{EngineInit, MailEngine};

use crate::logging::Logging;
use crate::settings::AppSettings;

/// 外壳状态：共享引擎句柄 + 只读摘要 + 日志句柄 + 应用设置。
pub struct AppState {
    engine: Arc<MailEngine>,
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
    /// 通知智能识别开关；后台轮询线程直接读它，改了立刻生效。
    notify_ai_enabled: Arc<AtomicBool>,
    /// 新邮件提醒时机（`NotifyNewMailWhen` 的编号）；后台轮询线程直接读它。
    notify_new_mail_when: Arc<AtomicU8>,
    /// 邮件自动分类开关；后台轮询线程直接读它，关了立刻停止外发。
    classify_enabled: Arc<AtomicBool>,
    /// 分类把握程度门槛（百分数，50–99）；后台轮询线程直接读它。
    classify_confidence_percent: Arc<AtomicU8>,
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        engine: MailEngine,
        logging: Logging,
        default_data_dir: PathBuf,
        settings: AppSettings,
        notify_enabled: Arc<AtomicBool>,
        notify_ai_enabled: Arc<AtomicBool>,
        notify_new_mail_when: Arc<AtomicU8>,
        classify_enabled: Arc<AtomicBool>,
        classify_confidence_percent: Arc<AtomicU8>,
        first_run: bool,
    ) -> Self {
        let init = engine.init_summary();
        // 引擎自己持有这个原子开关，外壳复用同一个句柄，改设置就能让读信立刻换规则。
        let block_remote_images = engine.block_remote_images_handle();
        block_remote_images.store(settings.block_remote_images_by_default, Ordering::Relaxed);
        Self {
            engine: Arc::new(engine),
            init,
            logging,
            default_data_dir,
            settings: StdMutex::new(settings),
            notify_enabled,
            notify_ai_enabled,
            notify_new_mail_when,
            classify_enabled,
            classify_confidence_percent,
            block_remote_images,
            migrating_data_dir: AtomicBool::new(false),
            first_run,
            pending_cleanup_dir: StdMutex::new(None),
        }
    }

    /// 取得共享引擎句柄；多个命令可以并行持有，内部数据库锁仍只包住同步操作。
    pub(crate) async fn engine(&self) -> Arc<MailEngine> {
        self.engine.clone()
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
        self.notify_ai_enabled
            .store(settings.notify_ai_enabled, Ordering::Relaxed);
        self.notify_new_mail_when
            .store(settings.notify_new_mail_when.code(), Ordering::Relaxed);
        let classify_percent = settings.normalized_classify_confidence_percent();
        self.classify_enabled
            .store(settings.classify_enabled, Ordering::Relaxed);
        self.classify_confidence_percent
            .store(classify_percent, Ordering::Relaxed);
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

    /// 当前新邮件提醒时机。
    pub fn notify_new_mail_when(&self) -> mail_core::NotifyNewMailWhen {
        mail_core::NotifyNewMailWhen::from_code(self.notify_new_mail_when.load(Ordering::Relaxed))
    }

    /// 当前「邮件自动分类」开关状态。
    pub fn classify_enabled(&self) -> bool {
        self.classify_enabled.load(Ordering::Relaxed)
    }

    /// 当前分类把握程度门槛（百分数，50–99）。
    pub fn classify_confidence_percent(&self) -> u8 {
        self.classify_confidence_percent.load(Ordering::Relaxed)
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

#[cfg(test)]
mod engine_concurrency_tests {
    use std::sync::atomic::{AtomicBool, AtomicU8};
    use std::sync::Arc;
    use std::time::Duration;

    use mail_core::{MailEngine, MemorySecretStore};

    use super::AppState;
    use crate::logging::Logging;
    use crate::settings::AppSettings;

    #[tokio::test]
    async fn 取引擎句柄不会等待另一个长任务() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = MailEngine::initialize_with_secrets(dir.path(), Arc::new(MemorySecretStore::new()))
            .expect("初始化引擎");
        let settings = AppSettings::default();
        let state = AppState::new(
            engine,
            Logging::init(dir.path().join("logs")),
            dir.path().to_path_buf(),
            settings.clone(),
            Arc::new(AtomicBool::new(settings.notify_new_mail)),
            Arc::new(AtomicBool::new(settings.notify_ai_enabled)),
            Arc::new(AtomicU8::new(settings.notify_new_mail_when.code())),
            Arc::new(AtomicBool::new(settings.classify_enabled)),
            Arc::new(AtomicU8::new(settings.normalized_classify_confidence_percent())),
            false,
        );

        // 模拟一个长任务已经拿到引擎句柄；其它命令仍应马上拿到自己的句柄。
        let first = state.engine().await;
        let second = tokio::time::timeout(Duration::from_millis(50), state.engine()).await;
        assert!(second.is_ok(), "全局引擎锁不应阻塞其它命令");
        drop(first);
    }
}
