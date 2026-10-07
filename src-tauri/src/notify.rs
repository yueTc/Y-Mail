//! 新邮件提醒：轮询本地库，发现新的未读邮件就通知界面刷新，必要时再弹系统通知。
//!
//! 为什么用轮询：同步线程只把新邮件写进本地库，不往外发事件；本地库查询很便宜，
//! 五秒一次的开销可以忽略，等以后确实需要更实时再改成推送。
//!
//! 弹窗条件：只有主窗口不在前台（被切走、最小化、收进托盘）才弹 Windows 系统通知。
//! 窗口就在眼前时不弹，用户已经看得到邮件了，再弹一次只会打扰。
//!
//! 安全约定：通知正文只放发件人和主题，绝不涉及凭据；邮件正文仍然当作不可信内容，
//! 只做展示，不据此触发任何动作。
//!
//! 应用名约定：Windows 只按「应用编号」认通知上的名字。走通知插件的默认编号时，
//! 通知会挂在拉起本程序的 PowerShell 名下（开发模式必现）。这里改成自己弹通知、
//! 明确带上安装标识当编号，并在启动时把它登记成「Y-Mail」，见
//! [`register_notification_identity`]。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mail_core::InboxQuery;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
#[cfg(not(windows))]
use tauri_plugin_notification::NotificationExt;

use crate::state::AppState;

/// 轮询间隔；规格要求新邮件十秒内出现，这里取五秒留余量。
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// 每轮扫几条未读，够用来判断有没有新邮件。
const SCAN_LIMIT: i64 = 20;
/// 一轮最多弹几条通知，避免长时间离线后一次性刷屏。
const MAX_NOTIFICATIONS: usize = 3;
/// 主窗口标签；判断前台只看这个窗口。
const MAIN_WINDOW_LABEL: &str = "main";

/// 新邮件事件名；界面监听它刷新列表。这条事件跟窗口在不在前台无关，永远都发。
pub const NEW_MAIL_EVENT: &str = "inbox:new-mail";

/// 通知里显示的应用名；也是这个软件的名字。
#[cfg(windows)]
const DEFAULT_NOTIFICATION_NAME: &str = "Y-Mail";

/// 随事件发给界面的一条新邮件摘要。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewMailEvent {
    /// 本地邮件编号。
    pub message_id: i64,
    /// 所属账号。
    pub account_id: i64,
    /// 账号显示名。
    pub account_name: String,
    /// 主题。
    pub subject: String,
    /// 发件人。
    pub from: String,
}

/// 启动后台提醒循环。
///
/// `enabled` 是用户在设置里控制的新邮件通知开关；关掉后只发界面刷新事件，
/// 再怎么切走也不弹系统通知。开关之外的第二个条件是窗口焦点：窗口在前台时同样只发事件。
pub fn spawn(app: AppHandle, enabled: Arc<AtomicBool>) {
    tauri::async_runtime::spawn(async move {
        let mut seen: HashMap<i64, i64> = HashMap::new();
        let mut primed = false;
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            let batch = match scan(&app).await {
                Ok(batch) => batch,
                Err(message) => {
                    tracing::debug!(error = %message, "查询新邮件失败，下一轮再试");
                    continue;
                }
            };

            if !primed {
                // 启动时先把已有邮件记下来，不补弹历史通知。
                for item in batch {
                    let last = seen.entry(item.account_id).or_insert(0);
                    *last = (*last).max(item.message_id);
                }
                primed = true;
                continue;
            }

            let mut fresh: Vec<NewMailEvent> = Vec::new();
            for item in batch {
                let last = seen.entry(item.account_id).or_insert(0);
                if item.message_id > *last {
                    *last = item.message_id;
                    fresh.push(item);
                }
            }
            fresh.sort_by_key(|item| item.message_id);

            // 一批只判断一次前台状态：判定用的就是这一刻的焦点，之后不再变。
            let pop_system_notification =
                should_pop_system_notification(enabled.load(Ordering::Relaxed), main_window_focused(&app));

            for (index, item) in fresh.iter().enumerate() {
                if index < MAX_NOTIFICATIONS && pop_system_notification {
                    notify(&app, item);
                }
                if let Err(error) = app.emit(NEW_MAIL_EVENT, item) {
                    tracing::debug!(error = %error, "推送新邮件事件失败");
                }
            }
        }
    });
}

/// 扫一遍未读邮件；按邮件编号倒序取前若干条。
async fn scan(app: &AppHandle) -> Result<Vec<NewMailEvent>, String> {
    let page = {
        let state = app.state::<AppState>();
        let engine = state.engine().await;
        engine
            .inbox_messages(&InboxQuery {
                unread_only: true,
                limit: SCAN_LIMIT,
                ..InboxQuery::default()
            })
            .map_err(|error| error.to_string())?
    };
    Ok(page
        .items
        .into_iter()
        .map(|item| NewMailEvent {
            message_id: item.id,
            account_id: item.account_id,
            account_name: display_name(&item.account_display_name, &item.account_email),
            subject: mail_core::decode_encoded_words(&item.subject),
            from: display_name(&mail_core::decode_encoded_words(&item.from_name), &item.from_addr),
        })
        .collect())
}

/// 显示名留空时回落到地址。
fn display_name(name: &str, address: &str) -> String {
    if name.trim().is_empty() {
        address.to_string()
    } else {
        name.to_string()
    }
}

