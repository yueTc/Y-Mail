//! GitHub 登录状态的落盘编排。
//!
//! 规矩（规格 3.7 / 3.8）：
//! - 访问令牌只进系统保险箱，键固定 `sync/github-token`；数据库里只允许出现这个引用键。
//! - 登录名 / 昵称 / 头像地址是公开信息，写 `setting` 表，界面直接读，不必每次请求网络。
//! - 先写保险箱再写库；写库失败要把保险箱回滚成写之前的样子，避免「有令牌没资料」的半登录态。
//! - 退出登录两处一起清。

use std::collections::{BTreeMap, HashMap};

use mail_domain::account::{AccountDraft, AccountProxyMode, AuthType, OAuthProvider, Security, ServerConfig};
use mail_domain::proxy::{GlobalProxyMode, Secret};
use mail_store::{
    AccountImport, AiFunction, AiModelMapImport, AiProviderImport, AiProviderKind, AiThinkingLevel,
    GlobalProxyImport, NewAiProvider, ProxyImport, SettingsImportPlan, SettingsImportValues, SignatureImport,
    SETTING_BLOCK_REMOTE_IMAGES, SETTING_MINIMIZE_TO_TRAY, SETTING_NOTIFY_AI_ENABLED,
    SETTING_NOTIFY_NEW_MAIL, SETTING_START_MINIMIZED,
};
use mail_sync::github::GitHubProfile;
use serde::{Deserialize, Serialize};

use crate::engine::{EngineError, MailEngine};
use crate::proxies::new_credential_key;

/// GitHub 访问令牌在保险箱里的键（规格 3.8 固定，不许改）。
pub const GITHUB_TOKEN_KEY: &str = "sync/github-token";

/// 登录名在 `setting` 表里的键。
pub const GITHUB_LOGIN_KEY: &str = "sync.github-login";
/// 昵称在 `setting` 表里的键。
pub const GITHUB_NAME_KEY: &str = "sync.github-name";
/// 头像地址在 `setting` 表里的键。
pub const GITHUB_AVATAR_KEY: &str = "sync.github-avatar";

/// 界面要的一份登录信息（不含令牌）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubLoginView {
    /// 登录名（账号名）。
    pub login: String,
    /// 昵称；用户没设就是空。
    pub name: Option<String>,
    /// 头像地址；用户没设就是空。
    pub avatar_url: Option<String>,
}

impl GitHubLoginView {
    /// 展示名：有昵称用昵称，没有就退回登录名。
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&self.login)
    }
}

impl MailEngine {
    /// 登录成功后落盘：令牌进保险箱，资料进库。
    ///
    /// 先写保险箱、再写库；写库中途失败会把保险箱恢复成写之前的样子。
    pub fn save_github_login(
        &self,
        token: &Secret,
        profile: &GitHubProfile,
    ) -> Result<GitHubLoginView, EngineError> {
        if token.is_empty() {
            return Err(EngineError::BadRequest("登录令牌为空".to_string()));
        }
        if !profile.is_authenticated() {
            return Err(EngineError::BadRequest("没拿到 GitHub 登录名".to_string()));
        }

        let view = GitHubLoginView {
            login: profile.login.trim().to_string(),
            name: text_field(profile.name.as_deref()),
            avatar_url: text_field(profile.avatar_url.as_deref()),
        };

        let previous = self.secrets().get(GITHUB_TOKEN_KEY)?;
        self.secrets().set(GITHUB_TOKEN_KEY, token)?;
        match self.write_login_settings(&view) {
            Ok(()) => Ok(view),
            Err(err) => {
                // 回滚：有旧令牌就还原，没有就删掉，别留半登录态。
                match previous {
                    Some(old) => {
                        let _ = self.secrets().set(GITHUB_TOKEN_KEY, &old);
                    }
                    None => {
                        let _ = self.secrets().delete(GITHUB_TOKEN_KEY);
                    }
                }
                Err(err)
            }
        }
    }

    /// 读本机已存的登录信息；没登录返回 `None`（只读库，不碰网络）。
    pub fn github_login(&self) -> Result<Option<GitHubLoginView>, EngineError> {
        let (login, name, avatar) = {
            let guard = self.store();
            (
                guard.get_setting(GITHUB_LOGIN_KEY)?,
                guard.get_setting(GITHUB_NAME_KEY)?,
                guard.get_setting(GITHUB_AVATAR_KEY)?,
            )
        };
        let Some(login) = non_empty(login) else {
            return Ok(None);
        };
        Ok(Some(GitHubLoginView {
            login,
            name: non_empty(name),
            avatar_url: non_empty(avatar),
        }))
    }

    /// 取保险箱里的 GitHub 令牌；没登录返回 `None`。
    pub fn github_token(&self) -> Result<Option<Secret>, EngineError> {
        Ok(self.secrets().get(GITHUB_TOKEN_KEY)?)
    }

    /// 退出登录：清保险箱令牌，清库里的资料行（规格 R2）。
    pub fn sign_out_github(&self) -> Result<(), EngineError> {
        self.secrets().delete(GITHUB_TOKEN_KEY)?;
        let guard = self.store();
        guard.set_setting(GITHUB_LOGIN_KEY, "")?;
        guard.set_setting(GITHUB_NAME_KEY, "")?;
        guard.set_setting(GITHUB_AVATAR_KEY, "")?;
        Ok(())
    }

