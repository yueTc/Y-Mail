//! 凭据存取：真实实现走系统凭据管理器，测试用内存实现。
//!
//! 约定：数据库只存引用键，密文本体只经保险箱读写；任何日志与错误都不得携带明文。

use std::collections::HashMap;
use std::sync::Mutex;

use mail_domain::proxy::Secret;
use thiserror::Error;

/// 保险箱操作失败。
///
/// 对外只给通用提示，底层细节只写本地日志（且先脱敏），避免服务器或系统
/// 报错里的敏感串意外出现在界面与上层错误里。
#[derive(Debug, Error)]
pub enum SecretStoreError {
    /// 系统凭据管理器拒绝操作或不可用。
    #[error("系统凭据库操作失败")]
    Backend,
    /// 内存实现内部状态异常（锁被投毒等）。
    #[error("凭据存储器状态异常")]
    State,
}

/// 清洗底层报错里的敏感串；单独抽出来方便回归测试。
fn redacted_backend_detail(detail: &str, secrets: &[&str]) -> String {
    mail_net::error::redact(detail, secrets)
}

/// 把底层报错清洗后写入本地日志；调用方只拿到通用错误。
fn log_backend_error(action: &str, key: &str, detail: &str, secrets: &[&str]) {
    let cleaned = redacted_backend_detail(detail, secrets);
    tracing::warn!(action, key, detail = %cleaned, "系统凭据库操作失败，详情仅供本地排查");
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
        keyring::Entry::new(&self.service, key).map_err(|err| {
            log_backend_error("打开凭据条目", key, &err.to_string(), &[]);
            SecretStoreError::Backend
        })
    }
}

impl SecretStore for KeyringSecretStore {
    fn get(&self, key: &str) -> Result<Option<Secret>, SecretStoreError> {
        match self.entry(key)?.get_password() {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => {
                log_backend_error("读取凭据", key, &err.to_string(), &[]);
                Err(SecretStoreError::Backend)
            }
        }
    }

    fn set(&self, key: &str, secret: &Secret) -> Result<(), SecretStoreError> {
        self.entry(key)?.set_password(secret.expose()).map_err(|err| {
            log_backend_error("写入凭据", key, &err.to_string(), &[secret.expose()]);
            SecretStoreError::Backend
        })
    }

    fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => {
                log_backend_error("删除凭据", key, &err.to_string(), &[]);
                Err(SecretStoreError::Backend)
            }
        }
    }
}

/// 单条系统凭据的写入上限（保守取值）。
///
/// Windows「凭据管理器」限定一条凭据的密文最多 2560 字节，而密文按 UTF-16 存放，
/// 换算下来约 1280 个 UTF-16 码元。微软 OAuth 的访问令牌本身就是一千多字符的长串，
/// 再带上刷新令牌，整包必然超限，直接写就会被系统拒绝。
/// 这里每片只放 1000 个 UTF-16 码元，留出余量，保证任何一片都不触顶。
const CREDENTIAL_CHUNK_UNITS: usize = 1000;

/// 分片包在主条目里的标记前缀，后面紧跟分片数量。
///
/// 主条目只放这一行标记，真实内容全在 `<键>#1`、`<键>#2`……这些分片条目里。
const CHUNK_MARKER_PREFIX: &str = "\u{1}ymail-chunked:";

