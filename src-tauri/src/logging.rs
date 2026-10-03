//! 日志初始化与句柄保管。
//!
//! 约定（决策 W0-D4）：日志默认只写本地文件，不外传；写日志走后台线程，避免拖慢启动。
//! 日志目录创建失败时降级为标准输出，保证外壳仍能启动。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// 日志系统的存活句柄。
///
/// `tracing-appender` 的后台写线程由 `WorkerGuard` 控制：它一旦被丢弃，缓冲里的日志就可能丢失。
/// 因此本对象必须与进程同寿命，由外壳的应用状态持有。
/// 句柄套一层 `Mutex`，是为了让本类型无论 `WorkerGuard` 是否 `Sync` 都能安全放进 Tauri 状态。
pub struct Logging {
    /// 后台写线程句柄；字段本身不参与业务逻辑，只负责「保活」。
    guard: Mutex<Option<WorkerGuard>>,
    /// 日志目录（文件名形如 `em-master.log.<日期>`）。
    dir: PathBuf,
}

impl Logging {
    /// 初始化全局日志订阅者。
    ///
    /// 级别可用环境变量 `RUST_LOG` 覆盖，默认 `info`。重复初始化不会中断启动，只提示一声。
    pub fn init(log_dir: impl AsRef<Path>) -> Self {
        let dir = log_dir.as_ref().to_path_buf();
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

        let mut guard = None;
        let init_result = match std::fs::create_dir_all(&dir) {
            Ok(()) => {
                let appender = tracing_appender::rolling::daily(&dir, "em-master.log");
                let (writer, worker_guard) = tracing_appender::non_blocking(appender);
                guard = Some(worker_guard);
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_ansi(false)
                    .with_writer(writer)
                    .try_init()
            }
            Err(err) => {
                eprintln!("无法创建日志目录 {}：{err}；日志改写到标准输出", dir.display());
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_ansi(false)
                    .try_init()
            }
        };

        if let Err(err) = init_result {
            eprintln!("日志订阅者已存在，跳过初始化：{err}");
        }

        Self {
            guard: Mutex::new(guard),
            dir,
        }
    }

    /// 日志目录。
    pub fn dir(&self) -> &Path {
        // 访问一次 guard：确认「本对象存活 = 日志线程存活」的语义。
        if let Ok(guard) = self.guard.lock() {
            let _keep_alive = guard.as_ref();
        }
        &self.dir
    }
}
