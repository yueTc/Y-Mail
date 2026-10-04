//! 账号连接自检编排：把代理决策、凭据注入、收件/发件探测串成一条链路。
//!
//! 安全约定：授权码只在内存里传递；错误文案由协议层脱敏后返回，本模块只加环节前缀。

use mail_domain::account::AccountDraft;
use mail_domain::error::ConnectionError;
use mail_domain::proxy::{ProxyRoute, Secret};

/// 一次账号自检的完整结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionReport {
    /// 收件服务器上能看到的文件夹数量。
    pub imap_folder_count: usize,
    /// 发件服务器实际使用的登录方式（`PLAIN` / `LOGIN`）。
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
    /// 由账号草稿、授权码与代理路线组装探测计划。
    pub fn new(draft: &AccountDraft, secret: &Secret, route: Option<ProxyRoute>) -> Self {
        let timeout = mail_net::DEFAULT_TIMEOUT;
        Self {
            imap: mail_imap::ProbeRequest {
                host: draft.imap.host.clone(),
                port: draft.imap.port,
                security: draft.imap.security,
                username: draft.username.clone(),
                password: secret.clone(),
                timeout,
            },
            smtp: mail_smtp::ProbeRequest {
                host: draft.smtp.host.clone(),
                port: draft.smtp.port,
                security: draft.smtp.security,
                username: draft.username.clone(),
                password: secret.clone(),
                timeout,
            },
            route,
        }
    }
}

/// 执行自检：先收件后发件，任何一步失败都标出出问题的环节。
pub(crate) async fn run(plan: &ProbePlan) -> Result<ConnectionReport, ConnectionError> {
    let imap = mail_imap::probe(&plan.imap, plan.route.as_ref())
        .await
        .map_err(|err| with_stage("收件服务器", err))?;
    let smtp = mail_smtp::probe(&plan.smtp, plan.route.as_ref())
        .await
        .map_err(|err| with_stage("发件服务器", err))?;

    Ok(ConnectionReport {
        imap_folder_count: imap.folder_count,
        smtp_mechanism: smtp.mechanism,
    })
}

/// 保留错误分类，只给描述加上环节名。
fn with_stage(stage: &str, error: ConnectionError) -> ConnectionError {
    ConnectionError::new(error.kind, format!("{stage}：{}", error.message))
}

#[cfg(test)]
mod tests {
    use mail_domain::account::{AccountDraft, AccountProxyMode, AuthType, Security, ServerConfig};
    use mail_domain::error::ConnectionErrorKind;
    use mail_domain::proxy::Secret;

    use super::{with_stage, ProbePlan};

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
        assert_eq!(plan.imap.password.expose(), "pw");
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
}
