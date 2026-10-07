//! 日志初始化与句柄保管。
//!
//! 约定（决策 W0-D4）：日志默认只写本地文件，不外传。
//! 这里刻意走同步写文件：宁可慢一点，也不能在崩溃时把最后几条——也就是最关键的——记录丢在缓冲区里。
//! 早先用的是 `tracing_appender::non_blocking`，进程被强杀时缓冲随进程消失，崩溃现场就没了。
//! 日志目录创建失败时降级为标准输出，保证外壳仍能启动。

use std::path::{Path, PathBuf};

use tracing_subscriber::EnvFilter;

/// 日志系统的存活句柄。
pub struct Logging {
    /// 日志目录（文件名形如 `ymail.log.<日期>`）。
    dir: PathBuf,
}

impl Logging {
    /// 初始化全局日志订阅者。
    ///
    /// 级别可用环境变量 `RUST_LOG` 覆盖，默认 `info`。重复初始化不会中断启动，只提示一声。
    pub fn init(log_dir: impl AsRef<Path>) -> Self {
        let dir = log_dir.as_ref().to_path_buf();
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

        let init_result = match std::fs::create_dir_all(&dir) {
            Ok(()) => {
                let appender = tracing_appender::rolling::daily(&dir, "ymail.log");
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_ansi(false)
                    .with_writer(appender)
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

        Self { dir }
    }

    /// 日志目录。
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}
