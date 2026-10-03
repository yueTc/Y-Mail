//! 读信与附件下载的编排（Wave 4）。
//!
//! 设计要点：
//! - 正文懒加载：先看本地缓存，没有才联网拉原文；缓存写库后界面只读本地；
//! - 远程图片默认拦截，用户放行只影响本次返回的 HTML，不回写数据库；
//! - 附件按需下载到本地，文件名先消毒，防止路径穿越；
//! - 邮件正文是不可信内容，本模块只做解析与清洗，不据此触发任何动作。
//!
//! 锁纪律：存储锁只在同步代码里短暂持有，绝不跨 `.await`。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use mail_domain::AccountId;
use mail_imap::{ClientConfig, ImapClient};
use mail_mime::{
    attachment_content, parse_message, restore_remote_images, ParsedAttachment, REMOTE_SRC_ATTRIBUTE,
};
use mail_store::{AttachmentState, BodyState, MessageLocation, NewAttachment, Store, StoredAttachment};

use crate::engine::{EngineError, MailEngine};
use crate::proxies::resolve_route_with;

/// 读信窗格要展示的一封邮件：正文、被拦图片数量与附件清单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageBodyView {
    /// 邮件主键。
    pub message_id: i64,
    /// 纯文本正文；没有则为 None。
    pub text_plain: Option<String>,
    /// 清洗后的 HTML 正文；没有则为 None。
    pub html: Option<String>,
    /// 被拦下的远程图片数量（按清洗结果里的占位属性统计）。
    pub blocked_remote_images: usize,
    /// 附件清单（含本地保存状态）。
    pub attachments: Vec<StoredAttachment>,
}

impl MailEngine {
    /// 取一封邮件的正文；本地没有就联网拉一次并落库。
    ///
    /// `allow_remote_images` 只是本封邮件的临时放行：命中缓存时把占位属性还原成
    /// 真实地址返回给界面，但库里保存的始终是清洗后的版本。
    pub async fn get_message_body(
        &self,
        message_id: i64,
        allow_remote_images: bool,
    ) -> Result<MessageBodyView, EngineError> {
        let (cached, location) = {
            let store = lock_store(&self.store);
            let cached = store.get_message_body(message_id)?;
            let location = store.message_location(message_id)?;
            (cached, location)
        };

        let location = location.ok_or(EngineError::MessageNotFound(message_id))?;
        if let Some(body) = cached {
            let attachments = {
                let store = lock_store(&self.store);
                store.list_attachments(message_id)?
            };
            return Ok(self.view_of(
                message_id,
                body.text_plain,
                body.html_sanitized,
                allow_remote_images,
                attachments,
            ));
        }

        {
            let store = lock_store(&self.store);
            let _ = store.set_message_body_state(message_id, BodyState::Loading);
        }

        let raw = match self.fetch_raw(&location).await {
            Ok(raw) => raw,
            Err(error) => {
                let store = lock_store(&self.store);
                let _ = store.set_message_body_state(message_id, BodyState::Failed);
                return Err(error);
            }
        };

        let parsed = match parse_message(&raw) {
            Ok(parsed) => parsed,
            Err(error) => {
                let store = lock_store(&self.store);
                let _ = store.set_message_body_state(message_id, BodyState::Failed);
                return Err(EngineError::BadRequest(format!(
                    "这封邮件的正文无法解析：{error}"
                )));
            }
        };

        let attachments = {
            let store = lock_store(&self.store);
            store.save_message_body(
                message_id,
                parsed.text_plain.as_deref(),
                parsed.html_sanitized.as_deref(),
            )?;
            let new_items: Vec<NewAttachment> = parsed.attachments.iter().map(new_attachment).collect();
            let count = store.replace_attachments(message_id, &new_items)?;
            store.set_message_has_attachments(message_id, count > 0)?;
            store.list_attachments(message_id)?
        };

        Ok(self.view_of(
            message_id,
            parsed.text_plain,
            parsed.html_sanitized,
            allow_remote_images,
            attachments,
        ))
    }