    /// 把资料写进 `setting` 表；任一条失败由调用方回滚保险箱。
    fn write_login_settings(&self, view: &GitHubLoginView) -> Result<(), EngineError> {
        let guard = self.store();
        guard.set_setting(GITHUB_LOGIN_KEY, &view.login)?;
        guard.set_setting(GITHUB_NAME_KEY, view.name.as_deref().unwrap_or(""))?;
        guard.set_setting(GITHUB_AVATAR_KEY, view.avatar_url.as_deref().unwrap_or(""))?;
        Ok(())
    }
}

/// 去掉首尾空白；清空后算没设。
fn text_field(value: Option<&str>) -> Option<String> {
    non_empty(value.map(str::to_string))
}

/// 空串（或只有空白）当作没设。
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

/// 同步快照的结构版本号（规格 3.4）。
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// 账号逻辑键：邮箱地址，比较时不分大小写（规格 3.4）。
pub fn account_logical_key(email: &str) -> String {
    format!("account:{}", email.trim().to_ascii_lowercase())
}

/// 代理逻辑键：标签 + 主机 + 端口（规格 3.4）。
pub fn proxy_logical_key(label: &str, host: &str, port: u16) -> String {
    format!("proxy:{}|{}|{}", label.trim(), host.trim(), port)
}

/// AI 站点逻辑键：标签（规格 3.4）。
pub fn ai_logical_key(label: &str) -> String {
    format!("ai:{}", label.trim())
}

/// 加密前的明文快照（规格 3.4）。
///
/// 可以含密钥本体，只在内存里存在；绝不直接落库、绝不进日志。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    /// 结构版本号。
    pub schema_version: u32,
    /// 本机随机标识。
    pub device_id: String,
    /// 本机可读名。
    pub device_label: String,
    /// 版本号；每次上传加一。
    pub revision: u64,
    /// 生成时间（UTC）。
    pub updated_at: String,
    /// 界面 / 行为开关。
    pub settings: SnapshotSettings,
    /// 邮箱账号。
    pub accounts: Vec<SnapshotAccount>,
    /// 代理条目。
    pub proxies: Vec<SnapshotProxy>,
    /// 全局代理。
    pub global_proxy: SnapshotGlobalProxy,
    /// 签名。
    pub signatures: Vec<SnapshotSignature>,
    /// AI 站点。
    pub ai_providers: Vec<SnapshotAiProvider>,
    /// AI 功能映射。
    pub ai_model_map: Vec<SnapshotAiModelMap>,
    /// 密钥字典：逻辑键 → 密钥明文，只在内存里。
    pub secrets: BTreeMap<String, String>,
}

/// 只进包的那几个界面 / 行为开关（规格 3.2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSettings {
    /// 新邮件通知。
    pub notify_new_mail: bool,
    /// 新邮件 AI 智能识别通知。
    pub notify_ai_enabled: bool,
    /// 默认拦截远程图片。
    pub block_remote_images_by_default: bool,
    /// 关闭主窗口时最小化到托盘。
    pub minimize_to_tray_on_close: bool,
    /// 启动时最小化到托盘。
    pub start_minimized_to_tray: bool,
}

/// 一台服务器的地址、端口与加密方式。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotServer {
    /// 服务器主机名。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 加密方式：`tls` / `starttls` / `plain`。
    pub security: String,
}

/// 一个账号（不含凭据列）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotAccount {
    /// 邮箱地址。
    pub email: String,
    /// 显示名。
    pub display_name: String,
    /// 认证方式：`password` / `oauth2`。
    pub auth_type: String,
    /// 登录名。
    pub username: String,
    /// 收件服务器。
    pub imap: SnapshotServer,
    /// 发件服务器。
    pub smtp: SnapshotServer,
    /// 账号级代理策略。
    pub proxy: SnapshotProxyRef,
    /// 界面色标。
    pub color: String,
    /// 是否启用。
    pub enabled: bool,
    /// OAuth2 服务商：`gmail` / `microsoft`。
    pub oauth_provider: Option<String>,
    /// OAuth2 客户端编号。
    pub oauth_client_id: String,
}

/// 账号级代理策略的对外表示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotProxyRef {
    /// 跟随全局。
    Inherit,
    /// 强制直连。
    Direct,
    /// 指定代理（值为代理逻辑键）。
    Custom(String),
}

/// 一个代理条目（含逻辑键，不含密码）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotProxy {
    /// 逻辑键：标签 + 主机 + 端口。
    pub key: String,
    /// 标签。
    pub label: String,
    /// 类型：`socks5` / `http`。
    pub kind: String,
    /// 主机。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 登录名，可空。
    pub username: String,
}

/// 全局代理策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotGlobalProxy {
    /// 跟随系统。
    System,
    /// 直连。
    Direct,
    /// 使用指定代理（值为代理逻辑键）。
    Custom(String),
}

/// 一个账号的签名。
///
/// 规格说「签名按名称找」，但当前 `signature` 表只有 `account_id` 主键、没有名称列，
/// 因此实际按账号邮箱关联；等表结构补上名称列再改。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSignature {
    /// 所属账号邮箱。
    pub account_email: String,
    /// 签名 HTML。
    pub html: String,
    /// 是否启用。
    pub enabled: bool,
}

