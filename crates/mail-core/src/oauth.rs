//! OAuth2 授权编排（Wave 6）。
//!
//! 流程：界面点「授权」→ 本模块在本机开一个回环端口、生成授权地址 →
//! 外壳用系统浏览器打开 → 用户点同意 → 授权服务器回调本机端口 →
//! 本模块拿授权码换令牌 → 令牌进系统保险箱 → 顺手做一次连接自检 → 落库。
//!
//! 令牌刷新：OAuth2 账号每次取用凭据前先看是否临近到期，过期就用刷新令牌换新的，
//! 并把新令牌写回保险箱；刷新被拒（`invalid_grant`）就冒「需要重新授权」信号。
//!
//! 安全约定：访问令牌与刷新令牌只进保险箱；本模块的日志、错误与 Debug 输出都不带令牌原文。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use mail_domain::account::{Account, AccountDraft, AccountId, AuthType, OAuthProvider};
use mail_domain::proxy::Secret;
use mail_oauth::{
    bind_loopback, build_authorize_request, exchange_code_at_with_client, exchange_code_with_client,
    now_unix, refresh_at_with_client, refresh_with_client, resolve_client_id, ClientCredentials, Loopback,
    OAuthError, Pkce, ProviderKind, TokenSet,
};
use mail_store::Store;
use serde::{Deserialize, Serialize};

use crate::checks::ConnectionReport;
use crate::engine::{EngineError, MailEngine};
use crate::proxies::new_credential_key;
use crate::secrets::SecretStore;

/// 等用户在浏览器里完成授权的上限。
const AWAIT_AUTHORIZATION: Duration = Duration::from_secs(300);
/// 换令牌与刷新令牌的超时。
const TOKEN_TIMEOUT: Duration = Duration::from_secs(30);
/// 到期前多久就当作已过期，提前刷新，避免连接建到一半正好失效。
const EXPIRY_SKEW_SECS: i64 = 120;

/// 绑定配套密钥的原服务商与客户端编号，防止账号设置改变后串用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ClientBinding {
    provider: String,
    client_id: String,
}

/// 保险箱里保存的令牌包。
///
/// 整包以 JSON 存在一条凭据里：访问令牌、刷新令牌、应用密钥及其绑定信息、到期时间、权限范围。
/// 旧版本可能直接存了访问令牌明文，读取时按「只有访问令牌」兼容。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TokenBundle {
    access_token: String,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    client_binding: Option<ClientBinding>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

impl std::fmt::Debug for TokenBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenBundle")
            .field("access_token", &"<隐藏>")
            .field("refresh_token", &self.refresh_token.is_some())
            .field("client_secret", &self.client_secret.is_some())
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl TokenBundle {
    fn matches_client(&self, kind: ProviderKind, client_id: &str) -> bool {
        self.client_binding
            .as_ref()
            .is_some_and(|binding| binding.provider == kind.as_str() && binding.client_id == client_id)
    }

    /// 由一次换令牌 / 刷新结果构造。
    fn from_tokens(tokens: TokenSet) -> Self {
        Self {
            access_token: tokens.access_token,
            client_secret: None,
            client_binding: None,
            refresh_token: tokens.refresh_token,
            expires_at: tokens.expires_at,
            scope: tokens.scope,
        }
    }

    /// 用新一次刷新结果覆盖，保留没返回的刷新令牌与权限范围。
    fn apply(&mut self, tokens: TokenSet) {
        self.access_token = tokens.access_token;
        if tokens.refresh_token.is_some() {
            self.refresh_token = tokens.refresh_token;
        }
        self.expires_at = tokens.expires_at;
        if tokens.scope.is_some() {
            self.scope = tokens.scope;
        }
    }

    /// 序列化成存进保险箱的文本。
    fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// 从保险箱文本还原；认不出来就当成「裸访问令牌」。
    fn decode(raw: &str) -> Option<Self> {
        if let Ok(bundle) = serde_json::from_str::<Self>(raw) {
            return Some(bundle);
        }
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self {
            access_token: trimmed.to_string(),
            client_secret: None,
            client_binding: None,
            refresh_token: None,
            expires_at: None,
            scope: None,
        })
    }

    /// 是否该刷新了（带提前量）。
    fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(at) => at - EXPIRY_SKEW_SECS <= now_unix(),
            None => false,
        }
    }
}

/// 发起授权后要交给外壳的信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthAuthorization {
    /// 用系统浏览器打开的授权页地址。
    pub authorize_url: String,
    /// 本次授权的校验串；回调完成后拿它来收口，取消时也用它。
    pub state: String,
    /// 本机回调地址；已经在服务商后台登记过的地址必须跟它一致。
    pub redirect_uri: String,
}

/// 授权完成后的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthOutcome {
    /// 落库后的账号。
    pub account: Account,
    /// 顺手做的一次连接自检结果。
    pub report: ConnectionReport,
}

/// 一个 OAuth2 账号当前的授权状态，供界面显示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthStatus {
    /// 保险箱里是否有可用的访问令牌。
    pub authorized: bool,
    /// 访问令牌到期时间（Unix 秒）；`None` 表示服务器没给。
    pub expires_at: Option<i64>,
    /// 申请的权限范围。
    pub scope: Option<String>,
    /// 有没有刷新令牌（没有就只能重新授权）。
    pub has_refresh_token: bool,
}

impl OAuthStatus {
    fn unauthorized() -> Self {
        Self {
            authorized: false,
            expires_at: None,
            scope: None,
            has_refresh_token: false,
        }
    }
}

/// 取账号凭据时可能出现的失败，交给调用方翻译成各自体系的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolveError {
    /// 保险箱里没有这条凭据。
    Missing,
    /// 保险箱本身故障。
    Backend,
    /// 刷新令牌已失效，必须用户重新授权。
    ReauthRequired,
    /// 其他失败，带一句可读文案。
    Failed(String),
}

