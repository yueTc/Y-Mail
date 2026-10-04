//! 账号编排：校验、查重、连接自检、凭据与数据库的一致性。
//!
//! 关键不变量（W1-D5）：保存账号必须先连接自检通过；先写保险箱再写库，
//! 任一步失败都要回滚另一侧，避免出现「半个账号」。

use mail_domain::account::{Account, AccountDraft, AccountId};
use mail_domain::proxy::Secret;

use crate::checks::ConnectionReport;
use crate::engine::{EngineError, MailEngine};
use crate::proxies::new_credential_key;

impl MailEngine {
    /// 列出全部账号（不碰网络）。
    pub fn list_accounts(&self) -> Result<Vec<Account>, EngineError> {
        Ok(self.store().list_accounts()?)
    }

    /// 按编号取账号。
    pub fn get_account(&self, id: AccountId) -> Result<Account, EngineError> {
        self.store()
            .get_account(id)?
            .ok_or(EngineError::AccountNotFound(id.0))
    }

    /// 用一份尚未保存的草稿做连接自检；不写库、不写保险箱。
    pub async fn test_account_connection(
        &self,
        draft: &AccountDraft,
        secret: &Secret,
    ) -> Result<ConnectionReport, EngineError> {
        let normalized = draft.normalized();
        let draft = &normalized;
        draft.validate()?;
        if secret.is_empty() {
            return Err(EngineError::BadRequest("请先填写授权码".to_string()));
        }
        let route = self.resolve_route(draft.proxy)?;
        let plan = crate::checks::ProbePlan::new(draft, secret, route);
        Ok(crate::checks::run(&plan).await?)
    }

    /// 对已经保存的账号再做一次自检，密码从保险箱取。
    pub async fn test_saved_account(&self, id: AccountId) -> Result<ConnectionReport, EngineError> {
        let account = self.get_account(id)?;
        let secret = self.saved_secret(&account)?;
        let route = self.resolve_route(account.proxy)?;
        let plan = crate::checks::ProbePlan::new(&account.draft(), &secret, route);
        Ok(crate::checks::run(&plan).await?)
    }

    /// 新建账号：自检通过后，先写保险箱、再写库；任一步失败都回滚。
    pub async fn create_account(
        &self,
        draft: &AccountDraft,
        secret: &Secret,
    ) -> Result<Account, EngineError> {
        let normalized = draft.normalized();
        let draft = &normalized;
        draft.validate()?;
        if secret.is_empty() {
            return Err(EngineError::BadRequest("请先填写授权码".to_string()));
        }
        if self.store().email_taken(&draft.email, None)? {
            return Err(EngineError::EmailTaken(draft.email.clone()));
        }

        // 1) 自检：失败就直接结束，不留任何痕迹。
        let route = self.resolve_route(draft.proxy)?;
        let plan = crate::checks::ProbePlan::new(draft, secret, route);
        crate::checks::run(&plan).await?;

        // 2) 先写保险箱，再写库；插入失败就把保险箱条目删掉。
        let key = new_credential_key("account", &draft.email);
        self.secrets().set(&key, secret)?;
        let inserted = self.store().insert_account(draft, Some(&key));
        match inserted {
            Ok(id) => self.get_account(id),
            Err(err) => {
                let _ = self.secrets().delete(&key);
                Err(err.into())
            }
        }
    }

