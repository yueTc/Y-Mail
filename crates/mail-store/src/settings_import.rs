//! 设置同步：把一份「导入计划」原子地写回本机数据库。
//!
//! 安全约定：
//! - 本模块只处理逻辑键与引用键，密钥本体由上层（mail-core）先写进系统保险箱；
//! - 整段写入放在一个事务里，中途失败自动回滚，不留半成品。

use std::collections::HashMap;

use mail_domain::account::{AccountDraft, AccountId};
use mail_domain::proxy::{GlobalProxyMode, ProxyConfig, ProxyId};
use rusqlite::{OptionalExtension, Transaction};

use crate::ai::{AiFunction, AiThinkingLevel, NewAiProvider};
use crate::connection::Store;
use crate::error::StoreError;

/// 全局代理导入意图；自定义代理只带快照逻辑键，落库时换成本机主键。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum GlobalProxyImport {
    /// 跟随系统。
    #[default]
    System,
    /// 直连。
    Direct,
    /// 使用某个自定义代理（值为快照逻辑键）。
    Custom(String),
}

/// 一个代理条目的导入意图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyImport {
    /// 快照逻辑键：标签 + 主机 + 端口。
    pub key: String,
    /// 标签。
    pub label: String,
    /// 类型（`socks5` / `http`）。
    pub kind: String,
    /// 主机。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 登录名，可空。
    pub username: String,
}

/// 一个账号条目的导入意图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountImport {
    /// 账号草稿（邮箱、服务器、显示名等）。
    pub draft: AccountDraft,
    /// 该账号引用的代理快照逻辑键；`None` 表示跟随全局。
    pub proxy_key: Option<String>,
}

/// 一个 AI 站点条目的导入意图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProviderImport {
    /// 快照逻辑键（标签）。
    pub key: String,
    /// 站点字段。
    pub provider: NewAiProvider,
}

/// 一个功能模型映射的导入意图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiModelMapImport {
    /// 功能。
    pub function: AiFunction,
    /// 引用的站点快照逻辑键（标签）。
    pub provider_key: String,
    /// 功能级模型；空串表示回退站点默认。
    pub model: String,
    /// 功能级思考程度；`None` 表示回退站点默认。
    pub thinking_level: Option<AiThinkingLevel>,
}

/// 一个账号签名的导入意图（签名按账号邮箱找账号）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureImport {
    /// 所属账号邮箱。
    pub account_email: String,
    /// 签名 HTML。
    pub html: String,
    /// 是否启用。
    pub enabled: bool,
}

/// 一份完整的设置导入计划（不含密钥本体）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsImportPlan {
    /// 代理条目。
    pub proxies: Vec<ProxyImport>,
    /// 账号条目。
    pub accounts: Vec<AccountImport>,
    /// 全局代理意图。
    pub global_proxy: GlobalProxyImport,
    /// 签名条目。
    pub signatures: Vec<SignatureImport>,
    /// AI 站点。
    pub ai_providers: Vec<AiProviderImport>,
    /// AI 功能映射。
    pub ai_model_map: Vec<AiModelMapImport>,
    /// 只进包的界面开关。
    pub settings: SettingsImportValues,
    /// 密钥：逻辑键 → 引用键。密钥本体已由上层写入保险箱，这里只落引用。
    pub secrets: HashMap<String, Option<String>>,
}

/// 只进包的那几个界面 / 行为开关。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SettingsImportValues {
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

/// `setting` 表里界面开关的键名（与外壳 `settings.json` 字段一一对应）。
pub const SETTING_NOTIFY_NEW_MAIL: &str = "ui.notifyNewMail";
/// 新邮件 AI 智能识别通知的键名。
pub const SETTING_NOTIFY_AI_ENABLED: &str = "ui.notifyAiEnabled";
/// 默认拦截远程图片的键名。
pub const SETTING_BLOCK_REMOTE_IMAGES: &str = "ui.blockRemoteImagesByDefault";
/// 关闭主窗口最小化到托盘的键名。
pub const SETTING_MINIMIZE_TO_TRAY: &str = "ui.minimizeToTrayOnClose";
/// 启动最小化到托盘的键名。
pub const SETTING_START_MINIMIZED: &str = "ui.startMinimizedToTray";