    /// 下载一个附件到本地，返回保存路径。
    ///
    /// 流程：查本地附件元数据 → 联网重拉整封原文 → 取分片字节 → 文件名消毒 → 落盘 → 标记已下载。
    /// 任一步失败都把附件标成失败并返回可读错误。
    pub async fn download_attachment(&self, attachment_id: i64) -> Result<String, EngineError> {
        let (attachment, location) = {
            let store = lock_store(&self.store);
            let attachment = store
                .get_attachment(attachment_id)?
                .ok_or(EngineError::AttachmentNotFound(attachment_id))?;
            let location = store
                .message_location(attachment.message_id)?
                .ok_or(EngineError::MessageNotFound(attachment.message_id))?;
            (attachment, location)
        };

        // 已经下载过且文件还在，直接复用，不必再联网。
        if attachment.state == AttachmentState::Downloaded {
            if let Some(path) = attachment.local_path.as_deref() {
                if Path::new(path).is_file() {
                    return Ok(path.to_string());
                }
            }
        }

        {
            let store = lock_store(&self.store);
            let _ = store.set_attachment_state(attachment_id, AttachmentState::Downloading);
        }

        match self.save_attachment(&attachment, &location).await {
            Ok(path) => Ok(path),
            Err(error) => {
                let store = lock_store(&self.store);
                let _ = store.set_attachment_state(attachment_id, AttachmentState::Failed);
                Err(error)
            }
        }
    }

    /// 把解析结果拼成界面视图；只有本次放行时才还原远程图片地址。
    fn view_of(
        &self,
        message_id: i64,
        text_plain: Option<String>,
        html: Option<String>,
        allow_remote_images: bool,
        attachments: Vec<StoredAttachment>,
    ) -> MessageBodyView {
        let blocked_remote_images = html
            .as_deref()
            .map(|value| value.matches(REMOTE_SRC_ATTRIBUTE).count())
            .unwrap_or(0);
        let html = match (html, allow_remote_images) {
            (Some(value), true) => Some(restore_remote_images(&value)),
            (other, _) => other,
        };
        MessageBodyView {
            message_id,
            text_plain,
            html,
            blocked_remote_images,
            attachments,
        }
    }

    /// 联网取一封邮件的原始字节。
    async fn fetch_raw(&self, location: &MessageLocation) -> Result<Vec<u8>, EngineError> {
        if location.folder_path.trim().is_empty() {
            return Err(EngineError::BadRequest(
                "这封邮件缺少文件夹信息，无法读取正文".to_string(),
            ));
        }
        let mut client = self.connect_client(location.account_id).await?;
        let outcome = async {
            client.select(&location.folder_path).await?;
            Ok::<Vec<u8>, EngineError>(client.fetch_body_raw(location.uid).await?)
        }
        .await;
        if outcome.is_ok() {
            client.logout().await;
        }
        outcome
    }

    /// 取附件分片字节并写入本地下载目录。
    async fn save_attachment(
        &self,
        attachment: &StoredAttachment,
        location: &MessageLocation,
    ) -> Result<String, EngineError> {
        if location.folder_path.trim().is_empty() {
            return Err(EngineError::BadRequest(
                "这封邮件缺少文件夹信息，无法下载附件".to_string(),
            ));
        }
        let mut client = self.connect_client(location.account_id).await?;
        let raw = async {
            client.select(&location.folder_path).await?;
            Ok::<Vec<u8>, EngineError>(client.fetch_body_raw(location.uid).await?)
        }
        .await;
        let raw = match raw {
            Ok(raw) => {
                client.logout().await;
                raw
            }
            Err(error) => return Err(error),
        };

        let bytes = attachment_content(&raw, attachment.part_index)
            .map_err(|error| EngineError::BadRequest(format!("这封邮件里找不到该附件：{error}")))?;
        let path = self.attachment_path(attachment)?;
        std::fs::write(&path, bytes)?;
        {
            let store = lock_store(&self.store);
            store.set_attachment_downloaded(attachment.id, &path.to_string_lossy())?;
        }
        Ok(path.to_string_lossy().to_string())
    }

