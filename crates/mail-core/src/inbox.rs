//! 统一收件箱的只读门面（Wave 3）。
//!
//! 只做编排：把存储层的查询结果加上分页信息交给外壳。这里不写库、不碰网络，
//! 存储锁在一次调用里只取一次，绝不跨 `.await`。

use mail_store::{AccountInboxSummary, InboxFolder, InboxMessage, InboxQuery, InboxThread};

use crate::engine::{EngineError, MailEngine};

/// 一页收件箱邮件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxMessagePage {
    /// 本页邮件。
    pub items: Vec<InboxMessage>,
    /// 符合条件的总条数。
    pub total: i64,
    /// 本页跳过的条数。
    pub offset: i64,
    /// 本页的最大条数。
    pub limit: i64,
}

/// 一页会话线程。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxThreadPage {
    /// 本页线程。
    pub items: Vec<InboxThread>,
    /// 符合条件的总行数。
    pub total: i64,
    /// 本页跳过的条数。
    pub offset: i64,
    /// 本页的最大条数。
    pub limit: i64,
}

impl MailEngine {
    /// 平铺模式：一页邮件 + 总数。
    pub fn inbox_messages(&self, query: &InboxQuery) -> Result<InboxMessagePage, EngineError> {
        let store = self.store();
        let total = store.count_inbox_messages(query)?;
        let items = store.list_inbox_messages(query)?;
        Ok(InboxMessagePage {
            items,
            total,
            offset: query.offset,
            limit: query.limit,
        })
    }

    /// 会话模式：一页线程 + 总数。
    pub fn inbox_threads(&self, query: &InboxQuery) -> Result<InboxThreadPage, EngineError> {
        let store = self.store();
        let total = store.count_inbox_threads(query)?;
        let items = store.list_inbox_threads(query)?;
        Ok(InboxThreadPage {
            items,
            total,
            offset: query.offset,
            limit: query.limit,
        })
    }

    /// 展开一条会话：取该账号该线程的邮件（新的在前）。
    pub fn thread_messages(
        &self,
        account_id: i64,
        thread_key: &str,
        limit: i64,
    ) -> Result<Vec<InboxMessage>, EngineError> {
        Ok(self.store().list_thread_messages(account_id, thread_key, limit)?)
    }

    /// 每个账号的收件箱汇总（用于未读合计与账号切换）。
    pub fn inbox_account_summary(&self) -> Result<Vec<AccountInboxSummary>, EngineError> {
        Ok(self.store().account_inbox_summary()?)
    }

    /// 全部账号的文件夹列表（带本地条数，供左侧文件夹树）。
    pub fn inbox_folders(&self) -> Result<Vec<InboxFolder>, EngineError> {
        Ok(self.store().list_inbox_folders()?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mail_store::InboxQuery;

    use super::MailEngine;
    use crate::secrets::MemorySecretStore;

    #[test]
    fn 空库时各查询返回空页与零合计() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let engine = MailEngine::initialize_with_secrets(dir.path(), Arc::new(MemorySecretStore::new()))
            .expect("初始化引擎");

        let messages = engine.inbox_messages(&InboxQuery::default()).expect("平铺查询");
        assert_eq!(messages.total, 0);
        assert!(messages.items.is_empty());

        let threads = engine.inbox_threads(&InboxQuery::default()).expect("线程查询");
        assert_eq!(threads.total, 0);
        assert!(threads.items.is_empty());

        assert!(engine.inbox_account_summary().expect("汇总").is_empty());
        assert!(engine.inbox_folders().expect("文件夹").is_empty());
    }
}