    /// 更新账号：先自检通过，再改凭据与库。
    ///
    /// `secret` 为 `None`（或空）表示沿用保险箱里已有的授权码。
    pub async fn update_account(
        &self,
        id: AccountId,
        draft: &AccountDraft,
        secret: Option<&Secret>,
    ) -> Result<Account, EngineError> {
        let normalized = draft.normalized();
        let draft = &normalized;
        draft.validate()?;
        let existing = self
            .store()
            .get_account(id)?
            .ok_or(EngineError::AccountNotFound(id.0))?;
        if self.store().email_taken(&draft.email, Some(id))? {
            return Err(EngineError::EmailTaken(draft.email.clone()));
        }

        let new_secret = secret.filter(|value| !value.is_empty());
        let probe_secret = match new_secret {
            Some(value) => value.clone(),
            None => self.saved_secret(&existing)?,
        };

        // 自检：不通过就直接结束，凭据与库都不动。
        let route = self.resolve_route(draft.proxy)?;
        let plan = crate::checks::ProbePlan::new(draft, &probe_secret, route);
        crate::checks::run(&plan).await?;

        match new_secret {
            Some(value) => {
                // 换密码：写新条目 → 改库 → 删旧条目。
                let old_key = existing.credential_key.clone();
                let new_key = new_credential_key("account", &draft.email);
                self.secrets().set(&new_key, value)?;
                let updated = self.store().update_account(id, draft, Some(&new_key));
                match updated {
                    Ok(true) => {
                        if let Some(old) = old_key.as_deref() {
                            if let Err(err) = self.secrets().delete(old) {
                                tracing::debug!(error = %err, "更新账号后旧凭据删除失败");
                            }
                        }
                        self.get_account(id)
                    }
                    Ok(false) => {
                        let _ = self.secrets().delete(&new_key);
                        Err(EngineError::AccountNotFound(id.0))
                    }
                    Err(err) => {
                        let _ = self.secrets().delete(&new_key);
                        Err(err.into())
                    }
                }
            }
            None => {
                // 先把结果取出来、放掉数据库锁，再按结果读账号；
                // 锁没放就在分支里再读一次，会把自己锁死（曾经的挂起根因）。
                let updated = self
                    .store()
                    .update_account(id, draft, existing.credential_key.as_deref());
                match updated {
                    Ok(true) => self.get_account(id),
                    Ok(false) => Err(EngineError::AccountNotFound(id.0)),
                    Err(err) => Err(err.into()),
                }
            }
        }
    }

    /// 删除账号：先删保险箱、再删库；库删失败时尽力把凭据放回去。
    pub fn delete_account(&self, id: AccountId) -> Result<(), EngineError> {
        let existing = self.get_account(id)?;

        let backup = match existing.credential_key.as_deref() {
            Some(key) => {
                let value = self.secrets().get(key)?;
                self.secrets().delete(key)?;
                Some((key.to_string(), value))
            }
            None => None,
        };

        let deleted = self.store().delete_account(id);
        match deleted {
            Ok(true) => Ok(()),
            Ok(false) => {
                restore_secret(self, backup);
                Err(EngineError::AccountNotFound(id.0))
            }
            Err(err) => {
                restore_secret(self, backup);
                Err(err.into())
            }
        }
    }

    /// 从保险箱读取账号的授权码。
    fn saved_secret(&self, account: &Account) -> Result<Secret, EngineError> {
        let key = account
            .credential_key
            .as_deref()
            .ok_or_else(|| EngineError::BadRequest("该账号还没有保存授权码，请重新填写".to_string()))?;
        self.secrets().get(key)?.ok_or_else(|| {
            EngineError::BadRequest("系统凭据管理器里找不到该账号的授权码，请重新填写".to_string())
        })
    }
}

