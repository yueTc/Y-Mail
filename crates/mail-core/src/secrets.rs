//! 凭据存取：真实实现走系统凭据管理器，测试用内存实现。
//!
//! 约定：数据库只存引用键，密文本体只经保险箱读写；任何日志与错误都不得携带明文。

use std::collections::HashMap;
use std::sync::Mutex;

use mail_domain::proxy::Secret;
use thiserror::Error;

/// 保险箱操作失败。
#[derive(Debug, Error)]
pub enum SecretStoreError {
    /// 系统凭据管理器拒绝操作或不可用。
    #[error("系统凭据管理器操作失败：{0}")]
    Backend(String),
    /// 内存实现内部状态异常（锁被投毒等）。
    #[error("凭据存储器状态异常：{0}")]
    State(String),
}

/// 凭据存取接口。
///
/// 真实实现是系统凭据管理器；测试注入内存实现，保证 CI 不依赖真实保险箱。
pub trait SecretStore: Send + Sync {
    /// 读取一个凭据；不存在返回 `None`。
    fn get(&self, key: &str) -> Result<Option<Secret>, SecretStoreError>;

    /// 写入（或覆盖）一个凭据。
    fn set(&self, key: &str, secret: &Secret) -> Result<(), SecretStoreError>;

    /// 删除一个凭据；不存在也算成功。
    fn delete(&self, key: &str) -> Result<(), SecretStoreError>;
}

/// 基于系统凭据管理器的实现（Windows 走「凭据管理器」）。
pub struct KeyringSecretStore {
    service: String,
}

impl KeyringSecretStore {
    /// 用服务名构造；`service` 是同一应用下凭据的归属标识。
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry, SecretStoreError> {
        keyring::Entry::new(&self.service, key).map_err(|err| SecretStoreError::Backend(err.to_string()))
    }
}

impl SecretStore for KeyringSecretStore {
    fn get(&self, key: &str) -> Result<Option<Secret>, SecretStoreError> {
        match self.entry(key)?.get_password() {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(SecretStoreError::Backend(err.to_string())),
        }
    }

    fn set(&self, key: &str, secret: &Secret) -> Result<(), SecretStoreError> {
        self.entry(key)?
            .set_password(secret.expose())
            .map_err(|err| SecretStoreError::Backend(err.to_string()))
    }

    fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(SecretStoreError::Backend(err.to_string())),
        }
    }
}

/// 内存实现：只给测试与临时验证用。
#[derive(Default)]
pub struct MemorySecretStore {
    entries: Mutex<HashMap<String, Secret>>,
}

impl MemorySecretStore {
    /// 建一个空的内存保险箱。
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前保存的凭据条数。
    pub fn len(&self) -> usize {
        self.entries.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    /// 是否一条凭据都没有。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 是否保存了指定键（供测试断言）。
    pub fn contains(&self, key: &str) -> bool {
        self.entries
            .lock()
            .map(|guard| guard.contains_key(key))
            .unwrap_or(false)
    }

    /// 明文取值（仅测试断言用，绕开 `Secret` 的调试保护）。
    pub fn plain(&self, key: &str) -> Option<String> {
        self.entries
            .lock()
            .ok()
            .and_then(|guard| guard.get(key).map(|secret| secret.expose().to_string()))
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, key: &str) -> Result<Option<Secret>, SecretStoreError> {
        let guard = self
            .entries
            .lock()
            .map_err(|err| SecretStoreError::State(err.to_string()))?;
        Ok(guard.get(key).cloned())
    }

    fn set(&self, key: &str, secret: &Secret) -> Result<(), SecretStoreError> {
        let mut guard = self
            .entries
            .lock()
            .map_err(|err| SecretStoreError::State(err.to_string()))?;
        guard.insert(key.to_string(), secret.clone());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
        let mut guard = self
            .entries
            .lock()
            .map_err(|err| SecretStoreError::State(err.to_string()))?;
        guard.remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use mail_domain::proxy::Secret;

    use super::{MemorySecretStore, SecretStore};

    #[test]
    fn 内存保险箱可读写删() {
        let store = MemorySecretStore::new();
        assert!(store.is_empty());
        assert!(store.get("k").expect("读取").is_none());

        store.set("k", &Secret::new("pw")).expect("写入");
        assert!(store.contains("k"));
        assert_eq!(store.get("k").expect("读取").expect("应存在").expose(), "pw");

        store.delete("k").expect("删除");
        assert!(store.get("k").expect("读取").is_none());
        store.delete("k").expect("重复删除也应成功");
    }
}
