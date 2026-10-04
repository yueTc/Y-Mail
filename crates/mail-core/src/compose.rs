//! 写信、回复、转发与发件队列的编排（Wave 5）。
//!
//! 设计要点：
//! - 草稿、待发、已发都走本地 outbox 表，界面永远只读本地；
//! - 发送前用户必须点头（界面上的「发送」按钮），本模块不会因为收到邮件就自动外发；
//! - 真正投递时先原子认领队列里最早的一封，失败按错误分类决定是否重试，
//!   临时错误最多 3 次尝试（1 次首发 + 2 次重试），重试用尽就标失败并保留草稿；
//! - 投递成功后尽量 APPEND 一份到「已发送」文件夹；这一步失败不影响发送状态，
//!   避免把已经发出去的邮件又标成失败导致重复投递。
//!
//! 安全：邮件正文是不可信内容，回信 / 转发只做文本引用与附件挂载，不据此触发任何动作；
//! 授权码只在建连前从系统保险箱取出，绝不写库、绝不出现在日志里。

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use mail_domain::auth::AuthMaterial;
use mail_domain::dates::unix_now;
use mail_domain::{Account, AccountId, FolderKind};
use mail_imap::{ClientConfig, ImapClient};
use mail_smtp::{
    build_message, guess_mime_type, Mailbox, OutgoingAttachment, OutgoingMessage, SendErrorKind, SendReport,
    SendRequest,
};
use mail_store::{
    NewOutbox, OutboxKind, OutboxState, SearchPage, SearchQuery, Store, StoredContact, StoredOutbox,
    StoredSignature,
};

use crate::engine::{EngineError, MailEngine};
use crate::proxies::resolve_route_with;

/// 一封信最多挂多少附件，防止一次塞爆内存。
const MAX_ATTACHMENTS: usize = 50;

/// 联系人自动补全一次最多返回多少条。
const MAX_CONTACT_HITS: usize = 20;

/// 发件箱列表一次最多读多少条。
const MAX_OUTBOX_PAGE: usize = 200;

/// 回信 / 转发的预填内容（界面拿到后可直接摆进写信框）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftSeed {
    /// 写信类型。
    pub kind: OutboxKind,
    /// 所属账号。
    pub account_id: i64,
    /// 收件人 JSON。
    pub to_json: String,
    /// 抄送 JSON。
    pub cc_json: String,
    /// 密送 JSON（新写信为空）。
    pub bcc_json: String,
    /// 主题。
    pub subject: String,
    /// HTML 正文。
    pub body_html: String,
    /// 纯文本正文。
    pub body_text: String,
    /// 回复的原始 Message-ID。
    pub in_reply_to: Option<String>,
    /// References JSON。
    pub references_json: String,
    /// 附件清单 JSON（转发时带上原邮件附件文件名与提示）。
    pub attachments_json: String,
}

/// 发件队列里的一条记录，附带账号展示信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxItem {
    /// 库里的原始记录。
    pub outbox: StoredOutbox,
    /// 账号邮箱地址。
    pub account_email: String,
    /// 账号显示名。
    pub account_display_name: String,
}

/// 一次发送队列的汇总结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOutcome {
    /// 本次尝试投递的封数（含失败）。
    pub attempted: usize,
    /// 投递成功的封数。
    pub sent: usize,
    /// 交给用户看的失败说明（可能多封）。
    pub errors: Vec<String>,
    /// 成功投递的报告（每封一条），供界面显示。
    pub reports: Vec<SendReport>,
}

/// 送进邮件组装层的一位收件人。
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ComposeParticipant {
    /// 显示名，可为空。
    pub name: String,
    /// 邮箱地址。
    pub address: String,
}

/// 附件引用：本地路径 + 展示文件名。
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ComposeAttachment {
    /// 本地文件路径。
    pub path: String,
    /// 展示文件名。
    pub filename: String,
}