fn restore_secret(engine: &MailEngine, backup: Option<(String, Option<Secret>)>) {
    if let Some((key, Some(value))) = backup {
        if let Err(err) = engine.secrets().set(&key, &value) {
            tracing::warn!(error = %err, "恢复账号凭据失败，请手工检查系统凭据管理器");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mail_domain::account::{AccountDraft, AccountProxyMode, AuthType, Security, ServerConfig};
    use mail_domain::error::ConnectionErrorKind;
    use mail_domain::proxy::Secret;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    use crate::engine::MailEngine;
    use crate::secrets::MemorySecretStore;

    fn draft(imap_port: u16, smtp_port: u16, email: &str) -> AccountDraft {
        AccountDraft {
            display_name: "测试邮箱".to_string(),
            email: email.to_string(),
            auth_type: AuthType::Password,
            username: email.to_string(),
            imap: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: imap_port,
                security: Security::Plain,
            },
            smtp: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: smtp_port,
                security: Security::Plain,
            },
            proxy: AccountProxyMode::Direct,
            color: String::new(),
            enabled: true,
            oauth_provider: None,
            oauth_client_id: String::new(),
        }
    }

    async fn read_line(reader: &mut BufReader<TcpStream>) -> String {
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("读取失败");
        line.trim_end().to_string()
    }

    /// 读到一行就返回；对端断开时返回 `None`。
    async fn read_optional_line(reader: &mut BufReader<TcpStream>) -> Option<String> {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim_end().to_string()),
        }
    }

    /// 往假服务器回一段应答。
    async fn reply(reader: &mut BufReader<TcpStream>, payload: &str) {
        reader
            .get_mut()
            .write_all(payload.as_bytes())
            .await
            .expect("写应答失败");
    }

    /// 起一个能多次应答的假 IMAP 服务器；按客户端实际发出的命令逐条应答。
    async fn spawn_imap(accept_login: bool) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let port = listener.local_addr().expect("取地址失败").port();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut reader = BufReader::new(socket);
                    reader
                        .get_mut()
                        .write_all(b"* OK IMAP4rev1 ready\r\n")
                        .await
                        .expect("写欢迎语失败");
                    while let Some(line) = read_optional_line(&mut reader).await {
                        let upper = line.to_ascii_uppercase();
                        let tag = line.split_whitespace().next().unwrap_or_default().to_string();
                        if upper.ends_with("LOGIN") || upper.contains(" LOGIN ") {
                            if accept_login {
                                reply(&mut reader, &format!("{tag} OK LOGIN completed\r\n")).await;
                            } else {
                                reply(&mut reader, &format!("{tag} NO login failed\r\n")).await;
                                break;
                            }
                        } else if upper.contains("CAPABILITY") {
                            reply(
                                &mut reader,
                                &format!("* CAPABILITY IMAP4rev1 ID\r\n{tag} OK CAPABILITY completed\r\n"),
                            )
                            .await;
                        } else if upper.ends_with("ID") || upper.contains(" ID ") {
                            reply(
                                &mut reader,
                                &format!("* ID (\"name\" \"EmMaster\")\r\n{tag} OK ID completed\r\n"),
                            )
                            .await;
                        } else if upper.contains("LIST") {
                            reply(
                                &mut reader,
                                &format!(
                                    "* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n{tag} OK LIST completed\r\n"
                                ),
                            )
                            .await;
                        } else if upper.contains("SELECT") {
                            reply(
                                &mut reader,
                                &format!("* 1 EXISTS\r\n{tag} OK SELECT completed\r\n"),
                            )
                            .await;
                        } else if upper.contains("LOGOUT") {
                            reply(&mut reader, &format!("* BYE\r\n{tag} OK\r\n")).await;
                            break;
                        } else {
                            reply(&mut reader, &format!("{tag} BAD unexpected\r\n")).await;
                        }
                    }
                });
            }
        });
        port
    }

    /// 起一个能多次应答的假 SMTP 服务器。
    async fn spawn_smtp(accept_auth: bool) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let port = listener.local_addr().expect("取地址失败").port();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut reader = BufReader::new(socket);
                    reader
                        .get_mut()
                        .write_all(b"220 smtp ready\r\n")
                        .await
                        .expect("写欢迎语失败");
                    let _ehlo = read_line(&mut reader).await;
                    reader
                        .get_mut()
                        .write_all(b"250-localhost\r\n250-AUTH PLAIN LOGIN\r\n250 OK\r\n")
                        .await
                        .expect("写 EHLO 结果失败");
                    let _auth = read_line(&mut reader).await;
                    if accept_auth {
                        reader
                            .get_mut()
                            .write_all(b"235 2.7.0 authenticated\r\n")
                            .await
                            .expect("写认证结果失败");
                        let _quit = read_line(&mut reader).await;
                        reader
                            .get_mut()
                            .write_all(b"221 bye\r\n")
                            .await
                            .expect("写退出结果失败");
                    } else {
                        reader
                            .get_mut()
                            .write_all(b"535 5.7.8 bad credentials\r\n")
                            .await
                            .expect("写拒绝结果失败");
                    }
                });
            }
        });
        port
    }

    async fn ok_servers() -> (u16, u16) {
        (spawn_imap(true).await, spawn_smtp(true).await)
    }

    fn engine(dir: &std::path::Path) -> (MailEngine, Arc<MemorySecretStore>) {
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = MailEngine::initialize_with_secrets(dir, secrets.clone()).expect("初始化引擎");
        (engine, secrets)
    }

    #[tokio::test]
    async fn 保存账号前会先自检并落库() {
        let (imap_port, smtp_port) = ok_servers().await;
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());

        let account = engine
            .create_account(
                &draft(imap_port, smtp_port, "someone@example.com"),
                &Secret::new("pw-secret"),
            )
            .await
            .expect("应保存成功");

        assert_eq!(account.email, "someone@example.com");
        assert!(account.credential_key.is_some(), "应记录凭据引用键");
        assert_eq!(engine.list_accounts().expect("列表").len(), 1);
        assert_eq!(secrets.len(), 1, "密码应写进保险箱");
    }

    #[tokio::test]
    async fn 自检通过的报告包含两边结果() {
        let (imap_port, smtp_port) = ok_servers().await;
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _secrets) = engine(dir.path());

        let report = engine
            .test_account_connection(
                &draft(imap_port, smtp_port, "someone@example.com"),
                &Secret::new("pw-secret"),
            )
            .await
            .expect("自检应通过");
        assert_eq!(report.imap_folder_count, 1);
        assert_eq!(report.smtp_mechanism, "PLAIN");
    }

    #[tokio::test]
    async fn 收件自检失败不会留下半成品() {
        let imap_port = spawn_imap(false).await;
        let smtp_port = spawn_smtp(true).await;
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());

        let err = engine
            .create_account(
                &draft(imap_port, smtp_port, "bad@example.com"),
                &Secret::new("pw-secret"),
            )
            .await
            .expect_err("认证失败应拦住保存");
        match err {
            crate::engine::EngineError::Connection(connection) => {
                assert_eq!(connection.kind, ConnectionErrorKind::AuthFailed);
                assert!(connection.message.starts_with("收件服务器："));
            }
            other => panic!("应返回连接错误，实际是 {other}"),
        }
        assert!(engine.list_accounts().expect("列表").is_empty());
        assert!(secrets.is_empty(), "失败时不应写入任何凭据");
    }

    #[tokio::test]
    async fn 重复邮箱会被拦住() {
        let (imap_port, smtp_port) = ok_servers().await;
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _secrets) = engine(dir.path());

        engine
            .create_account(
                &draft(imap_port, smtp_port, "dup@example.com"),
                &Secret::new("pw"),
            )
            .await
            .expect("首次应成功");

        let err = engine
            .create_account(
                &draft(imap_port, smtp_port, "dup@example.com"),
                &Secret::new("pw"),
            )
            .await
            .expect_err("重复邮箱应被拦住");
        assert!(matches!(err, crate::engine::EngineError::EmailTaken(_)));
        assert_eq!(engine.list_accounts().expect("列表").len(), 1);
    }

    #[tokio::test]
    async fn 已保存账号可以再次自检并删除凭据() {
        let (imap_port, smtp_port) = ok_servers().await;
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());

        let account = engine
            .create_account(
                &draft(imap_port, smtp_port, "reuse@example.com"),
                &Secret::new("pw"),
            )
            .await
            .expect("保存成功");
        let key = account.credential_key.clone().expect("应有凭据键");

        let report = engine
            .test_saved_account(account.id)
            .await
            .expect("再次自检应通过");
        assert_eq!(report.imap_folder_count, 1);

        engine.delete_account(account.id).expect("删除账号");
        assert!(engine.list_accounts().expect("列表").is_empty());
        assert!(!secrets.contains(&key), "删除账号应一并删掉凭据");
    }

    #[tokio::test]
    async fn 修改账号会先自检再改库() {
        let (imap_port, smtp_port) = ok_servers().await;
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());

        let account = engine
            .create_account(
                &draft(imap_port, smtp_port, "edit@example.com"),
                &Secret::new("old-pw"),
            )
            .await
            .expect("保存成功");
        let old_key = account.credential_key.clone().expect("应有凭据键");

        let mut edited = draft(imap_port, smtp_port, "edit@example.com");
        edited.display_name = "改过的名字".to_string();
        let updated = engine
            .update_account(account.id, &edited, Some(&Secret::new("new-pw")))
            .await
            .expect("更新应成功");

        assert_eq!(updated.display_name, "改过的名字");
        let new_key = updated.credential_key.clone().expect("应有新凭据键");
        assert_ne!(new_key, old_key, "换密码应换引用键");
        assert!(!secrets.contains(&old_key), "旧条目应被清掉");
        assert_eq!(secrets.plain(&new_key).as_deref(), Some("new-pw"));
    }

    #[tokio::test]
    async fn 显示名留空保存为邮箱地址() {
        let (imap_port, smtp_port) = ok_servers().await;
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _secrets) = engine(dir.path());

        let mut blank = draft(imap_port, smtp_port, "blank@example.com");
        blank.display_name = "   ".to_string();
        let account = engine
            .create_account(&blank, &Secret::new("pw"))
            .await
            .expect("显示名留空也应保存成功");
        assert_eq!(account.display_name, "blank@example.com");

        // 编辑时把显示名清空，保存后同样回落到邮箱地址。
        let mut cleared = draft(imap_port, smtp_port, "blank@example.com");
        cleared.display_name = String::new();
        let updated = engine
            .update_account(account.id, &cleared, None)
            .await
            .expect("清空显示名后仍应更新成功");
        assert_eq!(updated.display_name, "blank@example.com");
    }
    #[tokio::test]
    async fn 修改时自检失败会保持原样() {
        let (imap_port, smtp_port) = ok_servers().await;
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());

        let account = engine
            .create_account(
                &draft(imap_port, smtp_port, "keep@example.com"),
                &Secret::new("keep-pw"),
            )
            .await
            .expect("保存成功");
        let key = account.credential_key.clone().expect("应有凭据键");

        // 指向一个没人监听的端口，让自检必然失败。
        let mut broken = draft(imap_port, smtp_port, "keep@example.com");
        broken.imap.port = 1;
        let err = engine
            .update_account(account.id, &broken, Some(&Secret::new("brand-new")))
            .await
            .expect_err("自检失败应拦住更新");
        assert!(matches!(err, crate::engine::EngineError::Connection(_)));

        let reloaded = engine.get_account(account.id).expect("账号应还在");
        assert_eq!(reloaded.credential_key.as_deref(), Some(key.as_str()));
        assert_eq!(secrets.plain(&key).as_deref(), Some("keep-pw"));
        assert_eq!(secrets.len(), 1, "不应残留新凭据");
    }

    #[tokio::test]
    async fn 数据库文件里查不到明文授权码() {
        let (imap_port, smtp_port) = ok_servers().await;
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _secrets) = engine(dir.path());
        let exposed = "very-secret-token-9527";

        engine
            .create_account(
                &draft(imap_port, smtp_port, "leak@example.com"),
                &Secret::new(exposed),
            )
            .await
            .expect("保存成功");
        drop(engine);

        let mut found = false;
        for entry in std::fs::read_dir(dir.path()).expect("列目录") {
            let path = entry.expect("目录项").path();
            if !path.is_file() {
                continue;
            }
            let bytes = std::fs::read(&path).expect("读文件");
            if bytes
                .windows(exposed.len())
                .any(|window| window == exposed.as_bytes())
            {
                found = true;
            }
        }
        assert!(!found, "任何数据文件里都不应出现明文授权码");
    }
}