/// 决定这一批要不要弹系统通知。
///
/// `enabled` 是设置页的开关；`focused` 是主窗口焦点。`None` 表示窗口还没建好或焦点读不出来，
/// 这种情况按「不在前台」处理：宁可多弹一条，也别在用户切走之后漏掉新邮件。
fn should_pop_system_notification(enabled: bool, focused: Option<bool>) -> bool {
    enabled && focused != Some(true)
}

/// 读主窗口当前是否在前台；窗口不可用或查询失败都返回 `None`。
fn main_window_focused(app: &AppHandle) -> Option<bool> {
    let window = app.get_webview_window(MAIN_WINDOW_LABEL)?;
    match window.is_focused() {
        Ok(focused) => Some(focused),
        Err(error) => {
            tracing::debug!(error = %error, "读取主窗口焦点失败，按不在前台处理");
            None
        }
    }
}

/// 弹一条系统通知。
fn notify(app: &AppHandle, item: &NewMailEvent) {
    let title = format!("{} 收到新邮件", item.account_name);
    let body = if item.subject.trim().is_empty() {
        format!("来自 {}", item.from)
    } else {
        format!("{}：{}", item.from, item.subject)
    };
    show_system_notification(app, title, body);
}

/// 通知归属用的应用编号：用安装标识，不用中文名。
///
/// 改名之后编号不变，用户此前在系统里对该应用的通知设置（开关、提示音）还能对上。
fn notification_app_id(app: &AppHandle) -> String {
    app.config().identifier.clone()
}

/// 通知里显示的应用名；打包配置里的产品名读不到或写成空白时，退回默认名。
#[cfg(windows)]
fn notification_display_name(product_name: Option<&str>) -> String {
    match product_name.map(str::trim) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => DEFAULT_NOTIFICATION_NAME.to_string(),
    }
}

/// 把「应用编号 → Y-Mail」登记进当前用户注册表，让 Windows 通知写对名字。
///
/// 为什么非登记不可：Windows 只认登记过的应用编号；没登记时通知上的名字会退回到唤起
/// 本程序的那个进程，开发模式跑就是 PowerShell。只写当前用户（HKEY_CURRENT_USER）、
/// 只加不删，不动系统设置、不要管理员权限，也不碰邮件数据。
///
/// 失败了不影响收信：只记一条警告，通知照弹，只是名字可能还是旧的。
#[cfg(windows)]
pub fn register_notification_identity(app: &AppHandle) {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let app_id = notification_app_id(app);
    let display_name = notification_display_name(app.config().product_name.as_deref());
    let key_path = format!("Software\\Classes\\AppUserModelId\\{app_id}");
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    match hkcu.create_subkey(&key_path) {
        Ok((key, _)) => match key.set_value("DisplayName", &display_name) {
            Ok(()) => tracing::info!(app_id = %app_id, name = %display_name, "通知应用名已登记"),
            Err(error) => tracing::warn!(error = %error, "写通知应用名失败"),
        },
        Err(error) => tracing::warn!(error = %error, "登记通知应用名失败"),
    }
}

/// 非 Windows 平台不用登记，通知名由系统按程序包显示。
#[cfg(not(windows))]
pub fn register_notification_identity(_app: &AppHandle) {}

/// Windows：自己弹通知，一定带上应用编号。
///
/// 通知插件在「程序不在 target 调试目录下」时才会传编号；开发模式和绿色版都在
/// target 目录里跑，插件就不传，Windows 便算到 PowerShell 头上。这里不走插件，
/// 直接带编号，名字由 [`register_notification_identity`] 登记，图标由系统兜底。
#[cfg(windows)]
fn show_system_notification(app: &AppHandle, title: String, body: String) {
    let app_id = notification_app_id(app);
    tauri::async_runtime::spawn(async move {
        let mut notification = notify_rust::Notification::new();
        notification.summary(&title).body(&body).app_id(&app_id);
        if let Err(error) = notification.show() {
            tracing::debug!(error = %error, "弹系统通知失败");
        }
    });
}

/// 非 Windows：照旧走通知插件，那边不需要自己管编号。
#[cfg(not(windows))]
fn show_system_notification(app: &AppHandle, title: String, body: String) {
    if let Err(error) = app.notification().builder().title(title).body(body).show() {
        tracing::debug!(error = %error, "弹系统通知失败");
    }
}

#[cfg(test)]
mod tests {
    use super::should_pop_system_notification;
    #[cfg(windows)]
    use super::{notification_display_name, DEFAULT_NOTIFICATION_NAME};

    #[test]
    fn 窗口在前台时不弹系统通知() {
        assert!(!should_pop_system_notification(true, Some(true)));
    }

    #[test]
    fn 窗口不在前台时才弹系统通知() {
        assert!(should_pop_system_notification(true, Some(false)));
    }

    #[test]
    fn 焦点读不出来时按不在前台处理() {
        assert!(should_pop_system_notification(true, None));
    }

    #[test]
    fn 设置里关掉通知后前台后台都不弹() {
        assert!(!should_pop_system_notification(false, Some(true)));
        assert!(!should_pop_system_notification(false, Some(false)));
        assert!(!should_pop_system_notification(false, None));
    }

    #[cfg(windows)]
    #[test]
    fn 通知应用名取打包配置里的产品名() {
        assert_eq!(notification_display_name(Some("Y-Mail")), "Y-Mail");
    }

    #[cfg(windows)]
    #[test]
    fn 读不到产品名时通知应用名退回默认值() {
        assert_eq!(notification_display_name(None), DEFAULT_NOTIFICATION_NAME);
        assert_eq!(notification_display_name(Some("   ")), DEFAULT_NOTIFICATION_NAME);
    }
}
