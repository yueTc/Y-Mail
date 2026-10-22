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
#[cfg(windows)]
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mail_core::{InboxQuery, NotifyNewMailWhen, VerificationFinding};
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
/// 一轮最多处理几封分类；剩下的留到下一轮。
const CLASSIFY_BATCH_LIMIT: usize = 20;
/// 分类失败后隔多久再试；免得每轮都去碰同一封一时拉不到正文的邮件。
const CLASSIFY_RETRY_DELAY: Duration = Duration::from_secs(60);
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
pub fn spawn(
    app: AppHandle,
    enabled: Arc<AtomicBool>,
    ai_enabled: Arc<AtomicBool>,
    notify_when: Arc<AtomicU8>,
    classify_enabled: Arc<AtomicBool>,
) {
    tauri::async_runtime::spawn(async move {
        let mut seen: HashMap<i64, i64> = HashMap::new();
        let mut primed = false;
        let mut classify_retry_at: HashMap<i64, Instant> = HashMap::new();
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            let batch = match scan(&app).await {
                Ok(batch) => batch,
                Err(message) => {
                    tracing::debug!(error = %message, "查询新邮件失败，下一轮再试");
                    continue;
                }
            };

            // 分类消费跟通知开关无关，只看分类开关；开关关着时队列原样保留。
            // 每组的门槛在编排里逐组读，这里不再传全局百分比。
            drain_classify_queue(&app, &classify_enabled, &mut classify_retry_at).await;

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

            // 一批只判断一次窗口状态：判定用的就是这一刻的焦点与可见性，之后不再变。
            let mode = NotifyNewMailWhen::from_code(notify_when.load(Ordering::Relaxed));
            let pop_system_notification = should_pop_system_notification(
                enabled.load(Ordering::Relaxed),
                mode,
                main_window_focused(&app),
                main_window_in_background(&app),
            );

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

/// 分类开关关着时后台消费直接返回：不读正文、不调模型、队列原样留着。
fn classify_drain_enabled(enabled: &AtomicBool) -> bool {
    enabled.load(Ordering::Relaxed)
}

/// 从号码列表里截出这一轮要处理的（最多 20 个）。
fn take_classify_batch(ids: Vec<i64>) -> Vec<i64> {
    ids.into_iter().take(CLASSIFY_BATCH_LIMIT).collect()
}

/// 消费待分类队列。
///
/// 开关关着时原样保留队列、一次模型都不调；开着时每轮最多处理 20 封。
/// 每封独立取一次引擎锁，避免长时间占住界面命令要用的锁；存储锁绝不跨 await。
/// 失败的邮件不丢：原样放回队列，隔一会儿再试，别让一封邮件因为一时拉不到正文就永远不分类。
async fn drain_classify_queue(app: &AppHandle, enabled: &AtomicBool, retry_at: &mut HashMap<i64, Instant>) {
    if !classify_drain_enabled(enabled) {
        return;
    }
    let ids = {
        let state = app.state::<AppState>();
        let engine = state.engine().await;
        take_classify_batch(engine.drain_classify_queue(CLASSIFY_BATCH_LIMIT))
    };
    if ids.is_empty() {
        return;
    }
    let now = Instant::now();
    // 这一轮要放回队列的：还没到重试时间的，加上本轮又失败的。
    let mut deferred: Vec<i64> = Vec::new();
    for message_id in ids {
        if !classify_retry_ready(retry_at.get(&message_id), now) {
            deferred.push(message_id);
            continue;
        }
        let state = app.state::<AppState>();
        let engine = state.engine().await;
        // 每封按当前开启的标签组分类；门槛从各组的配置读，不看全局百分比。
        // 已有结果的组在编排层直接跳过，绝不覆盖用户手改的结果。
        match engine.classify_message_if_untagged(message_id).await {
            Ok(_) => {
                retry_at.remove(&message_id);
            }
            Err(error) => {
                tracing::debug!(message_id, error = %error, "自动分类失败，稍后重试");
                retry_at.insert(message_id, Instant::now() + CLASSIFY_RETRY_DELAY);
                deferred.push(message_id);
            }
        }
    }
    if !deferred.is_empty() {
        let state = app.state::<AppState>();
        let engine = state.engine().await;
        engine.enqueue_classify_ids(&deferred);
    }
}

/// 这封到点能再试了吗：没记录过、或者已经过了重试时间，都算到点。
fn classify_retry_ready(retry_at: Option<&Instant>, now: Instant) -> bool {
    match retry_at {
        Some(at) => *at <= now,
        None => true,
    }
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
/// `enabled` 是设置页的总开关；`when` 是用户选的提醒时机；`focused` 是主窗口焦点；
/// `background` 是主窗口是不是「最小化或看不见」。
///
/// `None` 表示窗口还没建好或状态读不出来。除了 `Background` 需要窗口确实在背景才弹之外，
/// 一律按「宁可多弹一条，也别漏掉新邮件」处理。
fn should_pop_system_notification(
    enabled: bool,
    when: NotifyNewMailWhen,
    focused: Option<bool>,
    background: Option<bool>,
) -> bool {
    if !enabled {
        return false;
    }
    match when {
        NotifyNewMailWhen::Always => true,
        NotifyNewMailWhen::Unfocused => focused != Some(true),
        NotifyNewMailWhen::Background => background != Some(false),
    }
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

/// 读主窗口是不是在「后台」：最小化了，或者收进托盘、根本看不见。
///
/// 窗口不可用或状态读不出来返回 `None`，交给判定函数按「宁可多弹」处理。
fn main_window_in_background(app: &AppHandle) -> Option<bool> {
    let window = app.get_webview_window(MAIN_WINDOW_LABEL)?;
    let visible = match window.is_visible() {
        Ok(visible) => visible,
        Err(error) => {
            tracing::debug!(error = %error, "读取主窗口可见状态失败，按看不见处理");
            return None;
        }
    };
    let minimized = match window.is_minimized() {
        Ok(minimized) => minimized,
        Err(error) => {
            tracing::debug!(error = %error, "读取主窗口最小化状态失败，按看不见处理");
            return None;
        }
    };
    Some(minimized || !visible)
}

/// 尝试用 AI 识别这封新邮件里的验证码和验证链接。
///
/// 正文本地没有就先联网下载一次（下载失败也算失败）；正文为空、AI 报错、超时、没识别到都返回 `None`，
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
    /// 点击通知本身，只打开内部提供编号的邮件。
    #[cfg(windows)]
    ReadMessage(i64),
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

/// 两类通知共用响应分流；按钮与通知本身的点击必须分开处理。
#[cfg(windows)]
fn notification_response_action(
    response: &notify_rust::NotificationResponse,
    message_id: i64,
    code: Option<&str>,
    link: Option<&str>,
) -> Option<SafeAction> {
    match response {
        notify_rust::NotificationResponse::Default if message_id > 0 => {
            Some(SafeAction::ReadMessage(message_id))
        }
        notify_rust::NotificationResponse::Action(action) => safe_action_value(action, code, link),
        _ => None,
    }
}

#[cfg(windows)]
fn handle_notification_response(
    app: &AppHandle,
    response: &notify_rust::NotificationResponse,
    message_id: i64,
    code: Option<&str>,
    link: Option<&str>,
) {
    match notification_response_action(response, message_id, code, link) {
        Some(SafeAction::ReadMessage(id)) => open_message_from_notification(app, id),
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

/// Windows：验证通知保留原生按钮和长档停留，通知本身的点击仍然打开邮件。
#[cfg(windows)]
fn show_verification_notification(app: &AppHandle, item: &NewMailEvent, finding: VerificationFinding) {
    let app_id = notification_app_id(app);
    let (first_line, second_line) = verification_lines(&finding);
    let mut toast = tauri_winrt_notification::Toast::new(&app_id)
        .title(&item.from)
        .text1(&first_line)
        .text2(&second_line)
        .duration(tauri_winrt_notification::Duration::Long);
    if finding.code.is_some() {
        toast = toast.add_button("复制验证码", "copy_code");
    }
    if finding.link.is_some() {
        toast = toast.add_button("打开验证链接", "open_link");
    }
    show_windows_toast(app, toast, item.message_id, Some(finding));
}

/// 直接注册系统点击回调，不在横幅超时消失时结束监听。
///
/// notify-rust 的 wait_for_response 只消费第一个事件；若先收到超时关闭，
/// 通知中心后续点击就会丢失。底层回调没有这条单次接收限制，也不需要等待线程。
#[cfg(windows)]
fn show_windows_toast(
    app: &AppHandle,
    toast: tauri_winrt_notification::Toast,
    message_id: i64,
    finding: Option<VerificationFinding>,
) {
    let app = app.clone();
    let toast = toast.on_activated(move |action| {
        tracing::info!(message_id, action = ?action, "收到系统通知点击");
        let response = action.map_or(
            notify_rust::NotificationResponse::Default,
            notify_rust::NotificationResponse::Action,
        );
        let code = finding.as_ref().and_then(|item| item.code.as_deref());
        let link = finding.as_ref().and_then(|item| item.link.as_deref());
        handle_notification_response(&app, &response, message_id, code, link);
        Ok(())
    });
    if let Err(error) = toast.show() {
        tracing::debug!(error = %error, "弹系统通知失败");
    }
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

/// 生成 `System.AppUserModel.ID` 属性；Windows 靠它把开始菜单快捷方式和 Toast 认成同一个应用。
#[cfg(windows)]
fn app_user_model_property_store(app_id: &str) -> Vec<u8> {
    // 一条 VT_LPWSTR 属性：ValueSize + 属性编号 + 保留字节 + 类型头 + 字符串长度 + UTF-16 文本。
    let units = app_id.encode_utf16().count() as u32 + 1;
    let value_size = 17 + units * 2;
    let mut property = Vec::with_capacity(value_size as usize);
    property.extend_from_slice(&value_size.to_le_bytes());
    property.extend_from_slice(&5u32.to_le_bytes()); // System.AppUserModel.ID
    property.push(0);
    property.extend_from_slice(&0x001fu16.to_le_bytes()); // VT_LPWSTR
    property.extend_from_slice(&0u16.to_le_bytes());
    property.extend_from_slice(&units.to_le_bytes());
    for unit in app_id.encode_utf16().chain(std::iter::once(0)) {
        property.extend_from_slice(&unit.to_le_bytes());
    }

    let storage_size = 24 + property.len() as u32 + 4;
    let mut store = Vec::with_capacity(storage_size as usize + 4);
    store.extend_from_slice(&storage_size.to_le_bytes());
    store.extend_from_slice(&0x5350_5331u32.to_le_bytes()); // "1SPS"

    // {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}
    store.extend_from_slice(&[
        0x55, 0x28, 0x4c, 0x9f, 0x79, 0x9f, 0x39, 0x4b, 0xa8, 0xd0, 0xe1, 0xd4, 0x2d, 0xe1, 0xd5, 0xf3,
    ]);
    store.extend_from_slice(&property);
    store.extend_from_slice(&0u32.to_le_bytes());
    store.extend_from_slice(&0u32.to_le_bytes());
    store
}

/// 把显示名变成可安全放进开始菜单路径的文件夹名。
#[cfg(windows)]
fn notification_shortcut_name(display_name: &str) -> String {
    let cleaned: String = display_name
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(|ch| ch == ' ' || ch == '.').to_string();
    if cleaned.is_empty() {
        DEFAULT_NOTIFICATION_NAME.to_string()
    } else {
        cleaned
    }
}

/// 当前用户开始菜单里的通知身份快捷方式。
#[cfg(windows)]
fn notification_shortcut_path(display_name: &str) -> Result<PathBuf, String> {
    let app_data = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "读不到当前用户的 APPDATA，无法修复通知快捷方式".to_string())?;
    let folder = notification_shortcut_name(display_name);
    Ok(app_data
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join(&folder)
        .join(format!("{folder}.lnk")))
}

/// 检查快捷方式是否已经带同一个应用身份。
#[cfg(windows)]
fn shortcut_has_app_id(path: &Path, app_id: &str) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(link) = rclip_shell_link::ShellLink::parse(&bytes) else {
        return false;
    };
    link.app_user_model_id()
        .is_some_and(|value| value.to_string_lossy() == app_id)
}

/// 检查快捷方式是否指向当前这份程序；新旧安装目录切换后要重写，避免点到旧程序。
#[cfg(windows)]
fn shortcut_targets(path: &Path, target: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(link) = rclip_shell_link::ShellLink::parse(&bytes) else {
        return false;
    };
    link.link_info
        .and_then(|info| info.local_base_path().ok().flatten())
        .is_some_and(|value| {
            value
                .to_string_lossy()
                .eq_ignore_ascii_case(&target.to_string_lossy())
        })
}

/// 生成一个带应用身份的开始菜单快捷方式，并先自检再落盘。
#[cfg(windows)]
fn write_notification_shortcut(
    path: &Path,
    target: &Path,
    app_id: &str,
    display_name: &str,
) -> Result<(), String> {
    let target_text = target.to_string_lossy();
    let working_dir = target.parent().unwrap_or(target).to_string_lossy();
    let property_store = app_user_model_property_store(app_id);
    let bytes = rclip_shell_link::ShellLinkBuilder::new()
        .name(display_name)
        .local_path(&target_text)
        .working_dir(&working_dir)
        .icon_location(&target_text)
        .show_command(rclip_shell_link::ShowCommand::NORMAL)
        .extra_block(rclip_shell_link::extra::SIG_PROPERTY_STORE, &property_store)
        .build()
        .map_err(|error| format!("生成通知快捷方式失败：{error}"))?;

    let check = rclip_shell_link::ShellLink::parse(&bytes)
        .map_err(|error| format!("检查通知快捷方式失败：{error}"))?;
    if !check
        .app_user_model_id()
        .is_some_and(|value| value.to_string_lossy() == app_id)
    {
        return Err("生成的通知快捷方式没有带上应用身份".to_string());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建通知快捷方式目录失败：{error}"))?;
    }
    let temp = path.with_extension("lnk.tmp");
    std::fs::write(&temp, bytes).map_err(|error| format!("写通知快捷方式临时文件失败：{error}"))?;
    let backup = path.with_extension("lnk.bak");
    let had_old = path.exists();
    if had_old {
        std::fs::rename(path, &backup).map_err(|error| format!("备份旧通知快捷方式失败：{error}"))?;
    }
    match std::fs::rename(&temp, path) {
        Ok(()) => {
            if had_old {
                let _ = std::fs::remove_file(backup);
            }
            Ok(())
        }
        Err(error) => {
            if had_old {
                let _ = std::fs::rename(&backup, path);
            }
            let _ = std::fs::remove_file(temp);
            Err(format!("替换通知快捷方式失败：{error}"))
        }
    }
}

/// 确保开始菜单里有带应用身份的快捷方式；这是 Windows Toast 点击能回到应用的系统要求。
#[cfg(windows)]
fn ensure_notification_shortcut(app_id: &str, display_name: &str) -> Result<PathBuf, String> {
    let path = notification_shortcut_path(display_name)?;
    let target = std::env::current_exe()
        .map_err(|error| format!("读不到当前程序路径，无法修复通知快捷方式：{error}"))?;
    if shortcut_has_app_id(&path, app_id) && shortcut_targets(&path, &target) {
        return Ok(path);
    }
    write_notification_shortcut(&path, &target, app_id, display_name)?;
    Ok(path)
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

    match ensure_notification_shortcut(&app_id, &display_name) {
        Ok(path) => {
            tracing::info!(path = %path.display(), "通知快捷方式已带应用身份")
        }
        Err(error) => tracing::warn!(error = %error, "修复通知快捷方式失败，点击通知可能无法打开邮件"),
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
    let toast = tauri_winrt_notification::Toast::new(&app_id)
        .title(&title)
        .text2(&body);
    show_windows_toast(app, toast, message_id, None);
}

/// 用户点了通知体：把主窗口叫回前台，再让界面定位并打开那封邮件。
#[cfg(windows)]
fn open_message_from_notification(app: &AppHandle, message_id: i64) {
    tracing::info!(message_id, "通知点击：恢复主窗口并打开邮件");
    crate::show_main_window(app);
    if let Err(error) = app.emit(OPEN_MESSAGE_EVENT, OpenMessageEvent { message_id }) {
        tracing::warn!(error = %error, message_id, "推送打开邮件事件失败");
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
    use super::{
        classify_drain_enabled, classify_retry_ready, safe_action_value, should_pop_system_notification,
        should_run_verification, take_classify_batch, verification_lines, SafeAction,
    };
    #[cfg(windows)]
    use super::{notification_display_name, DEFAULT_NOTIFICATION_NAME};
    use mail_core::NotifyNewMailWhen;
    use mail_core::VerificationFinding;

    #[cfg(windows)]
    #[test]
    fn 通知快捷方式带应用身份和程序路径() {
        use std::path::Path;

        let dir = tempfile::tempdir().expect("临时目录");
        let shortcut = dir.path().join("Y-Mail.lnk");
        let target = Path::new(r"C:\Program Files\Y-Mail\ymail.exe");
        super::write_notification_shortcut(&shortcut, target, "com.ymail.desktop", "Y-Mail")
            .expect("写通知快捷方式");

        let bytes = std::fs::read(&shortcut).expect("读通知快捷方式");
        let link = rclip_shell_link::ShellLink::parse(&bytes).expect("快捷方式应能解析");
        assert_eq!(
            link.app_user_model_id().expect("应带应用身份").to_string_lossy(),
            "com.ymail.desktop"
        );
        assert_eq!(
            link.link_info
                .expect("应有目标路径")
                .local_base_path()
                .expect("路径字段应能解析")
                .expect("应写本地目标")
                .to_string_lossy(),
            target.to_string_lossy()
        );
        assert!(super::shortcut_has_app_id(&shortcut, "com.ymail.desktop"));
        assert!(super::shortcut_targets(&shortcut, target));
    }

    #[cfg(windows)]
    #[test]
    fn 点击两类通知本身都打开内部编号对应的邮件() {
        use notify_rust::NotificationResponse;
        assert_eq!(
            super::notification_response_action(&NotificationResponse::Default, 42, None, None),
            Some(SafeAction::ReadMessage(42))
        );
        assert_eq!(
            super::notification_response_action(
                &NotificationResponse::Default,
                42,
                Some("482913"),
                Some("https://example.com/verify")
            ),
            Some(SafeAction::ReadMessage(42))
        );
    }

    #[cfg(windows)]
    #[test]
    fn 通知按钮保留原动作且关闭和无效编号不打开邮件() {
        use notify_rust::{CloseReason, NotificationResponse};
        assert_eq!(
            super::notification_response_action(
                &NotificationResponse::Action("copy_code".into()),
                42,
                Some("482913"),
                None
            ),
            Some(SafeAction::Copy("482913".into()))
        );
        assert_eq!(
            super::notification_response_action(
                &NotificationResponse::Action("open_link".into()),
                42,
                None,
                Some("https://example.com/verify")
            ),
            Some(SafeAction::Open("https://example.com/verify".into()))
        );
        for response in [
            NotificationResponse::Closed(CloseReason::Dismissed),
            NotificationResponse::Closed(CloseReason::Expired),
            NotificationResponse::Action("unknown".into()),
            NotificationResponse::Reply("https://example.com".into()),
        ] {
            assert_eq!(
                super::notification_response_action(&response, 42, Some("482913"), None),
                None
            );
        }
        assert_eq!(
            super::notification_response_action(&NotificationResponse::Default, 0, None, None),
            None
        );
    }

    #[test]
    fn classify_drain_stops_when_switch_is_off() {
        let off = std::sync::atomic::AtomicBool::new(false);
        assert!(
            !classify_drain_enabled(&off),
            "开关关着时不消费队列、一次模型都不调"
        );
        let on = std::sync::atomic::AtomicBool::new(true);
        assert!(classify_drain_enabled(&on));
    }

    #[test]
    fn classify_drain_uses_group_thresholds() {
        // 总开关关着：一次都不处理。
        let off = std::sync::atomic::AtomicBool::new(false);
        assert!(!classify_drain_enabled(&off));
        // 总开关开着：走一次多次分类编排；门槛在编排里逐组读，不再看全局百分比。
        let on = std::sync::atomic::AtomicBool::new(true);
        assert!(classify_drain_enabled(&on));
    }

    #[test]
    fn classify_retry_waits_until_due_then_fires() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        // 没记录过：立刻可试。
        assert!(classify_retry_ready(None, now));
        // 还没到重试时间：这一轮先跳过。
        let fut = now + Duration::from_secs(60);
        assert!(!classify_retry_ready(Some(&fut), now));
        // 已经过了重试时间：放行。
        let past = now - Duration::from_secs(1);
        assert!(classify_retry_ready(Some(&past), now));
    }

    #[test]
    fn classify_batch_is_capped_at_twenty() {
        let ids: Vec<i64> = (1..=25).collect();
        assert_eq!(
            take_classify_batch(ids),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
        );
    }

    #[test]
    fn 不聚焦时通知遇上前台窗口就不弹() {
        assert!(!should_pop_system_notification(
            true,
            NotifyNewMailWhen::Unfocused,
            Some(true),
            Some(false),
        ));
    }

    #[test]
    fn 不聚焦时通知遇上失焦窗口就弹() {
        assert!(should_pop_system_notification(
            true,
            NotifyNewMailWhen::Unfocused,
            Some(false),
            Some(false),
        ));
    }

    #[test]
    fn 不聚焦时通知遇上最小化窗口也弹() {
        assert!(should_pop_system_notification(
            true,
            NotifyNewMailWhen::Unfocused,
            Some(false),
            Some(true),
        ));
    }

    #[test]
    fn 任何时刻都通知在前台也弹() {
        assert!(should_pop_system_notification(
            true,
            NotifyNewMailWhen::Always,
            Some(true),
            Some(false),
        ));
    }

    #[test]
    fn 仅在后台通知时窗口看得见但失焦不弹() {
        assert!(!should_pop_system_notification(
            true,
            NotifyNewMailWhen::Background,
            Some(false),
            Some(false),
        ));
    }

    #[test]
    fn 仅在后台通知时最小化或收托盘才弹() {
        assert!(should_pop_system_notification(
            true,
            NotifyNewMailWhen::Background,
            Some(false),
            Some(true),
        ));
    }

    #[test]
    fn 焦点和可见状态读不出来时按宁可多弹处理() {
        assert!(should_pop_system_notification(
            true,
            NotifyNewMailWhen::Unfocused,
            None,
            None,
        ));
        assert!(should_pop_system_notification(
            true,
            NotifyNewMailWhen::Always,
            None,
            None,
        ));
        assert!(should_pop_system_notification(
            true,
            NotifyNewMailWhen::Background,
            None,
            None,
        ));
    }

    #[test]
    fn 设置里关掉通知后前台后台都不弹() {
        for when in [
            NotifyNewMailWhen::Unfocused,
            NotifyNewMailWhen::Always,
            NotifyNewMailWhen::Background,
        ] {
            assert!(!should_pop_system_notification(false, when, None, None));
            assert!(!should_pop_system_notification(
                false,
                when,
                Some(true),
                Some(false)
            ));
            assert!(!should_pop_system_notification(
                false,
                when,
                Some(false),
                Some(true)
            ));
        }
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