/// 一次还没收口的授权：回环端口、校验材料与待落库的账号草稿。
pub(crate) struct PendingAuthorization {
    /// 服务商。
    kind: ProviderKind,
    /// 本次授权用的客户端编号。
    client_id: String,
    /// 只存在本次授权内存和最终保险箱条目中。
    client_secret: Option<Secret>,
    /// 回调地址，换令牌时必须与授权时完全一致。
    redirect_uri: String,
    /// PKCE 材料。
    pkce: Pkce,
    /// 待保存的账号草稿。
    draft: AccountDraft,
    /// `Some` 表示给已有账号重新授权；`None` 表示新建账号。
    existing: Option<AccountId>,
    /// 令牌地址覆盖；只有测试会用到。
    token_endpoint: Option<String>,
    /// 本机回环端口。
    loopback: Loopback,
}

impl PendingAuthorization {
    /// 等回调 → 换令牌 → 存保险箱 → 自检 → 落库。
    async fn finish(self, engine: &MailEngine, state: &str) -> Result<OAuthOutcome, EngineError> {
        let code = self
            .loopback
            .wait_for_code(state, AWAIT_AUTHORIZATION)
            .await
            .map_err(from_oauth)?;

        let route = engine.resolve_route(self.draft.proxy)?;
        let client = ClientCredentials {
            client_id: &self.client_id,
            client_secret: self.client_secret.as_ref(),
        };
        let tokens = match self.token_endpoint.as_deref() {
            Some(endpoint) => {
                exchange_code_at_with_client(
                    endpoint,
                    route.as_ref(),
                    client,
                    &code,
                    &self.pkce.verifier,
                    &self.redirect_uri,
                    TOKEN_TIMEOUT,
                )
                .await
            }
            None => {
                exchange_code_with_client(
                    route.as_ref(),
                    self.kind,
                    client,
                    &code,
                    &self.pkce.verifier,
                    &self.redirect_uri,
                    TOKEN_TIMEOUT,
                )
                .await
            }
        }
        .map_err(from_oauth)?;

        let mut bundle = TokenBundle::from_tokens(tokens);
        bundle.client_binding = Some(ClientBinding {
            provider: self.kind.as_str().to_string(),
            client_id: self.client_id.clone(),
        });
        bundle.client_secret = self.client_secret.map(|secret| secret.expose().to_string());
        let access = Secret::new(bundle.access_token.clone());

        // 先写新条目，再自检；自检不过就把新条目删掉，旧凭据保持原样。
        let old_key = match self.existing {
            Some(id) => engine.get_account(id)?.credential_key,
            None => None,
        };
        let new_key = new_credential_key("account", &self.draft.email);
        engine.secrets().set(&new_key, &Secret::new(bundle.encode()))?;

        let plan = crate::checks::ProbePlan::new(&self.draft, &access, route);
        let report = match crate::checks::run(&plan).await {
            Ok(report) => report,
            Err(error) => {
                let _ = engine.secrets().delete(&new_key);
                return Err(error.into());
            }
        };

        let account = match self.existing {
            Some(id) => {
                let updated = { engine.store().update_account(id, &self.draft, Some(&new_key))? };
                if !updated {
                    let _ = engine.secrets().delete(&new_key);
                    return Err(EngineError::AccountNotFound(id.0));
                }
                if let Some(old) = old_key.filter(|old| old != &new_key) {
                    let _ = engine.secrets().delete(&old);
                }
                engine.get_account(id)?
            }
            None => {
                if engine.store().email_taken(&self.draft.email, None)? {
                    let _ = engine.secrets().delete(&new_key);
                    return Err(EngineError::EmailTaken(self.draft.email.clone()));
                }
                let inserted = engine.store().insert_account(&self.draft, Some(&new_key));
                match inserted {
                    Ok(id) => engine.get_account(id)?,
                    Err(error) => {
                        let _ = engine.secrets().delete(&new_key);
                        return Err(error.into());
                    }
                }
            }
        };

        tracing::info!(account = account.id.0, "OAuth2 授权完成");
        Ok(OAuthOutcome { account, report })
    }
}

impl MailEngine {
    /// 开始一次浏览器授权，返回要打开的地址与本次校验串。
    ///
    /// `existing` 为 `Some` 表示给已有账号重新授权。
    pub async fn begin_oauth_authorize(
        &self,
        draft: &AccountDraft,
        existing: Option<AccountId>,
    ) -> Result<OAuthAuthorization, EngineError> {
        self.begin_oauth_authorize_with_secret(draft, existing, None)
            .await
    }

    /// 应用密钥独立于普通账号配置传入，只存系统保险箱。
    pub async fn begin_oauth_authorize_with_secret(
        &self,
        draft: &AccountDraft,
        existing: Option<AccountId>,
        client_secret: Option<&Secret>,
    ) -> Result<OAuthAuthorization, EngineError> {
        self.begin_oauth_authorize_at(draft, existing, client_secret, None)
            .await
    }

