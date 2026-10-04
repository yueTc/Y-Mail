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

use mail_domain::auth::AuthMaterial;
use mail_domain::AccountId;
use mail_imap::{ClientConfig, ImapClient};
use mail_mime::{
    attachment_content, inline_image_data_url, is_renderable_inline_image_mime, normalize_content_id,
    parse_message, restore_remote_images, ParsedAttachment, MAX_INLINE_IMAGE_BYTES, REMOTE_SRC_ATTRIBUTE,
};
use mail_store::{AttachmentState, BodyState, MessageLocation, NewAttachment, Store, StoredAttachment};

use crate::engine::{EngineError, MailEngine};
use crate::proxies::resolve_route_with;

/// 内嵌图片（cid:）在本地是否可用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineImageState {
    /// 本地已有可用字节，视图里带 data URL。
    Available,
    /// 本地还没下载；前端显示占位与「点一下加载」，由用户点击后走既有附件下载。
    NotDownloaded,
    /// 单张超过内联上限，拒绝渲染。
    TooLarge,
    /// 类型不在内联白名单里（例如 SVG 或非图片），拒绝渲染。
    Unsupported,
}

impl InlineImageState {
    /// 存库 / 过接口用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::NotDownloaded => "not-downloaded",
            Self::TooLarge => "too-large",
            Self::Unsupported => "unsupported",
        }
    }
}

/// 正文里一个 cid 引用对应的内嵌图片。
///
/// 只描述本地已有或缺失的状态，绝不在这里联网抓图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineImageView {
    /// 规范化后的 Content-ID。
    pub content_id: String,
    /// 对应的附件编号；有记录时前端可复用既有下载按钮。
    pub attachment_id: Option<i64>,
    /// MIME 类型。
    pub mime_type: String,
    /// 字节数。
    pub size: u64,
    /// 本地可用状态。
    pub state: InlineImageState,
    /// 本地可用时的受控 data URL；其余状态为 None。
    pub data_url: Option<String>,
}

/// 读信窗格要展示的一封邮件：正文、被拦图片数量、内嵌图片与附件清单。
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
    /// 正文里可能用到的内嵌图片（只含本地状态，不含远程探测）。
    pub inline_images: Vec<InlineImageView>,
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
        let inline_images = attachments.iter().filter_map(inline_image_view).collect();
        MessageBodyView {
            message_id,
            text_plain,
            html,
            blocked_remote_images,
            inline_images,
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
        ImapClient::connect(&config, route.as_ref())
            .await
            .map_err(EngineError::from)
    }
}

