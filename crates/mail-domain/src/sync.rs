//! 同步相关的纯逻辑：文件夹归类、历史范围、退避计算、线程键。
//!
//! 本模块不做任何输入输出，方便在没有网络和数据库的情况下完整测试。
//! 这些规则是同步引擎与存储层共用的判断口径。

use std::time::Duration;

/// 文件夹类别（对应规格 4.3 里 `folder.kind`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderKind {
    /// 收件箱。
    Inbox,
    /// 已发送。
    Sent,
    /// 草稿箱。
    Draft,
    /// 已删除。
    Trash,
    /// 垃圾邮件。
    Junk,
    /// 其它自定义文件夹。
    Custom,
}

impl FolderKind {
    /// 存库用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Sent => "sent",
            Self::Draft => "draft",
            Self::Trash => "trash",
            Self::Junk => "junk",
            Self::Custom => "custom",
        }
    }

    /// 从存库字符串还原。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inbox" => Some(Self::Inbox),
            "sent" => Some(Self::Sent),
            "draft" => Some(Self::Draft),
            "trash" => Some(Self::Trash),
            "junk" => Some(Self::Junk),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::Inbox => "收件箱",
            Self::Sent => "已发送",
            Self::Draft => "草稿箱",
            Self::Trash => "已删除",
            Self::Junk => "垃圾邮件",
            Self::Custom => "自定义",
        }
    }

    /// 是否是收件箱。
    pub fn is_inbox(self) -> bool {
        matches!(self, Self::Inbox)
    }

    /// 是否是服务商维护的系统文件夹（不允许用户改名或删除）。
    pub fn is_system(self) -> bool {
        matches!(
            self,
            Self::Inbox | Self::Sent | Self::Draft | Self::Trash | Self::Junk
        )
    }

    /// 由服务器 LIST 返回的属性与文件夹路径推断类别。
    ///
    /// 优先用服务器广告的专用属性（\\Sent / \\Drafts / \\Trash / \\Junk）；
    /// 国内邮箱常不广告这些属性，再用路径末段兜底。
    pub fn classify(full_path: &str, attributes: &[String]) -> Self {
        let upper: Vec<String> = attributes.iter().map(|item| item.to_ascii_uppercase()).collect();
        let has = |name: &str| upper.iter().any(|item| item == name);
        if has("\\SENT") {
            return Self::Sent;
        }
        if has("\\DRAFTS") {
            return Self::Draft;
        }
        if has("\\TRASH") {
            return Self::Trash;
        }
        if has("\\JUNK") {
            return Self::Junk;
        }
        let leaf = full_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(full_path)
            .to_ascii_lowercase();
        match leaf.as_str() {
            "inbox" => Self::Inbox,
            "sent" | "sent items" | "sent messages" | "已发送" | "已发送邮件" => Self::Sent,
            "drafts" | "draft" | "草稿" | "草稿箱" => Self::Draft,
            "trash" | "deleted" | "deleted items" | "已删除" | "已删除邮件" => Self::Trash,
            "junk" | "spam" | "bulk mail" | "垃圾邮件" | "广告邮件" => Self::Junk,
            _ => Self::Custom,
        }
    }
}

/// 文件夹能不能被用户重命名或删除。
///
/// 判定分三层：服务商维护的系统类别（收件箱 / 已发送 / 草稿箱 / 已删除 / 垃圾邮件）、
/// 协议属性（`\Noselect` 只是空壳、`\NonExistent` 已不存在）、以及 Gmail 的 `[Gmail]` 命名空间。
pub fn folder_changeable(full_path: &str, kind: FolderKind, attributes: &[String]) -> bool {
    if kind.is_system() {
        return false;
    }
    if full_path.trim().eq_ignore_ascii_case("INBOX") {
        return false;
    }
    if full_path.trim_start().starts_with("[Gmail]") {
        return false;
    }
    !has_protocol_attribute(attributes, "\\Noselect") && !has_protocol_attribute(attributes, "\\NonExistent")
}

/// 文件夹下能不能新建子文件夹。
///
/// 交给服务器的能力说话：带 `\\NoInferiors` 说明服务器明确不能再分层，带 `\\Noselect`
/// 的空壳也不能当父级。系统文件夹不在这里一刀切，按用户确认的口径跟服务器来。
pub fn folder_allows_children(attributes: &[String]) -> bool {
    !has_protocol_attribute(attributes, "\\Noselect") && !has_protocol_attribute(attributes, "\\NoInferiors")
}