    /// 与 [`MailEngine::begin_oauth_authorize`] 相同，但可以指定令牌地址（测试用）。
    pub(crate) async fn begin_oauth_authorize_at(
        &self,
        draft: &AccountDraft,
        existing: Option<AccountId>,
        client_secret: Option<&Secret>,
        token_endpoint: Option<String>,
    ) -> Result<OAuthAuthorization, EngineError> {
        let normalized = draft.normalized();
        let draft = normalized;
        draft.validate()?;
        if draft.auth_type != AuthType::OAuth2 {
            return Err(EngineError::BadRequest("该账号不是 OAuth2 登录方式".to_string()));
        }
        let kind = provider_kind(draft.oauth_provider)?;
        // 用户没填编号就用软件内置的；内置也没有才报错，提示里指明去哪儿填。
        let client_id = resolve_client_id(kind, &draft.oauth_client_id).map_err(from_oauth)?;
        let mut client_secret = client_secret
            .filter(|secret| !secret.expose().trim().is_empty())
            .cloned();
        if client_secret.is_none() {
            if let Some(id) = existing {
                let account = self.get_account(id)?;
                // 只有同一服务商、同一客户端编号才能沿用原密钥。
                if account.auth_type == AuthType::OAuth2
                    && account.oauth_provider == draft.oauth_provider
                    && resolve_client_id(kind, &account.oauth_client_id).ok().as_deref()
                        == Some(client_id.as_str())
                {
                    if let Some(key) = account.credential_key.as_deref() {
                        if let Some(stored) = self.secrets().get(key)? {
                            client_secret = TokenBundle::decode(stored.expose())
                                .filter(|bundle| bundle.matches_client(kind, &client_id))
                                .and_then(|bundle| bundle.client_secret)
                                .filter(|secret| !secret.trim().is_empty())
                                .map(Secret::new);
                        }
                    }
                }
            }
        }
        if kind == ProviderKind::Gmail && client_secret.is_none() {
            // 已在本机配置的应用密钥按服务商与编号取用，无需每个邮箱重复填写。
            client_secret = self
                .secrets()
                .get(&oauth_app_secret_key(kind, &client_id))?
                .filter(|secret| !secret.expose().trim().is_empty());
        }
        if kind == ProviderKind::Gmail && client_secret.is_none() {
            return Err(from_oauth(OAuthError::MissingClientSecret));
        }
        let mut draft = draft;
        // 把解析结果写回草稿：授权成功后落库，令牌到期刷新时才有编号可用。
        draft.oauth_client_id = client_id.clone();
        let loopback = bind_loopback().await.map_err(from_oauth)?;
        let request = build_authorize_request(kind, &client_id, loopback.port, Some(&draft.email))
            .map_err(from_oauth)?;

        let authorization = OAuthAuthorization {
            authorize_url: request.url.clone(),
            state: request.state.clone(),
            redirect_uri: request.redirect_uri.clone(),
        };
        let pending = PendingAuthorization {
            kind,
            client_id: client_id.clone(),
            client_secret,
            redirect_uri: request.redirect_uri.clone(),
            pkce: request.pkce,
            draft,
            existing,
            token_endpoint,
            loopback,
        };
        let mut map = lock_pending(&self.oauth_pending);
        map.insert(authorization.state.clone(), pending);
        Ok(authorization)
    }

    /// 收口一次授权：等回调、换令牌、存保险箱、自检、落库。
    pub async fn complete_oauth_authorize(&self, state: &str) -> Result<OAuthOutcome, EngineError> {
        let pending = {
            let mut map = lock_pending(&self.oauth_pending);
            map.remove(state)
        };
        let pending = pending
            .ok_or_else(|| EngineError::BadRequest("这次授权已过期或已取消，请重新发起授权".to_string()))?;
        pending.finish(self, state).await
    }

    /// 放弃一次还没收口的授权；返回是否真的取消掉了。
    pub fn cancel_oauth_authorize(&self, state: &str) -> bool {
        let mut map = lock_pending(&self.oauth_pending);
        map.remove(state).is_some()
    }

    /// 一个 OAuth2 账号当前的授权状态。
    pub fn oauth_status(&self, id: AccountId) -> Result<OAuthStatus, EngineError> {
        let account = self.get_account(id)?;
        let Some(key) = account.credential_key.as_deref() else {
            return Ok(OAuthStatus::unauthorized());
        };
        let Some(secret) = self.secrets().get(key)? else {
            return Ok(OAuthStatus::unauthorized());
        };
        let Some(bundle) = TokenBundle::decode(secret.expose()) else {
            return Ok(OAuthStatus::unauthorized());
        };
        Ok(OAuthStatus {
            authorized: !bundle.access_token.trim().is_empty(),
            expires_at: bundle.expires_at,
            scope: bundle.scope,
            has_refresh_token: bundle.refresh_token.is_some(),
        })
    }

    /// 取账号当前可用的凭据。
    ///
    /// 普通账号直接给授权码；OAuth2 账号给访问令牌，临近到期先用刷新令牌换新的。
    pub(crate) async fn resolved_secret(&self, account: &Account) -> Result<Secret, EngineError> {
        active_secret(&self.store, self.secrets(), account)
            .await
            .map_err(|error| resolve_error(account.id, error))
    }
}

/// 取账号当前可用的凭据；同步线程与界面路径共用这一套口径。
pub(crate) async fn active_secret(
    store: &Mutex<Store>,
    secrets: &dyn SecretStore,
    account: &Account,
) -> Result<Secret, ResolveError> {
    active_secret_with(store, secrets, account, None).await
}

/// 与 [`active_secret`] 相同，但可以指定刷新用的令牌地址（测试用）。
pub(crate) async fn active_secret_with(
    store: &Mutex<Store>,
    secrets: &dyn SecretStore,
    account: &Account,
    token_endpoint: Option<&str>,
) -> Result<Secret, ResolveError> {
    let key = account.credential_key.clone().ok_or(ResolveError::Missing)?;
    let stored = secrets
        .get(&key)
        .map_err(|_| ResolveError::Backend)?
        .ok_or(ResolveError::Missing)?;

    if account.auth_type != AuthType::OAuth2 {
        return Ok(stored);
    }

    let mut bundle = TokenBundle::decode(stored.expose()).ok_or(ResolveError::Missing)?;
    if !bundle.is_expired() {
        return Ok(Secret::new(bundle.access_token));
    }

    let refresh_token = bundle.refresh_token.clone().ok_or(ResolveError::ReauthRequired)?;
    let route = crate::proxies::resolve_route_with(store, secrets, account.proxy)
        .map_err(|error| ResolveError::Failed(error.to_string()))?;
    let kind =
        provider_kind(account.oauth_provider).map_err(|error| ResolveError::Failed(error.to_string()))?;
    // 老账号可能没存编号（用软件内置编号授权时），这里同样按内置编号兜底；
    // 不然令牌一到期就再也刷新不了，只能重新授权。
    let client_id = resolve_client_id(kind, &account.oauth_client_id)
        .map_err(|error| ResolveError::Failed(error.to_string()))?;
    // 即使账号配置从同步或其他入口改变，也不能把旧应用密钥发给另一个客户端或服务商。
    if bundle.client_secret.is_some() && !bundle.matches_client(kind, &client_id) {
        return Err(ResolveError::ReauthRequired);
    }
    let mut client_secret = bundle
        .client_secret
        .as_ref()
        .map(|secret| Secret::new(secret.clone()));
    if kind == ProviderKind::Gmail && client_secret.is_none() {
        client_secret = secrets
            .get(&oauth_app_secret_key(kind, &client_id))
            .map_err(|_| ResolveError::Backend)?
            .filter(|secret| !secret.expose().trim().is_empty());
    }
    let client = ClientCredentials {
        client_id: &client_id,
        client_secret: client_secret.as_ref(),
    };
    let refreshed = match token_endpoint {
        Some(endpoint) => {
            refresh_at_with_client(endpoint, route.as_ref(), client, &refresh_token, TOKEN_TIMEOUT).await
        }
        None => refresh_with_client(route.as_ref(), kind, client, &refresh_token, TOKEN_TIMEOUT).await,
    };
    let tokens = refreshed.map_err(|error| {
        if error.needs_reauth() {
            ResolveError::ReauthRequired
        } else {
            ResolveError::Failed(error.to_string())
        }
    })?;

    bundle.apply(tokens);
    // 旧账号首次成功续期后，也将实际使用的配套信息纳入自己的令牌包。
    if bundle.client_secret.is_none() {
        if let Some(secret) = client_secret {
            bundle.client_secret = Some(secret.expose().to_string());
            bundle.client_binding = Some(ClientBinding {
                provider: kind.as_str().to_string(),
                client_id: client_id.clone(),
            });
        }
    }
    secrets
        .set(&key, &Secret::new(bundle.encode()))
        .map_err(|_| ResolveError::Backend)?;
    tracing::debug!(account = account.id.0, "OAuth2 访问令牌已刷新");
    Ok(Secret::new(bundle.access_token))
}

