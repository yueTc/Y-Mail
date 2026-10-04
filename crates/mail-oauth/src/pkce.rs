//! PKCE（RFC 7636）与随机串。
//!
//! verifier 只留在本机内存里，授权地址里只放它的 SHA-256 摘要。

use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::error::OAuthError;

/// 一次授权用的 PKCE 材料。
pub struct Pkce {
    /// 只回传给令牌接口的原始随机串。
    pub verifier: String,
    /// 放在授权地址里的摘要。
    pub challenge: String,
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &"<隐藏>")
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// 生成 PKCE 材料：verifier 是 43 字节系统随机数的 base64url。
pub fn generate_pkce() -> Result<Pkce, OAuthError> {
    let verifier = random_urlsafe(43)?;
    let challenge = challenge_for(&verifier);
    Ok(Pkce { verifier, challenge })
}

/// 计算 challenge：base64url(SHA256(verifier))，不带填充。
pub fn challenge_for(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// 生成 `bytes` 字节系统随机数的 base64url 文本。
pub fn random_urlsafe(bytes: usize) -> Result<String, OAuthError> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|_| OAuthError::Config("系统随机数不可用"))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 附录 B 的官方向量。
    #[test]
    fn 摘要算法符合官方测试向量() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge_for(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn 生成的验证串长度与摘要长度稳定() {
        let pkce = generate_pkce().expect("随机源可用");
        assert_eq!(pkce.verifier.len(), 58, "43 字节 base64url 应为 58 字符");
        assert_eq!(pkce.challenge.len(), 43, "SHA-256 摘要 base64url 应为 43 字符");
        assert_ne!(pkce.verifier, pkce.challenge);
        assert!(!pkce.verifier.contains('='));
        assert!(!pkce.challenge.contains('='));
    }

    #[test]
    fn 两次生成不重复() {
        let first = generate_pkce().expect("随机源可用");
        let second = generate_pkce().expect("随机源可用");
        assert_ne!(first.verifier, second.verifier);
    }

    #[test]
    fn 调试输出不打印验证串原文() {
        let pkce = generate_pkce().expect("随机源可用");
        let text = format!("{pkce:?}");
        assert!(!text.contains(&pkce.verifier));
        assert!(text.contains("<隐藏>"));
    }
}
