//! 代理编排：全局策略、代理清单、凭据注入与连接测试。
//!
//! 安全约定：代理密码只在建连前的瞬间从保险箱取出，不落库、不进日志。

use mail_domain::account::AccountProxyMode;
use mail_domain::proxy::{decide_proxy, GlobalProxyMode, ProxyConfig, ProxyId, ProxyRoute, Secret};
use mail_domain::ValidationError;
use mail_store::{Store, StoredProxy};

use crate::engine::{EngineError, MailEngine};
use crate::secrets::SecretStore;

impl MailEngine {
    /// 列出全部代理。
    pub fn list_proxies(&self) -> Result<Vec<StoredProxy>, EngineError> {
        Ok(self.store().list_proxies()?)
    }

    /// 按编号取代理。
    pub fn get_proxy(&self, id: ProxyId) -> Result<StoredProxy, EngineError> {
        self.store()
            .get_proxy(id)?
            .ok_or(EngineError::ProxyNotFound(id.0))
    }

    /// 当前全局代理策略。
    pub fn global_proxy_mode(&self) -> Result<GlobalProxyMode, EngineError> {
        Ok(self.store().global_proxy_mode()?)
    }

    /// 设置全局代理策略；指向自定义代理时会先确认该代理存在。
    pub fn set_global_proxy_mode(&self, mode: GlobalProxyMode) -> Result<(), EngineError> {
        if let GlobalProxyMode::Custom(id) = mode {
            if self.store().get_proxy(id)?.is_none() {
                return Err(EngineError::ProxyNotFound(id.0));
            }
        }
        self.store().set_global_proxy_mode(mode)?;
        Ok(())
    }

    /// 新建或更新一个代理。
    ///
    /// `password` 的语义：`None` 保持原密码不变；`Some(非空)` 替换密码；
    /// `Some(空)` 清空密码（用于改成无认证代理）。
    pub fn save_proxy(
        &self,
        config: &ProxyConfig,
        password: Option<&Secret>,
    ) -> Result<StoredProxy, EngineError> {
        config.validate()?;
        self.ensure_proxy_credentials(config, password)?;

        match config.id {
            None => {
                let key = match password {
                    Some(secret) if !secret.is_empty() => {
                        let key = new_credential_key("proxy", &format!("{}:{}", config.host, config.port));
                        self.secrets().set(&key, secret)?;
                        Some(key)
                    }
                    _ => None,
                };
                let inserted = self.store().insert_proxy(config, key.as_deref());
                match inserted {
                    Ok(id) => self.get_proxy(id),
                    Err(err) => {
                        if let Some(key) = key {
                            let _ = self.secrets().delete(&key);
                        }
                        Err(err.into())
                    }
                }
            }
            Some(id) => {
                let existing = self.get_proxy(id)?;
                match password {
                    None => {
                        let updated = self
                            .store()
                            .update_proxy(id, config, existing.password_key.as_deref());
                        match updated {
                            Ok(true) => self.get_proxy(id),
                            Ok(false) => Err(EngineError::ProxyNotFound(id.0)),
                            Err(err) => Err(err.into()),
                        }
                    }
                    Some(secret) if secret.is_empty() => {
                        // 先让数据库不再引用旧凭据，再删保险箱，避免出现「引用存在但凭据没了」。
                        let cleared = self.store().update_proxy(id, config, None);
                        match cleared {
                            Ok(true) => {
                                if let Some(old) = existing.password_key.as_deref() {
                                    if let Err(err) = self.secrets().delete(old) {
                                        tracing::debug!(error = %err, "清空代理密码时保险箱删除失败，条目成为孤儿");
                                    }
                                }
                                self.get_proxy(id)
                            }
                            Ok(false) => Err(EngineError::ProxyNotFound(id.0)),
                            Err(err) => Err(err.into()),
                        }
                    }
                    Some(secret) => {
                        let old_key = existing.password_key.clone();
                        let new_key =
                            new_credential_key("proxy", &format!("{}:{}", config.host, config.port));
                        self.secrets().set(&new_key, secret)?;
                        let replaced = self.store().update_proxy(id, config, Some(&new_key));
                        match replaced {
                            Ok(true) => {
                                if let Some(old) = old_key.as_deref() {
                                    if let Err(err) = self.secrets().delete(old) {
                                        tracing::debug!(error = %err, "替换代理密码后旧条目删除失败");
                                    }
                                }
                                self.get_proxy(id)
                            }
                            Ok(false) => {
                                let _ = self.secrets().delete(&new_key);
                                Err(EngineError::ProxyNotFound(id.0))
                            }
                            Err(err) => {
                                let _ = self.secrets().delete(&new_key);
                                Err(err.into())
                            }
                        }
                    }
                }
            }
        }
    }