/// 数据库里一行账号的最小信息（用于按邮箱匹配）。
struct AccountMatch {
    id: AccountId,
    credential_key: Option<String>,
}

/// 数据库里一行 AI 站点的最小信息（用于按标签匹配）。
struct ProviderMatch {
    id: i64,
    api_key_ref: Option<String>,
}

impl Store {
    /// 取当前 UTC 时间（毫秒精度，与库内其它时间字段同一格式）。
    pub fn now_utc(&self) -> Result<String, StoreError> {
        Ok(self
            .conn()
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |row| {
                row.get(0)
            })?)
    }

    /// 把一份导入计划原子写回本机数据库。
    ///
    /// 顺序：代理 → 账号 → 签名 → AI 站点 → AI 映射 → 全局代理 → 界面开关。
    /// 账号与全局代理引用的代理编号在本次事务里换成新生成的本机编号。
    pub fn apply_settings_import(&self, plan: &SettingsImportPlan) -> Result<(), StoreError> {
        let tx = self.conn().unchecked_transaction()?;
        self.apply_settings_import_in(&tx, plan)?;
        tx.commit()?;
        Ok(())
    }

    /// 事务内部的导入实现；调用方负责提交或回滚。
    fn apply_settings_import_in(
        &self,
        tx: &Transaction<'_>,
        plan: &SettingsImportPlan,
    ) -> Result<(), StoreError> {
        // 1) 代理。
        let mut proxy_ids: HashMap<String, ProxyId> = HashMap::new();
        for import in &plan.proxies {
            let existing = find_proxy_id(tx, &import.label, &import.host, import.port)?;
            let config = ProxyConfig {
                id: existing,
                label: import.label.clone(),
                kind: mail_domain::proxy::ProxyKind::parse(&import.kind)
                    .unwrap_or(mail_domain::proxy::ProxyKind::Http),
                host: import.host.clone(),
                port: import.port,
                username: import.username.clone(),
            };
            let password_key = plan.secrets.get(&import.key).and_then(|value| value.clone());
            let id = self.save_proxy_in(tx, &config, password_key.as_deref())?;
            proxy_ids.insert(import.key.clone(), id);
        }

        // 2) 账号（按邮箱匹配）；代理引用换成新编号。
        for import in &plan.accounts {
            let mut draft = import.draft.clone();
            draft.proxy = match &import.proxy_key {
                None => mail_domain::account::AccountProxyMode::InheritGlobal,
                Some(key) => match proxy_ids.get(key) {
                    Some(id) => mail_domain::account::AccountProxyMode::Custom(*id),
                    None => mail_domain::account::AccountProxyMode::InheritGlobal,
                },
            };
            let key = account_key(&draft.email);
            let credential_key = plan.secrets.get(&key).and_then(|value| value.clone());
            self.save_account_in(tx, &draft, credential_key.as_deref())?;
        }

        // 3) 签名（按账号邮箱匹配；账号不存在就跳过）。
        for import in &plan.signatures {
            if let Some(account_id) = find_account_id(tx, &import.account_email)? {
                upsert_signature_in(tx, account_id.0, &import.html, import.enabled)?;
            }
        }

        // 4) AI 站点（按标签匹配）。
        let mut provider_ids: HashMap<String, i64> = HashMap::new();
        for import in &plan.ai_providers {
            let key = ai_key(&import.provider.label);
            let api_key_ref = plan.secrets.get(&key).and_then(|value| value.clone());
            let id = self.save_ai_provider_in(tx, &import.provider, api_key_ref.as_deref())?;
            provider_ids.insert(import.key.clone(), id);
        }

        // 5) AI 功能映射（站点按标签换成本机编号；找不到站点就跳过）。
        for import in &plan.ai_model_map {
            if let Some(provider_id) = provider_ids.get(&import.provider_key) {
                delete_ai_model_map_in(tx, import.function)?;
                upsert_ai_model_map_in(
                    tx,
                    import.function,
                    *provider_id,
                    &import.model,
                    import.thinking_level,
                )?;
            }
        }

        // 6) 全局代理。
        let mode = match &plan.global_proxy {
            GlobalProxyImport::System => GlobalProxyMode::System,
            GlobalProxyImport::Direct => GlobalProxyMode::Direct,
            GlobalProxyImport::Custom(key) => match proxy_ids.get(key) {
                Some(id) => GlobalProxyMode::Custom(*id),
                None => GlobalProxyMode::System,
            },
        };
        set_global_proxy_mode_in(tx, mode)?;

        // 7) 界面开关。
        let settings = &plan.settings;
        set_setting_in(tx, SETTING_NOTIFY_NEW_MAIL, bool_str(settings.notify_new_mail))?;
        set_setting_in(
            tx,
            SETTING_NOTIFY_AI_ENABLED,
            bool_str(settings.notify_ai_enabled),
        )?;
        set_setting_in(
            tx,
            SETTING_BLOCK_REMOTE_IMAGES,
            bool_str(settings.block_remote_images_by_default),
        )?;
        set_setting_in(
            tx,
            SETTING_MINIMIZE_TO_TRAY,
            bool_str(settings.minimize_to_tray_on_close),
        )?;
        set_setting_in(
            tx,
            SETTING_START_MINIMIZED,
            bool_str(settings.start_minimized_to_tray),
        )?;

        Ok(())
    }

    /// 事务内保存一个代理：按主键更新，没有就新建。
    fn save_proxy_in(
        &self,
        tx: &Transaction<'_>,
        config: &ProxyConfig,
        password_key: Option<&str>,
    ) -> Result<ProxyId, StoreError> {
        match config.id {
            Some(id) => {
                let changed = tx.execute(
                    "UPDATE proxy
                     SET label = ?2, kind = ?3, host = ?4, port = ?5, username = ?6,
                         password_key = ?7, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE id = ?1",
                    rusqlite::params![
                        id.0,
                        config.label,
                        config.kind.as_str(),
                        config.host,
                        i64::from(config.port),
                        config.username,
                        password_key,
                    ],
                )?;
                if changed == 0 {
                    return Err(StoreError::InvalidData(format!("代理编号 {} 不存在", id.0)));
                }
                Ok(id)
            }
            None => {
                tx.execute(
                    "INSERT INTO proxy (label, kind, host, port, username, password_key)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        config.label,
                        config.kind.as_str(),
                        config.host,
                        i64::from(config.port),
                        config.username,
                        password_key,
                    ],
                )?;
                Ok(ProxyId(tx.last_insert_rowid()))
            }
        }
    }

    /// 事务内保存一个账号：按邮箱匹配，命中就沿用编号更新，否则新建。
    fn save_account_in(
        &self,
        tx: &Transaction<'_>,
        draft: &AccountDraft,
        credential_key: Option<&str>,
    ) -> Result<AccountId, StoreError> {
        let existing = find_account_match(tx, &draft.email)?;
        match existing {
            Some(found) => {
                let key = credential_key.or(found.credential_key.as_deref());
                tx.execute(
                    "UPDATE account SET
                        display_name = ?2, email = ?3, auth_type = ?4, username = ?5,
                        imap_host = ?6, imap_port = ?7, imap_security = ?8,
                        smtp_host = ?9, smtp_port = ?10, smtp_security = ?11,
                        proxy_mode = ?12, proxy_id = ?13, color = ?14, enabled = ?15,
                        credential_key = ?16, oauth_provider = ?17, oauth_client_id = ?18,
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE id = ?1",
                    rusqlite::params![
                        found.id.0,
                        draft.display_name,
                        draft.email,
                        draft.auth_type.as_str(),
                        draft.username,
                        draft.imap.host,
                        i64::from(draft.imap.port),
                        draft.imap.security.as_str(),
                        draft.smtp.host,
                        i64::from(draft.smtp.port),
                        draft.smtp.security.as_str(),
                        draft.proxy.as_str(),
                        proxy_id_of(draft.proxy),
                        draft.color,
                        i64::from(draft.enabled),
                        key,
                        draft.oauth_provider.map(|provider| provider.as_str()),
                        draft.oauth_client_id,
                    ],
                )?;
                Ok(found.id)
            }
            None => {
                tx.execute(
                    "INSERT INTO account (
                        display_name, email, auth_type, username,
                        imap_host, imap_port, imap_security,
                        smtp_host, smtp_port, smtp_security,
                        proxy_mode, proxy_id, color, enabled, credential_key,
                        oauth_provider, oauth_client_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                    rusqlite::params![
                        draft.display_name,
                        draft.email,
                        draft.auth_type.as_str(),
                        draft.username,
                        draft.imap.host,
                        i64::from(draft.imap.port),
                        draft.imap.security.as_str(),
                        draft.smtp.host,
                        i64::from(draft.smtp.port),
                        draft.smtp.security.as_str(),
                        draft.proxy.as_str(),
                        proxy_id_of(draft.proxy),
                        draft.color,
                        i64::from(draft.enabled),
                        credential_key,
                        draft.oauth_provider.map(|provider| provider.as_str()),
                        draft.oauth_client_id,
                    ],
                )?;
                Ok(AccountId(tx.last_insert_rowid()))
            }
        }
    }

    /// 事务内保存一个 AI 站点：按标签匹配，命中就沿用编号更新，否则新建。
    fn save_ai_provider_in(
        &self,
        tx: &Transaction<'_>,
        provider: &NewAiProvider,
        api_key_ref: Option<&str>,
    ) -> Result<i64, StoreError> {
        let models = serde_json::to_string(&provider.models)
            .map_err(|_| StoreError::InvalidData("AI 模型列表无法序列化".to_string()))?;
        let existing = find_provider_match(tx, &provider.label)?;
        match existing {
            Some(found) => {
                let key = api_key_ref.or(found.api_key_ref.as_deref());
                tx.execute(
                    "UPDATE ai_provider
                     SET label = ?2, kind = ?3, base_url = ?4, default_model = ?5,
                         models_json = ?6, thinking_level = ?7, enabled = ?8, api_key_ref = ?9,
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE id = ?1",
                    rusqlite::params![
                        found.id,
                        provider.label,
                        provider.kind.as_str(),
                        provider.base_url,
                        provider.default_model,
                        models,
                        provider.thinking_level.as_str(),
                        i64::from(provider.enabled),
                        key,
                    ],
                )?;
                Ok(found.id)
            }
            None => {
                tx.execute(
                    "INSERT INTO ai_provider
                        (label, kind, base_url, default_model, models_json, thinking_level, enabled, api_key_ref)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        provider.label,
                        provider.kind.as_str(),
                        provider.base_url,
                        provider.default_model,
                        models,
                        provider.thinking_level.as_str(),
                        i64::from(provider.enabled),
                        api_key_ref,
                    ],
                )?;
                Ok(tx.last_insert_rowid())
            }
        }
    }
}