/// 把一条超长凭据按 UTF-16 码元切成多片。
///
/// 按「码元」而不是「字符」切，是因为系统上限按 UTF-16 字节算：
/// emoji 这类字符一个就占 2 个码元，按字符数切会悄悄超限。
fn split_secret(value: &str) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut units = 0usize;
    for ch in value.chars() {
        let width = ch.len_utf16();
        if units + width > CREDENTIAL_CHUNK_UNITS {
            chunks.push(std::mem::take(&mut current));
            units = 0;
        }
        current.push(ch);
        units += width;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// 从主条目内容里读出分片数量；不是分片包就返回 `None`。
fn chunk_count(marker: &str) -> Option<usize> {
    marker.strip_prefix(CHUNK_MARKER_PREFIX)?.trim().parse().ok()
}

/// 第 `index` 片（从 1 开始数）在保险箱里的键。
fn chunk_key(key: &str, index: usize) -> String {
    format!("{key}#{index}")
}

/// 分片保险箱：包一层底层实现，把超长凭据拆成多条存放。
///
/// 为什么要有它：系统凭据库对单条密文有硬上限，超了就直接拒写，
/// 而 OAuth 令牌又天生很长。拆片后每条都不触顶，凭据依然只存在系统凭据库里，
/// 不落数据库、不落日志，规格里「凭据 MUST 存于 OS keyring」的要求不变。
pub struct ChunkedSecretStore<S: SecretStore> {
    inner: S,
}

impl<S: SecretStore> ChunkedSecretStore<S> {
    /// 包住底层保险箱。
    pub fn new(inner: S) -> Self {
        Self { inner }
    }

    /// 底层实现；测试要检查真实落库的分片就靠它。
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// 清掉上一次分片留下的残片。主条目不存在、或不带分片标记时什么都不做。
    fn drop_old_chunks(&self, key: &str) -> Result<(), SecretStoreError> {
        let Some(previous) = self.inner.get(key)? else {
            return Ok(());
        };
        let Some(count) = chunk_count(previous.expose()) else {
            return Ok(());
        };
        for index in 1..=count {
            self.inner.delete(&chunk_key(key, index))?;
        }
        Ok(())
    }
}

impl<S: SecretStore> SecretStore for ChunkedSecretStore<S> {
    fn get(&self, key: &str) -> Result<Option<Secret>, SecretStoreError> {
        let Some(stored) = self.inner.get(key)? else {
            return Ok(None);
        };
        let Some(count) = chunk_count(stored.expose()) else {
            return Ok(Some(stored));
        };
        let mut value = String::new();
        for index in 1..=count {
            match self.inner.get(&chunk_key(key, index))? {
                Some(part) => value.push_str(part.expose()),
                None => {
                    tracing::warn!(key, index, "凭据分片缺失，没法完整还原");
                    return Err(SecretStoreError::Backend);
                }
            }
        }
        Ok(Some(Secret::new(value)))
    }

    fn set(&self, key: &str, secret: &Secret) -> Result<(), SecretStoreError> {
        self.drop_old_chunks(key)?;
        let chunks = split_secret(secret.expose());
        if chunks.len() <= 1 {
            return self.inner.set(key, secret);
        }
        for (offset, part) in chunks.iter().enumerate() {
            self.inner
                .set(&chunk_key(key, offset + 1), &Secret::new(part.clone()))?;
        }
        // 标记最后写：标记落库之前，主条目还是旧内容，读不出半新半旧的令牌包。
        self.inner.set(
            key,
            &Secret::new(format!("{CHUNK_MARKER_PREFIX}{}", chunks.len())),
        )
    }

    fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
        self.drop_old_chunks(key)?;
        self.inner.delete(key)
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

    /// 当前保存的全部键（仅测试断言用）。
    #[cfg(test)]
    pub fn keys(&self) -> Vec<String> {
        self.entries
            .lock()
            .map(|guard| guard.keys().cloned().collect())
            .unwrap_or_default()
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
        let guard = self.entries.lock().map_err(|err| {
            tracing::warn!(detail = %err, "凭据存储器内部状态异常");
            SecretStoreError::State
        })?;
        Ok(guard.get(key).cloned())
    }

    fn set(&self, key: &str, secret: &Secret) -> Result<(), SecretStoreError> {
        let mut guard = self.entries.lock().map_err(|err| {
            tracing::warn!(detail = %err, "凭据存储器内部状态异常");
            SecretStoreError::State
        })?;
        guard.insert(key.to_string(), secret.clone());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
        let mut guard = self.entries.lock().map_err(|err| {
            tracing::warn!(detail = %err, "凭据存储器内部状态异常");
            SecretStoreError::State
        })?;
        guard.remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use mail_domain::proxy::Secret;

    use super::{ChunkedSecretStore, MemorySecretStore, SecretStore, SecretStoreError};

    /// 内存保险箱里的每一条都不许超过系统单条凭据上限（2560 字节，UTF-16 编码）。
    fn assert_within_platform_limit(store: &MemorySecretStore) {
        for key in store.keys() {
            let value = store.plain(&key).expect("键上必须有值");
            let bytes = value.encode_utf16().count() * 2;
            assert!(bytes <= 2560, "键 {key} 存了 {bytes} 字节，超了系统上限");
        }
    }

    fn chunked() -> ChunkedSecretStore<MemorySecretStore> {
        ChunkedSecretStore::new(MemorySecretStore::new())
    }

    #[test]
    fn 超长凭据拆片存放且能原样取回() {
        let store = chunked();
        let long = "tok".repeat(3000);
        store.set("account/x", &Secret::new(long.clone())).expect("写入");
        let read = store
            .get("account/x")
            .expect("读取")
            .expect("应能取回")
            .expose()
            .to_string();
        assert_eq!(read, long);
        assert_within_platform_limit(store.inner());
        assert!(store.inner().contains("account/x#1"), "该有分片条目");
        let marker = store.inner().plain("account/x").expect("主条目");
        assert!(!marker.contains("tok"), "主条目只该放标记，不该放正文");
    }

    #[test]
    fn 短凭据不分片保持原样() {
        let store = chunked();
        store.set("k", &Secret::new("授权码")).expect("写入");
        assert_eq!(store.inner().plain("k").expect("主条目"), "授权码");
        assert_eq!(store.inner().keys(), vec!["k".to_string()], "短凭据不该多出分片");
    }

    #[test]
    fn 换短凭据会把旧分片清干净() {
        let store = chunked();
        store.set("k", &Secret::new("a".repeat(5000))).expect("写长凭据");
        assert!(store.inner().keys().len() > 1, "该拆成多片");
        store.set("k", &Secret::new("短")).expect("改短");
        assert_eq!(store.inner().keys(), vec!["k".to_string()], "旧分片该清掉");
        assert_eq!(store.get("k").expect("读取").expect("应存在").expose(), "短");
    }

    #[test]
    fn 删分片凭据连分片一起删() {
        let store = chunked();
        store.set("k", &Secret::new("b".repeat(4000))).expect("写入");
        assert!(store.inner().keys().len() > 1);
        store.delete("k").expect("删除");
        assert!(store.inner().keys().is_empty(), "分片不该有残留");
        assert!(store.get("k").expect("读取").is_none());
    }

    #[test]
    fn 分片缺一片时按保险箱故障报错() {
        let store = chunked();
        store.set("k", &Secret::new("c".repeat(4000))).expect("写入");
        store.inner().delete("k#2").expect("删掉第二片");
        assert!(matches!(store.get("k"), Err(SecretStoreError::Backend)));
    }

    #[test]
    fn 非基本平面字符分片后也不超上限() {
        let store = chunked();
        let long = "🦀".repeat(2000);
        store.set("k", &Secret::new(long.clone())).expect("写入");
        assert_eq!(store.get("k").expect("读取").expect("应存在").expose(), long);
        assert_within_platform_limit(store.inner());
    }

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

    #[test]
    fn 保险箱错误只给通用提示不携带底层细节() {
        use super::{redacted_backend_detail, SecretStoreError};

        let backend = SecretStoreError::Backend;
        assert_eq!(backend.to_string(), "系统凭据库操作失败");
        assert!(!backend.to_string().contains("keyring"));

        let state = SecretStoreError::State;
        assert_eq!(state.to_string(), "凭据存储器状态异常");

        let engine = crate::engine::EngineError::Secrets(SecretStoreError::Backend);
        assert_eq!(engine.to_string(), "凭据保存失败：系统凭据库操作失败");

        let detail = redacted_backend_detail(
            "调用 keyring 失败：password=pw 令牌 token-abc",
            &["pw", "token-abc"],
        );
        assert!(!detail.contains("pw "));
        assert!(!detail.contains("token-abc"));
        assert!(detail.contains("***"));
    }
}
