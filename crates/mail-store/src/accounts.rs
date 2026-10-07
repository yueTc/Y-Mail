//! 账号表的读写。
//!
//! 安全约定：本模块不接受、也不保存授权码本体。授权码存进系统凭据管理器，
//! 表里只留引用键（`credential_key`）。

use mail_domain::{
    Account, AccountDraft, AccountId, AccountProxyMode, AuthType, OAuthProvider, Security, ServerConfig,
};
use rusqlite::OptionalExtension;

use crate::connection::Store;
use crate::error::StoreError;

/// 账号表的全部列，顺序与 `row_to_account` 一一对应。
const SELECT_ACCOUNT: &str = "SELECT
    id, display_name, email, auth_type, username,
    imap_host, imap_port, imap_security,
    smtp_host, smtp_port, smtp_security,
    proxy_mode, proxy_id, color, enabled, credential_key, created_at, updated_at,
    oauth_provider, oauth_client_id
    FROM account";

fn row_to_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    let auth_type: String = row.get(3)?;
    let imap_port: i64 = row.get(6)?;
    let imap_security: String = row.get(7)?;
    let smtp_port: i64 = row.get(9)?;
    let smtp_security: String = row.get(10)?;
    let proxy_mode: String = row.get(11)?;
    let proxy_id: Option<i64> = row.get(12)?;
    let enabled: i64 = row.get(14)?;
    let oauth_provider: Option<String> = row.get(18)?;
    let oauth_client_id: String = row.get(19)?;

    Ok(Account {
        id: AccountId(row.get(0)?),
        display_name: row.get(1)?,
        email: row.get(2)?,
        auth_type: AuthType::parse(&auth_type).unwrap_or(AuthType::Password),
        username: row.get(4)?,
        imap: ServerConfig {
            host: row.get(5)?,
            port: u16::try_from(imap_port).unwrap_or(0),
            security: Security::parse(&imap_security).unwrap_or(Security::Tls),
        },
        smtp: ServerConfig {
            host: row.get(8)?,
            port: u16::try_from(smtp_port).unwrap_or(0),
            security: Security::parse(&smtp_security).unwrap_or(Security::Tls),
        },
        proxy: AccountProxyMode::parse(&proxy_mode, proxy_id).unwrap_or(AccountProxyMode::InheritGlobal),
        color: row.get(13)?,
        enabled: enabled != 0,
        oauth_provider: oauth_provider.as_deref().and_then(OAuthProvider::parse),
        oauth_client_id,
        credential_key: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

impl Store {
    /// 列出全部账号（按主键升序）。
    pub fn list_accounts(&self) -> Result<Vec<Account>, StoreError> {
        let sql = format!("{SELECT_ACCOUNT} ORDER BY id ASC");
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut accounts = Vec::new();
        while let Some(row) = rows.next()? {
            accounts.push(row_to_account(row)?);
        }
        Ok(accounts)
    }

    /// 按主键取账号。
    pub fn get_account(&self, id: AccountId) -> Result<Option<Account>, StoreError> {
        let sql = format!("{SELECT_ACCOUNT} WHERE id = ?1");
        self.conn()
            .query_row(&sql, [id.0], row_to_account)
            .optional()
            .map_err(StoreError::from)
    }

    /// 邮箱地址是否已被占用（新增与修改时查重，不区分大小写）。
    pub fn email_taken(&self, email: &str, exclude: Option<AccountId>) -> Result<bool, StoreError> {
        let found: Option<i64> = match exclude {
            Some(id) => self
                .conn()
                .query_row(
                    "SELECT id FROM account WHERE lower(email) = lower(?1) AND id <> ?2",
                    rusqlite::params![email, id.0],
                    |row| row.get(0),
                )
                .optional()?,
            None => self
                .conn()
                .query_row(
                    "SELECT id FROM account WHERE lower(email) = lower(?1)",
                    [email],
                    |row| row.get(0),
                )
                .optional()?,
        };
        Ok(found.is_some())
    }

    /// 新建账号，返回新主键。
    pub fn insert_account(
        &self,
        draft: &AccountDraft,
        credential_key: Option<&str>,
    ) -> Result<AccountId, StoreError> {
        self.conn().execute(
            "INSERT INTO account (
                display_name, email, auth_type, username,
                imap_host, imap_port, imap_security,
                smtp_host, smtp_port, smtp_security,
                proxy_mode, proxy_id, color, enabled, credential_key,
                oauth_provider, oauth_client_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            rusqlite::params![
                draft.display_name,
                draft.email,
                draft.auth_type.as_str(),
                draft.username,
                draft.imap.host,
                i64::from(draft.imap.port),
                draft.imap.security.as_str(),
                draft.smtp.host,
                i64::from(draft.smtp.port),
                draft.smtp.security.as_str(),
                draft.proxy.as_str(),
                proxy_id_of(draft.proxy),
                draft.color,
                i64::from(draft.enabled),
                credential_key,
                draft.oauth_provider.map(|provider| provider.as_str()),
                draft.oauth_client_id,
            ],
        )?;
        Ok(AccountId(self.conn().last_insert_rowid()))
    }

    /// 更新账号；不存在时返回 `false`。
    pub fn update_account(
        &self,
        id: AccountId,
        draft: &AccountDraft,
        credential_key: Option<&str>,
    ) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "UPDATE account SET
                display_name = ?2, email = ?3, auth_type = ?4, username = ?5,
                imap_host = ?6, imap_port = ?7, imap_security = ?8,
                smtp_host = ?9, smtp_port = ?10, smtp_security = ?11,
                proxy_mode = ?12, proxy_id = ?13, color = ?14, enabled = ?15,
                credential_key = ?16, oauth_provider = ?17, oauth_client_id = ?18,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![
                id.0,
                draft.display_name,
                draft.email,
                draft.auth_type.as_str(),
                draft.username,
                draft.imap.host,
                i64::from(draft.imap.port),
                draft.imap.security.as_str(),
                draft.smtp.host,
                i64::from(draft.smtp.port),
                draft.smtp.security.as_str(),
                draft.proxy.as_str(),
                proxy_id_of(draft.proxy),
                draft.color,
                i64::from(draft.enabled),
                credential_key,
                draft.oauth_provider.map(|provider| provider.as_str()),
                draft.oauth_client_id,
            ],
        )?;
        Ok(changed > 0)
    }

    /// 删除账号（邮件等关联数据由外键级联，Wave 2 起生效）。
    pub fn delete_account(&self, id: AccountId) -> Result<bool, StoreError> {
        let changed = self.conn().execute("DELETE FROM account WHERE id = ?1", [id.0])?;
        Ok(changed > 0)
    }

    /// 账号在系统凭据管理器里的引用键。
    pub fn account_credential_key(&self, id: AccountId) -> Result<Option<String>, StoreError> {
        let key = self
            .conn()
            .query_row(
                "SELECT credential_key FROM account WHERE id = ?1",
                [id.0],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(key.flatten())
    }
}

/// 草稿里代理编号的落库形式（非「指定代理」写 NULL）。
fn proxy_id_of(mode: AccountProxyMode) -> Option<i64> {
    match mode {
        AccountProxyMode::Custom(id) => Some(id.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use mail_domain::{AccountId, AccountProxyMode, AuthType, OAuthProvider, Security, ServerConfig};

    use crate::Store;

    fn migrated() -> Store {
        let mut store = Store::open_in_memory().expect("打开内存库");
        store.run_migrations().expect("迁移");
        store
    }

    pub(crate) fn sample_draft() -> mail_domain::AccountDraft {
        mail_domain::AccountDraft {
            display_name: "我的邮箱".to_string(),
            email: "someone@example.com".to_string(),
            auth_type: AuthType::Password,
            username: "someone@example.com".to_string(),
            imap: ServerConfig {
                host: "imap.example.com".to_string(),
                port: 993,
                security: Security::Tls,
            },
            smtp: ServerConfig {
                host: "smtp.example.com".to_string(),
                port: 465,
                security: Security::Tls,
            },
            proxy: AccountProxyMode::InheritGlobal,
            color: "#3366ff".to_string(),
            enabled: true,
            oauth_provider: None,
            oauth_client_id: String::new(),
        }
    }

    #[test]
    fn 账号增删改查可用() {
        let store = migrated();
        let id = store
            .insert_account(&sample_draft(), Some("account/1/secret"))
            .expect("插入账号");

        let account = store.get_account(id).expect("查询").expect("应存在");
        assert_eq!(account.email, "someone@example.com");
        assert_eq!(account.imap.host, "imap.example.com");
        assert_eq!(account.imap.security, Security::Tls);
        assert_eq!(account.smtp.port, 465);
        assert_eq!(account.proxy, AccountProxyMode::InheritGlobal);
        assert!(account.enabled);
        assert_eq!(account.credential_key.as_deref(), Some("account/1/secret"));

        let mut changed = sample_draft();
        changed.display_name = "改过的名字".to_string();
        changed.imap.security = Security::StartTls;
        changed.imap.port = 143;
        assert!(store.update_account(id, &changed, None).expect("更新"));
        let account = store.get_account(id).expect("查询").expect("应存在");
        assert_eq!(account.display_name, "改过的名字");
        assert_eq!(account.imap.security, Security::StartTls);
        assert_eq!(account.credential_key, None);

        assert_eq!(store.list_accounts().expect("列表").len(), 1);
        assert!(store.delete_account(id).expect("删除"));
        assert!(store.get_account(id).expect("查询").is_none());
    }

    #[test]
    fn 邮箱地址不区分大小写地查重() {
        let store = migrated();
        let id = store.insert_account(&sample_draft(), None).expect("插入");
        assert!(store.email_taken("SOMEONE@example.com", None).expect("查重"));
        assert!(!store
            .email_taken("SOMEONE@example.com", Some(id))
            .expect("排除自己后不算重复"));
    }
    #[test]
    fn 邮箱唯一索引也不区分大小写() {
        let store = migrated();
        store.insert_account(&sample_draft(), None).expect("插入第一个");

        let mut duplicate = sample_draft();
        duplicate.email = "SOMEONE@EXAMPLE.COM".to_string();
        let err = store
            .insert_account(&duplicate, None)
            .expect_err("大小写不同的同一邮箱应被唯一索引拦住");
        assert!(
            matches!(err, crate::StoreError::Sqlite(_)),
            "应是唯一约束错误：{err}"
        );
        assert_eq!(store.list_accounts().expect("列表").len(), 1);
    }

    #[test]
    fn 数据库文件里查不到明文授权码() {
        let dir = tempfile::tempdir().expect("临时目录");
        let db_path = dir.path().join("ymail.db");
        let mut store = Store::open(&db_path).expect("打开");
        store.run_migrations().expect("迁移");

        let auth_code = "SuperSecretAuthCode123!";
        let _ = auth_code; // 授权码只留在内存里，不应该进入任何落库调用。
        store
            .insert_account(&sample_draft(), Some("account/1/secret"))
            .expect("插入账号");

        let bytes = std::fs::read(&db_path).expect("读取数据库文件");
        let needle = auth_code.as_bytes();
        assert!(
            !bytes.windows(needle.len()).any(|window| window == needle),
            "数据库文件中不应出现明文授权码"
        );
    }

    #[test]
    fn 账号主键可读() {
        assert_eq!(AccountId(7).to_string(), "7");
    }

    #[test]
    fn 授权账号字段可存取() {
        let store = migrated();
        let mut draft = sample_draft();
        draft.auth_type = AuthType::OAuth2;
        draft.oauth_provider = Some(OAuthProvider::Gmail);
        draft.oauth_client_id = "client-abc.apps.googleusercontent.com".to_string();
        draft.username = draft.email.clone();

        let id = store.insert_account(&draft, None).expect("插入授权账号");
        let account = store.get_account(id).expect("查询").expect("应存在");
        assert_eq!(account.auth_type, AuthType::OAuth2);
        assert_eq!(account.oauth_provider, Some(OAuthProvider::Gmail));
        assert_eq!(account.oauth_client_id, "client-abc.apps.googleusercontent.com");

        let mut changed = draft.clone();
        changed.oauth_provider = Some(OAuthProvider::Microsoft);
        changed.oauth_client_id = "ms-client".to_string();
        assert!(store.update_account(id, &changed, None).expect("更新"));
        let account = store.get_account(id).expect("查询").expect("应存在");
        assert_eq!(account.oauth_provider, Some(OAuthProvider::Microsoft));
        assert_eq!(account.oauth_client_id, "ms-client");
    }

    #[test]
    fn 密码账号的授权字段为空() {
        let store = migrated();
        let id = store.insert_account(&sample_draft(), None).expect("插入");
        let account = store.get_account(id).expect("查询").expect("应存在");
        assert_eq!(account.oauth_provider, None);
        assert!(account.oauth_client_id.is_empty());
    }
}