/// 账号逻辑键（邮箱小写去空白）。
fn account_key(email: &str) -> String {
    format!("account:{}", email.trim().to_ascii_lowercase())
}

/// AI 站点逻辑键（标签）。
fn ai_key(label: &str) -> String {
    format!("ai:{}", label.trim())
}

/// 布尔值落库文本。
fn bool_str(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

/// 草稿里代理编号的落库形式（非「指定代理」写 NULL）。
fn proxy_id_of(mode: mail_domain::account::AccountProxyMode) -> Option<i64> {
    match mode {
        mail_domain::account::AccountProxyMode::Custom(id) => Some(id.0),
        _ => None,
    }
}

/// 按「标签 + 主机 + 端口」查代理编号。
fn find_proxy_id(
    tx: &Transaction<'_>,
    label: &str,
    host: &str,
    port: u16,
) -> Result<Option<ProxyId>, StoreError> {
    let found: Option<i64> = tx
        .query_row(
            "SELECT id FROM proxy WHERE label = ?1 AND host = ?2 AND port = ?3 ORDER BY id LIMIT 1",
            rusqlite::params![label, host, i64::from(port)],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.map(ProxyId))
}

/// 按邮箱（不分大小写）找账号编号。
fn find_account_id(tx: &Transaction<'_>, email: &str) -> Result<Option<AccountId>, StoreError> {
    Ok(find_account_match(tx, email)?.map(|found| found.id))
}

/// 按邮箱（不分大小写）找账号编号与现有凭据引用键。
fn find_account_match(tx: &Transaction<'_>, email: &str) -> Result<Option<AccountMatch>, StoreError> {
    Ok(tx
        .query_row(
            "SELECT id, credential_key FROM account WHERE lower(email) = lower(?1) ORDER BY id LIMIT 1",
            [email],
            |row| {
                Ok(AccountMatch {
                    id: AccountId(row.get(0)?),
                    credential_key: row.get(1)?,
                })
            },
        )
        .optional()?)
}

/// 按标签找 AI 站点编号与现有密钥引用键。
fn find_provider_match(tx: &Transaction<'_>, label: &str) -> Result<Option<ProviderMatch>, StoreError> {
    Ok(tx
        .query_row(
            "SELECT id, api_key_ref FROM ai_provider WHERE label = ?1 ORDER BY id LIMIT 1",
            [label],
            |row| {
                Ok(ProviderMatch {
                    id: row.get(0)?,
                    api_key_ref: row.get(1)?,
                })
            },
        )
        .optional()?)
}

/// 事务内写一个签名。
fn upsert_signature_in(
    tx: &Transaction<'_>,
    account_id: i64,
    html: &str,
    enabled: bool,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO signature (account_id, html, enabled, updated_at)
         VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT(account_id) DO UPDATE SET
             html = excluded.html, enabled = excluded.enabled, updated_at = excluded.updated_at",
        rusqlite::params![account_id, html, i64::from(enabled)],
    )?;
    Ok(())
}

