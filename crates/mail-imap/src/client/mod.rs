//! IMAP 完整客户端：连接、认证、列文件夹、抓取元数据与 IDLE 实时推送。
//!
//! 拆成三块：本文件只放对外类型；`session` 负责连接与命令收发；
//! `parse` 负责把服务器应答拆成结构。网络层走 `mail-net`，代理与加密对它透明。

mod parse;
mod session;
#[cfg(test)]
mod tests;
pub(crate) mod utf7;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mail_domain::account::Security;
use mail_domain::auth::AuthMaterial;
use mail_net::Stream;
use tokio::sync::Notify;

/// 连一条 IMAP 连接所需的全部信息。
#[derive(Clone, PartialEq, Eq)]
pub struct ClientConfig {
    /// 收件服务器地址。
    pub host: String,
    /// 收件服务器端口。
    pub port: u16,
    /// 加密方式。
    pub security: Security,
    /// 登录名（多数邮箱就是邮箱地址）。
    pub username: String,
    /// 认证材料：授权码或 OAuth2 访问令牌；绝不出现在日志与错误里。
    pub auth: AuthMaterial,
    /// 单条命令的超时时间。
    pub timeout: Duration,
}

/// 服务器上的一个文件夹。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderInfo {
    /// 展示用完整路径，已把 Modified UTF-7 解码成正常文字。
    pub full_path: String,
    /// 服务器上的原始完整路径，用于 SELECT 等命令。
    pub server_path: String,
    /// 层级分隔符；服务器没给时为空串。
    pub delimiter: String,
    /// 服务器广告的属性（如 `\HasNoChildren`、`\Sent`）。
    pub attributes: Vec<String>,
}

impl FolderInfo {
    /// 服务器是否给了某个属性（大小写不敏感）。
    pub fn has_attribute(&self, name: &str) -> bool {
        self.attributes.iter().any(|attr| attr.eq_ignore_ascii_case(name))
    }

    /// 这个文件夹能不能被 SELECT。
    ///
    /// 带 `\NoSelect` 的文件夹只是「装子文件夹的空壳」，里面永远不会有邮件。
    /// 部分服务器（如腾讯企业邮）对它发 SELECT 还会假装回 `OK` 却不真正选中，
    /// 紧接着的搜索就会报「先选文件夹」。所以这类文件夹一律不去打开。
    pub fn is_selectable(&self) -> bool {
        !self.has_attribute("\\Noselect")
    }

    /// 这个文件夹能不能被重命名或删除。
    ///
    /// 只看协议属性；「收件箱 / 已发送 / 草稿箱 / 已删除 / 垃圾邮件」这类
    /// 业务判定由上层结合 FolderKind 完成。
    pub fn is_changeable(&self) -> bool {
        !self.has_attribute("\\Noselect") && !self.has_attribute("\\NonExistent")
    }

    /// 这个文件夹下能不能再建子文件夹。
    pub fn allows_children(&self) -> bool {
        !self.has_attribute("\\Noselect") && !self.has_attribute("\\NoInferiors")
    }
}

/// 服务商明确说「这里不能再建子文件夹」时的统一中文说法。
///
/// 单独抽成常量：`session` 用它拼错误文案，上层用它判断该不该关掉入口。
pub const NO_CHILD_FOLDER_REASON: &str = "服务商不允许在这个文件夹下面建子文件夹";

/// 把服务商拒绝文件夹操作的原文翻成用户能看懂的中文。
///
/// 认得出常见说法就给对应中文；服务商本来就是中文的照原样用；两边都认不出时
/// 只给一句通用说明，不把英文原文端给用户。
pub fn folder_rejection_reason(detail: &str) -> String {
    let trimmed = detail.trim();
    let lower = trimmed.to_ascii_lowercase();
    let mapped: Option<&str> = if lower.contains("subfolder")
        || lower.contains("sub folder")
        || lower.contains("noinferiors")
        || lower.contains("inferior")
        || lower.contains("child folder")
    {
        Some(NO_CHILD_FOLDER_REASON)
    } else if lower.contains("nonexistent")
        || lower.contains("does not exist")
        || lower.contains("no such")
        || lower.contains("not found")
    {
        Some("服务商那边没有这个文件夹了")
    } else if lower.contains("already exist") || lower.contains("exists already") {
        Some("已经有同名的文件夹了")
    } else if lower.contains("permission")
        || lower.contains("denied")
        || lower.contains("not allowed")
        || lower.contains("forbidden")
        || lower.contains("authorization")
    {
        Some("服务商没有开放这项操作的权限")
    } else if lower.contains("not empty") || lower.contains("has children") || lower.contains("in use") {
        Some("这个文件夹里还有内容，服务商不允许直接删")
    } else if lower.contains("quota") || lower.contains("limit reached") || lower.contains("too many") {
        Some("已经达到服务商允许的文件夹上限了")
    } else if lower.contains("invalid") || lower.contains("illegal") {
        Some("服务商不接受这个名字")
    } else {
        None
    };
    if let Some(text) = mapped {
        return text.to_string();
    }
    if !trimmed.is_empty() && trimmed.chars().any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch)) {
        return trimmed.to_string();
    }
    "服务商拒绝了这次操作，没有给出可读的原因".to_string()
}