/// 一个 AI 站点（不含密钥本体）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotAiProvider {
    /// 逻辑键：标签。
    pub key: String,
    /// 界面显示名。
    pub label: String,
    /// 站点类型。
    pub kind: AiProviderKind,
    /// 站点根地址。
    pub base_url: String,
    /// 站点默认模型。
    pub default_model: String,
    /// 模型列表。
    pub models: Vec<String>,
    /// 站点默认思考程度。
    pub thinking_level: AiThinkingLevel,
    /// 是否启用。
    pub enabled: bool,
}

/// 一个功能模型映射。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotAiModelMap {
    /// 功能。
    pub function: AiFunction,
    /// 站点逻辑键（标签）。
    pub provider_key: String,
    /// 功能级模型；空串表示回退站点默认。
    pub model: String,
    /// 功能级思考程度；`None` 表示回退站点默认。
    pub thinking_level: Option<AiThinkingLevel>,
}

impl MailEngine {
    /// 导出：把本机配置拼成一份明文快照（含密钥本体，只在内存里）。
    ///
    /// `device_id` / `device_label` / `revision` 由同步编排（Wave S5）给出；
    /// 本函数只负责把数据库与保险箱里的配置读出来拼成结构。
    pub fn export_settings_snapshot(
        &self,
        device_id: impl Into<String>,
        device_label: impl Into<String>,
        revision: u64,
    ) -> Result<SettingsSnapshot, EngineError> {
        let (accounts, proxies, global_mode, providers, model_map, signatures, settings, updated_at) = {
            let guard = self.store();
            (
                guard.list_accounts()?,
                guard.list_proxies()?,
                guard.global_proxy_mode()?,
                guard.list_ai_providers()?,
                guard.list_ai_model_maps()?,
                guard.list_signatures()?,
                read_snapshot_settings(&guard)?,
                guard.now_utc()?,
            )
        };

        // 代理编号 → 逻辑键，供账号与全局代理换算。
        let mut proxy_keys: HashMap<i64, String> = HashMap::new();
        let mut snapshot_proxies = Vec::with_capacity(proxies.len());
        for stored in &proxies {
            let key = proxy_logical_key(&stored.config.label, &stored.config.host, stored.config.port);
            if let Some(id) = stored.config.id {
                proxy_keys.insert(id.0, key.clone());
            }
            snapshot_proxies.push(SnapshotProxy {
                key,
                label: stored.config.label.clone(),
                kind: stored.config.kind.as_str().to_string(),
                host: stored.config.host.clone(),
                port: stored.config.port,
                username: stored.config.username.clone(),
            });
        }

        // 密钥只在内存里过一手，绝不落库、绝不进日志。
        let mut secrets: BTreeMap<String, String> = BTreeMap::new();

        let mut snapshot_accounts = Vec::with_capacity(accounts.len());
        for account in &accounts {
            let logical = account_logical_key(&account.email);
            if let Some(value) = read_secret(self, account.credential_key.as_deref())? {
                secrets.insert(logical, value);
            }
            let proxy = match account.proxy {
                AccountProxyMode::InheritGlobal => SnapshotProxyRef::Inherit,
                AccountProxyMode::Direct => SnapshotProxyRef::Direct,
                AccountProxyMode::Custom(id) => match proxy_keys.get(&id.0) {
                    Some(key) => SnapshotProxyRef::Custom(key.clone()),
                    None => SnapshotProxyRef::Inherit,
                },
            };
            snapshot_accounts.push(SnapshotAccount {
                email: account.email.clone(),
                display_name: account.display_name.clone(),
                auth_type: account.auth_type.as_str().to_string(),
                username: account.username.clone(),
                imap: SnapshotServer {
                    host: account.imap.host.clone(),
                    port: account.imap.port,
                    security: account.imap.security.as_str().to_string(),
                },
                smtp: SnapshotServer {
                    host: account.smtp.host.clone(),
                    port: account.smtp.port,
                    security: account.smtp.security.as_str().to_string(),
                },
                proxy,
                color: account.color.clone(),
                enabled: account.enabled,
                oauth_provider: account
                    .oauth_provider
                    .map(|provider| provider.as_str().to_string()),
                oauth_client_id: account.oauth_client_id.clone(),
            });
        }

        for stored in &proxies {
            let logical = proxy_logical_key(&stored.config.label, &stored.config.host, stored.config.port);
            if let Some(value) = read_secret(self, stored.password_key.as_deref())? {
                secrets.insert(logical, value);
            }
        }

        let global_proxy = match global_mode {
            GlobalProxyMode::System => SnapshotGlobalProxy::System,
            GlobalProxyMode::Direct => SnapshotGlobalProxy::Direct,
            GlobalProxyMode::Custom(id) => match proxy_keys.get(&id.0) {
                Some(key) => SnapshotGlobalProxy::Custom(key.clone()),
                None => SnapshotGlobalProxy::System,
            },
        };

        // 签名只导出有记录的；按账号邮箱关联。
        let account_emails: HashMap<i64, String> =
            accounts.iter().map(|a| (a.id.0, a.email.clone())).collect();
        let mut snapshot_signatures = Vec::new();
        for signature in &signatures {
            if let Some(email) = account_emails.get(&signature.account_id) {
                snapshot_signatures.push(SnapshotSignature {
                    account_email: email.clone(),
                    html: signature.html.clone(),
                    enabled: signature.enabled,
                });
            }
        }

        // AI 站点编号 → 逻辑键，供功能映射换算。
        let mut provider_keys: HashMap<i64, String> = HashMap::new();
        let mut snapshot_providers = Vec::with_capacity(providers.len());
        for provider in &providers {
            let key = ai_logical_key(&provider.label);
            if let Some(value) = read_secret(self, provider.api_key_ref.as_deref())? {
                secrets.insert(key.clone(), value);
            }
            provider_keys.insert(provider.id, key.clone());
            snapshot_providers.push(SnapshotAiProvider {
                key,
                label: provider.label.clone(),
                kind: provider.kind,
                base_url: provider.base_url.clone(),
                default_model: provider.default_model.clone(),
                models: provider.models.clone(),
                thinking_level: provider.thinking_level,
                enabled: provider.enabled,
            });
        }

        let mut snapshot_model_map = Vec::with_capacity(model_map.len());
        for entry in &model_map {
            if let Some(key) = provider_keys.get(&entry.provider_id) {
                snapshot_model_map.push(SnapshotAiModelMap {
                    function: entry.function,
                    provider_key: key.clone(),
                    model: entry.model.clone(),
                    thinking_level: entry.thinking_level,
                });
            }
        }

        Ok(SettingsSnapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            device_id: device_id.into(),
            device_label: device_label.into(),
            revision,
            updated_at,
            settings,
            accounts: snapshot_accounts,
            proxies: snapshot_proxies,
            global_proxy,
            signatures: snapshot_signatures,
            ai_providers: snapshot_providers,
            ai_model_map: snapshot_model_map,
            secrets,
        })
    }

    /// 导入：按逻辑键匹配本机已有条目，命中就更新、没命中就新建。
    ///
    /// 顺序：先把密钥写回保险箱，再让存储层用一个事务写库；
    /// 任一步失败都把保险箱恢复成写之前的样子，避免「有密钥没配置」或反之。
    pub fn import_settings_snapshot(&self, snapshot: &SettingsSnapshot) -> Result<(), EngineError> {
        if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
            return Err(EngineError::BadRequest(format!(
                "同步包格式版本不支持：{}",
                snapshot.schema_version
            )));
        }

        let mut plan = build_import_plan(snapshot);

        // 先写保险箱，记下旧值以便回滚。
        let mut written: Vec<(String, Option<Secret>)> = Vec::new();
        for (logical, prefix, seed, plain) in secret_writes(snapshot) {
            let key = new_credential_key(prefix, &seed);
            let old = match self.secrets().get(&key) {
                Ok(value) => value,
                Err(err) => {
                    restore_secrets(self, &written);
                    return Err(err.into());
                }
            };
            if let Err(err) = self.secrets().set(&key, &Secret::new(plain)) {
                restore_secrets(self, &written);
                return Err(err.into());
            }
            written.push((key.clone(), old));
            plan.secrets.insert(logical, Some(key));
        }

        match self.store().apply_settings_import(&plan) {
            Ok(()) => Ok(()),
            Err(err) => {
                restore_secrets(self, &written);
                Err(err.into())
            }
        }
    }
}