/// 事务内替换一个功能的模型映射。
fn upsert_ai_model_map_in(
    tx: &Transaction<'_>,
    function: AiFunction,
    provider_id: i64,
    model: &str,
    thinking_level: Option<AiThinkingLevel>,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO ai_model_map (function, provider_id, model, thinking_level)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(function) DO UPDATE SET
             provider_id = excluded.provider_id,
             model = excluded.model,
             thinking_level = excluded.thinking_level,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        rusqlite::params![
            function.as_str(),
            provider_id,
            model,
            thinking_level.map(AiThinkingLevel::as_str),
        ],
    )?;
    Ok(())
}

/// 事务内删除一个功能的模型映射。
fn delete_ai_model_map_in(tx: &Transaction<'_>, function: AiFunction) -> Result<(), StoreError> {
    tx.execute(
        "DELETE FROM ai_model_map WHERE function = ?1",
        [function.as_str()],
    )?;
    Ok(())
}

/// 事务内设置全局代理模式。
fn set_global_proxy_mode_in(tx: &Transaction<'_>, mode: GlobalProxyMode) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO setting (key, value) VALUES ('proxy.global_mode', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [mode.as_str()],
    )?;
    match mode {
        GlobalProxyMode::Custom(id) => {
            tx.execute(
                "INSERT INTO setting (key, value) VALUES ('proxy.global_id', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [id.0.to_string()],
            )?;
        }
        _ => {
            tx.execute("DELETE FROM setting WHERE key = 'proxy.global_id'", [])?;
        }
    }
    Ok(())
}

