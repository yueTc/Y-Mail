//! 代理与全局代理设置的读写。
//!
//! 与账号一样：代理密码永远不落库，表里只留系统凭据管理器的引用键。

use mail_domain::{GlobalProxyMode, ProxyConfig, ProxyId, ProxyKind};
use rusqlite::OptionalExtension;

use crate::connection::Store;
use crate::error::StoreError;

/// 数据库里的代理行（含系统凭据管理器引用键，不含密码本体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredProxy {
    /// 代理配置（不含密码）。
    pub config: ProxyConfig,
    /// 系统凭据管理器引用键。
    pub password_key: Option<String>,
}

/// 把一行查询结果转成代理行。
fn row_to_proxy(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredProxy> {
    let kind: String = row.get(2)?;
    let port: i64 = row.get(4)?;
    Ok(StoredProxy {
        config: ProxyConfig {
            id: Some(ProxyId(row.get(0)?)),
            label: row.get(1)?,
            kind: ProxyKind::parse(&kind).unwrap_or(ProxyKind::Http),
            host: row.get(3)?,
            port: u16::try_from(port).unwrap_or(0),
            username: row.get(5)?,
        },
        password_key: row.get(6)?,
    })
}

const SELECT_PROXY: &str = "SELECT id, label, kind, host, port, username, password_key FROM proxy";

impl Store {
    /// 列出全部代理（按主键升序）。
    pub fn list_proxies(&self) -> Result<Vec<StoredProxy>, StoreError> {
        let sql = format!("{SELECT_PROXY} ORDER BY id ASC");
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut proxies = Vec::new();
        while let Some(row) = rows.next()? {
            proxies.push(row_to_proxy(row)?);
        }
        Ok(proxies)
    }

    /// 按主键取一个代理。
    pub fn get_proxy(&self, id: ProxyId) -> Result<Option<StoredProxy>, StoreError> {
        let sql = format!("{SELECT_PROXY} WHERE id = ?1");
        self.conn()
            .query_row(&sql, [id.0], row_to_proxy)
            .optional()
            .map_err(StoreError::from)
    }

    /// 新建代理，返回新主键。
    pub fn insert_proxy(
        &self,
        config: &ProxyConfig,
        password_key: Option<&str>,
    ) -> Result<ProxyId, StoreError> {
        self.conn().execute(
            "INSERT INTO proxy (label, kind, host, port, username, password_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                config.label,
                config.kind.as_str(),
                config.host,
                i64::from(config.port),
                config.username,
                password_key,
            ],
        )?;
        Ok(ProxyId(self.conn().last_insert_rowid()))
    }

    /// 更新代理；不存在时返回 `false`。
    pub fn update_proxy(
        &self,
        id: ProxyId,
        config: &ProxyConfig,
        password_key: Option<&str>,
    ) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "UPDATE proxy
             SET label = ?2, kind = ?3, host = ?4, port = ?5, username = ?6,
                 password_key = ?7, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![
                id.0,
                config.label,
                config.kind.as_str(),
                config.host,
                i64::from(config.port),
                config.username,
                password_key,
            ],
        )?;
        Ok(changed > 0)
    }

    /// 删除代理；引用它的账号自动改回「跟随全局」。
    pub fn delete_proxy(&self, id: ProxyId) -> Result<bool, StoreError> {
        let tx = self.conn().unchecked_transaction()?;
        tx.execute(
            "UPDATE account SET proxy_mode = 'inherit', proxy_id = NULL WHERE proxy_id = ?1",
            [id.0],
        )?;
        let changed = tx.execute("DELETE FROM proxy WHERE id = ?1", [id.0])?;
        tx.commit()?;
        Ok(changed > 0)
    }

    /// 全局代理策略。
    pub fn global_proxy_mode(&self) -> Result<GlobalProxyMode, StoreError> {
        let mode: Option<String> = self
            .conn()
            .query_row(
                "SELECT value FROM setting WHERE key = 'proxy.global_mode'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let proxy_id: Option<i64> = self
            .conn()
            .query_row(
                "SELECT value FROM setting WHERE key = 'proxy.global_id'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|value| value.parse::<i64>().ok());

        Ok(
            GlobalProxyMode::parse(mode.as_deref().unwrap_or("system"), proxy_id)
                .unwrap_or(GlobalProxyMode::System),
        )
    }

    /// 保存全局代理策略。
    pub fn set_global_proxy_mode(&self, mode: GlobalProxyMode) -> Result<(), StoreError> {
        let tx = self.conn().unchecked_transaction()?;
        tx.execute(
            "INSERT INTO setting (key, value) VALUES ('proxy.global_mode', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [mode.as_str()],
        )?;
        match mode {
            GlobalProxyMode::Custom(id) => {
                tx.execute(
                    "INSERT INTO setting (key, value) VALUES ('proxy.global_id', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    [id.0.to_string()],
                )?;
            }
            _ => {
                tx.execute("DELETE FROM setting WHERE key = 'proxy.global_id'", [])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use mail_domain::{GlobalProxyMode, ProxyConfig, ProxyId, ProxyKind};

    use crate::Store;

    fn migrated() -> Store {
        let mut store = Store::open_in_memory().expect("打开内存库");
        store.run_migrations().expect("迁移");
        store
    }

    fn config(host: &str, port: u16) -> ProxyConfig {
        ProxyConfig {
            id: None,
            label: "测试代理".to_string(),
            kind: ProxyKind::Socks5,
            host: host.to_string(),
            port,
            username: "user".to_string(),
        }
    }

    #[test]
    fn 代理增删改查可用() {
        let store = migrated();
        let id = store
            .insert_proxy(&config("127.0.0.1", 1080), Some("proxy/1"))
            .expect("插入代理");

        let loaded = store.get_proxy(id).expect("查询").expect("应存在");
        assert_eq!(loaded.config.host, "127.0.0.1");
        assert_eq!(loaded.config.kind, ProxyKind::Socks5);
        assert_eq!(loaded.password_key.as_deref(), Some("proxy/1"));
        assert_eq!(store.list_proxies().expect("列表").len(), 1);

        let mut updated = config("127.0.0.2", 1081);
        updated.label = "改过的代理".to_string();
        assert!(store.update_proxy(id, &updated, None).expect("更新"));
        let loaded = store.get_proxy(id).expect("查询").expect("应存在");
        assert_eq!(loaded.config.label, "改过的代理");
        assert_eq!(loaded.password_key, None);

        assert!(store.delete_proxy(id).expect("删除"));
        assert!(store.get_proxy(id).expect("查询").is_none());
    }

    #[test]
    fn 删除代理会把账号改回跟随全局() {
        let store = migrated();
        let proxy_id = store
            .insert_proxy(&config("127.0.0.1", 1080), None)
            .expect("插入代理");
        let account_id = store
            .insert_account(
                &sample_draft(mail_domain::AccountProxyMode::Custom(proxy_id)),
                None,
            )
            .expect("插入账号");

        store.delete_proxy(proxy_id).expect("删除代理");
        let account = store.get_account(account_id).expect("查询账号").expect("应存在");
        assert_eq!(account.proxy, mail_domain::AccountProxyMode::InheritGlobal);
    }

    #[test]
    fn 全局代理设置可读写() {
        let store = migrated();
        assert_eq!(
            store.global_proxy_mode().expect("默认值"),
            GlobalProxyMode::System
        );

        let proxy_id = store
            .insert_proxy(&config("127.0.0.1", 1080), None)
            .expect("插入");
        store
            .set_global_proxy_mode(GlobalProxyMode::Custom(proxy_id))
            .expect("保存");
        assert_eq!(
            store.global_proxy_mode().expect("读取"),
            GlobalProxyMode::Custom(ProxyId(proxy_id.0))
        );

        store
            .set_global_proxy_mode(GlobalProxyMode::Direct)
            .expect("保存直连");
        assert_eq!(store.global_proxy_mode().expect("读取"), GlobalProxyMode::Direct);
    }

    fn sample_draft(proxy: mail_domain::AccountProxyMode) -> mail_domain::AccountDraft {
        mail_domain::AccountDraft {
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
            proxy,
            color: String::new(),
            enabled: true,
        }
    }
}