/// 把解析失败翻译成引擎错误。
fn resolve_error(account: AccountId, error: ResolveError) -> EngineError {
    match error {
        ResolveError::Missing => EngineError::BadRequest(format!(
            "账号 {} 还没有可用的凭据，请到账号设置里填写授权码或重新授权",
            account.0
        )),
        ResolveError::Backend => EngineError::Secrets(crate::secrets::SecretStoreError::Backend),
        ResolveError::ReauthRequired => EngineError::OAuthReauthRequired,
        ResolveError::Failed(message) => EngineError::BadRequest(message),
    }
}

/// 本机应用凭据与邮箱账号凭据分开，键只由公开的服务商与客户端编号组成。
fn oauth_app_secret_key(kind: ProviderKind, client_id: &str) -> String {
    format!("oauth-app/{}/{}", kind.as_str(), client_id)
}

/// 把授权服务商从账号字段翻译成协议层枚举。
fn provider_kind(provider: Option<OAuthProvider>) -> Result<ProviderKind, EngineError> {
    match provider {
        Some(OAuthProvider::Gmail) => Ok(ProviderKind::Gmail),
        Some(OAuthProvider::Microsoft) => Ok(ProviderKind::Microsoft),
        None => Err(EngineError::BadRequest(
            "OAuth2 账号需要先选择服务商（Gmail 或 Outlook）".to_string(),
        )),
    }
}

/// OAuth 层的错误转成引擎错误；保留「需要重新授权」这一档。
fn from_oauth(error: OAuthError) -> EngineError {
    match error {
        OAuthError::ReauthRequired => EngineError::OAuthReauthRequired,
        OAuthError::Network(error) => EngineError::Connection(error),
        other => EngineError::BadRequest(other.to_string()),
    }
}