impl MailEngine {
    /// 组装一封回信 / 转发的预填内容。
    ///
    /// 新建邮件不走这里，界面直接给空模板。原邮件不存在时返回可读错误。
    pub fn compose_draft(&self, kind: OutboxKind, source_message_id: i64) -> Result<DraftSeed, EngineError> {
        let source = {
            let store = lock_store(&self.store);
            store.get_compose_source(source_message_id)?
        }
        .ok_or(EngineError::MessageNotFound(source_message_id))?;

        match kind {
            OutboxKind::New => Err(EngineError::BadRequest(
                "新建邮件不需要原邮件；请直接填写收件人与正文".to_string(),
            )),
            OutboxKind::Reply => Ok(reply_seed(&source)),
            OutboxKind::Forward => {
                let (body_text, attachments_json) = {
                    let store = lock_store(&self.store);
                    let body = store.get_message_body(source_message_id)?;
                    let attachments = store.list_attachments(source_message_id)?;
                    forward_bodies(
                        &source,
                        body.as_ref().and_then(|b| b.text_plain.clone()),
                        &attachments,
                    )
                };
                Ok(forward_seed(&source, body_text, attachments_json))
            }
        }
    }

    /// 保存一封草稿（新建或覆盖）。
    ///
    /// `id` 为 None 时新建；Some 时只允许改 draft / failed 的记录，其它状态返回 false。
    pub fn save_draft(&self, id: Option<i64>, draft: &NewOutbox) -> Result<i64, EngineError> {
        let store = lock_store(&self.store);
        match id {
            None => Ok(store.create_outbox(draft, OutboxState::Draft)?),
            Some(id) => {
                let updated = store.update_outbox_draft(id, draft)?;
                if !updated {
                    return Err(EngineError::BadRequest(
                        "这封邮件已经发出或正在发送，不能再当草稿改".to_string(),
                    ));
                }
                Ok(id)
            }
        }
    }

    /// 入队等待发送：草稿 / 失败件变成待发。
    pub fn enqueue_outbox(&self, id: i64) -> Result<bool, EngineError> {
        let store = lock_store(&self.store);
        Ok(store.enqueue_outbox(id)?)
    }

    /// 把失败件退回队列，尝试次数清零（用户点重试用）。
    pub fn retry_outbox(&self, id: i64) -> Result<bool, EngineError> {
        let store = lock_store(&self.store);
        Ok(store.retry_outbox(id)?)
    }

    /// 读取发件队列；账号为 None 时列全部。
    pub fn list_outbox(&self, account_id: Option<i64>, limit: usize) -> Result<Vec<OutboxItem>, EngineError> {
        let limit = limit.clamp(1, MAX_OUTBOX_PAGE);
        let store = lock_store(&self.store);
        let accounts = store.list_accounts()?;
        let items = store.list_outbox(account_id, limit)?;
        Ok(items
            .into_iter()
            .map(|outbox| {
                let account = accounts.iter().find(|item| item.id.0 == outbox.account_id);
                OutboxItem {
                    account_email: account.map_or_else(String::new, |item| item.email.clone()),
                    account_display_name: account.map_or_else(String::new, |item| item.display_name.clone()),
                    outbox,
                }
            })
            .collect())
    }

    /// 读一条发件记录。
    pub fn get_outbox(&self, id: i64) -> Result<Option<StoredOutbox>, EngineError> {
        let store = lock_store(&self.store);
        Ok(store.get_outbox(id)?)
    }

    /// 删除一封草稿 / 失败件；已发送的记录不删。
    pub fn delete_outbox(&self, id: i64) -> Result<bool, EngineError> {
        let store = lock_store(&self.store);
        Ok(store.delete_outbox(id)?)
    }

    /// 收件人自动补全：按名字或邮箱片段搜联系人。
    pub fn search_contacts(
        &self,
        account_id: i64,
        keyword: &str,
        limit: usize,
    ) -> Result<Vec<StoredContact>, EngineError> {
        let store = lock_store(&self.store);
        Ok(store.search_contacts(Some(account_id), keyword, limit.min(MAX_CONTACT_HITS))?)
    }

    /// 读一个账号的签名。
    pub fn get_signature(&self, account_id: i64) -> Result<StoredSignature, EngineError> {
        let store = lock_store(&self.store);
        ensure_account(&store, account_id)?;
        Ok(store.get_signature(account_id)?)
    }

