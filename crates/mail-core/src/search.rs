//! 搜索编排（Wave 5）。
//!
//! 主路径：先在本地库里搜（FTS5，快）；结果不足时，再看有没有「还没补完的收件箱」，
//! 有就联网补一批历史，再搜一次。这就是规格 R7 的「按需深拉」。
//!
//! 边界：深拉沿用 Wave 2 的历史范围（默认近 1 年），每次只补有界的一批，
//! 不做全量拖取；联网失败不拦本地结果，只把失败原因带回给界面。

use mail_store::{SearchPage, SearchQuery};

use crate::engine::{EngineError, MailEngine};

/// 一次深搜最多补几个账号，防止一次点搜索连一堆服务器。
const MAX_DEEP_ACCOUNTS: usize = 3;

/// 深拉返回：本地结果 + 是否真的补过历史 + 补历史时的问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepSearchPage {
    /// 最终这一页结果。
    pub page: SearchPage,
    /// 是否真的联网补过历史。
    pub deep_synced: bool,
    /// 补历史失败时的可读原因；成功或无需要时为 None。
    pub deep_error: Option<String>,
}

impl MailEngine {
    /// 本地搜索：只查已经同步下来的邮件，不联网。
    pub fn search_messages(&self, query: &SearchQuery) -> Result<SearchPage, EngineError> {
        Ok(self.store().search_messages(query)?)
    }

    /// 深搜：本地搜一遍；结果不够铺满这一页，且有收件箱还没补到最早就联网补一批，再搜一遍。
    pub async fn search_messages_deep(&self, query: &SearchQuery) -> Result<DeepSearchPage, EngineError> {
        let first = self.search_messages(query)?;
        if first.total >= query.limit.max(1) {
            return Ok(DeepSearchPage {
                page: first,
                deep_synced: false,
                deep_error: None,
            });
        }

        let pending = self.accounts_with_pending_backfill()?;
        if pending.is_empty() {
            return Ok(DeepSearchPage {
                page: first,
                deep_synced: false,
                deep_error: None,
            });
        }

        let mut synced_any = false;
        let mut last_error = None;
        for account_id in pending {
            match self.sync.backfill_once(account_id).await {
                Ok(true) => synced_any = true,
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(account = account_id, error = %error, "深搜补齐历史失败");
                    last_error = Some(format!("账号 {account_id} 的历史补齐失败：{error}"));
                }
            }
        }

        let page = self.search_messages(query)?;
        Ok(DeepSearchPage {
            page,
            deep_synced: synced_any,
            deep_error: last_error,
        })
    }

    /// 找出还有收件箱没补到最早的启用账号（最多去重几个，避免一次搜太久）。
    fn accounts_with_pending_backfill(&self) -> Result<Vec<i64>, EngineError> {
        let store = self.store();
        let accounts = store.list_accounts()?;
        let mut out = Vec::new();
        for account in accounts {
            if !account.enabled {
                continue;
            }
            let folders = store.list_folders(account.id.0)?;
            let pending = folders
                .iter()
                .any(|folder| folder.kind.is_inbox() && folder.synced_min_uid.is_some_and(|uid| uid > 1));
            if pending {
                out.push(account.id.0);
            }
            if out.len() >= MAX_DEEP_ACCOUNTS {
                break;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mail_store::SearchQuery;

    use super::MailEngine;
    use crate::secrets::MemorySecretStore;

    fn engine() -> (tempfile::TempDir, MailEngine) {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let engine = MailEngine::initialize_with_secrets(dir.path(), Arc::new(MemorySecretStore::new()))
            .expect("初始化引擎");
        (dir, engine)
    }

    #[test]
    fn 标签筛选能搜到贴了标签的邮件() {
        use mail_domain::FolderKind;
        use mail_store::{InboxQuery, MailLabelSource, NewMessage};

        let (_dir, engine) = engine();
        let store = engine.store();
        let draft = mail_domain::AccountDraft {
            display_name: "测试".to_string(),
            email: "t@example.com".to_string(),
            auth_type: mail_domain::AuthType::Password,
            username: "t@example.com".to_string(),
            imap: mail_domain::ServerConfig {
                host: "imap.example.com".to_string(),
                port: 993,
                security: mail_domain::Security::Tls,
            },
            smtp: mail_domain::ServerConfig {
                host: "smtp.example.com".to_string(),
                port: 465,
                security: mail_domain::Security::Tls,
            },
            proxy: mail_domain::AccountProxyMode::InheritGlobal,
            color: String::new(),
            enabled: true,
            oauth_provider: None,
            oauth_client_id: String::new(),
        };
        let account_id = store.insert_account(&draft, None).expect("插入账号").0;
        let folder_id = store
            .upsert_folder(account_id, "INBOX", "/", FolderKind::Inbox)
            .expect("插入文件夹");
        store
            .insert_messages(&[NewMessage {
                account_id,
                folder_id,
                uid: 1,
                message_id_header: "<m1@x>".to_string(),
                thread_key: "t1".to_string(),
                subject: "本月账单".to_string(),
                from_name: "张三".to_string(),
                from_addr: "z@example.com".to_string(),
                to_json: "[]".to_string(),
                cc_json: "[]".to_string(),
                date_utc: "2026-10-10T01:00:00Z".to_string(),
                size: 1000,
                has_attachments: false,
                is_read: true,
                is_flagged: false,
                is_answered: false,
                is_draft: false,
            }])
            .expect("插入邮件");
        let message_id = store
            .list_inbox_messages(&InboxQuery {
                account_id: Some(account_id),
                ..Default::default()
            })
            .expect("读邮件")
            .first()
            .expect("应有邮件")
            .id;
        let label_id = store
            .list_mail_labels()
            .expect("标签")
            .into_iter()
            .find(|label| label.name == "账单")
            .expect("预置账单")
            .id;
        store
            .set_message_label(message_id, label_id, MailLabelSource::Auto, Some(0.9))
            .expect("贴标签");
        // 引擎的 store() 是普通互斥锁，调用引擎方法前必须先放掉手里的守卫。
        drop(store);

        let page = engine
            .search_messages(&SearchQuery::new("label:账单", Some(account_id), 0, 50))
            .expect("搜索标签");
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].message.label_name.as_deref(), Some("账单"));
        assert_eq!(page.items[0].message.label_color.as_deref(), Some("orange"));
    }

    #[test]
    fn 空库搜索返回空页() {
        let (_dir, engine) = engine();
        let page = engine
            .search_messages(&SearchQuery::new("发票", None, 0, 50))
            .expect("本地搜索");
        assert_eq!(page.total, 0);
        assert!(page.items.is_empty());
    }

    #[test]
    fn 没有待补账号时深搜不联网() {
        let (_dir, engine) = engine();
        let result = tokio::runtime::Runtime::new()
            .expect("运行时")
            .block_on(engine.search_messages_deep(&SearchQuery::new("发票", None, 0, 50)))
            .expect("深搜");
        assert!(!result.deep_synced, "没有账号就不该联网");
        assert!(result.deep_error.is_none());
    }
}