/// 这次拒绝是不是「这个位置不能再分层」。
///
/// 有的服务商（如 QQ 邮箱的「订阅邮件」）LIST 里不给 `\NoInferiors`，
/// 只有真发 CREATE 才回 NO；上层拿这个判断把结论记下来。
pub fn rejection_blocks_children(message: &str) -> bool {
    message.contains(NO_CHILD_FOLDER_REASON)
}

/// 把用户输入的显示名拼成服务器要的路径。
///
/// 父路径必须是服务器原始名（可能已是 Modified UTF-7），只有新名字需要编码。
/// `delimiter` 为空表示服务器不支持层级，此时 `parent` 必须为空。
pub fn build_server_path(parent: Option<&str>, delimiter: &str, name: &str) -> String {
    let encoded = utf7::encode(name);
    match parent {
        Some(path) if !path.is_empty() => {
            let separator = if delimiter.is_empty() { "/" } else { delimiter };
            format!("{path}{separator}{encoded}")
        }
        _ => encoded,
    }
}

/// SELECT 之后邮箱的关键状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MailboxStatus {
    /// 服务器给的 UIDVALIDITY。
    pub uidvalidity: Option<u32>,
    /// 服务器给的下一封邮件 UID。
    pub uidnext: Option<u32>,
    /// 邮件总数。
    pub exists: u32,
    /// 未读数。
    pub unseen: u32,
}

/// 一个邮件地址（显示名可为空）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// 显示名。
    pub name: String,
    /// 邮箱地址。
    pub address: String,
}

/// 信封里的常用字段，对应 IMAP ENVELOPE。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Envelope {
    /// 发信时间原文（RFC 5322）。
    pub date: String,
    /// 主题。
    pub subject: String,
    /// 发件人。
    pub from: Vec<Address>,
    /// 收件人。
    pub to: Vec<Address>,
    /// 抄送。
    pub cc: Vec<Address>,
    /// 这是回复哪一封。
    pub in_reply_to: String,
    /// 消息编号。
    pub message_id: String,
}

/// 一封邮件的元数据（不含正文）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageMeta {
    /// 服务器 UID。
    pub uid: u32,
    /// 标志（`\Seen`、`\Flagged` 等）。
    pub flags: Vec<String>,
    /// 服务器内部日期原文。
    pub internal_date: String,
    /// 邮件大小（字节）。
    pub size: u32,
    /// 信封。
    pub envelope: Envelope,
    /// 从 BODYSTRUCTURE 判断出的「有附件」（含内嵌图片）。
    pub has_attachments: bool,
}

/// IDLE 一次等待的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleOutcome {
    /// 邮箱有变化。
    Changed,
    /// 等到超时都没有变化。
    Timeout,
    /// 被上层要求提前收尾（停止同步）。
    Cancelled,
}

/// 打断一次 IDLE 等待的信号。
///
/// 停止同步时，后台线程可能正挂在「等新邮件」上；发一次 [IdleInterrupt::interrupt]
/// 就能让那次 IDLE 立刻发 DONE 收尾返回，不必等满超时（默认 4 分钟）。
#[derive(Clone, Default)]
pub struct IdleInterrupt {
    inner: Arc<IdleInterruptInner>,
}

#[derive(Default)]
struct IdleInterruptInner {
    flag: AtomicBool,
    notify: Notify,
}

impl IdleInterrupt {
    /// 新建一个未打断的信号。
    pub fn new() -> Self {
        Self::default()
    }

    /// 标记打断并唤醒等待者；重复调用无副作用。
    pub fn interrupt(&self) {
        self.inner.flag.store(true, Ordering::SeqCst);
        self.inner.notify.notify_one();
    }

    /// 是否已经被打断。
    pub fn is_interrupted(&self) -> bool {
        self.inner.flag.load(Ordering::SeqCst)
    }

    /// 等到被打断为止；已经打断过就立刻返回。
    pub async fn wait(&self) {
        if self.is_interrupted() {
            return;
        }
        let notified = self.inner.notify.notified();
        tokio::pin!(notified);
        // 先把等待注册好再复查一次，堵住「检查完才打断」的窗口。
        if self.is_interrupted() {
            return;
        }
        notified.await;
    }
}

/// 手写的 Debug：只显示能力清单与超时，连接与敏感值一律不进日志。
impl std::fmt::Debug for ImapClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImapClient")
            .field("capabilities", &self.capabilities)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}
/// 一条已登录的 IMAP 长连接。
pub struct ImapClient {
    /// 底层连接（明文或 TLS）。
    stream: Stream,
    /// 下一条命令的编号。
    tag: u32,
    /// 服务器能力清单（大写）。
    capabilities: Vec<String>,
    /// 错误信息里要抹掉的敏感值。
    redactor: Vec<String>,
    /// 单条命令超时。
    timeout: Duration,
    /// 最近一次成功打开的文件夹；服务器意外取消选中时用它自动重开。
    selected_folder: Option<String>,
}