/// 从 `setting` 表读只进包的那几个开关；缺键按默认值处理。
fn read_snapshot_settings(store: &mail_store::Store) -> Result<SnapshotSettings, EngineError> {
    Ok(SnapshotSettings {
        notify_new_mail: setting_bool(store.get_setting(SETTING_NOTIFY_NEW_MAIL)?, true),
        notify_ai_enabled: setting_bool(store.get_setting(SETTING_NOTIFY_AI_ENABLED)?, false),
        block_remote_images_by_default: setting_bool(store.get_setting(SETTING_BLOCK_REMOTE_IMAGES)?, true),
        minimize_to_tray_on_close: setting_bool(store.get_setting(SETTING_MINIMIZE_TO_TRAY)?, true),
        start_minimized_to_tray: setting_bool(store.get_setting(SETTING_START_MINIMIZED)?, false),
    })
}

/// 把 `setting` 表里的文本按布尔解释；缺省或认不出来时用默认值。
fn setting_bool(value: Option<String>, default: bool) -> bool {
    match value.as_deref().map(str::trim) {
        Some("true") | Some("1") => true,
        Some("false") | Some("0") => false,
        _ => default,
    }
}

/// 从保险箱按引用键取明文；键为空或不存在返回 `None`。
fn read_secret(engine: &MailEngine, key: Option<&str>) -> Result<Option<String>, EngineError> {
    match key {
        Some(key) if !key.is_empty() => Ok(engine
            .secrets()
            .get(key)?
            .map(|secret| secret.expose().to_string())),
        _ => Ok(None),
    }
}

