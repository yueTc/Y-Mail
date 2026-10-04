//! Tauri 2 桌面外壳。
//!
//! 职责边界（规格 4.1）：只做窗口、生命周期、命令转发；
//! 数据库与业务逻辑都在 `mail-core` / `mail-store`，本 crate 不直接开数据库。
//!
//! 启动顺序：确定应用数据目录 → 建引擎（建库 + 跑迁移）→ 起日志 → 注册状态与命令。

pub mod commands;
pub mod logging;
pub mod mcp_commands;
pub mod notify;
pub mod state;

use mail_core::MailEngine;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};

use crate::logging::Logging;
use crate::state::AppState;

/// 应用启动入口。
///
/// 初始化失败会直接退出并给出可读原因：Wave 0 的目标是「要么完整就绪，要么明确报错」，
/// 不做静默降级（例如数据库打不开却显示一个空窗口）。
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            commands::db_status,
            commands::list_accounts,
            commands::create_account,
            commands::update_account,
            commands::delete_account,
            commands::test_account_connection,
            commands::test_saved_account,
            commands::begin_oauth_authorize,
            commands::complete_oauth_authorize,
            commands::cancel_oauth_authorize,
            commands::oauth_status,
            commands::list_proxies,
            commands::save_proxy,
            commands::delete_proxy,
            commands::get_proxy_settings,
            commands::set_proxy_settings,
            commands::test_proxy,
            commands::sync_status,
            commands::start_sync,
            commands::stop_sync,
            commands::inbox_summary,
            commands::list_inbox_folders,
            commands::list_inbox_messages,
            commands::list_inbox_threads,
            commands::list_thread_messages,
            commands::get_message_body,
            commands::download_attachment,
            commands::search_messages,
            commands::compose_draft,
            commands::save_draft,
            commands::enqueue_outbox,
            commands::retry_outbox,
            commands::list_outbox,
            commands::get_outbox,
            commands::delete_outbox,
            commands::search_contacts,
            commands::get_signature,
            commands::save_signature,
            commands::send_outbox,
            commands::list_ai_providers,
            commands::save_ai_provider,
            commands::delete_ai_provider,
            commands::test_ai_provider,
            commands::refresh_ai_provider_models,
            commands::list_ai_model_maps,
            commands::set_ai_feature,
            commands::clear_ai_feature,
            commands::ai_authorization_preview,
            commands::translate_message,
            commands::summarize_message,
            commands::polish_text,
            commands::draft_text,
            commands::list_ai_audit,
            commands::disable_all_ai,
            commands::clear_ai_cache,
            // Wave 8：MCP 外部接入（开关 / 工具清单 / 审计）。放在末尾，减少与其它工序的冲突。
            mcp_commands::mcp_status,
            mcp_commands::mcp_set_enabled,
            mcp_commands::mcp_set_write_tools,
            mcp_commands::mcp_tools,
            mcp_commands::mcp_audit,
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

            // 6) 托盘常驻：关窗只是收起来，后台继续收信。
            setup_tray(app)?;

            // 7) 新邮件提醒：后台轮询本地库，发现新未读就弹通知并通知界面刷新。
            notify::spawn(app.handle().clone());

            Ok(())
        })
        .on_window_event(|window, event| {
            // 主窗口按关闭键时收进托盘，不退出进程；真要退出走托盘菜单的「退出」。
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}

/// 建托盘图标与中文菜单：左键点图标显示主窗口，菜单里可以显示或退出。
fn setup_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("统一收件箱")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    tracing::info!("托盘图标已就绪");
    Ok(())
}

/// 显示主窗口并置前。
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