/// 把一条附件记录转成内嵌图片视图。
///
/// 只读本地已下载的文件；没下载、文件缺失或读不出来都返回 NotDownloaded，
/// 由用户点击后走既有附件下载路径，绝不在渲染时联网。
fn inline_image_view(attachment: &StoredAttachment) -> Option<InlineImageView> {
    let content_id = attachment.content_id.as_deref().and_then(normalize_content_id)?;
    let mime_type = attachment.mime_type.trim().to_string();
    let view = |state: InlineImageState, data_url: Option<String>| InlineImageView {
        content_id: content_id.clone(),
        attachment_id: Some(attachment.id),
        mime_type: mime_type.clone(),
        size: attachment.size,
        state,
        data_url,
    };

    if !is_renderable_inline_image_mime(&mime_type) {
        return Some(view(InlineImageState::Unsupported, None));
    }
    if attachment.size > MAX_INLINE_IMAGE_BYTES as u64 {
        return Some(view(InlineImageState::TooLarge, None));
    }
    if attachment.state != AttachmentState::Downloaded {
        return Some(view(InlineImageState::NotDownloaded, None));
    }
    let Some(path) = attachment.local_path.as_deref() else {
        return Some(view(InlineImageState::NotDownloaded, None));
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        return Some(view(InlineImageState::NotDownloaded, None));
    };
    if !metadata.is_file() {
        return Some(view(InlineImageState::NotDownloaded, None));
    }
    if metadata.len() > MAX_INLINE_IMAGE_BYTES as u64 {
        return Some(view(InlineImageState::TooLarge, None));
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Some(view(InlineImageState::NotDownloaded, None));
    };
    match inline_image_data_url(&mime_type, &bytes) {
        Ok(data_url) => Some(view(InlineImageState::Available, Some(data_url))),
        Err(mail_mime::InlineImageError::TooLarge { .. }) => Some(view(InlineImageState::TooLarge, None)),
        Err(_) => Some(view(InlineImageState::Unsupported, None)),
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

    use mail_store::{AttachmentState, StoredAttachment};

    use crate::secrets::MemorySecretStore;

    use super::{inline_image_view, sanitize_filename, InlineImageState, MailEngine};

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

    /// 造一条附件记录，方便测内嵌图映射。
    fn stored(
        id: i64,
        mime: &str,
        size: u64,
        content_id: Option<&str>,
        local_path: Option<&str>,
        state: AttachmentState,
    ) -> StoredAttachment {
        StoredAttachment {
            id,
            message_id: 42,
            part_index: 2,
            filename: String::new(),
            mime_type: mime.to_string(),
            size,
            content_id: content_id.map(str::to_string),
            is_inline: true,
            local_path: local_path.map(str::to_string),
            state,
        }
    }

    /// 一段最小 PNG 文件头，够嗅探识别。
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    #[test]
    fn 已下载的内嵌图会给出受控data_url() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("图.png");
        std::fs::write(&path, PNG).expect("写图片");
        let attachment = stored(
            7,
            "image/png",
            PNG.len() as u64,
            Some("<Img-1@Example.com>"),
            Some(&path.to_string_lossy()),
            AttachmentState::Downloaded,
        );

        let view = inline_image_view(&attachment).expect("应有内嵌图视图");
        assert_eq!(view.content_id, "img-1@example.com");
        assert_eq!(view.attachment_id, Some(7));
        assert_eq!(view.state, InlineImageState::Available);
        assert!(view
            .data_url
            .as_deref()
            .unwrap_or_default()
            .starts_with("data:image/png;base64,"));
    }

    #[test]
    fn 没缓存的内嵌图只标未下载不给data_url() {
        let attachment = stored(
            8,
            "image/png",
            12,
            Some("img-2@example.com"),
            None,
            AttachmentState::Pending,
        );
        let view = inline_image_view(&attachment).expect("应有视图");
        assert_eq!(view.state, InlineImageState::NotDownloaded);
        assert!(view.data_url.is_none());
    }

    #[test]
    fn 记录说已下载但文件不在也只标未下载() {
        let attachment = stored(
            9,
            "image/png",
            12,
            Some("img-3@example.com"),
            Some("D:/not-there/x.png"),
            AttachmentState::Downloaded,
        );
        let view = inline_image_view(&attachment).expect("应有视图");
        assert_eq!(view.state, InlineImageState::NotDownloaded);
        assert!(view.data_url.is_none());
    }

    #[test]
    fn 非图片类型与svg不会被内联渲染() {
        for mime in ["application/pdf", "image/svg+xml", "text/html"] {
            let attachment = stored(
                10,
                mime,
                100,
                Some("part@example.com"),
                None,
                AttachmentState::Pending,
            );
            let view = inline_image_view(&attachment).expect("应有视图");
            assert_eq!(view.state, InlineImageState::Unsupported, "{mime} 不应内联");
            assert!(view.data_url.is_none());
        }
    }

    #[test]
    fn 超上限的内嵌图被拒() {
        let attachment = stored(
            11,
            "image/png",
            super::MAX_INLINE_IMAGE_BYTES as u64 + 1,
            Some("big@example.com"),
            None,
            AttachmentState::Pending,
        );
        let view = inline_image_view(&attachment).expect("应有视图");
        assert_eq!(view.state, InlineImageState::TooLarge);
        assert!(view.data_url.is_none());
    }

    #[test]
    fn 不安全的cid不会进入内嵌图映射() {
        for cid in ["../../etc/passwd", "a/../b@x", "bad\"id"] {
            let attachment = stored(12, "image/png", 12, Some(cid), None, AttachmentState::Pending);
            assert!(inline_image_view(&attachment).is_none(), "{cid} 不应有映射");
        }
    }

    #[test]
    fn 解析结果里的cid能与正文引用对应上() {
        let raw = concat!(
            "From: a@example.com\r\n",
            "Subject: 内嵌图\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/related; boundary=\"B\"\r\n",
            "\r\n",
            "--B\r\n",
            "Content-Type: text/html; charset=utf-8\r\n",
            "\r\n",
            "<p>看图</p><img src=\"cid:Inline-1@Example.com\">",
            "<img src=\"https://tracker.example/1.gif\">\r\n",
            "--B\r\n",
            "Content-Type: image/png\r\n",
            "Content-Disposition: inline\r\n",
            "Content-ID: <Inline-1@Example.com>\r\n",
            "Content-Transfer-Encoding: base64\r\n",
            "\r\n",
            "iVBORw0KGgo=\r\n",
            "--B--\r\n",
        );
        let parsed = mail_mime::parse_message(raw.as_bytes()).expect("应解析成功");
        assert_eq!(parsed.blocked_remote_images, 1, "远程图仍要被拦");
        let html = parsed.html_sanitized.as_deref().expect("应有 HTML");
        let lower = html.to_ascii_lowercase();
        assert!(
            lower.contains("cid:inline-1@example.com"),
            "cid 引用应保留：{html}"
        );
        assert!(
            !html.contains("<img src=\"http"),
            "远程图片不该还原成 src：{html}"
        );

        let image = parsed
            .attachments
            .iter()
            .find(|item| item.content_id.is_some())
            .expect("应有带 cid 的图片");
        assert_eq!(image.content_id.as_deref(), Some("inline-1@example.com"));
        assert_eq!(image.mime_type, "image/png");

        let stored = stored(
            13,
            &image.mime_type,
            image.size,
            image.content_id.as_deref(),
            None,
            AttachmentState::Pending,
        );
        let view = inline_image_view(&stored).expect("应有视图");
        assert_eq!(view.content_id, "inline-1@example.com");
        assert_eq!(view.state, InlineImageState::NotDownloaded);
    }
}