/// 取待处理授权的锁；锁中毒时取回内部值继续用（都是短临界区）。
fn lock_pending(
    pending: &Mutex<HashMap<String, PendingAuthorization>>,
) -> std::sync::MutexGuard<'_, HashMap<String, PendingAuthorization>> {
    pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    #[test]
    fn 授权网络错误向界面保留分类和固定提示() {
        let source = mail_domain::error::ConnectionError::network("与授权服务器的连接中断");
        let error = super::from_oauth(mail_oauth::OAuthError::Network(source.clone()));
        match error {
            crate::EngineError::Connection(actual) => assert_eq!(actual, source),
            other => panic!("网络错误不应丢失分类：{other}"),
        }
    }

    use std::sync::Arc;

    use mail_domain::account::{AccountProxyMode, Security, ServerConfig};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;
    use crate::secrets::{ChunkedSecretStore, MemorySecretStore};

    fn oauth_draft(imap_port: u16, smtp_port: u16) -> AccountDraft {
        AccountDraft {
            display_name: "授权测试".to_string(),
            email: "oauth@example.com".to_string(),
            auth_type: AuthType::OAuth2,
            username: "oauth@example.com".to_string(),
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
            oauth_provider: Some(OAuthProvider::Gmail),
            oauth_client_id: "client-test".to_string(),
        }
    }

    /// 假令牌服务器：收一次 POST，回一段固定 JSON。
    async fn fake_token_server(payload: &'static str) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定成功");
        let port = listener.local_addr().expect("取地址").port();
        let received = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接");
            let mut request = Vec::new();
            loop {
                let mut chunk = [0u8; 4096];
                let read = socket.read(&mut chunk).await.expect("读请求");
                assert!(read > 0, "请求提前结束");
                request.extend_from_slice(&chunk[..read]);
                if let Some(boundary) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..boundary]).to_ascii_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .expect("请求长度")
                        .trim()
                        .parse()
                        .expect("长度是数字");
                    if request.len() >= boundary + 4 + length {
                        break;
                    }
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(response.as_bytes()).await.expect("写响应");
            String::from_utf8(request).expect("请求文本")
        });
        (port, received)
    }

    fn engine(dir: &std::path::Path, secrets: Arc<MemorySecretStore>) -> MailEngine {
        MailEngine::initialize_with_secrets(dir, secrets).expect("初始化引擎")
    }

    #[test]
    fn 令牌包可以往返编解码() {
        let bundle = TokenBundle {
            access_token: "at-1".to_string(),
            client_secret: Some("client-secret-test".to_string()),
            client_binding: None,
            refresh_token: Some("rt-1".to_string()),
            expires_at: Some(1234),
            scope: Some("mail".to_string()),
        };
        let decoded = TokenBundle::decode(&bundle.encode()).expect("应能还原");
        assert_eq!(decoded, bundle);
    }

    #[test]
    fn 裸访问令牌按没有刷新令牌处理() {
        let decoded = TokenBundle::decode("plain-access-token").expect("应能兼容");
        assert_eq!(decoded.access_token, "plain-access-token");
        assert!(decoded.refresh_token.is_none());
        assert!(decoded.expires_at.is_none());
    }

    #[test]
    fn 旧令牌包兼容且调试隐藏所有凭据() {
        let old = TokenBundle::decode(r#"{"access_token":"old-access","refresh_token":"old-refresh"}"#)
            .expect("兼容旧包");
        assert!(old.client_secret.is_none());
        let mut bundle = old;
        bundle.client_secret = Some("hidden-client-secret".to_string());
        let text = format!("{bundle:?}");
        for value in ["old-access", "old-refresh", "hidden-client-secret"] {
            assert!(!text.contains(value));
        }
    }

    #[tokio::test]
    async fn 谷歌缺少应用密钥时不创建回调或凭据() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets.clone());
        let error = engine
            .begin_oauth_authorize(&oauth_draft(1, 1), None)
            .await
            .expect_err("缺密钥");
        assert!(error.to_string().contains("缺少应用密钥"));
        assert!(lock_pending(&engine.oauth_pending).is_empty());
        assert!(secrets.is_empty());
        assert!(engine.list_accounts().expect("账号列表").is_empty());
    }

    #[tokio::test]
    async fn 内置谷歌编号自动读取本机应用密钥且不会串给其他客户端() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets.clone());
        let client_id = resolve_client_id(ProviderKind::Gmail, "").expect("内置编号");
        let key = oauth_app_secret_key(ProviderKind::Gmail, &client_id);
        secrets
            .set(&key, &Secret::new("local-app-secret"))
            .expect("应用保险箱");
        let mut draft = oauth_draft(1, 1);
        draft.oauth_client_id.clear();
        let auth = engine
            .begin_oauth_authorize(&draft, None)
            .await
            .expect("自动使用已配置应用");
        {
            let pending = lock_pending(&engine.oauth_pending);
            assert_eq!(pending[&auth.state].client_id, client_id);
            assert_eq!(
                pending[&auth.state]
                    .client_secret
                    .as_ref()
                    .expect("应用密钥")
                    .expose(),
                "local-app-secret"
            );
        }
        assert!(!auth.authorize_url.contains("local-app-secret"));
        assert!(engine.cancel_oauth_authorize(&auth.state));
        assert_eq!(secrets.plain(&key).as_deref(), Some("local-app-secret"));

        let override_secret = Secret::new("explicit-app-secret");
        let auth = engine
            .begin_oauth_authorize_with_secret(&draft, None, Some(&override_secret))
            .await
            .expect("显式覆盖");
        assert_eq!(
            lock_pending(&engine.oauth_pending)[&auth.state]
                .client_secret
                .as_ref(),
            Some(&override_secret)
        );
        assert!(engine.cancel_oauth_authorize(&auth.state));
        assert_eq!(
            secrets.plain(&key).as_deref(),
            Some("local-app-secret"),
            "覆盖不改应用配置"
        );

        draft.oauth_client_id = "another-client".to_string();
        let error = engine
            .begin_oauth_authorize(&draft, None)
            .await
            .expect_err("不同编号不能沿用");
        assert!(error.to_string().contains("缺少应用密钥"));
        draft.oauth_client_id = client_id;
        draft.oauth_provider = Some(OAuthProvider::Microsoft);
        let auth = engine
            .begin_oauth_authorize(&draft, None)
            .await
            .expect("微软无需密钥");
        assert!(lock_pending(&engine.oauth_pending)[&auth.state]
            .client_secret
            .is_none());
        assert!(engine.cancel_oauth_authorize(&auth.state));
        assert!(engine.list_accounts().expect("账号").is_empty());
    }

    #[tokio::test]
    async fn 重新授权仅同服务商同编号沿用保险箱密钥() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets.clone());
        let draft = oauth_draft(1, 1);
        let key = "account/client-secret-reuse";
        let stored = r#"{"access_token":"old-access","client_secret":"reuse-client-secret","client_binding":{"provider":"gmail","client_id":"client-test"}}"#;
        secrets.set(key, &Secret::new(stored)).expect("保存保险箱");
        let id = engine.store().insert_account(&draft, Some(key)).expect("账号");
        let auth = engine
            .begin_oauth_authorize(&draft, Some(id))
            .await
            .expect("沿用密钥");
        assert!(!auth.authorize_url.contains("reuse-client-secret"));
        assert!(!auth.authorize_url.contains("client_secret"));
        {
            let pending = lock_pending(&engine.oauth_pending);
            assert_eq!(
                pending[&auth.state]
                    .client_secret
                    .as_ref()
                    .expect("密钥")
                    .expose(),
                "reuse-client-secret"
            );
            assert!(!format!("{:?}", pending[&auth.state].draft).contains("reuse-client-secret"));
        }
        assert!(engine.cancel_oauth_authorize(&auth.state));
        assert_eq!(secrets.plain(key).as_deref(), Some(stored), "取消不改原凭据");

        let mut changed = draft.clone();
        changed.oauth_client_id = "another-client".to_string();
        let error = engine
            .begin_oauth_authorize(&changed, Some(id))
            .await
            .expect_err("禁止混用密钥");
        assert!(error.to_string().contains("缺少应用密钥"));
        changed.oauth_client_id = draft.oauth_client_id.clone();
        changed.oauth_provider = Some(OAuthProvider::Microsoft);
        let auth = engine
            .begin_oauth_authorize(&changed, Some(id))
            .await
            .expect("公共客户端无需密钥");
        assert!(lock_pending(&engine.oauth_pending)[&auth.state]
            .client_secret
            .is_none());
        assert!(engine.cancel_oauth_authorize(&auth.state));
        assert_eq!(secrets.plain(key).as_deref(), Some(stored));

        // 库中仅保存引用键与公开配置。
        let account = engine.get_account(id).expect("账号");
        assert!(!format!("{account:?}").contains("reuse-client-secret"));
    }

    #[test]
    fn 空凭据解不出来() {
        assert!(TokenBundle::decode("   ").is_none());
        assert!(TokenBundle::decode("").is_none());
    }

    #[test]
    fn 到期判断带提前量() {
        let mut bundle = TokenBundle {
            access_token: "at".to_string(),
            client_secret: None,
            client_binding: None,
            refresh_token: None,
            expires_at: Some(now_unix() + EXPIRY_SKEW_SECS + 60),
            scope: None,
        };
        assert!(!bundle.is_expired());
        bundle.expires_at = Some(now_unix() + EXPIRY_SKEW_SECS - 60);
        assert!(bundle.is_expired(), "提前量之内就该当过期");
        bundle.expires_at = None;
        assert!(!bundle.is_expired(), "没有到期时间就保守地当没过期");
    }

    #[tokio::test]
    async fn 超长令牌包经分片保险箱也能存能读() {
        let dir = tempfile::tempdir().expect("临时目录");
        // 走真实装配方式：分片保险箱包住底层实现。
        let engine = MailEngine::initialize_with_secrets(
            dir.path(),
            Arc::new(ChunkedSecretStore::new(MemorySecretStore::new())),
        )
        .expect("初始化引擎");

        // 微软个人账号的实际量级：光访问令牌就一千多字符，刷新令牌还要更长。
        let expires_at = now_unix() + 3600;
        let bundle = TokenBundle {
            access_token: "a".repeat(1600),
            client_secret: None,
            client_binding: None,
            refresh_token: Some("r".repeat(2200)),
            expires_at: Some(expires_at),
            scope: Some("https://outlook.office.com/IMAP.AccessAsUser.All offline_access".to_string()),
        };
        let key = "account/long-bundle";
        let id = {
            let store = engine.store();
            store
                .insert_account(&oauth_draft(1, 1), Some(key))
                .expect("插入账号")
        };
        engine
            .secrets()
            .set(key, &Secret::new(bundle.encode()))
            .expect("长令牌包该能写进保险箱");

        let status = engine.oauth_status(id).expect("查状态");
        assert!(status.authorized, "该能读回令牌");
        assert!(status.has_refresh_token, "刷新令牌也该读回");
        assert_eq!(status.expires_at, Some(expires_at));
    }
    #[tokio::test]
    async fn 没授权过的账号状态是未授权() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets);
        let id = {
            let store = engine.store();
            store.insert_account(&oauth_draft(1, 1), None).expect("插入账号")
        };
        let status = engine.oauth_status(id).expect("查状态");
        assert!(!status.authorized);
        assert!(!status.has_refresh_token);
    }

    /// 只接受令牌认证的本地收发服务器，完成一次连接自检。
    async fn probe_server(smtp: bool) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("监听");
        let port = listener.local_addr().expect("端口").port();
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("连接");
            let mut reader = BufReader::new(socket);
            reader
                .get_mut()
                .write_all(if smtp { b"220 ready\r\n" } else { b"* OK ready\r\n" })
                .await
                .expect("欢迎");
            let mut authenticated = false;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).await.expect("命令") > 0);
                let response = if smtp {
                    if line.starts_with("EHLO ") {
                        "250-localhost\r\n250 AUTH XOAUTH2\r\n".to_string()
                    } else if line.starts_with("AUTH XOAUTH2 ") {
                        authenticated = true;
                        "235 authenticated\r\n".to_string()
                    } else {
                        assert!(line.starts_with("QUIT"), "仅测试认证与退出");
                        "221 bye\r\n".to_string()
                    }
                } else {
                    let (tag, command) = line.split_once(' ').expect("标记与命令");
                    if command.starts_with("AUTHENTICATE XOAUTH2 ") {
                        authenticated = true;
                        format!("{tag} OK authenticated\r\n")
                    } else if command.starts_with("CAPABILITY") {
                        format!("* CAPABILITY IMAP4rev1\r\n{tag} OK capability\r\n")
                    } else if command.starts_with("LIST ") {
                        format!("* LIST () \"/\" \"INBOX\"\r\n{tag} OK list\r\n")
                    } else {
                        assert!(command.starts_with("SELECT ") || command.starts_with("LOGOUT"));
                        format!("{tag} OK completed\r\n")
                    }
                };
                reader
                    .get_mut()
                    .write_all(response.as_bytes())
                    .await
                    .expect("应答");
                if line.contains("QUIT") || line.contains("LOGOUT") {
                    assert!(authenticated, "必须真正进行令牌认证");
                    break;
                }
            }
        });
        (port, task)
    }

    #[tokio::test]
    async fn 授权成功将配套密钥与令牌仅存保险箱并可重新授权沿用() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets.clone());
        let (imap_port, imap) = probe_server(false).await;
        let (smtp_port, smtp) = probe_server(true).await;
        let (token_port, received) = fake_token_server(
            r#"{"access_token":"access-success","refresh_token":"refresh-success","expires_in":3600}"#,
        )
        .await;
        let draft = oauth_draft(imap_port, smtp_port);
        let app_key = oauth_app_secret_key(ProviderKind::Gmail, &draft.oauth_client_id);
        secrets
            .set(&app_key, &Secret::new("success-client-secret"))
            .expect("本机应用密钥");
        let auth = engine
            .begin_oauth_authorize_at(
                &draft,
                None,
                None,
                Some(format!("http://127.0.0.1:{token_port}/token")),
            )
            .await
            .expect("开始授权");
        let callback = auth.redirect_uri.clone();
        let state = auth.state.clone();
        let browser = tokio::spawn(async move {
            let port: u16 = callback
                .trim_start_matches("http://127.0.0.1:")
                .split('/')
                .next()
                .expect("端口")
                .parse()
                .expect("数字");
            let mut stream = TcpStream::connect(("127.0.0.1", port)).await.expect("回调连接");
            stream.write_all(format!("GET /oauth/callback?code=success-code&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes()).await.expect("回调");
            let mut response = String::new();
            stream.read_to_string(&mut response).await.expect("回调网页");
            assert!(response.contains("授权信息已收到"));
            assert!(!response.contains("授权成功"));
        });
        let outcome = engine
            .complete_oauth_authorize(&auth.state)
            .await
            .expect("授权完成");
        browser.await.expect("浏览器");
        imap.await.expect("收件自检");
        smtp.await.expect("发件自检");
        assert_eq!(outcome.report.imap_folder_count, 1);
        assert_eq!(outcome.report.smtp_mechanism, "XOAUTH2");
        assert!(received
            .await
            .expect("令牌请求")
            .contains("client_secret=success-client-secret"));
        let key = outcome.account.credential_key.as_deref().expect("引用键");
        let stored = secrets.plain(key).expect("保险箱");
        let bundle = TokenBundle::decode(&stored).expect("令牌包");
        assert_eq!(bundle.client_secret.as_deref(), Some("success-client-secret"));
        assert_eq!(bundle.access_token, "access-success");
        assert_eq!(bundle.refresh_token.as_deref(), Some("refresh-success"));
        for entry in std::fs::read_dir(dir.path()).expect("数据库目录") {
            let path = entry.expect("目录项").path();
            if path.is_file() {
                let bytes = std::fs::read(path).expect("数据库文件");
                for value in ["success-client-secret", "access-success", "refresh-success"] {
                    assert!(
                        !bytes
                            .windows(value.len())
                            .any(|window| window == value.as_bytes()),
                        "凭据不能写库"
                    );
                }
            }
        }
        let retry = engine
            .begin_oauth_authorize(&draft, Some(outcome.account.id))
            .await
            .expect("沿用密钥");
        assert!(engine.cancel_oauth_authorize(&retry.state));
        assert_eq!(secrets.plain(key).as_deref(), Some(stored.as_str()));
    }

    #[tokio::test]
    async fn 授权收口时自检失败不留半成品() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets.clone());
        let (token_port, received) =
            fake_token_server(r#"{"access_token":"at-new","refresh_token":"rt-new","expires_in":3600}"#)
                .await;

        // 收件端口指向没人监听的 1，自检必然失败。
        let draft = oauth_draft(1, 1);
        let auth = engine
            .begin_oauth_authorize_at(
                &draft,
                None,
                Some(&Secret::new("test-client-secret")),
                Some(format!("http://127.0.0.1:{token_port}/token")),
            )
            .await
            .expect("应能发起授权");
        assert!(auth.authorize_url.contains("code_challenge_method=S256"));
        assert!(auth.redirect_uri.starts_with("http://127.0.0.1:"));

        // 模拟浏览器回调。
        let state = auth.state.clone();
        let port = auth
            .redirect_uri
            .trim_start_matches("http://127.0.0.1:")
            .split('/')
            .next()
            .expect("带端口")
            .parse::<u16>()
            .expect("端口是数字");
        tokio::spawn(async move {
            let mut client = TcpStream::connect(("127.0.0.1", port)).await.expect("连回调端口");
            let request =
                format!("GET /oauth/callback?code=abc123&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
            client.write_all(request.as_bytes()).await.expect("发回调");
        });

        let error = engine
            .complete_oauth_authorize(&auth.state)
            .await
            .expect_err("自检不过就该失败");
        assert!(
            matches!(error, EngineError::Connection(_)),
            "应是连接类错误：{error:?}"
        );
        assert!(received
            .await
            .expect("令牌请求")
            .contains("client_secret=test-client-secret"));
        assert!(secrets.is_empty(), "自检没过不该留下凭据");
        assert!(engine.list_accounts().expect("列账号").is_empty(), "不该留下账号");
    }

    #[tokio::test]
    async fn 取消授权后收口会报找不到() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets);
        let auth = engine
            .begin_oauth_authorize_with_secret(
                &oauth_draft(1, 1),
                None,
                Some(&Secret::new("test-client-secret")),
            )
            .await
            .expect("应能发起授权");
        assert!(engine.cancel_oauth_authorize(&auth.state));
        let error = engine
            .complete_oauth_authorize(&auth.state)
            .await
            .expect_err("取消后不该还能收口");
        assert!(matches!(error, EngineError::BadRequest(_)), "{error:?}");
    }

    #[tokio::test]
    async fn 不填编号时用内置编号或给可读错误() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets);
        let mut draft = oauth_draft(1, 1);
        draft.oauth_provider = Some(OAuthProvider::Microsoft);
        draft.oauth_client_id = String::new();

        match engine.begin_oauth_authorize(&draft, None).await {
            // 本机配了内置编号：应该照常发起，且授权地址里带上这个编号。
            Ok(auth) => {
                assert!(auth.authorize_url.contains("client_id="), "授权地址该带编号");
                assert!(auth.redirect_uri.starts_with("http://127.0.0.1:"));
            }
            // 本机没配内置编号：应该给一句能看懂的提示，并指明去哪儿填。
            Err(error) => {
                let text = error.to_string();
                assert!(text.contains("登录编号"), "应提示登录编号：{text}");
                assert!(text.contains("高级设置"), "应指明去哪儿填：{text}");
            }
        }
    }

    #[tokio::test]
    async fn 密码账号不能走oauth授权() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets);
        let mut draft = oauth_draft(1, 1);
        draft.auth_type = AuthType::Password;
        draft.oauth_provider = None;
        draft.oauth_client_id = String::new();
        let error = engine
            .begin_oauth_authorize(&draft, None)
            .await
            .expect_err("密码账号不该走 OAuth");
        assert!(matches!(error, EngineError::BadRequest(_)), "{error:?}");
    }

    #[tokio::test]
    async fn 令牌过期会用刷新令牌换新的并写回保险箱() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets.clone());
        let (token_port, received) =
            fake_token_server(r#"{"access_token":"at-fresh","expires_in":3600}"#).await;

        let expired = TokenBundle {
            access_token: "at-old".to_string(),
            client_secret: Some("refresh-client-secret".to_string()),
            client_binding: Some(ClientBinding {
                provider: "gmail".to_string(),
                client_id: "client-test".to_string(),
            }),
            refresh_token: Some("rt-old".to_string()),
            expires_at: Some(now_unix() - 10),
            scope: None,
        };
        let key = "account/test-refresh";
        secrets
            .set(key, &Secret::new(expired.encode()))
            .expect("写保险箱");
        let id = {
            let store = engine.store();
            store
                .insert_account(&oauth_draft(1, 1), Some(key))
                .expect("插入账号")
        };
        let account = engine.get_account(id).expect("取账号");

        let secret = active_secret_with(
            &engine.store,
            secrets.as_ref(),
            &account,
            Some(&format!("http://127.0.0.1:{token_port}/token")),
        )
        .await
        .expect("应能刷新");
        assert_eq!(secret.expose(), "at-fresh");
        assert!(received
            .await
            .expect("刷新请求")
            .contains("client_secret=refresh-client-secret"));

        let stored = secrets.plain(key).expect("应有凭据");
        let bundle = TokenBundle::decode(&stored).expect("应是令牌包");
        assert_eq!(bundle.access_token, "at-fresh");
        assert_eq!(bundle.client_secret.as_deref(), Some("refresh-client-secret"));
        assert_eq!(
            bundle.client_binding, expired.client_binding,
            "刷新保留原密钥绑定"
        );
        assert_eq!(
            bundle.refresh_token.as_deref(),
            Some("rt-old"),
            "刷新没返回新刷新令牌就沿用旧的"
        );
    }

    #[tokio::test]
    async fn 旧谷歌账号续期自动读取本机应用密钥并保存绑定() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets.clone());
        let draft = oauth_draft(1, 1);
        let app_key = oauth_app_secret_key(ProviderKind::Gmail, &draft.oauth_client_id);
        secrets
            .set(&app_key, &Secret::new("legacy-app-secret"))
            .expect("应用密钥");
        let key = "account/legacy-app-refresh";
        let legacy = TokenBundle::from_tokens(TokenSet {
            access_token: "old-access".to_string(),
            refresh_token: Some("old-refresh".to_string()),
            expires_at: Some(now_unix() - 10),
            scope: None,
        });
        secrets.set(key, &Secret::new(legacy.encode())).expect("旧令牌包");
        let id = engine.store().insert_account(&draft, Some(key)).expect("账号");
        let account = engine.get_account(id).expect("账号");
        let (port, received) = fake_token_server(r#"{"access_token":"new-access","expires_in":3600}"#).await;
        let access = active_secret_with(
            &engine.store,
            secrets.as_ref(),
            &account,
            Some(&format!("http://127.0.0.1:{port}/token")),
        )
        .await
        .expect("旧账号续期");
        assert_eq!(access.expose(), "new-access");
        assert!(received
            .await
            .expect("续期请求")
            .contains("client_secret=legacy-app-secret"));
        let bundle = TokenBundle::decode(&secrets.plain(key).expect("保险箱")).expect("令牌包");
        assert_eq!(bundle.client_secret.as_deref(), Some("legacy-app-secret"));
        assert!(bundle.matches_client(ProviderKind::Gmail, &draft.oauth_client_id));
        assert_eq!(secrets.plain(&app_key).as_deref(), Some("legacy-app-secret"));
    }

    #[tokio::test]
    async fn 账号配置变更后刷新不得向其他客户端或服务商发送旧密钥() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("本地端点");
        let endpoint = format!(
            "http://127.0.0.1:{}/token",
            listener.local_addr().expect("端口").port()
        );
        let key = "account/bound-client";
        let mut bundle = TokenBundle::from_tokens(TokenSet {
            access_token: "bound-access".to_string(),
            refresh_token: Some("bound-refresh".to_string()),
            expires_at: Some(now_unix() - 10),
            scope: None,
        });
        bundle.client_secret = Some("bound-secret".to_string());
        bundle.client_binding = Some(ClientBinding {
            provider: "gmail".to_string(),
            client_id: "client-test".to_string(),
        });
        secrets.set(key, &Secret::new(bundle.encode())).expect("保险箱");
        let original = oauth_draft(1, 1);
        let id = engine.store().insert_account(&original, Some(key)).expect("账号");
        for (provider, client_id) in [
            (OAuthProvider::Gmail, "changed-client"),
            (OAuthProvider::Microsoft, "client-test"),
        ] {
            let mut changed = original.clone();
            changed.oauth_provider = Some(provider);
            changed.oauth_client_id = client_id.to_string();
            engine
                .store()
                .update_account(id, &changed, Some(key))
                .expect("模拟配置变更");
            let account = engine.get_account(id).expect("账号");
            let error = active_secret_with(&engine.store, secrets.as_ref(), &account, Some(&endpoint))
                .await
                .expect_err("应重新授权");
            assert_eq!(error, ResolveError::ReauthRequired);
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err(),
            "不应发出任何请求"
        );
        assert_eq!(
            secrets.plain(key).as_deref(),
            Some(bundle.encode().as_str()),
            "失败不修改原凭据"
        );
    }

    #[tokio::test]
    async fn 刷新被拒会冒重新授权() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = engine(dir.path(), secrets.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定成功");
        let port = listener.local_addr().expect("取地址").port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接");
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await.expect("读请求");
            let payload = r#"{"error":"invalid_grant"}"#;
            let response = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(response.as_bytes()).await.expect("写响应");
        });

        let expired = TokenBundle {
            access_token: "at-old".to_string(),
            client_secret: Some("refresh-client-secret".to_string()),
            client_binding: Some(ClientBinding {
                provider: "gmail".to_string(),
                client_id: "client-test".to_string(),
            }),
            refresh_token: Some("rt-revoked".to_string()),
            expires_at: Some(now_unix() - 10),
            scope: None,
        };
        let key = "account/test-reauth";
        secrets
            .set(key, &Secret::new(expired.encode()))
            .expect("写保险箱");
        let id = {
            let store = engine.store();
            store
                .insert_account(&oauth_draft(1, 1), Some(key))
                .expect("插入账号")
        };
        let account = engine.get_account(id).expect("取账号");

        let error = active_secret_with(
            &engine.store,
            secrets.as_ref(),
            &account,
            Some(&format!("http://127.0.0.1:{port}/token")),
        )
        .await
        .expect_err("应要重新授权");
        assert_eq!(error, ResolveError::ReauthRequired);
    }
}