/// 事务内写一个 `setting` 键值。
fn set_setting_in(tx: &Transaction<'_>, key: &str, value: &str) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO setting (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let mut store = Store::open_in_memory().expect("打开内存库");
        store.run_migrations().expect("迁移");
        store
    }

    fn draft(email: &str) -> AccountDraft {
        AccountDraft {
            display_name: email.to_string(),
            email: email.to_string(),
            auth_type: mail_domain::account::AuthType::Password,
            username: email.to_string(),
            imap: mail_domain::account::ServerConfig {
                host: "imap.example.com".to_string(),
                port: 993,
                security: mail_domain::account::Security::Tls,
            },
            smtp: mail_domain::account::ServerConfig {
                host: "smtp.example.com".to_string(),
                port: 465,
                security: mail_domain::account::Security::Tls,
            },
            proxy: mail_domain::account::AccountProxyMode::InheritGlobal,
            color: String::new(),
            enabled: true,
            oauth_provider: None,
            oauth_client_id: String::new(),
        }
    }

    #[test]
    fn 导入计划能原子写回并二次导入不重复() {
        let store = store();
        let mut secrets = HashMap::new();
        secrets.insert("proxy:公司代理".to_string(), Some("proxy/ref-1".to_string()));
        secrets.insert(
            "account:a@example.com".to_string(),
            Some("account/ref-1".to_string()),
        );
        secrets.insert("ai:deepseek".to_string(), Some("ai/ref-1".to_string()));

        let plan = SettingsImportPlan {
            proxies: vec![ProxyImport {
                key: "proxy:公司代理".to_string(),
                label: "公司代理".to_string(),
                kind: "http".to_string(),
                host: "127.0.0.1".to_string(),
                port: 8080,
                username: "u".to_string(),
            }],
            accounts: vec![AccountImport {
                draft: draft("a@example.com"),
                proxy_key: Some("proxy:公司代理".to_string()),
            }],
            global_proxy: GlobalProxyImport::Custom("proxy:公司代理".to_string()),
            signatures: vec![SignatureImport {
                account_email: "a@example.com".to_string(),
                html: "<p>你好</p>".to_string(),
                enabled: true,
            }],
            ai_providers: vec![AiProviderImport {
                key: "ai:deepseek".to_string(),
                provider: NewAiProvider {
                    label: "deepseek".to_string(),
                    kind: crate::ai::AiProviderKind::OpenAiCompatible,
                    base_url: "https://api.deepseek.com".to_string(),
                    default_model: "deepseek-chat".to_string(),
                    models: vec!["deepseek-chat".to_string()],
                    thinking_level: AiThinkingLevel::Off,
                    enabled: true,
                },
            }],
            ai_model_map: vec![AiModelMapImport {
                function: AiFunction::Translate,
                provider_key: "ai:deepseek".to_string(),
                model: "deepseek-chat".to_string(),
                thinking_level: Some(AiThinkingLevel::Low),
            }],
            settings: SettingsImportValues {
                notify_new_mail: true,
                notify_ai_enabled: false,
                block_remote_images_by_default: true,
                minimize_to_tray_on_close: true,
                start_minimized_to_tray: false,
            },
            secrets,
        };

        store.apply_settings_import(&plan).expect("首次导入");
        // 再导入一次，应该按逻辑键更新而不是重复新建。
        store.apply_settings_import(&plan).expect("二次导入");

        let accounts = store.list_accounts().expect("列账号");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].credential_key.as_deref(), Some("account/ref-1"));
        assert_eq!(
            accounts[0].proxy,
            mail_domain::account::AccountProxyMode::Custom(ProxyId(1))
        );
        assert_eq!(store.list_proxies().expect("列代理").len(), 1);
        assert_eq!(
            store.list_proxies().expect("列代理")[0].password_key.as_deref(),
            Some("proxy/ref-1")
        );
        assert_eq!(store.list_ai_providers().expect("列站点").len(), 1);
        assert_eq!(
            store.list_ai_providers().expect("列站点")[0]
                .api_key_ref
                .as_deref(),
            Some("ai/ref-1")
        );
        assert_eq!(
            store.global_proxy_mode().expect("全局代理"),
            GlobalProxyMode::Custom(ProxyId(1))
        );
        assert_eq!(store.get_signature(1).expect("读签名").html, "<p>你好</p>");
        assert_eq!(
            store
                .get_setting(SETTING_NOTIFY_NEW_MAIL)
                .expect("读开关")
                .as_deref(),
            Some("true")
        );
    }
    #[test]
    fn 导入中途失败会整体回滚() {
        let store = store();
        let mut bad = draft("bad@example.com");
        bad.imap.port = 0;

        let plan = SettingsImportPlan {
            proxies: vec![ProxyImport {
                key: "proxy:回滚代理".to_string(),
                label: "回滚代理".to_string(),
                kind: "http".to_string(),
                host: "127.0.0.1".to_string(),
                port: 1080,
                username: String::new(),
            }],
            accounts: vec![
                AccountImport {
                    draft: draft("good@example.com"),
                    proxy_key: None,
                },
                AccountImport {
                    draft: bad,
                    proxy_key: None,
                },
            ],
            global_proxy: GlobalProxyImport::System,
            ..Default::default()
        };

        assert!(
            store.apply_settings_import(&plan).is_err(),
            "非法端口应让事务失败"
        );
        assert!(
            store.list_accounts().expect("列账号").is_empty(),
            "前面写成功的账号也要回滚"
        );
        assert!(
            store.list_proxies().expect("列代理").is_empty(),
            "前面写成功的代理也要回滚"
        );
    }
}