    /// 写一个账号的签名；返回写入后的记录。
    pub fn save_signature(
        &self,
        account_id: i64,
        html: &str,
        enabled: bool,
    ) -> Result<StoredSignature, EngineError> {
        let store = lock_store(&self.store);
        ensure_account(&store, account_id)?;
        store.save_signature(account_id, html, enabled)?;
        Ok(store.get_signature(account_id)?)
    }

    /// 跑一轮发送：先收尾上次残留，再按队列顺序逐封投递，直到队列空。
    ///
    /// 每次最多认领一封、发送完再认领下一封，从根上避免并发重复投递。
    pub async fn send_outbox(&self) -> Result<SendOutcome, EngineError> {
        {
            let store = lock_store(&self.store);
            let recovered = store.recover_stuck_sending()?;
            if recovered > 0 {
                tracing::warn!(count = recovered, "发现上次中断的发送任务，已标失败等人工确认");
            }
        }

        let mut outcome = SendOutcome {
            attempted: 0,
            sent: 0,
            errors: Vec::new(),
            reports: Vec::new(),
        };

        loop {
            let claimed = {
                let store = lock_store(&self.store);
                store.claim_next_outbox()?
            };
            let Some(outbox) = claimed else {
                break;
            };
            outcome.attempted += 1;
            match self.deliver_claimed(&outbox).await {
                Ok(report) => {
                    {
                        let store = lock_store(&self.store);
                        store.mark_outbox_sent(outbox.id)?;
                    }
                    outcome.sent += 1;
                    outcome.reports.push(report);
                    self.append_to_sent(&outbox).await;
                }
                Err((message, retryable)) => {
                    let state = {
                        let store = lock_store(&self.store);
                        store.mark_outbox_failed(outbox.id, &message, retryable)?
                    };
                    // 还能重试的退回队列；本轮不再立刻重发，等下一次用户点发送。
                    outcome.errors.push(format!(
                        "第 {} 封（{}）：{}{}",
                        outbox.id,
                        outbox.subject,
                        message,
                        if state == OutboxState::Failed {
                            "；已停止自动重试"
                        } else {
                            "；已回到待发队列"
                        }
                    ));
                    if state == OutboxState::Queued {
                        break;
                    }
                }
            }
        }

        Ok(outcome)
    }

    /// 投递一封已经认领的邮件；返回投递报告或（可读原因, 是否可重试）。
    async fn deliver_claimed(&self, outbox: &StoredOutbox) -> Result<SendReport, (String, bool)> {
        let (account, secret) = self.sending_credentials(outbox.account_id).await?;
        let recipients = parse_participants(&outbox.to_json)?;
        let cc = parse_participants(&outbox.cc_json)?;
        let bcc = parse_participants(&outbox.bcc_json)?;
        if recipients.is_empty() {
            return Err(("收件人为空，无法发送".to_string(), false));
        }
        let attachments = load_attachments(&outbox.attachments_json)?;
        let references =
            parse_references(&outbox.references_json).map_err(|error| (error.to_string(), false))?;

        let built = build_message(&OutgoingMessage {
            from_name: account.display_name.clone(),
            from_address: account.email.clone(),
            to: recipients.iter().map(to_mailbox).collect(),
            cc: cc.iter().map(to_mailbox).collect(),
            bcc: bcc.iter().map(to_mailbox).collect(),
            subject: outbox.subject.clone(),
            body_text: outbox.body_text.clone(),
            body_html: outbox.body_html.clone(),
            in_reply_to: outbox.in_reply_to.clone(),
            references,
            attachments,
            date_unix: unix_now(),
        })
        .map_err(|error| (format!("邮件组装失败：{error}"), false))?;

        let route = resolve_route_with(&self.store, self.secrets(), account.proxy)
            .map_err(|error| (format!("选择代理失败：{error}"), false))?;
        let request = SendRequest {
            host: account.smtp.host.clone(),
            port: account.smtp.port,
            security: account.smtp.security,
            username: account.username.clone(),
            auth: AuthMaterial::for_account(account.auth_type, secret),
            timeout: mail_net::DEFAULT_TIMEOUT,
            mail_from: account.email.clone(),
            recipients: built.recipients.clone(),
            raw: built.raw.clone(),
        };
        match mail_smtp::send(&request, route.as_ref()).await {
            Ok(report) => Ok(report),
            Err(error) => {
                let retryable = error.kind.is_retryable();
                // 认证失败属于需要用户处理的问题，不算临时网络错误。
                let retryable = retryable && !matches!(error.kind, SendErrorKind::Auth);
                Err((error.message, retryable))
            }
        }
    }

