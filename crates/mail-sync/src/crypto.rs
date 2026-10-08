//! 口令派生与内容加解密。
//!
//! 规矩：派生键、密码、明文只在内存里短暂存在，用完立即擦除；任何错误信息都不得
//! 携带这些内容。解密失败不区分「密码错」与「内容被改」，统一报「解不开」，
//! 不给攻击者多余信息。

use std::fmt;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroize;

use crate::error::SyncError;

/// 盐的长度（字节）。
pub const SALT_LEN: usize = 16;
/// 每次加密用的随机数长度（字节）。
pub const NONCE_LEN: usize = 12;
/// 派生键长度（字节）。
pub const KEY_LEN: usize = 32;

/// 口令派生参数；随出网包一起存，日后可升级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    /// 内存开销，单位 KiB（64 MiB = 65536）。
    pub memory_kib: u32,
    /// 迭代次数。
    pub iterations: u32,
    /// 并行度。
    pub parallelism: u32,
}

impl Default for KdfParams {
    /// 规格 3.3 定的默认值：64 MiB / 3 次 / 并行 1。
    fn default() -> Self {
        Self {
            memory_kib: 65536,
            iterations: 3,
            parallelism: 1,
        }
    }
}

/// 派生出来的对称密钥；离开作用域自动擦除，调试输出只显示掩码。
pub struct SecretKey([u8; KEY_LEN]);

impl SecretKey {
    /// 包裹一个已经派生好的键。
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// 取只读字节视图，仅限加解密时使用。
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey(***)")
    }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// 生成指定长度的安全随机字节。
pub fn random_bytes(len: usize) -> Result<Vec<u8>, SyncError> {
    let mut buf = vec![0_u8; len];
    getrandom::fill(&mut buf).map_err(|_| SyncError::Random)?;
    Ok(buf)
}

/// 用 Argon2id 从密码与盐派生一个 32 字节的键。
///
/// 参数非法或底层失败统一报「口令派生失败」，不带出密码或盐。
pub fn derive_key(password: &str, salt: &[u8], params: KdfParams) -> Result<SecretKey, SyncError> {
    if salt.len() < SALT_LEN {
        return Err(SyncError::Kdf);
    }
    let params = Params::new(
        params.memory_kib,
        params.iterations,
        params.parallelism,
        Some(KEY_LEN),
    )
    .map_err(|_| SyncError::Kdf)?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0_u8; KEY_LEN];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|_| SyncError::Kdf)?;
    Ok(SecretKey::from_bytes(key))
}

/// 用密钥与随机数加密明文。
///
/// 随机数长度必须是 [`NONCE_LEN`]；每次调用都要传一个新的随机数，绝不复用。
pub fn encrypt(key: &SecretKey, nonce: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, SyncError> {
    if nonce.len() != NONCE_LEN {
        return Err(SyncError::Encrypt);
    }
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes()).map_err(|_| SyncError::Encrypt)?;
    cipher
        .encrypt(Nonce::from_slice(nonce), plaintext)
        .map_err(|_| SyncError::Encrypt)
}

/// 用密钥与随机数解密。
///
/// 密码不对与内容被改过一律报「解不开」，不区分原因。
pub fn decrypt(key: &SecretKey, nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, SyncError> {
    if nonce.len() != NONCE_LEN {
        return Err(SyncError::Decrypt);
    }
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes()).map_err(|_| SyncError::Decrypt)?;
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| SyncError::Decrypt)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::{
        decrypt, derive_key, encrypt, random_bytes, KdfParams, SecretKey, KEY_LEN, NONCE_LEN, SALT_LEN,
    };
    use crate::error::SyncError;

    /// 测试用的轻量参数：只验证逻辑，不追求真实强度。
    fn fast_params() -> KdfParams {
        KdfParams {
            memory_kib: 8192,
            iterations: 1,
            parallelism: 1,
        }
    }

    fn salt(fill: u8) -> Vec<u8> {
        vec![fill; SALT_LEN]
    }

    #[test]
    fn 同密码同盐派生出同一个键() {
        let a = derive_key("hunter2-abc", &salt(7), fast_params()).expect("派生");
        let b = derive_key("hunter2-abc", &salt(7), fast_params()).expect("派生");
        assert_eq!(a.as_bytes(), b.as_bytes());
    }

    #[test]
    fn 不同盐派生结果不同() {
        let a = derive_key("pw", &salt(1), fast_params()).expect("派生");
        let b = derive_key("pw", &salt(2), fast_params()).expect("派生");
        assert_ne!(a.as_bytes(), b.as_bytes());
    }

    #[test]
    fn 盐太短直接报派生失败() {
        assert!(matches!(
            derive_key("pw", &[0_u8; 4], fast_params()),
            Err(SyncError::Kdf)
        ));
    }

    #[test]
    fn 加密解密往返一致() {
        let key = derive_key("pw", &salt(3), fast_params()).expect("派生");
        let nonce = random_bytes(NONCE_LEN).expect("随机数");
        let plain = "账号授权码 abc-123".as_bytes();
        let ciphertext = encrypt(&key, &nonce, plain).expect("加密");
        let back = decrypt(&key, &nonce, &ciphertext).expect("解密");
        assert_eq!(back, plain);
    }

    #[test]
    fn 错密码解不开() {
        let good = derive_key("right-pw", &salt(3), fast_params()).expect("派生");
        let bad = derive_key("wrong-pw", &salt(3), fast_params()).expect("派生");
        let nonce = random_bytes(NONCE_LEN).expect("随机数");
        let ciphertext = encrypt(&good, &nonce, b"secret").expect("加密");
        assert!(matches!(
            decrypt(&bad, &nonce, &ciphertext),
            Err(SyncError::Decrypt)
        ));
    }

    #[test]
    fn 改动一个字节就解不开() {
        let key = derive_key("pw", &salt(4), fast_params()).expect("派生");
        let nonce = random_bytes(NONCE_LEN).expect("随机数");
        let mut ciphertext = encrypt(&key, &nonce, b"secret value").expect("加密");
        ciphertext[0] ^= 0x01;
        assert!(matches!(
            decrypt(&key, &nonce, &ciphertext),
            Err(SyncError::Decrypt)
        ));
    }

    #[test]
    fn 随机数每次都不一样() {
        let a = random_bytes(NONCE_LEN).expect("随机");
        let b = random_bytes(NONCE_LEN).expect("随机");
        assert_ne!(a, b);
        assert_eq!(a.len(), NONCE_LEN);
    }

    #[test]
    fn 密钥调试输出不泄露() {
        let key = deriv_key_for_debug();
        assert_eq!(format!("{key:?}"), "SecretKey(***)");
    }

    fn deriv_key_for_debug() -> SecretKey {
        derive_key("pw", &salt(5), fast_params()).expect("派生")
    }

    #[test]
    fn 默认参数能派生且耗时在可接受范围() {
        // N3：规格默认值在真机上的耗时是否可接受（目标 1 秒以内）。
        let started = Instant::now();
        let key = derive_key("sync-password", &salt(9), KdfParams::default()).expect("派生");
        let elapsed = started.elapsed();
        assert_eq!(key.as_bytes().len(), KEY_LEN);
        assert!(
            elapsed.as_secs_f64() < 3.0,
            "默认 Argon2id 参数耗时 {elapsed:?}，超过 3 秒"
        );
        println!("默认 Argon2id 参数耗时：{elapsed:?}");
    }
}