/// 把快照里的配置部分转成存储层的导入计划（密钥引用留空，稍后填）。
fn build_import_plan(snapshot: &SettingsSnapshot) -> SettingsImportPlan {
    let proxies = snapshot
        .proxies
        .iter()
        .map(|proxy| ProxyImport {
            key: proxy_logical_key(&proxy.label, &proxy.host, proxy.port),
            label: proxy.label.clone(),
            kind: proxy.kind.clone(),
            host: proxy.host.clone(),
            port: proxy.port,
            username: proxy.username.clone(),
        })
        .collect();

    let accounts = snapshot
        .accounts
        .iter()
        .map(|account| AccountImport {
            draft: draft_from_snapshot(account),
            proxy_key: match &account.proxy {
                SnapshotProxyRef::Custom(key) => Some(key.clone()),
                _ => None,
            },
        })
        .collect();

    let global_proxy = match &snapshot.global_proxy {
        SnapshotGlobalProxy::System => GlobalProxyImport::System,
        SnapshotGlobalProxy::Direct => GlobalProxyImport::Direct,
        SnapshotGlobalProxy::Custom(key) => GlobalProxyImport::Custom(key.clone()),
    };

    let signatures = snapshot
        .signatures
        .iter()
        .map(|signature| SignatureImport {
            account_email: signature.account_email.clone(),
            html: signature.html.clone(),
            enabled: signature.enabled,
        })
        .collect();

    let ai_providers = snapshot
        .ai_providers
        .iter()
        .map(|provider| AiProviderImport {
            key: provider.key.clone(),
            provider: NewAiProvider {
                label: provider.label.clone(),
                kind: provider.kind,
                base_url: provider.base_url.clone(),
                default_model: provider.default_model.clone(),
                models: provider.models.clone(),
                thinking_level: provider.thinking_level,
                enabled: provider.enabled,
            },
        })
        .collect();

    let ai_model_map = snapshot
        .ai_model_map
        .iter()
        .map(|entry| AiModelMapImport {
            function: entry.function,
            provider_key: entry.provider_key.clone(),
            model: entry.model.clone(),
            thinking_level: entry.thinking_level,
        })
        .collect();

    SettingsImportPlan {
        proxies,
        accounts,
        global_proxy,
        signatures,
        ai_providers,
        ai_model_map,
        settings: SettingsImportValues {
            notify_new_mail: snapshot.settings.notify_new_mail,
            notify_ai_enabled: snapshot.settings.notify_ai_enabled,
            block_remote_images_by_default: snapshot.settings.block_remote_images_by_default,
            minimize_to_tray_on_close: snapshot.settings.minimize_to_tray_on_close,
            start_minimized_to_tray: snapshot.settings.start_minimized_to_tray,
        },
        secrets: HashMap::new(),
    }
}

/// 把快照里的账号还原成新建 / 更新用的草稿（不含凭据）。
fn draft_from_snapshot(account: &SnapshotAccount) -> AccountDraft {
    AccountDraft {
        display_name: account.display_name.clone(),
        email: account.email.clone(),
        auth_type: AuthType::parse(&account.auth_type).unwrap_or(AuthType::Password),
        username: account.username.clone(),
        imap: server_from_snapshot(&account.imap),
        smtp: server_from_snapshot(&account.smtp),
        // 具体代理引用由导入计划里的 `proxy_key` 决定，这里占位即可。
        proxy: AccountProxyMode::InheritGlobal,
        color: account.color.clone(),
        enabled: account.enabled,
        oauth_provider: account.oauth_provider.as_deref().and_then(OAuthProvider::parse),
        oauth_client_id: account.oauth_client_id.clone(),
    }
}

/// 把快照里的服务器还原成领域类型。
fn server_from_snapshot(server: &SnapshotServer) -> ServerConfig {
    ServerConfig {
        host: server.host.clone(),
        port: server.port,
        security: Security::parse(&server.security).unwrap_or(Security::Tls),
    }
}

/// 快照里要写回保险箱的密钥：逻辑键、前缀、种子、明文。
fn secret_writes(snapshot: &SettingsSnapshot) -> Vec<(String, &'static str, String, String)> {
    let mut out = Vec::new();
    for account in &snapshot.accounts {
        let logical = account_logical_key(&account.email);
        if let Some(plain) = snapshot.secrets.get(&logical) {
            out.push((logical, "account", account.email.clone(), plain.clone()));
        }
    }
    for proxy in &snapshot.proxies {
        let logical = proxy_logical_key(&proxy.label, &proxy.host, proxy.port);
        if let Some(plain) = snapshot.secrets.get(&logical) {
            out.push((
                logical,
                "proxy",
                format!("{}:{}", proxy.host, proxy.port),
                plain.clone(),
            ));
        }
    }
    for provider in &snapshot.ai_providers {
        let logical = ai_logical_key(&provider.label);
        if let Some(plain) = snapshot.secrets.get(&logical) {
            out.push((logical, "ai", provider.label.clone(), plain.clone()));
        }
    }
    out
}