    /// 发送成功后尽量追加到「已发送」文件夹；失败只记日志，不改发送状态。
    async fn append_to_sent(&self, outbox: &StoredOutbox) {
        let result = self.try_append_to_sent(outbox).await;
        match result {
            Ok(Some(path)) => {
                tracing::info!(outbox = outbox.id, folder = %path, "已追加到已发送文件夹");
            }
            Ok(None) => {
                tracing::info!(outbox = outbox.id, "没有找到已发送文件夹，跳过追加");
            }
            Err(error) => {
                tracing::warn!(outbox = outbox.id, error = %error, "追加到已发送文件夹失败，不影响发送结果");
            }
        }
    }

    /// 追加一份原文到已发送文件夹；找不到文件夹返回 Ok(None)。
    async fn try_append_to_sent(&self, outbox: &StoredOutbox) -> Result<Option<String>, EngineError> {
        let (account, sent_path) = {
            let store = lock_store(&self.store);
            let account = store
                .get_account(AccountId(outbox.account_id))?
                .ok_or(EngineError::AccountNotFound(outbox.account_id))?;
            let folders = store.list_folders(outbox.account_id)?;
            let sent = folders
                .iter()
                .find(|folder| folder.kind == FolderKind::Sent)
                .map(|folder| folder.full_path.clone());
            (account, sent)
        };
        let Some(sent_path) = sent_path else {
            return Ok(None);
        };
        let secret = self.resolved_secret(&account).await?;

        let route = resolve_route_with(&self.store, self.secrets(), account.proxy)?;
        let config = ClientConfig {
            host: account.imap.host.clone(),
            port: account.imap.port,
            security: account.imap.security,
            username: account.username.clone(),
            auth: AuthMaterial::for_account(account.auth_type, secret),
            timeout: mail_net::DEFAULT_TIMEOUT,
        };
        let mut client = ImapClient::connect(&config, route.as_ref())
            .await
            .map_err(EngineError::from)?;
        let raw = self.raw_for_append(outbox).await?;
        client
            .append(&sent_path, &raw, true)
            .await
            .map_err(EngineError::from)?;
        let _ = client.logout().await;
        Ok(Some(sent_path))
    }
}

impl MailEngine {
    /// 深度搜索在写信模块不需要；这里只是保留同名门面给未来的「按会话搜」用。
    pub fn search_in_outbox(&self, raw: &str, account_id: Option<i64>) -> Result<SearchPage, EngineError> {
        self.search_messages(&SearchQuery::new(raw, account_id, 0, 50))
    }
}

/// 用账号信息 + 保险箱里的授权码组装发送凭据。
impl MailEngine {
    /// 取出一个账号与它当前可用的凭据，供投递使用。
    async fn sending_credentials(
        &self,
        account_id: i64,
    ) -> Result<(Account, mail_domain::proxy::Secret), (String, bool)> {
        let account = {
            let store = lock_store(&self.store);
            store
                .get_account(AccountId(account_id))
                .map_err(|error| (format!("读取账号失败：{error}"), false))?
                .ok_or_else(|| (format!("账号 {account_id} 不存在或已被删除"), false))?
        };
        let secret = self
            .resolved_secret(&account)
            .await
            .map_err(|error| (error.to_string(), false))?;
        Ok((account, secret))
    }

