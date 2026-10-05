//! 新邮件提醒：轮询本地库，发现新的未读邮件就弹系统通知并顺手通知界面刷新。
//!
//! 为什么用轮询：同步线程只把新邮件写进本地库，不往外发事件；本地库查询很便宜，
//! 五秒一次的开销可以忽略，等以后确实需要更实时再改成推送。
//!
//! 安全约定：通知正文只放发件人和主题，绝不涉及凭据；邮件正文仍然当作不可信内容，
//! 只做展示，不据此触发任何动作。

use std::collections::HashMap;
use std::time::Duration;

use mail_core::InboxQuery;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::state::AppState;

/// 轮询间隔；规格要求新邮件十秒内出现，这里取五秒留余量。
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// 每轮扫几条未读，够用来判断有没有新邮件。
const SCAN_LIMIT: i64 = 20;
/// 一轮最多弹几条通知，避免长时间离线后一次性刷屏。
const MAX_NOTIFICATIONS: usize = 3;

/// 新邮件事件名；界面监听它刷新列表。
pub const NEW_MAIL_EVENT: &str = "inbox:new-mail";

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
pub fn spawn(app: AppHandle) {
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

            for (index, item) in fresh.iter().enumerate() {
                if index < MAX_NOTIFICATIONS {
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

/// 弹一条系统通知。
fn notify(app: &AppHandle, item: &NewMailEvent) {
    let title = format!("{} 收到新邮件", item.account_name);
    let body = if item.subject.trim().is_empty() {
        format!("来自 {}", item.from)
    } else {
        format!("{}：{}", item.from, item.subject)
    };
    if let Err(error) = app.notification().builder().title(title).body(body).show() {
        tracing::debug!(error = %error, "弹系统通知失败");
    }
}
