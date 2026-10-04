//! 认证材料：把「授权码登录」和「OAuth2 令牌登录」分成两路。
//!
//! 协议层（mail-imap / mail-smtp）只认这个枚举，不关心账号是怎么拿到凭据的：
//! 授权码走 IMAP LOGIN、SMTP AUTH PLAIN / LOGIN；令牌走 XOAUTH2。
//!
//! 安全约定：材料本体包在 `Secret` 里，`Debug` 只打印类型名，绝不打印内容。

use std::fmt;

use crate::account::AuthType;
use crate::proxy::Secret;

/// 一次连接认证所用的凭据。
#[derive(Clone, PartialEq, Eq)]
pub enum AuthMaterial {
    /// 授权码 / 密码。
    Password(Secret),
    /// OAuth2 访问令牌（Bearer）。
    Bearer(Secret),
}

impl AuthMaterial {
    /// 用授权码 / 密码构造。
    pub fn password(value: impl Into<String>) -> Self {
        Self::Password(Secret::new(value))
    }

    /// 用 OAuth2 访问令牌构造。
    pub fn bearer(value: impl Into<String>) -> Self {
        Self::Bearer(Secret::new(value))
    }

    /// 按账号的认证方式，把已经取到的凭据包成对应材料。
    ///
    /// 注意：OAuth2 账号传进来的必须是「访问令牌」，不是刷新令牌。
    pub fn for_account(auth_type: AuthType, secret: Secret) -> Self {
        match auth_type {
            AuthType::Password => Self::Password(secret),
            AuthType::OAuth2 => Self::Bearer(secret),
        }
    }

    /// 取出秘密原文；只允许交给拼认证命令的代码。
    pub fn expose(&self) -> &str {
        match self {
            Self::Password(secret) | Self::Bearer(secret) => secret.expose(),
        }
    }

    /// 凭据是不是还没填。
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Password(secret) | Self::Bearer(secret) => secret.is_empty(),
        }
    }

    /// 是不是 OAuth2 令牌。
    pub fn is_bearer(&self) -> bool {
        matches!(self, Self::Bearer(_))
    }

    /// 面向日志的认证方式名，不含任何秘密。
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Password(_) => "password",
            Self::Bearer(_) => "bearer",
        }
    }
}

impl fmt::Debug for AuthMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind_label())
    }
}

/// 拼 SASL XOAUTH2 的初始响应原文（还没做 base64）。
///
/// 形如 `user=<邮箱>\x01auth=Bearer <令牌>\x01\x01`；
/// IMAP 与 SMTP 的 XOAUTH2 用同一份内容，所以放在这里共用。
pub fn xoauth2_sasl(username: &str, token: &str) -> Vec<u8> {
    let mut raw = Vec::with_capacity(username.len() + token.len() + 24);
    raw.extend_from_slice(b"user=");
    raw.extend_from_slice(username.as_bytes());
    raw.push(0x01);
    raw.extend_from_slice(b"auth=Bearer ");
    raw.extend_from_slice(token.as_bytes());
    raw.push(0x01);
    raw.push(0x01);
    raw
}

#[cfg(test)]
mod tests {
    use super::{xoauth2_sasl, AuthMaterial};
    use crate::account::AuthType;
    use crate::proxy::Secret;

    #[test]
    fn 按认证方式生成对应材料() {
        let password = AuthMaterial::for_account(AuthType::Password, Secret::new("pw"));
        assert!(!password.is_bearer());
        assert_eq!(password.kind_label(), "password");
        assert_eq!(password.expose(), "pw");

        let bearer = AuthMaterial::for_account(AuthType::OAuth2, Secret::new("tok"));
        assert!(bearer.is_bearer());
        assert_eq!(bearer.kind_label(), "bearer");
        assert_eq!(bearer.expose(), "tok");
    }

    #[test]
    fn 调试输出不泄露凭据() {
        let bearer = AuthMaterial::bearer("super-secret-token");
        let text = format!("{bearer:?}");
        assert!(!text.contains("super"), "{text}");
        assert!(!text.contains("secret"), "{text}");
    }

    #[test]
    fn 空凭据可识别() {
        assert!(AuthMaterial::password("").is_empty());
        assert!(AuthMaterial::bearer("").is_empty());
        assert!(!AuthMaterial::bearer("x").is_empty());
    }

    #[test]
    fn xoauth2初始响应拼装正确() {
        let raw = xoauth2_sasl("someone@example.com", "tok-123");
        let mut expected = Vec::new();
        expected.extend_from_slice(b"user=someone@example.com");
        expected.push(0x01);
        expected.extend_from_slice(b"auth=Bearer tok-123");
        expected.push(0x01);
        expected.push(0x01);
        assert_eq!(raw, expected);
    }
}