    /// 重新组装一次原文，用于追加到已发送文件夹。
    async fn raw_for_append(&self, outbox: &StoredOutbox) -> Result<Vec<u8>, EngineError> {
        let (account, _) = self
            .sending_credentials(outbox.account_id)
            .await
            .map_err(|(message, _)| EngineError::BadRequest(message))?;
        let recipients =
            parse_participants(&outbox.to_json).map_err(|(message, _)| EngineError::BadRequest(message))?;
        let cc =
            parse_participants(&outbox.cc_json).map_err(|(message, _)| EngineError::BadRequest(message))?;
        let bcc =
            parse_participants(&outbox.bcc_json).map_err(|(message, _)| EngineError::BadRequest(message))?;
        let attachments = load_attachments(&outbox.attachments_json)
            .map_err(|(message, _)| EngineError::BadRequest(message))?;
        let references = parse_references(&outbox.references_json)?;
        let built = build_message(&OutgoingMessage {
            from_name: account.display_name.clone(),
            from_address: account.email.clone(),
            to: recipients.iter().map(to_mailbox).collect(),
            cc: cc.iter().map(to_mailbox).collect(),
            bcc: bcc.iter().map(to_mailbox).collect(),
            subject: outbox.subject.clone(),
            body_text: outbox.body_text.clone(),
            body_html: outbox.body_html.clone(),
            in_reply_to: outbox.in_reply_to.clone(),
            references,
            attachments,
            date_unix: unix_now(),
        })
        .map_err(|error| EngineError::BadRequest(format!("邮件组装失败：{error}")))?;
        Ok(built.raw)
    }
}
/// 取存储锁；锁中毒时取回内部值继续用。
fn lock_store(store: &Mutex<Store>) -> MutexGuard<'_, Store> {
    store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 账号必须存在，否则给出可读错误。
fn ensure_account(store: &Store, account_id: i64) -> Result<(), EngineError> {
    let exists = store.get_account(AccountId(account_id))?.is_some();
    if !exists {
        return Err(EngineError::AccountNotFound(account_id));
    }
    Ok(())
}

/// 组装回信预填：给原发件人回信，主题加「回复：」前缀，正文引用原文。
fn reply_seed(source: &mail_store::ComposeSource) -> DraftSeed {
    let to = vec![ComposeParticipant {
        name: source.from_name.clone(),
        address: source.from_addr.clone(),
    }];
    let quoted = format!("\n\n在 {} 的来信里写道：\n> {}", source.date_utc, source.subject);
    let mut references = parse_references("[]").unwrap_or_default();
    if let Some(message_id) = &source.message_id_header {
        references.push(message_id.trim().to_string());
    }
    DraftSeed {
        kind: OutboxKind::Reply,
        account_id: source.account_id,
        to_json: to_json(&to),
        cc_json: "[]".to_string(),
        bcc_json: "[]".to_string(),
        subject: format!("回复：{}", source.subject),
        body_html: String::new(),
        body_text: quoted,
        in_reply_to: source.message_id_header.clone(),
        references_json: references_json(&references),
        attachments_json: "[]".to_string(),
    }
}

/// 组装转发预填：主题加「转发：」前缀，正文带原正文，附件按原清单挂载。
fn forward_seed(
    source: &mail_store::ComposeSource,
    body_text: String,
    attachments_json: String,
) -> DraftSeed {
    DraftSeed {
        kind: OutboxKind::Forward,
        account_id: source.account_id,
        to_json: "[]".to_string(),
        cc_json: "[]".to_string(),
        bcc_json: "[]".to_string(),
        subject: format!("转发：{}", source.subject),
        body_html: String::new(),
        body_text,
        in_reply_to: None,
        references_json: "[]".to_string(),
        attachments_json,
    }
}

