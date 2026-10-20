//! 账号连接自检编排：把代理决策、凭据注入、收件/发件探测串成一条链路。
//!
//! 安全约定：授权码只在内存里传递；错误文案由协议层脱敏后返回，本模块只加环节前缀。

use std::future::Future;
use std::time::Duration;

use mail_domain::account::AccountDraft;
use mail_domain::auth::AuthMaterial;
use mail_domain::error::ConnectionError;
use mail_domain::proxy::{ProxyRoute, Secret};

/// 单次自检遇到临时网络问题时最多尝试三次，总重试次数不超过两次。
const PROBE_ATTEMPTS: u8 = 3;
const PROBE_RETRY_DELAY: Duration = Duration::from_millis(350);

/// 一次账号自检的完整结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionReport {
    /// 收件服务器上能看到的文件夹数量。
    pub imap_folder_count: usize,
    /// 发件服务器实际使用的登录方式（`PLAIN` / `LOGIN` / `XOAUTH2`）。
    pub smtp_mechanism: String,
}

/// 自检计划：收件与发件服务器的全部参数，外加已经定好的代理路线。
pub(crate) struct ProbePlan {
    /// 收件服务器探测参数。
    pub imap: mail_imap::ProbeRequest,
    /// 发件服务器探测参数。
    pub smtp: mail_smtp::ProbeRequest,
    /// 本次连接使用的代理；`None` 表示直连。
    pub route: Option<ProxyRoute>,
}

impl ProbePlan {
    /// 由账号草稿、凭据与代理路线组装探测计划。
    ///
    /// 凭据按账号的认证方式包成材料：普通账号是授权码，OAuth2 账号是访问令牌。
    pub fn new(draft: &AccountDraft, secret: &Secret, route: Option<ProxyRoute>) -> Self {
        let timeout = mail_net::DEFAULT_TIMEOUT;
        let auth = AuthMaterial::for_account(draft.auth_type, secret.clone());
        Self {
            imap: mail_imap::ProbeRequest {
                host: draft.imap.host.clone(),
                port: draft.imap.port,
                security: draft.imap.security,
                username: draft.username.clone(),
                auth: auth.clone(),
                timeout,
            },
            smtp: mail_smtp::ProbeRequest {
                host: draft.smtp.host.clone(),
                port: draft.smtp.port,
                security: draft.smtp.security,
                username: draft.username.clone(),
                auth,
                timeout,
            },
            route,
        }
    }
}

/// 执行自检：先收件后发件；只有临时网络问题会重试，最终失败标出环节。
pub(crate) async fn run(plan: &ProbePlan) -> Result<ConnectionReport, ConnectionError> {
    let imap = retry_transient(|| mail_imap::probe(&plan.imap, plan.route.as_ref()))
        .await
        .map_err(|err| with_stage("收件服务器", err))?;
    let smtp = retry_transient(|| mail_smtp::probe(&plan.smtp, plan.route.as_ref()))
        .await
        .map_err(|err| with_stage("发件服务器", err))?;

    Ok(ConnectionReport {
        imap_folder_count: imap.folder_count,
        smtp_mechanism: smtp.mechanism,
    })
}

async fn retry_transient<F, Fut, T>(mut operation: F) -> Result<T, ConnectionError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ConnectionError>>,
{
    let mut attempt = 1;
    loop {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(error) if attempt >= PROBE_ATTEMPTS || !is_transient(&error) => {
                return Err(error);
            }
            Err(_) => {
                attempt += 1;
                tokio::time::sleep(PROBE_RETRY_DELAY).await;
            }
        }
    }
}

fn is_transient(error: &ConnectionError) -> bool {
    matches!(
        error.kind,
        mail_domain::error::ConnectionErrorKind::Timeout
            | mail_domain::error::ConnectionErrorKind::NetworkUnreachable
    )
}

/// 保留错误分类，只给描述加上环节名。
fn with_stage(stage: &str, error: ConnectionError) -> ConnectionError {
    ConnectionError::new(error.kind, format!("{stage}：{}", error.message))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use mail_domain::account::{AccountDraft, AccountProxyMode, AuthType, Security, ServerConfig};
    use mail_domain::error::{ConnectionError, ConnectionErrorKind};
    use mail_domain::proxy::Secret;

    use super::{retry_transient, with_stage, ProbePlan};

    fn draft() -> AccountDraft {
        AccountDraft {
            display_name: "测试".to_string(),
            email: "t@example.com".to_string(),
            auth_type: AuthType::Password,
            username: "t@example.com".to_string(),
            imap: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 993,
                security: Security::Tls,
            },
            smtp: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 465,
                security: Security::Tls,
            },
            proxy: AccountProxyMode::InheritGlobal,
            color: String::new(),
            enabled: true,
            oauth_provider: None,
            oauth_client_id: String::new(),
        }
    }

    #[test]
    fn 计划携带两边服务器参数与授权码() {
        let secret = Secret::new("pw");
        let plan = ProbePlan::new(&draft(), &secret, None);
        assert_eq!(plan.imap.port, 993);
        assert_eq!(plan.smtp.port, 465);
        assert_eq!(plan.imap.auth.expose(), "pw");
        assert!(plan.route.is_none());
    }

    #[test]
    fn 失败信息带上环节前缀() {
        let err = with_stage(
            "收件服务器",
            mail_domain::error::ConnectionError::auth("登录被拒绝"),
        );
        assert_eq!(err.kind, ConnectionErrorKind::AuthFailed);
        assert_eq!(err.message, "收件服务器：登录被拒绝");
    }

    #[tokio::test]
    async fn 临时超时第三次成功() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&attempts);
        let result = retry_transient(move || {
            let calls = Arc::clone(&calls);
            async move {
                let attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt < 3 {
                    Err(ConnectionError::timeout("暂时超时"))
                } else {
                    Ok(attempt)
                }
            }
        })
        .await;

        assert_eq!(result.expect("第三次应成功"), 3);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn 持续网络中断总共只尝试三次() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&attempts);
        let result: Result<(), ConnectionError> = retry_transient(move || {
            let calls = Arc::clone(&calls);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(ConnectionError::network("网络中断"))
            }
        })
        .await;

        assert_eq!(
            result.expect_err("网络仍未恢复").kind,
            ConnectionErrorKind::NetworkUnreachable
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn 认证失败只尝试一次() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&attempts);
        let result: Result<(), ConnectionError> = retry_transient(move || {
            let calls = Arc::clone(&calls);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(ConnectionError::auth("登录被拒绝"))
            }
        })
        .await;

        assert_eq!(
            result.expect_err("认证错误应立即返回").kind,
            ConnectionErrorKind::AuthFailed
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