    /// 代理登录名与密码必须成对：登录名非空时一定得能拿到密码。
    ///
    /// `None` 表示沿用旧密码，所以编辑时只要原凭据还在就放行；但「改成无认证」
    /// （清空密码）必须把登录名也一起清空，避免留下「有登录名却没密码」的坏配置。
    fn ensure_proxy_credentials(
        &self,
        config: &ProxyConfig,
        password: Option<&Secret>,
    ) -> Result<(), EngineError> {
        if config.username.trim().is_empty() {
            return Ok(());
        }
        // 本次带来了非空新密码。
        if matches!(password, Some(secret) if !secret.is_empty()) {
            return Ok(());
        }
        // 没带密码（None）且原本就存过密码：沿用旧密码。
        if password.is_none() {
            if let Some(id) = config.id {
                if self.get_proxy(id)?.password_key.is_some() {
                    return Ok(());
                }
            }
        }
        Err(ValidationError::new(vec![
            "填了代理登录名就必须有密码：新建请填密码，编辑可留空沿用原密码，或先清空登录名再清空密码"
                .to_string(),
        ])
        .into())
    }

    /// 删除代理；账号对它的引用由存储层改回「跟随全局」。
    pub fn delete_proxy(&self, id: ProxyId) -> Result<(), EngineError> {
        let existing = self.get_proxy(id)?;

        // 先删保险箱、再删库；库删失败时尽力把凭据放回去。
        let backup = match existing.password_key.as_deref() {
            Some(key) => {
                let value = self.secrets().get(key)?;
                self.secrets().delete(key)?;
                Some((key.to_string(), value))
            }
            None => None,
        };

        let deleted = self.store().delete_proxy(id);
        match deleted {
            Ok(true) => Ok(()),
            Ok(false) => {
                restore_secret(self, backup);
                Err(EngineError::ProxyNotFound(id.0))
            }
            Err(err) => {
                restore_secret(self, backup);
                Err(err.into())
            }
        }
    }

    /// 测试一个代理是否能连到目标服务器（默认 `www.google.com:443`）。
    ///
    /// 只要代理按要求完成了到目标的隧道建立，就算测试通过。
    pub async fn test_proxy(&self, id: ProxyId, target: Option<(String, u16)>) -> Result<(), EngineError> {
        let stored = self.get_proxy(id)?;
        let route = route_from_stored(&stored, self.secrets())?;
        let (host, port) = target.unwrap_or_else(|| ("www.google.com".to_string(), 443));
        if host.trim().is_empty() || port == 0 {
            return Err(EngineError::BadRequest(
                "测试目标必须填写主机名和 1-65535 之间的端口".to_string(),
            ));
        }

        let tcp = mail_net::connect_tcp(&host, port, Some(&route), mail_net::DEFAULT_TIMEOUT).await?;
        drop(tcp);
        Ok(())
    }

    /// 按「账号级 > 全局自定义 > 跟随系统 > 直连」选出本次连接要走的代理。
    pub(crate) fn resolve_route(
        &self,
        account_mode: AccountProxyMode,
    ) -> Result<Option<ProxyRoute>, EngineError> {
        resolve_route_with(&self.store, self.secrets(), account_mode)
    }
}

/// 不依赖引擎句柄的代理决议：同步线程与自检都能用。
///
/// 只短暂持有存储锁，绝不跨网络等待；密码在建连前一刻才从保险箱取出。
pub(crate) fn resolve_route_with(
    store: &std::sync::Mutex<Store>,
    secrets: &dyn SecretStore,
    account_mode: AccountProxyMode,
) -> Result<Option<ProxyRoute>, EngineError> {
    let global = {
        let guard = store.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.global_proxy_mode()?
    };

    // 只有「账号跟随全局、且全局跟随系统」时才需要读系统代理。
    let needs_system =
        matches!(account_mode, AccountProxyMode::InheritGlobal) && matches!(global, GlobalProxyMode::System);
    let mut system_route = None;
    if needs_system {
        match mail_net::read_system_proxy()? {
            mail_net::SystemProxy::None => {}
            mail_net::SystemProxy::Static(route) => system_route = Some(route),
            mail_net::SystemProxy::AutoConfig { url } => {
                return Err(EngineError::SystemProxyAutoConfig(url));
            }
        }
    }

    let proxies = {
        let guard = store.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.list_proxies()?
    };
    let lookup = |id: ProxyId| {
        proxies
            .iter()
            .find(|stored| stored.config.id == Some(id))
            .map(|stored| ProxyRoute {
                config: stored.config.clone(),
                password: None,
            })
    };
    let decision = decide_proxy(account_mode, global, &lookup, system_route)?;

    match decision.route {
        Some(route) if route.config.id.is_some() => {
            match proxies.iter().find(|stored| stored.config.id == route.config.id) {
                Some(stored) => Ok(Some(route_from_stored(stored, secrets)?)),
                None => Err(EngineError::ProxyNotFound(route.config.id.map_or(0, |id| id.0))),
            }
        }
        other => Ok(other),
    }
}

