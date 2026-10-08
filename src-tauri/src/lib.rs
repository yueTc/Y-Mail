//! Tauri 2 桌面外壳。
//!
//! 职责边界（规格 4.1）：只做窗口、生命周期、命令转发；
//! 数据库与业务逻辑都在 `mail-core` / `mail-store`，本 crate 不直接开数据库。
//!
//! 启动顺序：确定应用数据目录 → 建引擎（建库 + 跑迁移）→ 起日志 → 注册状态与命令。

pub mod commands;
pub mod compose_images;
pub mod logging;
pub mod mcp_commands;
pub mod notify;
pub mod settings;
pub mod state;
pub mod storage_dir;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use mail_core::MailEngine;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};

use crate::logging::Logging;
use crate::settings::AppSettings;
use crate::state::AppState;

/// 应用启动入口。
///
/// 初始化失败会直接退出并给出可读原因：Wave 0 的目标是「要么完整就绪，要么明确报错」，
/// 不做静默降级（例如数据库打不开却显示一个空窗口）。
pub fn run() {
    tauri::Builder::default()
        // 单实例必须第一个注册：插件按注册顺序初始化，第二实例得在打开数据库之前就被拦下。
        // 托盘常驻时再点一次图标、或再跑一次启动命令，都只把已有窗口叫出来，不再开第二个进程。
        // 两个进程同时开同一个库，是「数据库看起来坏了」的高风险来源。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        // 应用内更新：查新版本、下载并校验签名、装好由前端调重启。
        // 更新源与公钥写在 tauri.conf.json 的 plugins.updater 里。
        .plugin(tauri_plugin_updater::Builder::new().build())
        // 更新装完后要重启应用；进程控制单独一个插件，只开重启/退出这两个能力。
        .plugin(tauri_plugin_process::init())
        // 开机启动：写进启动项的那条命令会带 --autostart，程序据此静默进托盘。
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .arg("--autostart")
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::get_app_settings,
            commands::set_app_settings,
            commands::change_data_dir,
            commands::open_data_dir,
            commands::restart_app,
            // 开机启动：读真实状态、写 / 删启动项。
            commands::autostart_status,
            commands::set_autostart,
            // 启动与托盘：关闭收托盘 / 启动静默，写进设置文件。
            commands::set_tray_settings,
            commands::download_external_attachment,
            commands::open_downloaded_file,
            commands::open_downloaded_file_dir,
            commands::open_external_url,
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
            commands::set_message_read,
            commands::set_message_flagged,
            commands::get_inbox_message,
            commands::get_message_body,
            commands::download_attachment,
            commands::remember_remote_sender,
            commands::list_trusted_remote_senders,
            commands::forget_remote_sender,
            commands::search_messages,
            commands::compose_draft,
            commands::save_draft,
            commands::enqueue_outbox,
            commands::retry_outbox,
            commands::list_outbox,
            commands::get_outbox,
            commands::delete_outbox,
            commands::search_contacts,
            commands::list_contacts,
            commands::contact_counts,
            commands::create_contact,
            commands::update_contact,
            commands::hide_contact,
            commands::restore_contact,
            commands::purge_contact,
            commands::list_contact_groups,
            commands::create_contact_group,
            commands::rename_contact_group,
            commands::delete_contact_group,
            commands::clear_auto_contacts,
            commands::export_contacts,
            commands::preview_contact_import,
            commands::apply_contact_import,
            commands::get_signature,
            commands::save_signature,
            commands::send_outbox,
            // 写信配图：本地图片、粘贴图片、截屏取图。
            compose_images::read_inline_image,
            compose_images::save_inline_image,
            compose_images::open_screenshot_overlay,
            compose_images::take_screenshot_preview,
            compose_images::finish_screenshot,
            compose_images::cancel_screenshot,
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
            // 1) 默认应用数据目录。Windows 下形如 %APPDATA%\com.ymail.desktop。
            //    设置文件固定放这里，这样改过邮件目录之后下次启动还找得到。
            let default_dir = app
                .path()
                .app_data_dir()
                .map_err(|err| format!("无法确定应用数据目录：{err}"))?;
            std::fs::create_dir_all(&default_dir)?;

            // 是不是全新安装后的第一次启动。必须在引擎建库之前算：
            // 引擎一启动就会在默认目录里建库，建完就认不出「全新」了。
            let first_run = crate::settings::is_first_run(&default_dir);

            // 2) 读设置：邮件数据目录与附件目录都允许单独配置。
            let mut settings = AppSettings::load(&default_dir);
            let data_dir = settings.effective_data_dir(&default_dir);
            // 启动时要不要直接进托盘：设置持久化，读到后固定下来供下面第 7 步用。
            let start_minimized_to_tray = settings.start_minimized_to_tray;

            // 3) 日志先就绪，初始化过程本身也能留下记录。
            let logging = Logging::init(data_dir.join("logs"));

            // 4) 引擎：建目录、开库、跑迁移。
            let engine = MailEngine::initialize(&data_dir)
            .map_err(|err| {
                let message = format!("初始化引擎失败（数据目录 {}）：{err}", data_dir.display());
                tracing::error!(error = %message, "应用初始化失败，启动中止");
                message
            })?;

            let summary = engine.init_summary();
            tracing::info!(
                data_dir = %data_dir.display(),
                database = %summary.database_file,
                schema_version = summary.schema_version,
                applied = summary.applied_count(),
                fts5 = summary.fts5_available,
                "外壳初始化完成"
            );

            // 5) 上次迁移后用户选了清理：等新目录的引擎完全就绪，再清旧目录。
            //    清理成功或失败都先抹掉记录，避免以后每次启动都重复处理。
            if let Some(old_dir) = settings.pending_cleanup_dir.clone() {
                let outcome = storage_dir::cleanup_old_data_dir(&old_dir, &data_dir);
                tracing::info!(
                    old_dir = %old_dir.display(),
                    active_dir = %data_dir.display(),
                    removed = ?outcome.removed,
                    skipped = ?outcome.skipped,
                    "旧数据目录清理完成"
                );
                settings.pending_cleanup_dir = None;
                if let Err(error) = settings.save(&default_dir) {
                    tracing::error!(error = %error, "抹掉待清理记录失败");
                }
            }

            // 6) 把引擎、设置与摘要交给命令层；日志句柄随之进入应用状态，保证写线程存活。
            let notify_enabled = Arc::new(AtomicBool::new(settings.notify_new_mail));
            let notify_ai_enabled = Arc::new(AtomicBool::new(settings.notify_ai_enabled));
            app.manage(AppState::new(
                engine,
                logging,
                default_dir,
                settings,
                notify_enabled.clone(),
                notify_ai_enabled.clone(),
                first_run,
            ));

            // 7) 自动启动已启用账号的后台同步；起不来只记日志，不拦应用启动。
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

            // 7) 主窗口在 tauri.conf.json 里默认不显示，由这里决定亮不亮：
            //    勾了「启动时最小化到托盘」，或被开机启动项拉起来（带 --autostart），
            //    就静默进托盘；否则把窗口亮出来。
            if start_minimized_to_tray {
                tracing::info!("已开启「启动时最小化到托盘」，本次静默启动，只进托盘");
            } else if started_by_autostart() {
                tracing::info!("检测到 --autostart，本次静默启动，只进托盘");
            } else {
                show_main_window(app.handle());
            }

            // 8) 新邮件提醒：后台轮询本地库，发现新未读就弹系统通知并通知界面刷新。
            //    通知开关由设置页控制，改了立刻生效。
            //    先把通知里的应用名登记成「Y-Mail」，否则 Windows 会写成拉起本程序的
            //    PowerShell（开发模式、未安装场景都这样）。
            notify::register_notification_identity(app.handle());
            notify::spawn(app.handle().clone(), notify_enabled, notify_ai_enabled);

            Ok(())
        })
        .on_window_event(|window, event| {
            // 主窗口按关闭键：默认收进托盘不退出（可在「启动与托盘」里关掉）；
            // 关掉「关闭时最小化到托盘」后才真退出，真要退出也能走托盘菜单的「退出」。
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                let minimize = window
                    .app_handle()
                    .state::<AppState>()
                    .settings_snapshot()
                    .minimize_to_tray_on_close;
                if minimize {
                    api.prevent_close();
                    let _ = window.hide();
                }
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
        .tooltip("Y-Mail")
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

/// 本次进程是不是被开机启动项拉起来的：启动项里带了 --autostart。
fn started_by_autostart() -> bool {
    std::env::args().any(|arg| arg == "--autostart")
}

/// 显示主窗口并置前。
pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod config_tests {
    #[test]
    fn 桌面壳策略允许读信放行后的远程图片() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("配置应是合法 JSON");
        let csp = config["app"]["security"]["csp"]
            .as_str()
            .expect("桌面壳应配置内容安全策略");
        let img_src = csp
            .split(';')
            .map(str::trim)
            .find(|part| part.starts_with("img-src "))
            .expect("内容安全策略应有 img-src");
        assert!(img_src.contains("http:"), "{img_src}");
        assert!(img_src.contains("https:"), "{img_src}");
    }
}