fn has_protocol_attribute(attributes: &[String], name: &str) -> bool {
    attributes.iter().any(|item| item.eq_ignore_ascii_case(name))
}

/// 历史补齐的覆盖范围（规格 R3：全部 / 近 1 年 / 近 3 年 / 自定义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryRange {
    /// 尽量补到最早。
    All,
    /// 近一年。
    LastYear,
    /// 近三年。
    LastThreeYears,
    /// 自定义天数。
    CustomDays(u32),
}

impl HistoryRange {
    /// 存库用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::LastYear => "1y",
            Self::LastThreeYears => "3y",
            Self::CustomDays(_) => "custom",
        }
    }

    /// 从存库字符串还原；`custom` 需要带上天数。
    pub fn parse(value: &str, days: Option<u32>) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "1y" => Some(Self::LastYear),
            "3y" => Some(Self::LastThreeYears),
            "custom" => days.map(Self::CustomDays),
            _ => None,
        }
    }

    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "全部历史",
            Self::LastYear => "近一年",
            Self::LastThreeYears => "近三年",
            Self::CustomDays(_) => "自定义范围",
        }
    }

    /// 起始时间距今天的天数；`None` 表示没有下界（一直往更早拉）。
    pub fn cutoff_days(self) -> Option<u32> {
        match self {
            Self::All => None,
            Self::LastYear => Some(365),
            Self::LastThreeYears => Some(365 * 3),
            Self::CustomDays(days) => Some(days),
        }
    }
}

/// 同步任务种类（对应规格 4.3 里 `sync_job.kind`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncJobKind {
    /// 首次快照。
    Initial,
    /// 增量同步。
    Incremental,
    /// 空闲实时推送触发。
    Idle,
    /// 后台补齐历史。
    Backfill,
    /// 正文懒加载（Wave 4 使用）。
    BodyFetch,
}

impl SyncJobKind {
    /// 存库用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Incremental => "incremental",
            Self::Idle => "idle",
            Self::Backfill => "backfill",
            Self::BodyFetch => "body_fetch",
        }
    }

    /// 从存库字符串还原。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "initial" => Some(Self::Initial),
            "incremental" => Some(Self::Incremental),
            "idle" => Some(Self::Idle),
            "backfill" => Some(Self::Backfill),
            "body_fetch" => Some(Self::BodyFetch),
            _ => None,
        }
    }
}

/// 同步任务状态（对应规格 4.3 里 `sync_job.state`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncJobState {
    /// 排队中。
    Queued,
    /// 执行中。
    Running,
    /// 已完成。
    Completed,
    /// 失败。
    Failed,
    /// 被取消。
    Cancelled,
}