    /// 计算附件的落盘路径：下载目录 + 消毒后的文件名。
    fn attachment_path(&self, attachment: &StoredAttachment) -> Result<PathBuf, EngineError> {
        let dir = Path::new(&self.init_summary().root_dir).join("downloads");
        std::fs::create_dir_all(&dir)?;
        let name = sanitize_filename(&attachment.filename, attachment.id);
        let path = dir.join(&name);
        if path.exists() {
            return Ok(dir.join(unique_name(&name, attachment.id)));
        }
        Ok(path)
    }

    /// 按账号组装建连参数并连上服务器。
    ///
    /// 与同步工作线程保持同一套口径：账号来自本地库，授权码只在建连前从保险箱取出。
    async fn connect_client(&self, account_id: i64) -> Result<ImapClient, EngineError> {
        let account = {
            let store = lock_store(&self.store);
            store
                .get_account(AccountId(account_id))?
                .ok_or(EngineError::AccountNotFound(account_id))?
        };
        let key = account.credential_key.as_deref().ok_or_else(|| {
            EngineError::BadRequest("该账号还没有保存授权码，请到账号设置里重新填写".to_string())
        })?;
        let secret = self
            .secrets()
            .get(key)
            .map_err(|_| EngineError::BadRequest("读取系统凭据失败，请稍后重试".to_string()))?
            .ok_or_else(|| {
                EngineError::BadRequest("系统凭据管理器里找不到该账号的授权码，请重新填写".to_string())
            })?;
        let route = resolve_route_with(&self.store, self.secrets(), account.proxy)?;
        let config = ClientConfig {
            host: account.imap.host.clone(),
            port: account.imap.port,
            security: account.imap.security,
            username: account.username.clone(),
            password: secret,
            timeout: mail_net::DEFAULT_TIMEOUT,
        };
        ImapClient::connect(&config, route.as_ref())
            .await
            .map_err(EngineError::from)
    }
}

/// 解析出的附件元数据转成准备写库的形状。
fn new_attachment(parsed: &ParsedAttachment) -> NewAttachment {
    NewAttachment {
        part_index: parsed.part_index,
        filename: parsed.filename.clone(),
        mime_type: parsed.mime_type.clone(),
        size: parsed.size,
        content_id: parsed.content_id.clone(),
        is_inline: parsed.is_inline,
    }
}

/// 文件名消毒：只取最后一段，去掉路径与非法字符，空名给个兜底。
fn sanitize_filename(raw: &str, attachment_id: i64) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let cleaned: String = base
        .chars()
        .filter(|ch| !matches!(ch, '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    let trimmed = cleaned.replace("..", "");
    let trimmed = trimmed.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        format!("attachment-{attachment_id}")
    } else {
        trimmed.to_string()
    }
}

/// 重名时给文件名加上附件编号，避免互相覆盖。
fn unique_name(name: &str, attachment_id: i64) -> String {
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment");
    match path.extension().and_then(|value| value.to_str()) {
        Some(ext) if !ext.is_empty() => format!("{stem}-{attachment_id}.{ext}"),
        _ => format!("{stem}-{attachment_id}"),
    }
}

/// 取存储锁；锁中毒时取回内部值继续用。
fn lock_store(store: &Mutex<Store>) -> MutexGuard<'_, Store> {
    store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::secrets::MemorySecretStore;

    use super::{sanitize_filename, MailEngine};

    #[test]
    fn 文件名消毒能挡住路径穿越() {
        assert_eq!(sanitize_filename("../../evil.exe", 7), "evil.exe");
        assert_eq!(sanitize_filename("..\\..\\evil.exe", 7), "evil.exe");
        assert_eq!(sanitize_filename("a:b*c?.txt", 7), "abc.txt");
        assert_eq!(sanitize_filename("..", 7), "attachment-7");
        assert_eq!(sanitize_filename("", 7), "attachment-7");
    }

    #[tokio::test]
    async fn 不存在的邮件与附件给出可读错误() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let engine = MailEngine::initialize_with_secrets(dir.path(), Arc::new(MemorySecretStore::new()))
            .expect("初始化引擎");

        let err = engine.get_message_body(999, false).await.expect_err("应失败");
        assert!(err.to_string().contains("999"), "错误应含编号：{err}");

        let err = engine.download_attachment(999).await.expect_err("应失败");
        assert!(err.to_string().contains("999"), "错误应含编号：{err}");
    }
}