/// 转发时把原正文与附件清单拼成预填内容。
fn forward_bodies(
    source: &mail_store::ComposeSource,
    body_text: Option<String>,
    attachments: &[mail_store::StoredAttachment],
) -> (String, String) {
    let header = format!(
        "\n\n---------- 转发邮件 ----------\n发件人：{} <{}>\n时间：{}\n主题：{}\n\n",
        source.from_name, source.from_addr, source.date_utc, source.subject
    );
    let body = match body_text {
        Some(text) if !text.trim().is_empty() => format!("{header}{text}"),
        _ => format!("{header}（原邮件没有纯文本正文，请在附件里查看原文）"),
    };
    let list: Vec<ComposeAttachment> = attachments
        .iter()
        .filter(|item| !item.is_inline && item.local_path.is_some())
        .map(|item| ComposeAttachment {
            path: item.local_path.clone().unwrap_or_default(),
            filename: if item.filename.is_empty() {
                format!("附件-{}", item.part_index)
            } else {
                item.filename.clone()
            },
        })
        .collect();
    (body, attachments_json(&list))
}

/// 把一位收件人转成邮件组装层要的结构。
fn to_mailbox(person: &ComposeParticipant) -> Mailbox {
    Mailbox {
        name: person.name.clone(),
        address: person.address.clone(),
    }
}

/// 解析收件人 JSON。
fn parse_participants(raw: &str) -> Result<Vec<ComposeParticipant>, (String, bool)> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| (format!("收件人格式不对：{error}"), false))?;
    let items = value
        .as_array()
        .ok_or_else(|| ("收件人格式不对：应为数组".to_string(), false))?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let name = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let address = item
            .get("address")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if address.trim().is_empty() {
            return Err(("收件人里有空邮箱地址，请检查后再发".to_string(), false));
        }
        out.push(ComposeParticipant { name, address });
    }
    Ok(out)
}

/// 解析附件清单 JSON，逐个读成字节；文件读不到时给出可读错误。
fn load_attachments(raw: &str) -> Result<Vec<OutgoingAttachment>, (String, bool)> {
    let items: Vec<ComposeAttachment> =
        serde_json::from_str(raw).map_err(|error| (format!("附件清单格式不对：{error}"), false))?;
    if items.len() > MAX_ATTACHMENTS {
        return Err((format!("一次最多挂 {MAX_ATTACHMENTS} 个附件，请分批发送"), false));
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if item.path.trim().is_empty() {
            return Err(("附件缺少本地路径，请重新选择文件".to_string(), false));
        }
        let bytes = std::fs::read(&item.path)
            .map_err(|error| (format!("读取附件「{}」失败：{error}", item.filename), false))?;
        let filename = if item.filename.trim().is_empty() {
            PathBuf::from(&item.path)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("attachment")
                .to_string()
        } else {
            item.filename.clone()
        };
        let mime_type = guess_mime_type(&filename);
        out.push(OutgoingAttachment {
            filename,
            mime_type,
            bytes,
        });
    }
    Ok(out)
}

/// 收件人列表转 JSON。
fn to_json(people: &[ComposeParticipant]) -> String {
    let value: Vec<serde_json::Value> = people
        .iter()
        .map(|person| serde_json::json!({ "name": person.name, "address": person.address }))
        .collect();
    serde_json::Value::Array(value).to_string()
}

/// 附件清单转 JSON。
fn attachments_json(items: &[ComposeAttachment]) -> String {
    let value: Vec<serde_json::Value> = items
        .iter()
        .map(|item| serde_json::json!({ "path": item.path, "filename": item.filename }))
        .collect();
    serde_json::Value::Array(value).to_string()
}

/// 解析 References JSON；空串或格式不对时返回空列表。
fn parse_references(raw: &str) -> Result<Vec<String>, EngineError> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|error| EngineError::BadRequest(format!("References 格式不对：{error}")))?;
    let items = value.as_array().cloned().unwrap_or_default();
    Ok(items
        .into_iter()
        .filter_map(|item| match item {
            serde_json::Value::String(text) => Some(text),
            _ => None,
        })
        .collect())
}

/// References 列表转 JSON。
fn references_json(items: &[String]) -> String {
    let value: Vec<serde_json::Value> = items
        .iter()
        .map(|item| serde_json::Value::String(item.clone()))
        .collect();
    serde_json::Value::Array(value).to_string()
}

#[cfg(test)]
mod tests {
    use mail_store::{AttachmentState, ComposeSource, StoredAttachment};

    use super::{
        forward_bodies, forward_seed, load_attachments, parse_participants, parse_references, reply_seed,
        to_json, ComposeAttachment, ComposeParticipant, OutboxKind,
    };

