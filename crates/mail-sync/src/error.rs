//! mail-sync 的统一错误类型。
//!
//! 规矩：对外错误只给固定中文提示，最多带上状态码或格式名这类公开信息；
//! 绝不携带密码、令牌、授权码、明文或响应正文。底层细节只写本地日志。

use mail_domain::error::ConnectionError;
use thiserror::Error;

/// 同步底座的统一错误。
#[derive(Debug, Error)]
pub enum SyncError {
    /// 网络或传输链路失败。
    #[error("网络请求失败")]
    Network(#[source] ConnectionError),
    /// 请求超时。
    #[error("请求超时，请稍后重试")]
    Timeout,
    /// 响应不符合预期（不是合法响应、结构对不上等）。
    #[error("对方响应无法识别")]
    Protocol,
    /// 本地数据格式不对（字段缺失、编码解不开等）。
    #[error("数据格式不对")]
    Malformed,
    /// 对方返回了 HTTP 错误状态码。
    #[error("GitHub 返回错误（状态码 {status}）")]
    Http {
        /// HTTP 状态码，只用于区分 4xx / 5xx 等大类。
        status: u16,
    },
    /// 缺少管理 Gist 的权限。
    #[error("没有管理 Gist 的权限，无法同步")]
    MissingGistScope,
    /// 目标资源不存在（Gist 被删了，或账号下没有同步数据）。
    #[error("没有找到同步数据")]
    NotFound,
    /// 云端内容已被别的设备改过。
    #[error("云端内容已被别的设备改过，请先处理冲突")]
    Conflict,
    /// 设备码已过期。
    #[error("设备码已过期，请重新发起登录")]
    DeviceCodeExpired,
    /// 用户在浏览器里拒绝了授权。
    #[error("授权被拒绝")]
    AccessDenied,
    /// 授权还没确认，需要继续轮询。
    #[error("等待你在浏览器里确认授权")]
    AuthorizationPending,
    /// GitHub 要求放慢请求，需要拉长轮询间隔。
    #[error("GitHub 要求放慢请求，正在重试")]
    SlowDown,
    /// 系统随机数不可用。
    #[error("无法生成安全随机数")]
    Random,
    /// 口令派生失败。
    #[error("口令派生失败")]
    Kdf,
    /// 加密失败。
    #[error("加密失败")]
    Encrypt,
    /// 解密失败；密码不对与内容被改过故意不区分。
    #[error("解不开同步数据：密码不对或内容被改过")]
    Decrypt,
    /// 遇到不认识的格式或版本号。
    #[error("同步包格式不认识：{0}")]
    UnsupportedFormat(String),
    /// 本地配置不完整或参数校验不过；文案由调用方给出的安全提示构成。
    #[error("{0}")]
    Config(String),
}

#[cfg(test)]
mod tests {
    use super::SyncError;

    #[test]
    fn 通用错误文案里不带底层细节() {
        assert_eq!(
            SyncError::Decrypt.to_string(),
            "解不开同步数据：密码不对或内容被改过"
        );
        assert_eq!(SyncError::NotFound.to_string(), "没有找到同步数据");
        assert_eq!(
            SyncError::MissingGistScope.to_string(),
            "没有管理 Gist 的权限，无法同步"
        );
    }

    #[test]
    fn 状态码类错误只带状态码() {
        let text = SyncError::Http { status: 403 }.to_string();
        assert!(text.contains("403"));
        assert!(!text.contains("token"));
    }

    #[test]
    fn 格式错误只带格式名() {
        let text = SyncError::UnsupportedFormat("ymail-settings-sync/9".to_string()).to_string();
        assert!(text.contains("ymail-settings-sync/9"));
    }
}