impl SyncJobState {
    /// 存库用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// 从存库字符串还原。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// 是否是终结状态。
    pub fn is_final(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// 指数退避：第 `attempt` 次重试要等多久。
///
/// `attempt` 从 0 开始；结果不超过 `cap`。`base` 为零时立即返回（测试用）。
pub fn backoff_delay(attempt: u32, base: Duration, cap: Duration) -> Duration {
    if base.is_zero() || cap.is_zero() {
        return Duration::ZERO;
    }
    let factor = 1u128 << attempt.min(20);
    let millis = base.as_millis().saturating_mul(factor);
    Duration::from_millis(millis.min(cap.as_millis()) as u64)
}

/// 线程键：先按主题归并（去掉 Re: / Fwd: 之类前缀并统一大小写）；
/// 主题为空时退回消息编号。Wave 3 的会话聚合直接用它。
pub fn thread_key(subject: &str, message_id: Option<&str>) -> String {
    let normalized = normalize_subject(subject);
    if !normalized.is_empty() {
        return normalized;
    }
    message_id
        .unwrap_or_default()
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_ascii_lowercase()
}

/// 去掉常见的回复 / 转发前缀，折叠空白并统一小写。
pub fn normalize_subject(subject: &str) -> String {
    let prefixes = ["re:", "fw:", "fwd:", "答复:", "回复:", "转发:", "aw:", "sv:"];
    let mut current = subject.trim();
    loop {
        let lower = current.to_ascii_lowercase();
        let mut stripped = None;
        for prefix in prefixes {
            if lower.starts_with(prefix) {
                let rest = current[prefix.len()..].trim_start();
                if !rest.is_empty() {
                    stripped = Some(rest);
                }
                break;
            }
        }
        match stripped {
            Some(rest) => current = rest,
            None => break,
        }
    }
    current
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        backoff_delay, folder_allows_children, folder_changeable, normalize_subject, thread_key, FolderKind,
        HistoryRange, SyncJobKind,
    };

    #[test]
    fn 文件夹按专用属性归类() {
        assert_eq!(FolderKind::classify("随便", &["\\Sent".into()]), FolderKind::Sent);
        assert_eq!(FolderKind::classify("随便", &["\\Junk".into()]), FolderKind::Junk);
    }

    #[test]
    fn 文件夹按路径兜底归类() {
        assert_eq!(FolderKind::classify("INBOX", &[]), FolderKind::Inbox);
        assert_eq!(FolderKind::classify("[Gmail]/已发送邮件", &[]), FolderKind::Sent);
        assert_eq!(FolderKind::classify("其他文件夹/Spam", &[]), FolderKind::Junk);
        assert_eq!(FolderKind::classify("项目/2026", &[]), FolderKind::Custom);
    }

    #[test]
    fn 任务种类与状态能往返() {
        assert_eq!(
            SyncJobKind::parse(SyncJobKind::Backfill.as_str()),
            Some(SyncJobKind::Backfill)
        );
        assert_eq!(
            HistoryRange::parse("custom", Some(90)),
            Some(HistoryRange::CustomDays(90))
        );
        assert_eq!(HistoryRange::parse("custom", None), None);
        assert_eq!(HistoryRange::LastYear.cutoff_days(), Some(365));
        assert_eq!(HistoryRange::All.cutoff_days(), None);
    }

    #[test]
    fn 退避按倍数增长且封顶() {
        let base = Duration::from_secs(1);
        let cap = Duration::from_secs(10);
        assert_eq!(backoff_delay(0, base, cap), Duration::from_secs(1));
        assert_eq!(backoff_delay(1, base, cap), Duration::from_secs(2));
        assert_eq!(backoff_delay(2, base, cap), Duration::from_secs(4));
        assert_eq!(backoff_delay(9, base, cap), Duration::from_secs(10));
        assert_eq!(backoff_delay(3, Duration::ZERO, cap), Duration::ZERO);
    }

    #[test]
    fn 主题前缀会被去掉() {
        assert_eq!(normalize_subject("Re: 项目进度"), "项目进度");
        assert_eq!(normalize_subject("RE: Fwd:  周报 "), "周报");
        assert_eq!(normalize_subject("回复: 报价"), "报价");
        assert_eq!(thread_key("Re: 周报", Some("<abc@x>")), "周报");
        assert_eq!(thread_key("", Some("<ABC@x>")), "abc@x");
    }

    #[test]
    fn 系统文件夹不能改名但能按服务器能力建子文件夹() {
        for kind in [
            FolderKind::Inbox,
            FolderKind::Sent,
            FolderKind::Draft,
            FolderKind::Trash,
            FolderKind::Junk,
        ] {
            assert!(!folder_changeable("随便", kind, &[]), "{kind:?} 不应开放改名删除");
            // 用户已确认：系统文件夹也能建子文件夹，去留交给服务器能力。
            assert!(folder_allows_children(&[]), "{kind:?} 建子文件夹应当跟着服务器来");
        }
        // 类别认不出来，但路径就是收件箱时也要挡住改名删除。
        assert!(!folder_changeable("inbox", FolderKind::Custom, &[]));
        // 自定义文件夹默认两层都开放。
        assert!(folder_changeable("项目/2026", FolderKind::Custom, &[]));
        assert!(folder_allows_children(&[]));
    }

    #[test]
    fn 协议属性会挡住改名与建子文件夹() {
        let noselect = vec!["\\Noselect".to_string()];
        let inexistent = vec!["\\NonExistent".to_string()];
        let no_inferiors = vec!["\\NoInferiors".to_string()];
        // 空壳和已消失的文件夹既不能改也不能建子文件夹。
        assert!(!folder_changeable("归档/全部邮件", FolderKind::Custom, &noselect));
        assert!(!folder_changeable("旧目录", FolderKind::Custom, &inexistent));
        assert!(!folder_allows_children(&noselect));
        // 服务器说不能再分层时，只挡子文件夹，不挡改名。
        assert!(folder_changeable("项目", FolderKind::Custom, &no_inferiors));
        assert!(!folder_allows_children(&no_inferiors));
        // 系统文件夹只要服务器没说不能再分层，就能建子文件夹。
        assert!(folder_allows_children(&[]));
        // Gmail 的方括号命名空间不开放改名删除。
        assert!(!folder_changeable("[Gmail]/全部邮件", FolderKind::Custom, &[]));
    }
}