/// 把数据库里的代理行组装成可直接建连的路线（需要时从保险箱取密码）。
pub(crate) fn route_from_stored(
    stored: &StoredProxy,
    secrets: &dyn SecretStore,
) -> Result<ProxyRoute, EngineError> {
    let password = match stored.password_key.as_deref() {
        Some(key) => secrets.get(key)?,
        None => None,
    };
    Ok(ProxyRoute {
        config: stored.config.clone(),
        password,
    })
}

/// 生成凭据引用键：对「前缀 + 随机种子」取哈希前 16 字节。
///
/// 键本身不含任何明文信息，也无法从键反推出密码。
pub(crate) fn new_credential_key(prefix: &str, seed: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    hasher.update(nanos.to_le_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    format!("{prefix}/{hex}")
}

fn restore_secret(engine: &MailEngine, backup: Option<(String, Option<Secret>)>) {
    if let Some((key, Some(value))) = backup {
        if let Err(err) = engine.secrets().set(&key, &value) {
            tracing::warn!(error = %err, "恢复凭据失败，请手工检查系统凭据管理器");
        }
    }
}

#[cfg(test)]
mod tests {
    use mail_domain::proxy::{GlobalProxyMode, ProxyConfig, ProxyId, ProxyKind, Secret};
    use mail_domain::AccountProxyMode;

    use crate::engine::MailEngine;
    use crate::secrets::MemorySecretStore;

    use super::new_credential_key;

    pub(crate) fn temp_engine() -> (tempfile::TempDir, MailEngine, std::sync::Arc<MemorySecretStore>) {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let secrets = std::sync::Arc::new(MemorySecretStore::new());
        let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("初始化引擎");
        (dir, engine, secrets)
    }

    fn config(label: &str) -> ProxyConfig {
        ProxyConfig {
            id: None,
            label: label.to_string(),
            kind: ProxyKind::Socks5,
            host: "127.0.0.1".to_string(),
            port: 1080,
            username: "user".to_string(),
        }
    }
    /// 无认证代理：登录名与密码都留空。
    fn config_without_auth(label: &str) -> ProxyConfig {
        let mut config = config(label);
        config.username = String::new();
        config
    }

    #[test]
    fn 保存代理时密码进保险箱而不进库() {
        let (dir, engine, secrets) = temp_engine();
        let stored = engine
            .save_proxy(&config("本机"), Some(&Secret::new("proxy-pw")))
            .expect("保存代理");

        let key = stored.password_key.expect("应记录凭据键");
        assert!(secrets.contains(&key), "密码应写进保险箱");
        assert_eq!(secrets.plain(&key).as_deref(), Some("proxy-pw"));

        // 数据库文件里搜不到明文密码。
        drop(engine);
        let bytes = std::fs::read(dir.path().join("ymail.db")).expect("读库文件");
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("proxy-pw"), "数据库文件不应出现明文密码");
    }

    #[test]
    fn 更新代理时不传密码会保留原凭据() {
        let (_dir, engine, secrets) = temp_engine();
        let stored = engine
            .save_proxy(&config("本机"), Some(&Secret::new("keep-me")))
            .expect("保存代理");
        let id = stored.config.id.expect("应有编号");
        let old_key = stored.password_key.clone().expect("应有凭据键");

        let mut edited = config("改名");
        edited.id = Some(id);
        let updated = engine.save_proxy(&edited, None).expect("更新代理");
        assert_eq!(updated.password_key, Some(old_key.clone()));
        assert_eq!(secrets.plain(&old_key).as_deref(), Some("keep-me"));
    }

    #[test]
    fn 清空代理密码会同时去掉库引用与保险箱条目() {
        let (_dir, engine, secrets) = temp_engine();
        let stored = engine
            .save_proxy(&config("本机"), Some(&Secret::new("clear-me")))
            .expect("保存代理");
        let id = stored.config.id.expect("应有编号");
        let old_key = stored.password_key.clone().expect("应有凭据键");

        let mut edited = config("无认证");
        edited.username = String::new();
        edited.id = Some(id);
        let updated = engine
            .save_proxy(&edited, Some(&Secret::new("")))
            .expect("清空密码");
        assert_eq!(updated.password_key, None);
        assert!(!secrets.contains(&old_key), "旧条目应被删除");
    }

    #[test]
    fn 全局自定义代理必须存在() {
        let (_dir, engine, _secrets) = temp_engine();
        let err = engine
            .set_global_proxy_mode(GlobalProxyMode::Custom(ProxyId(999)))
            .expect_err("不存在的代理应被拦住");
        assert!(err.to_string().contains("999"), "错误应带编号：{err}");

        let stored = engine
            .save_proxy(&config_without_auth("可用"), None)
            .expect("保存代理");
        engine
            .set_global_proxy_mode(GlobalProxyMode::Custom(stored.config.id.expect("应有编号")))
            .expect("存在的代理应能设为全局");
    }

    #[test]
    fn 解析路线时账号直连优先() {
        let (_dir, engine, _secrets) = temp_engine();
        let stored = engine
            .save_proxy(&config_without_auth("可用"), None)
            .expect("保存代理");
        engine
            .set_global_proxy_mode(GlobalProxyMode::Custom(stored.config.id.expect("应有编号")))
            .expect("设为全局");

        let route = engine
            .resolve_route(AccountProxyMode::Direct)
            .expect("决策应成功");
        assert!(route.is_none(), "账号直连时应不走代理");
    }

    #[test]
    fn 解析路线时全局自定义生效并带上密码() {
        let (_dir, engine, _secrets) = temp_engine();
        let stored = engine
            .save_proxy(&config("可用"), Some(&Secret::new("pw-123")))
            .expect("保存代理");
        let id = stored.config.id.expect("应有编号");
        engine
            .set_global_proxy_mode(GlobalProxyMode::Custom(id))
            .expect("设为全局");

        let route = engine
            .resolve_route(AccountProxyMode::InheritGlobal)
            .expect("决策应成功")
            .expect("应走代理");
        assert_eq!(route.config.id, Some(id));
        assert_eq!(
            route.password.as_ref().map(|secret| secret.expose()),
            Some("pw-123")
        );
    }

    #[test]
    fn 凭据键不含明文且形状稳定() {
        let key = new_credential_key("account", "secret-value");
        assert!(key.starts_with("account/"), "键应带前缀：{key}");
        assert!(!key.contains("secret-value"), "键不应携带明文：{key}");
        assert_eq!(key.len(), "account/".len() + 32);
    }

    #[test]
    fn 代理可以明文删除且不影响库一致性() {
        let (_dir, engine, secrets) = temp_engine();
        let stored = engine
            .save_proxy(&config("可用"), Some(&Secret::new("gone")))
            .expect("保存代理");
        let id = stored.config.id.expect("应有编号");
        let key = stored.password_key.expect("应有凭据键");

        engine.delete_proxy(id).expect("删除代理");
        assert!(engine.get_proxy(id).is_err(), "删完应查不到");
        assert!(!secrets.contains(&key), "凭据应一并删除");
        // SecretStore 的引用只是为了确认 trait 可用。
        let _ = secrets.len();
    }

    #[test]
    fn 新建代理填了登录名却没密码会被引擎拦住() {
        let (_dir, engine, secrets) = temp_engine();
        let err = engine
            .save_proxy(&config("缺密码"), None)
            .expect_err("没有密码应被拦住");
        assert!(err.to_string().contains("登录名"), "错误应说明原因：{err}");
        let err = engine
            .save_proxy(&config("空密码"), Some(&Secret::new("")))
            .expect_err("空密码应被拦住");
        assert!(err.to_string().contains("登录名"), "错误应说明原因：{err}");
        assert!(
            engine.list_proxies().expect("列代理").is_empty(),
            "被拦下的代理不应落库"
        );
        assert_eq!(secrets.len(), 0, "被拦下时不该写保险箱");
    }

    #[test]
    fn 编辑代理时只清密码不清登录名会被拦住() {
        let (_dir, engine, secrets) = temp_engine();
        let stored = engine
            .save_proxy(&config("本机"), Some(&Secret::new("keep")))
            .expect("保存代理");
        let id = stored.config.id.expect("应有编号");
        let key = stored.password_key.clone().expect("应有凭据键");

        let mut edited = config("想清密码");
        edited.id = Some(id);
        let err = engine
            .save_proxy(&edited, Some(&Secret::new("")))
            .expect_err("登录名还在就不能清密码");
        assert!(err.to_string().contains("登录名"), "错误应说明原因：{err}");

        // 被拦下之后，库里的旧凭据必须原样保留。
        let reloaded = engine.get_proxy(id).expect("代理还在");
        assert_eq!(reloaded.password_key, Some(key.clone()));
        assert_eq!(secrets.plain(&key).as_deref(), Some("keep"));
    }

    #[test]
    fn 编辑代理时原本没密码又不补密码会被拦住() {
        let (_dir, engine, _secrets) = temp_engine();
        let stored = engine
            .save_proxy(&config_without_auth("无认证"), None)
            .expect("保存无认证代理");
        let id = stored.config.id.expect("应有编号");

        let mut edited = config("补登录名");
        edited.id = Some(id);
        let err = engine
            .save_proxy(&edited, None)
            .expect_err("补登录名却不补密码应被拦住");
        assert!(err.to_string().contains("登录名"), "错误应说明原因：{err}");
    }
}
