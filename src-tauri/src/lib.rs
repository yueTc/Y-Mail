//! Tauri 2 桌面外壳。
//!
//! 职责边界（规格 4.1）：只做窗口、生命周期、命令转发；
//! 数据库与业务逻辑都在 `mail-core` / `mail-store`，本 crate 不直接开数据库。
//!
//! 启动顺序：确定应用数据目录 → 建引擎（建库 + 跑迁移）→ 起日志 → 注册状态与命令。

pub mod commands;
pub mod logging;
pub mod state;

use mail_core::MailEngine;
use tauri::Manager;

use crate::logging::Logging;
use crate::state::AppState;

/// 应用启动入口。
///
/// 初始化失败会直接退出并给出可读原因：Wave 0 的目标是「要么完整就绪，要么明确报错」，
/// 不做静默降级（例如数据库打不开却显示一个空窗口）。
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::db_status,
            commands::list_accounts,
            commands::create_account,
            commands::update_account,
            commands::delete_account,
            commands::test_account_connection,
            commands::test_saved_account,
            commands::list_proxies,
            commands::save_proxy,
            commands::delete_proxy,
            commands::get_proxy_settings,
            commands::set_proxy_settings,
            commands::test_proxy,
            commands::sync_status,
            commands::start_sync,
            commands::stop_sync,
        ])
        .setup(|app| {
            // 1) 应用数据目录。Windows 下形如 %APPDATA%\com.emmaster.desktop。
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|err| format!("无法确定应用数据目录：{err}"))?;
            std::fs::create_dir_all(&data_dir)?;

            // 2) 日志先就绪，初始化过程本身也能留下记录。
            let logging = Logging::init(data_dir.join("logs"));

            // 3) 引擎：建目录、开库、跑迁移。
            let engine = MailEngine::initialize(&data_dir)
                .map_err(|err| format!("初始化引擎失败（数据目录 {}）：{err}", data_dir.display()))?;

            let summary = engine.init_summary();
            tracing::info!(
                data_dir = %data_dir.display(),
                database = %summary.database_file,
                schema_version = summary.schema_version,
                applied = summary.applied_count(),
                fts5 = summary.fts5_available,
                "外壳初始化完成"
            );

            // 4) 把引擎与摘要交给命令层；日志句柄随之进入应用状态，保证写线程存活。
            app.manage(AppState::new(engine, logging));

            // 5) 自动启动已启用账号的后台同步；起不来只记日志，不拦应用启动。
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<AppState>();
                let engine = state.engine().await;
                match engine.start_sync(None) {
                    Ok(started) => tracing::info!(started, "后台同步已自动启动"),
                    Err(err) => tracing::warn!(error = %err, "后台同步自动启动失败"),
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}