/// 把保险箱恢复成写之前的样子；回滚失败只记调试日志，不覆盖原始错误。
fn restore_secrets(engine: &MailEngine, written: &[(String, Option<Secret>)]) {
    for (key, old) in written.iter().rev() {
        let result = match old {
            Some(value) => engine.secrets().set(key, value),
            None => engine.secrets().delete(key),
        };
        if let Err(err) = result {
            tracing::debug!(error = %err, "回滚保险箱条目失败");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mail_domain::proxy::Secret;
    use mail_sync::github::GitHubProfile;

    use mail_domain::account::{AccountDraft, AccountProxyMode, AuthType, Security, ServerConfig};
    use mail_domain::proxy::{GlobalProxyMode, ProxyConfig, ProxyKind};
    use mail_store::{AiFunction, AiProviderKind, AiThinkingLevel, NewAiProvider, SETTING_NOTIFY_NEW_MAIL};

    use crate::engine::MailEngine;
    use crate::secrets::{MemorySecretStore, SecretStore};

    use super::{
        account_logical_key, ai_logical_key, proxy_logical_key, SettingsSnapshot, GITHUB_TOKEN_KEY,
        SNAPSHOT_SCHEMA_VERSION,
    };

    fn engine(dir: &std::path::Path) -> (MailEngine, Arc<MemorySecretStore>) {
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = MailEngine::initialize_with_secrets(dir, secrets.clone()).expect("初始化引擎");
        (engine, secrets)
    }

    fn profile() -> GitHubProfile {
        GitHubProfile {
            login: "octocat".to_string(),
            name: Some("八爪猫".to_string()),
            avatar_url: Some("https://avatars.githubusercontent.com/u/1".to_string()),
        }
    }

    #[test]
    fn 登录后能取到资料与令牌() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());

        let view = engine
            .save_github_login(&Secret::new("gho-token-123"), &profile())
            .expect("保存登录");
        assert_eq!(view.login, "octocat");
        assert_eq!(view.display_name(), "八爪猫");

        let loaded = engine.github_login().expect("读资料").expect("应已登录");
        assert_eq!(loaded, view);
        assert_eq!(
            loaded.avatar_url.as_deref(),
            Some("https://avatars.githubusercontent.com/u/1")
        );
        assert_eq!(
            engine.github_token().expect("读令牌").expect("应有令牌").expose(),
            "gho-token-123"
        );
        assert!(secrets.contains(GITHUB_TOKEN_KEY), "令牌该在保险箱里");
    }

    #[test]
    fn 退出登录后令牌和资料都清掉() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());
        engine
            .save_github_login(&Secret::new("gho-token-456"), &profile())
            .expect("保存登录");

        engine.sign_out_github().expect("退出登录");

        assert!(engine.github_login().expect("读资料").is_none(), "资料该清掉");
        assert!(engine.github_token().expect("读令牌").is_none(), "令牌该清掉");
        assert!(!secrets.contains(GITHUB_TOKEN_KEY), "保险箱里不该还有令牌");
    }

    #[test]
    fn 缺昵称时显示名退回登录名() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _secrets) = engine(dir.path());
        let mut sparse = profile();
        sparse.name = None;
        sparse.avatar_url = None;

        let view = engine
            .save_github_login(&Secret::new("gho-token-789"), &sparse)
            .expect("保存登录");

        assert_eq!(view.display_name(), "octocat");
        assert!(view.name.is_none(), "没昵称就该是空");
        assert!(view.avatar_url.is_none(), "没头像就该是空");
        let loaded = engine.github_login().expect("读资料").expect("应已登录");
        assert_eq!(loaded, view);
    }

    #[test]
    fn 空令牌或空登录名会被挡下() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());

        assert!(engine.save_github_login(&Secret::new(""), &profile()).is_err());
        let mut blank = profile();
        blank.login = "   ".to_string();
        assert!(engine
            .save_github_login(&Secret::new("gho-token-000"), &blank)
            .is_err());
        assert!(secrets.is_empty(), "被挡下的登录不该往保险箱写东西");
        assert!(engine.github_login().expect("读资料").is_none());
    }

    #[test]
    fn 数据库文件里查不到明文令牌() {
        let dir = tempfile::tempdir().expect("临时目录");
        let exposed = "gho-very-secret-token-9527";
        {
            let (engine, _secrets) = engine(dir.path());
            engine
                .save_github_login(&Secret::new(exposed), &profile())
                .expect("保存登录");
        }

        let mut found = false;
        for entry in std::fs::read_dir(dir.path()).expect("列目录") {
            let path = entry.expect("目录项").path();
            if !path.is_file() {
                continue;
            }
            let bytes = std::fs::read(&path).expect("读文件");
            if bytes
                .windows(exposed.len())
                .any(|window| window == exposed.as_bytes())
            {
                found = true;
            }
        }
        assert!(!found, "任何数据文件里都不应出现明文令牌");
    }

    // ------- Wave S4：快照导出与导入 -------

    /// 造一台带完整配置的引擎：代理、账号（带授权码）、全局代理、签名、AI 站点与映射、五个开关。
    fn seeded(dir: &std::path::Path) -> (MailEngine, Arc<MemorySecretStore>) {
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = MailEngine::initialize_with_secrets(dir, secrets.clone()).expect("初始化引擎");

        {
            let store = engine.store();
            let proxy_id = store
                .insert_proxy(
                    &ProxyConfig {
                        id: None,
                        label: "公司代理".to_string(),
                        kind: ProxyKind::Socks5,
                        host: "127.0.0.1".to_string(),
                        port: 1080,
                        username: "u".to_string(),
                    },
                    Some("proxy/ref"),
                )
                .expect("插代理");
            let mut account = draft("a@example.com");
            account.display_name = "甲".to_string();
            account.proxy = AccountProxyMode::Custom(proxy_id);
            let account_id = store
                .insert_account(&account, Some("account/ref"))
                .expect("插账号");
            store
                .set_global_proxy_mode(GlobalProxyMode::Custom(proxy_id))
                .expect("全局代理");
            store
                .save_signature(account_id.0, "<p>你好</p>", true)
                .expect("签名");

            let provider_id = store
                .insert_ai_provider(
                    &NewAiProvider {
                        label: "deepseek".to_string(),
                        kind: AiProviderKind::OpenAiCompatible,
                        base_url: "https://api.deepseek.com".to_string(),
                        default_model: "deepseek-chat".to_string(),
                        models: vec!["deepseek-chat".to_string()],
                        thinking_level: AiThinkingLevel::Off,
                        enabled: true,
                    },
                    Some("ai/ref"),
                )
                .expect("插站点");
            store
                .upsert_ai_model_map(
                    AiFunction::Translate,
                    provider_id,
                    "deepseek-chat",
                    Some(AiThinkingLevel::Low),
                )
                .expect("映射");
            store.set_setting(SETTING_NOTIFY_NEW_MAIL, "false").expect("开关");
        }

        secrets
            .set("proxy/ref", &Secret::new("proxy-pass-3812"))
            .expect("写代理密码");
        secrets
            .set("account/ref", &Secret::new("auth-code-9527"))
            .expect("写授权码");
        secrets
            .set("ai/ref", &Secret::new("sk-ai-key-7766"))
            .expect("写AI密钥");

        (engine, secrets)
    }

    /// 造一个账号草稿。
    fn draft(email: &str) -> AccountDraft {
        AccountDraft {
            display_name: "测试账号".to_string(),
            email: email.to_string(),
            auth_type: AuthType::Password,
            username: email.to_string(),
            imap: ServerConfig {
                host: "imap.example.com".to_string(),
                port: 993,
                security: Security::Tls,
            },
            smtp: ServerConfig {
                host: "smtp.example.com".to_string(),
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

    /// 目录里任一数据文件是否含某段明文。
    fn data_dir_contains(dir: &std::path::Path, needle: &str) -> bool {
        for entry in std::fs::read_dir(dir).expect("列目录") {
            let path = entry.expect("目录项").path();
            if !path.is_file() {
                continue;
            }
            let bytes = std::fs::read(&path).expect("读文件");
            if bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
            {
                return true;
            }
        }
        false
    }

    #[test]
    fn 快照序列化往返一致() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _secrets) = seeded(dir.path());
        let snapshot = engine.export_settings_snapshot("dev-1", "本机", 7).expect("导出");

        let json = serde_json::to_string(&snapshot).expect("序列化");
        let back: SettingsSnapshot = serde_json::from_str(&json).expect("反序列化");
        assert_eq!(snapshot, back);
    }

    #[test]
    fn 导出把密钥放进快照且数据库不留明文() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _secrets) = seeded(dir.path());

        let snapshot = engine.export_settings_snapshot("dev-1", "本机", 7).expect("导出");

        assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.device_id, "dev-1");
        assert_eq!(snapshot.revision, 7);
        assert_eq!(snapshot.accounts.len(), 1);
        assert_eq!(snapshot.proxies.len(), 1);
        assert_eq!(snapshot.ai_providers.len(), 1);
        assert_eq!(snapshot.ai_model_map.len(), 1);
        assert_eq!(snapshot.signatures.len(), 1);
        assert!(!snapshot.settings.notify_new_mail, "开关该跟着库里的值");

        assert_eq!(
            snapshot
                .secrets
                .get(&account_logical_key("a@example.com"))
                .map(String::as_str),
            Some("auth-code-9527")
        );
        assert_eq!(
            snapshot
                .secrets
                .get(&proxy_logical_key("公司代理", "127.0.0.1", 1080))
                .map(String::as_str),
            Some("proxy-pass-3812")
        );
        assert_eq!(
            snapshot
                .secrets
                .get(&ai_logical_key("deepseek"))
                .map(String::as_str),
            Some("sk-ai-key-7766")
        );

        // 只导出进包字段：本机专有字段绝不出现。
        let json = serde_json::to_string(&snapshot).expect("序列化");
        for forbidden in ["dataDir", "attachmentDir", "pendingCleanupDir", "sync/password"] {
            assert!(!json.contains(forbidden), "快照里不该出现 {forbidden}");
        }

        // 数据库与其它本机文件里都不许有明文密钥。
        for secret in ["auth-code-9527", "proxy-pass-3812", "sk-ai-key-7766"] {
            assert!(
                !data_dir_contains(dir.path(), secret),
                "数据文件里不该出现明文 {secret}"
            );
        }
    }

    #[test]
    fn 快照里的逻辑键不带密钥明文() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _secrets) = seeded(dir.path());
        let snapshot = engine.export_settings_snapshot("dev-1", "本机", 1).expect("导出");

        assert!(!snapshot.secrets.is_empty());
        for (logical, plain) in &snapshot.secrets {
            assert!(
                logical.starts_with("account:")
                    || logical.starts_with("proxy:")
                    || logical.starts_with("ai:"),
                "逻辑键前缀不对：{logical}"
            );
            assert!(
                !logical.contains(plain.as_str()),
                "逻辑键里混进了密钥明文：{logical}"
            );
        }
    }

    #[test]
    fn 导出再导入到空库两边一致() {
        let source_dir = tempfile::tempdir().expect("源目录");
        let (source, _source_secrets) = seeded(source_dir.path());
        let snapshot = source.export_settings_snapshot("dev-1", "甲机", 3).expect("导出");

        let target_dir = tempfile::tempdir().expect("目标目录");
        let (target, target_secrets) = engine(target_dir.path());
        target.import_settings_snapshot(&snapshot).expect("导入");

        let back = target
            .export_settings_snapshot("dev-2", "乙机", 4)
            .expect("再导出");

        assert_eq!(back.accounts, snapshot.accounts);
        assert_eq!(back.proxies, snapshot.proxies);
        assert_eq!(back.global_proxy, snapshot.global_proxy);
        assert_eq!(back.signatures, snapshot.signatures);
        assert_eq!(back.ai_providers, snapshot.ai_providers);
        assert_eq!(back.ai_model_map, snapshot.ai_model_map);
        assert_eq!(back.settings, snapshot.settings);
        assert_eq!(back.secrets, snapshot.secrets, "密钥明文也该一致");

        // 密钥确实写进了目标机保险箱：从库里读回账号真实引用再查。
        let account = target
            .store()
            .list_accounts()
            .expect("列账号")
            .into_iter()
            .find(|item| item.email == "a@example.com")
            .expect("目标机应有该账号");
        let account_ref = account.credential_key.expect("账号应有凭据引用");
        assert_eq!(
            target_secrets.plain(&account_ref).as_deref(),
            Some("auth-code-9527")
        );
    }

    #[test]
    fn 导入同邮箱账号按规则更新不新建() {
        let source_dir = tempfile::tempdir().expect("源目录");
        let (source, _source_secrets) = seeded(source_dir.path());
        let snapshot = source.export_settings_snapshot("dev-1", "甲机", 1).expect("导出");

        let target_dir = tempfile::tempdir().expect("目标目录");
        let (target, target_secrets) = engine(target_dir.path());
        {
            let store = target.store();
            // 故意用不同大小写，验证匹配不分大小写。
            store
                .insert_account(&draft("A@Example.com"), Some("account/old"))
                .expect("预置账号");
        }
        target_secrets
            .set("account/old", &Secret::new("old-code"))
            .expect("预置授权码");

        target.import_settings_snapshot(&snapshot).expect("导入");

        let store = target.store();
        let accounts = store.list_accounts().expect("列账号");
        assert_eq!(accounts.len(), 1, "同邮箱不该新建重复账号");
        assert_eq!(accounts[0].display_name, "甲", "该按快照更新");
        assert_eq!(accounts[0].email.to_ascii_lowercase(), "a@example.com");
    }

    #[test]
    fn 导入失败时库和保险箱都不留半成品() {
        let source_dir = tempfile::tempdir().expect("源目录");
        let (source, _source_secrets) = seeded(source_dir.path());
        let mut snapshot = source.export_settings_snapshot("dev-1", "甲机", 1).expect("导出");
        // 端口非法，写库时会让事务失败。
        snapshot.accounts[0].imap.port = 0;

        let target_dir = tempfile::tempdir().expect("目标目录");
        let (target, target_secrets) = engine(target_dir.path());

        assert!(
            target.import_settings_snapshot(&snapshot).is_err(),
            "非法端口应让导入失败"
        );

        {
            let store = target.store();
            assert!(
                store.list_accounts().expect("列账号").is_empty(),
                "账号不该留下半成品"
            );
            assert!(
                store.list_proxies().expect("列代理").is_empty(),
                "代理不该留下半成品"
            );
        }

        // 先写进保险箱的授权码必须被回滚。
        let account_ref = crate::proxies::new_credential_key("account", "a@example.com");
        assert!(
            !target_secrets.contains(&account_ref),
            "写库失败要把先写的授权码回滚掉"
        );
    }

    #[test]
    fn 逻辑键规则稳定且改名算新条目() {
        // 账号：不分大小写、去空白。
        assert_eq!(account_logical_key("  A@B.com "), account_logical_key("a@b.com"));
        assert_eq!(account_logical_key("a@b.com"), "account:a@b.com");

        // 代理：标签 + 主机 + 端口；改标签 / 主机 / 端口都算新条目。
        let base = proxy_logical_key("公司", "10.0.0.1", 1080);
        assert_eq!(base, proxy_logical_key(" 公司 ", "10.0.0.1", 1080));
        assert_ne!(base, proxy_logical_key("公司", "10.0.0.1", 1081));
        assert_ne!(base, proxy_logical_key("公司", "10.0.0.2", 1080));
        assert_ne!(base, proxy_logical_key("公司2", "10.0.0.1", 1080), "改名算新条目");

        // AI 站点：按标签。
        assert_eq!(ai_logical_key(" deepseek "), "ai:deepseek");
        assert_ne!(ai_logical_key("deepseek"), ai_logical_key("deepseek2"));
    }
}