    fn source() -> ComposeSource {
        ComposeSource {
            message_id: 7,
            account_id: 3,
            subject: "季度报价".to_string(),
            from_name: "李四".to_string(),
            from_addr: "lisi@example.com".to_string(),
            to_json: "[{\"name\":\"我\",\"address\":\"me@example.com\"}]".to_string(),
            cc_json: "[]".to_string(),
            message_id_header: Some("<abc@example.com>".to_string()),
            date_utc: "2026-10-03T09:15:00Z".to_string(),
        }
    }

    fn attachment() -> StoredAttachment {
        StoredAttachment {
            id: 1,
            message_id: 7,
            part_index: 0,
            filename: "报价.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            size: 10,
            content_id: None,
            is_inline: false,
            local_path: Some("D:\\mail\\报价.pdf".to_string()),
            state: AttachmentState::Downloaded,
        }
    }

    #[test]
    fn 收件人往返保持字段() {
        let people = vec![ComposeParticipant {
            name: "王五".to_string(),
            address: "wang@example.com".to_string(),
        }];
        let json = to_json(&people);
        let back = parse_participants(&json).expect("解析收件人");
        assert_eq!(back, people);
    }

    #[test]
    fn 收件人解析拒绝空地址且不可重试() {
        let (message, retryable) = parse_participants("[{\"name\":\"x\",\"address\":\"  \"}]").unwrap_err();
        assert!(!retryable, "空地址属于输入问题，不该自动重试");
        assert!(message.contains("空邮箱"), "错误要能看懂，实际：{message}");
    }

    #[test]
    fn 回信预填认原发件人当收件人() {
        let seed = reply_seed(&source());
        assert_eq!(seed.kind, OutboxKind::Reply);
        assert_eq!(seed.account_id, 3);
        assert_eq!(seed.subject, "回复：季度报价");
        assert_eq!(seed.in_reply_to.as_deref(), Some("<abc@example.com>"));
        let people = parse_participants(&seed.to_json).expect("解析收件人");
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].address, "lisi@example.com");
        let references = parse_references(&seed.references_json).expect("解析引用");
        assert_eq!(references, vec!["<abc@example.com>".to_string()]);
    }

    #[test]
    fn 转发带原文正文与附件清单() {
        let source = source();
        let (body, attachments) = forward_bodies(&source, Some("原件内容".to_string()), &[attachment()]);
        assert!(body.contains("原件内容"), "正文要带上原文");
        let seed = forward_seed(&source, body, attachments);
        assert_eq!(seed.kind, OutboxKind::Forward);
        assert_eq!(seed.subject, "转发：季度报价");
        assert!(seed.in_reply_to.is_none(), "转发不设 In-Reply-To");
        let parsed: Vec<ComposeAttachment> =
            serde_json::from_str(&seed.attachments_json).expect("解析附件清单");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].filename, "报价.pdf");
    }

    #[test]
    fn 转发跳过未下载与内嵌附件() {
        let mut pending = attachment();
        pending.local_path = None;
        let mut inline = attachment();
        inline.is_inline = true;
        let (_, attachments) = forward_bodies(&source(), None, &[pending, inline]);
        let parsed: Vec<ComposeAttachment> = serde_json::from_str(&attachments).expect("解析附件清单");
        assert!(parsed.is_empty(), "没下载的附件与内嵌图片不该进转发清单");
    }

    #[test]
    fn 附件读不到给出可读错误且不重试() {
        let json = "[{\"path\":\"D:\\\\不存在的目录\\\\没有.pdf\",\"filename\":\"没有.pdf\"}]";
        let (message, retryable) = load_attachments(json).unwrap_err();
        assert!(!retryable, "文件读不到属于输入问题，不该自动重试");
        assert!(
            message.contains("没有.pdf"),
            "错误要指出是哪个附件，实际：{message}"
        );
    }

    #[test]
    fn 空引用解析返回空列表() {
        assert!(parse_references("").expect("空串").is_empty());
        assert!(parse_references("[]").expect("空数组").is_empty());
    }
}
