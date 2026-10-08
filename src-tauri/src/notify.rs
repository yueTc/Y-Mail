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

use mail_core::{InboxQuery, VerificationFinding};
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
/// 通知被点击事件名；界面监听它定位并打开那封邮件。
pub const OPEN_MESSAGE_EVENT: &str = "inbox:open-message";

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

/// 通知被点击时发给界面的一封邮件编号。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenMessageEvent {
    /// 本地邮件编号。
    pub message_id: i64,
}

/// 启动后台提醒循环。
///
/// `enabled` 是用户在设置里控制的新邮件通知开关；关掉后只发界面刷新事件，
/// 再怎么切走也不弹系统通知。开关之外的第二个条件是窗口焦点：窗口在前台时同样只发事件。
pub fn spawn(app: AppHandle, enabled: Arc<AtomicBool>, ai_enabled: Arc<AtomicBool>) {
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
                let notify_this = index < MAX_NOTIFICATIONS && pop_system_notification;
                if notify_this {
                    // 智能识别是高级功能：开关关着就直接走普通通知，正文一点都不碰。
                    let finding = if should_run_verification(notify_this, ai_enabled.load(Ordering::Relaxed))
                    {
                        identify_verification(&app, item).await
                    } else {
                        None
                    };
                    match finding {
                        Some(finding) if !finding.is_empty() => {
                            notify_verification(&app, item, finding);
                        }
                        _ => notify(&app, item),
                    }
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
/// 这封新邮件要不要走 AI 识别：只有「系统通知要弹」且「通知识别开关开着」才发正文。
///
/// 开关默认关。关着时连正文都不读，绝不可能外发；窗口在前台时同样不识别。
fn should_run_verification(pop_system_notification: bool, ai_enabled: bool) -> bool {
    pop_system_notification && ai_enabled
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

/// 尝试用 AI 识别这封新邮件里的验证码和验证链接。
///
/// 任何一步失败（正文为空、还没下载、AI 报错、超时、没识别到）都返回 `None`，
/// 调用方据此退回普通通知。正文、验证码和链接都不写日志。
async fn identify_verification(app: &AppHandle, item: &NewMailEvent) -> Option<VerificationFinding> {
    let state = app.state::<AppState>();
    let engine = state.engine().await;
    match engine
        .identify_notification_verification(item.message_id, &item.from, &item.subject)
        .await
    {
        Ok(finding) if !finding.is_empty() => Some(finding),
        Ok(_) => None,
        Err(error) => {
            tracing::debug!(error = %error, "通知识别没成功，退回普通通知");
            None
        }
    }
}

/// 把识别结果排成通知正文的两行：第一行优先放验证码，第二行放验证链接。
fn verification_lines(finding: &VerificationFinding) -> (String, String) {
    match (
        finding.code.as_deref().map(|code| format!("验证码：{code}")),
        finding.link.as_deref().map(|link| format!("验证链接：{link}")),
    ) {
        (Some(code), Some(link)) => (code, link),
        (Some(code), None) => (code, String::new()),
        (None, Some(link)) => (link, String::new()),
        (None, None) => (String::new(), String::new()),
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
    show_system_notification(app, title, body, item.message_id);
}

/// 弹一条带按钮的验证通知；识别到验证码或验证链接时才走这里。
fn notify_verification(app: &AppHandle, item: &NewMailEvent, finding: VerificationFinding) {
    #[cfg(windows)]
    {
        show_verification_notification(app, item, finding);
    }
    #[cfg(not(windows))]
    {
        // 第一版只做 Windows 原生按钮；其它平台退回普通通知。
        let _ = finding;
        notify(app, item);
    }
}

/// 用户点击通知按钮后要执行的动作；只有通过本地校验的值才会走到这里。
#[derive(Debug, Clone, PartialEq, Eq)]
enum SafeAction {
    /// 复制校验通过的验证码。
    Copy(String),
    /// 用系统默认浏览器打开校验通过的验证链接。
    Open(String),
}

/// 把通知动作标识映射成安全动作；标识对不上或值校验不过一律返回 `None`。
///
/// 点击时再校验一遍：只认内部约定的两个标识，值还要过一次本地白名单，
/// 校验不过就什么都不做，绝不复制、绝不打开。这条函数不碰系统接口，方便单测。
fn safe_action_value(action: &str, code: Option<&str>, link: Option<&str>) -> Option<SafeAction> {
    match action {
        "copy_code" => code.and_then(mail_core::sanitize_code).map(SafeAction::Copy),
        "open_link" => link.and_then(mail_core::sanitize_link).map(SafeAction::Open),
        _ => None,
    }
}

/// Windows：弹带「复制验证码」「打开验证链接」按钮的原生通知，并请求系统「长」档停留。
///
/// 按钮点击才触发动作；动作标识 `copy_code` / `open_link` 只在内部匹配，不展示给用户。
/// 通知句柄必须留着等事件，所以这里另起线程等用户点击，不阻塞后台轮询。
#[cfg(windows)]
fn show_verification_notification(app: &AppHandle, item: &NewMailEvent, finding: VerificationFinding) {
    let app_id = notification_app_id(app);
    let (first_line, second_line) = verification_lines(&finding);
    let code = finding.code.clone();
    let link = finding.link.clone();

    let mut notification = notify_rust::Notification::new();
    notification
        .summary(&item.from)
        .subtitle(&first_line)
        .body(&second_line)
        .app_id(&app_id)
        .timeout(notify_rust::Timeout::Never);
    if code.is_some() {
        notification.action("copy_code", "复制验证码");
    }
    if link.is_some() {
        notification.action("open_link", "打开验证链接");
    }

    let handle = match notification.show() {
        Ok(handle) => handle,
        Err(error) => {
            tracing::debug!(error = %error, "弹验证通知失败");
            return;
        }
    };

    let _ = std::thread::spawn(move || {
        let _ = handle.wait_for_response(move |response: &notify_rust::NotificationResponse| {
            if let notify_rust::NotificationResponse::Action(action) = response {
                // 点击时再校验一遍：只有通过白名单的值才复制或打开，失败只记通用错误。
                match safe_action_value(action.as_str(), code.as_deref(), link.as_deref()) {
                    Some(SafeAction::Copy(text)) => {
                        if let Err(error) = copy_to_clipboard(&text) {
                            tracing::debug!(error = %error, "复制验证码失败");
                        }
                    }
                    Some(SafeAction::Open(url)) => {
                        if let Err(error) = crate::commands::open_in_browser(&url) {
                            tracing::debug!(error = %error, "打开验证链接失败");
                        }
                    }
                    None => {}
                }
            }
        });
    });
}

/// 把文本写进系统剪贴板；只在 Windows 用到。
#[cfg(windows)]
fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    clipboard
        .set_text(text.to_string())
        .map_err(|error| error.to_string())
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
fn show_system_notification(app: &AppHandle, title: String, body: String, message_id: i64) {
    let app_id = notification_app_id(app);
    let mut notification = notify_rust::Notification::new();
    notification.summary(&title).body(&body).app_id(&app_id);
    let handle = match notification.show() {
        Ok(handle) => handle,
        Err(error) => {
            tracing::debug!(error = %error, "弹系统通知失败");
            return;
        }
    };
    // 通知句柄必须留着等用户点击，所以另起线程等响应，不阻塞后台轮询。
    let app = app.clone();
    let _ = std::thread::spawn(move || {
        let _ = handle.wait_for_response(move |response: &notify_rust::NotificationResponse| {
            // 只有点了通知体（Default）才打开邮件；其它响应一律忽略。
            if matches!(response, notify_rust::NotificationResponse::Default) {
                open_message_from_notification(&app, message_id);
            }
        });
    });
}

/// 用户点了通知体：把主窗口叫回前台，再让界面定位并打开那封邮件。
#[cfg(windows)]
fn open_message_from_notification(app: &AppHandle, message_id: i64) {
    crate::show_main_window(app);
    if let Err(error) = app.emit(OPEN_MESSAGE_EVENT, OpenMessageEvent { message_id }) {
        tracing::debug!(error = %error, "推送打开邮件事件失败");
    }
}

/// 非 Windows：照旧走通知插件，那边不需要自己管编号。
#[cfg(not(windows))]
fn show_system_notification(app: &AppHandle, title: String, body: String, _message_id: i64) {
    if let Err(error) = app.notification().builder().title(title).body(body).show() {
        tracing::debug!(error = %error, "弹系统通知失败");
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::{notification_display_name, DEFAULT_NOTIFICATION_NAME};
    use super::{
        safe_action_value, should_pop_system_notification, should_run_verification, verification_lines,
        SafeAction,
    };
    use mail_core::VerificationFinding;

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

    #[test]
    fn 通知识别开关关着时不识别() {
        assert!(!should_run_verification(true, false));
    }

    #[test]
    fn 窗口在前台时不识别() {
        assert!(!should_run_verification(false, true));
    }

    #[test]
    fn 弹通知且开关开着才识别() {
        assert!(should_run_verification(true, true));
    }

    #[test]
    fn 验证码和链接都有时排成两行() {
        let finding = VerificationFinding {
            code: Some("123456".to_string()),
            link: Some("https://example.com/verify".to_string()),
        };
        let (first, second) = verification_lines(&finding);
        assert_eq!(first, "验证码：123456");
        assert_eq!(second, "验证链接：https://example.com/verify");
    }

    #[test]
    fn 只有验证码时第一行放验证码() {
        let finding = VerificationFinding {
            code: Some("ABCD-1234".to_string()),
            link: None,
        };
        let (first, second) = verification_lines(&finding);
        assert_eq!(first, "验证码：ABCD-1234");
        assert_eq!(second, "");
    }

    #[test]
    fn 只有验证链接时第一行放链接() {
        let finding = VerificationFinding {
            code: None,
            link: Some("https://example.com/x".to_string()),
        };
        let (first, second) = verification_lines(&finding);
        assert_eq!(first, "验证链接：https://example.com/x");
        assert_eq!(second, "");
    }

    #[test]
    fn 复制动作只在验证码合法时放行() {
        assert_eq!(
            safe_action_value("copy_code", Some("123456"), None),
            Some(SafeAction::Copy("123456".to_string()))
        );
        // 验证码不合法：点了复制也不动它。
        assert_eq!(safe_action_value("copy_code", Some("<b>1</b>"), None), None);
        assert_eq!(safe_action_value("copy_code", None, None), None);
    }

    #[test]
    fn 打开动作只放行网页地址() {
        assert_eq!(
            safe_action_value("open_link", None, Some("https://example.com/x")),
            Some(SafeAction::Open("https://example.com/x".to_string()))
        );
        assert_eq!(
            safe_action_value("open_link", None, Some("javascript:alert(1)")),
            None
        );
        assert_eq!(safe_action_value("open_link", None, Some("file:///C:/x")), None);
        assert_eq!(safe_action_value("open_link", None, None), None);
    }

    #[test]
    fn 未知动作标识一律忽略() {
        assert_eq!(
            safe_action_value("delete_everything", Some("123456"), Some("https://e.com")),
            None
        );
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
