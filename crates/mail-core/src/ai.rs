//! AI 与翻译编排（Wave 7）。
//!
//! 安全边界：
//! - AI 默认关闭：没有任何启用站点时，所有 AI 调用直接拒绝；
//! - 每次外发都必须由界面先做一次明确确认，并拿一次性授权令牌来消费；
//! - CDKey 只在调用前从系统保险箱取出，不落库、不进日志；
//! - 邮件正文与模型输出都当不可信字符串，最多写进本地缓存，绝不触发任何动作。
//!
//! 锁纪律：存储锁只在同步代码里短暂持有，绝不跨 `.await`。

use std::time::{Duration, Instant};

use mail_ai::{
    anchor_targets, chat, is_local_host, list_models, normalize_base_url, parse_verification, prompt,
    split_html, split_text, translate, AiError, ChatOutcome, Endpoint, ProviderKind, ThinkingLevel,
    TranslationOutcome, VerificationFinding,
};
use mail_domain::account::AccountProxyMode;
use mail_domain::proxy::Secret;
use mail_store::{
    AiFunction, AiModelMapEntry, AiProviderKind, AiThinkingLevel, NewAiAudit, NewAiProvider, StoredAiAudit,
    StoredAiProvider,
};

use crate::engine::{EngineError, MailEngine};
use crate::proxies::new_credential_key;

/// AI 请求超时：本地模型可能较慢，给到 60 秒。
pub const AI_TIMEOUT: Duration = Duration::from_secs(60);

/// 通知识别的单封等待上限；超了就退回普通通知。
pub const NOTIFICATION_VERIFY_TIMEOUT: Duration = Duration::from_secs(15);

/// 一次外发授权令牌的有效期。
pub const AI_AUTHORIZATION_TTL: Duration = Duration::from_secs(300);

/// 站点类型在存储层与 AI 层之间的映射。
fn provider_kind(kind: AiProviderKind) -> ProviderKind {
    match kind {
        AiProviderKind::OpenAiCompatible => ProviderKind::OpenAiCompatible,
        AiProviderKind::DeepL => ProviderKind::DeepL,
        AiProviderKind::Ollama => ProviderKind::Ollama,
    }
}

/// 思考程度在存储层与 AI 层之间的映射。
fn thinking_level(level: AiThinkingLevel) -> ThinkingLevel {
    match level {
        AiThinkingLevel::Off => ThinkingLevel::Off,
        AiThinkingLevel::Low => ThinkingLevel::Low,
        AiThinkingLevel::Medium => ThinkingLevel::Medium,
        AiThinkingLevel::High => ThinkingLevel::High,
    }
}

/// 从站点地址里取出主机名（授权弹窗与审计用）。
fn host_of(base_url: &str) -> String {
    base_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(base_url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string()
}

/// 一次 AI 调用的目标：站点、模型、思考程度。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiTarget {
    /// 站点主键。
    pub provider_id: i64,
    /// 站点显示名。
    pub provider_label: String,
    /// 站点地址。
    pub base_url: String,
    /// 站点类型。
    pub kind: AiProviderKind,
    /// 实际使用的模型。
    pub model: String,
    /// 实际使用的思考程度。
    pub thinking_level: AiThinkingLevel,
    /// 是否本机服务。
    pub local: bool,
    /// 保密的系统凭据引用键；不对外展示。
    pub api_key_ref: Option<String>,
}

/// 界面展示的站点信息（不含密钥本体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProviderView {
    /// 主键。
    pub id: i64,
    /// 显示名。
    pub label: String,
    /// 站点类型。
    pub kind: AiProviderKind,
    /// 站点地址。
    pub base_url: String,
    /// 默认模型。
    pub default_model: String,
    /// 模型列表。
    pub models: Vec<String>,
    /// 默认思考程度。
    pub thinking_level: AiThinkingLevel,
    /// 是否启用。
    pub enabled: bool,
    /// 保险箱里是否已有密钥。
    pub has_key: bool,
}

/// 新建 / 修改站点的入参（不含密钥本体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProviderInput {
    /// 主键；新建为空。
    pub id: Option<i64>,
    /// 显示名。
    pub label: String,
    /// 站点类型。
    pub kind: AiProviderKind,
    /// 站点地址。
    pub base_url: String,
    /// 默认模型。
    pub default_model: String,
    /// 模型列表。
    pub models: Vec<String>,
    /// 默认思考程度。
    pub thinking_level: AiThinkingLevel,
    /// 是否启用。
    pub enabled: bool,
}

/// 外发授权弹窗要展示的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiAuthorizationPreview {
    /// 功能标识。
    pub function: String,
    /// 站点主键。
    pub provider_id: i64,
    /// 站点显示名。
    pub provider_label: String,
    /// 外发域名。
    pub host: String,
    /// 模型。
    pub model: String,
    /// 是否本机服务。
    pub local: bool,
    /// 本次内容的哈希；只用于校验，不包含正文。
    pub content_hash: String,
    /// 一次性授权令牌。
    pub authorization_token: String,
    /// 令牌有效期（秒）。
    pub expires_in_seconds: u64,
    /// 是否已经有本地缓存；有缓存时不会真的外发。
    pub from_cache: bool,
}

/// 已经确认、等待真正外发的一次性授权。
#[derive(Clone)]
pub struct PendingAiAuthorization {
    /// 一次性令牌。
    pub token: String,
    /// 功能。
    pub function: AiFunction,
    /// 站点主键。
    pub provider_id: i64,
    /// 站点显示名。
    pub provider_label: String,
    /// 外发域名。
    pub host: String,
    /// 模型。
    pub model: String,
    /// 思考程度。
    pub thinking_level: AiThinkingLevel,
    /// 是否本机服务。
    pub local: bool,
    /// 目标语言；摘要 / 润色 / 起草为空串。
    pub target_language: String,
    /// 内容哈希。
    pub content_hash: String,
    /// 过期时刻。
    pub expires_at: Instant,
}

/// 一次摘要 / 润色 / 起草的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiTextOutcome {
    /// 模型输出的纯文本。
    pub text: String,
    /// 是否因为模型不支持思考程度而降级。
    pub thinking_downgraded: bool,
    /// 是否命中本地缓存。
    pub from_cache: bool,
}

/// 一次翻译的结果：原段与译文一一对齐。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiTranslation {
    /// 原文段落。
    pub original: Vec<String>,
    /// 对齐译文段落。
    pub translated: Vec<String>,
    /// 是否因为模型不支持思考程度而降级。
    pub thinking_downgraded: bool,
    /// 是否命中本地缓存。
    pub from_cache: bool,
}

/// 把 HTML 正文整理成送检文本：先按可见文字切段，再把超链接地址按行补在后面。
///
/// 只做文本整理，不解析、不下载、不跳转；地址是否可信由下游白名单再判。
fn build_notification_body(html: &str) -> String {
    let mut text = split_html(html)
        .into_iter()
        .map(|segment| segment.text)
        .collect::<Vec<_>>()
        .join("\n\n");
    let links = anchor_targets(html);
    if !links.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(
            &links
                .iter()
                .map(|url| format!("链接：{url}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    text
}

impl MailEngine {
    /// 列出全部 AI 站点（不含密钥本体）。
    pub fn list_ai_providers(&self) -> Result<Vec<AiProviderView>, EngineError> {
        let providers = self.store().list_ai_providers()?;
        Ok(providers.into_iter().map(provider_view).collect())
    }

    /// 取出某个功能的模型映射。
    pub fn list_ai_model_maps(&self) -> Result<Vec<AiModelMapEntry>, EngineError> {
        Ok(self.store().list_ai_model_maps()?)
    }

    /// 保存 AI 站点；密钥的语义与账号一致：`None` 沿用、`Some(空)` 清空、`Some(非空)` 替换。
    pub fn save_ai_provider(
        &self,
        input: &AiProviderInput,
        api_key: Option<&Secret>,
    ) -> Result<AiProviderView, EngineError> {
        let label = input.label.trim();
        if label.is_empty() {
            return Err(EngineError::BadRequest("请填写站点名称".to_string()));
        }
        let base_url = normalize_base_url(&input.base_url, provider_kind(input.kind))?;
        let new_provider = NewAiProvider {
            label: label.to_string(),
            kind: input.kind,
            base_url,
            default_model: input.default_model.trim().to_string(),
            models: input.models.clone(),
            thinking_level: input.thinking_level,
            enabled: input.enabled,
        };

        match input.id {
            None => {
                let key = match api_key {
                    Some(secret) if !secret.is_empty() => {
                        let key = new_credential_key("ai", label);
                        self.secrets().set(&key, secret)?;
                        Some(key)
                    }
                    _ => None,
                };
                let inserted = self.store().insert_ai_provider(&new_provider, key.as_deref());
                match inserted {
                    Ok(id) => self.get_ai_provider_view(id),
                    Err(err) => {
                        if let Some(key) = key {
                            let _ = self.secrets().delete(&key);
                        }
                        Err(err.into())
                    }
                }
            }
            Some(id) => {
                let existing = self
                    .store()
                    .get_ai_provider(id)?
                    .ok_or(EngineError::AiProviderNotFound(id))?;
                match api_key {
                    None => {
                        let updated = self.store().update_ai_provider(
                            id,
                            &new_provider,
                            existing.api_key_ref.as_deref(),
                        );
                        match updated {
                            Ok(true) => self.get_ai_provider_view(id),
                            Ok(false) => Err(EngineError::AiProviderNotFound(id)),
                            Err(err) => Err(err.into()),
                        }
                    }
                    Some(secret) if secret.is_empty() => {
                        let cleared = self.store().update_ai_provider(id, &new_provider, None);
                        match cleared {
                            Ok(true) => {
                                if let Some(old) = existing.api_key_ref.as_deref() {
                                    if let Err(err) = self.secrets().delete(old) {
                                        tracing::debug!(error = %err, "清空 AI 密钥时保险箱删除失败");
                                    }
                                }
                                self.get_ai_provider_view(id)
                            }
                            Ok(false) => Err(EngineError::AiProviderNotFound(id)),
                            Err(err) => Err(err.into()),
                        }
                    }
                    Some(secret) => {
                        let old_key = existing.api_key_ref.clone();
                        let new_key = new_credential_key("ai", label);
                        self.secrets().set(&new_key, secret)?;
                        let replaced = self.store().update_ai_provider(id, &new_provider, Some(&new_key));
                        match replaced {
                            Ok(true) => {
                                if let Some(old) = old_key.as_deref() {
                                    if let Err(err) = self.secrets().delete(old) {
                                        tracing::debug!(error = %err, "替换 AI 密钥后旧条目删除失败");
                                    }
                                }
                                self.get_ai_provider_view(id)
                            }
                            Ok(false) => {
                                let _ = self.secrets().delete(&new_key);
                                Err(EngineError::AiProviderNotFound(id))
                            }
                            Err(err) => {
                                let _ = self.secrets().delete(&new_key);
                                Err(err.into())
                            }
                        }
                    }
                }
            }
        }
    }

    /// 删除 AI 站点；同时清掉它在保险箱里的密钥与全部缓存。
    pub fn delete_ai_provider(&self, id: i64) -> Result<(), EngineError> {
        self.clear_ai_authorizations();
        let existing = self
            .store()
            .get_ai_provider(id)?
            .ok_or(EngineError::AiProviderNotFound(id))?;
        if let Some(key) = existing.api_key_ref.as_deref() {
            if let Err(err) = self.secrets().delete(key) {
                tracing::debug!(error = %err, "删除 AI 站点时保险箱删除失败");
            }
        }
        if !self.store().delete_ai_provider(id)? {
            return Err(EngineError::AiProviderNotFound(id));
        }
        Ok(())
    }

    /// 用一份还没保存的站点配置做「测试连接」并拉模型；不写库、不写保险箱。
    ///
    /// `provider_id` 是正在编辑的站点编号。密钥留空时（`None` 或空串），
    /// 若该站点已存过密钥就回退用它，这样编辑站点不必把密钥重打一遍。
    pub async fn test_ai_provider(
        &self,
        kind: AiProviderKind,
        base_url: &str,
        api_key: Option<&Secret>,
        provider_id: Option<i64>,
    ) -> Result<Vec<String>, EngineError> {
        let base_url = normalize_base_url(base_url, provider_kind(kind))?;
        let fallback = match provider_id {
            Some(id) if api_key.map(|secret| secret.is_empty()).unwrap_or(true) => {
                self.stored_ai_provider_secret(id)?
            }
            _ => None,
        };
        let secret = api_key.filter(|secret| !secret.is_empty()).or(fallback.as_ref());
        let route = self.resolve_route(AccountProxyMode::InheritGlobal)?;
        let endpoint = Endpoint {
            kind: provider_kind(kind),
            base_url: &base_url,
            api_key: secret,
        };
        Ok(list_models(route.as_ref(), &endpoint, AI_TIMEOUT).await?)
    }

    /// 取出某个已存站点的密钥；没有密钥或引用为空时返回 `None`。
    fn stored_ai_provider_secret(&self, id: i64) -> Result<Option<Secret>, EngineError> {
        let provider = self
            .store()
            .get_ai_provider(id)?
            .ok_or(EngineError::AiProviderNotFound(id))?;
        match provider.api_key_ref.as_deref() {
            Some(key) if !key.is_empty() => Ok(self.secrets().get(key)?),
            _ => Ok(None),
        }
    }

    /// 对已保存的站点重新拉一次模型列表并落库。
    pub async fn refresh_ai_provider_models(&self, id: i64) -> Result<Vec<String>, EngineError> {
        let provider = self
            .store()
            .get_ai_provider(id)?
            .ok_or(EngineError::AiProviderNotFound(id))?;
        let secret = match provider.api_key_ref.as_deref() {
            Some(key) => self.secrets().get(key)?,
            None => None,
        };
        let route = self.resolve_route(AccountProxyMode::InheritGlobal)?;
        let endpoint = Endpoint {
            kind: provider_kind(provider.kind),
            base_url: &provider.base_url,
            api_key: secret.as_ref(),
        };
        let models = list_models(route.as_ref(), &endpoint, AI_TIMEOUT).await?;
        if !self.store().update_ai_provider_models(id, &models)? {
            return Err(EngineError::AiProviderNotFound(id));
        }
        Ok(models)
    }

    /// 设置一个功能用哪个站点、哪个模型、哪档思考程度。
    pub fn set_ai_feature(
        &self,
        function: AiFunction,
        provider_id: i64,
        model: &str,
        thinking_level: Option<AiThinkingLevel>,
    ) -> Result<(), EngineError> {
        if self.store().get_ai_provider(provider_id)?.is_none() {
            return Err(EngineError::AiProviderNotFound(provider_id));
        }
        self.store()
            .upsert_ai_model_map(function, provider_id, model.trim(), thinking_level)?;
        Ok(())
    }

    /// 清除一个功能的站点选择，回退到第一个启用的站点。
    pub fn clear_ai_feature(&self, function: AiFunction) -> Result<(), EngineError> {
        self.store().delete_ai_model_map(function)?;
        Ok(())
    }

    /// 一键关闭全部 AI；返回停掉的站点数。
    pub fn disable_all_ai(&self) -> Result<usize, EngineError> {
        self.clear_ai_authorizations();
        Ok(self.store().disable_all_ai_providers()?)
    }

    /// 清空全部 AI 缓存；返回清掉条数。
    pub fn clear_ai_cache(&self) -> Result<usize, EngineError> {
        Ok(self.store().clear_ai_cache()?)
    }

    /// 最近的 AI 审计。
    pub fn list_ai_audit(&self, limit: usize) -> Result<Vec<StoredAiAudit>, EngineError> {
        Ok(self.store().list_ai_audit(limit)?)
    }

    /// 外发授权弹窗要展示的目标信息。
    ///
    /// 只读本地内容并计算缓存命中，不发网络请求；返回的一次性令牌随后交给
    /// `translate_message` / `summarize_message` 等真正执行。
    pub fn ai_authorization_preview(
        &self,
        function: AiFunction,
        message_id: Option<i64>,
        target_language: &str,
        text: Option<&str>,
    ) -> Result<AiAuthorizationPreview, EngineError> {
        let target = self.resolve_ai_target(function)?;
        let (source_hash, from_cache) =
            self.authorization_source(function, message_id, target_language, text, &target)?;
        let token = if from_cache {
            String::new()
        } else {
            self.issue_authorization(function, &target, target_language, &source_hash)?
        };
        Ok(AiAuthorizationPreview {
            function: function.as_str().to_string(),
            provider_id: target.provider_id,
            provider_label: target.provider_label,
            host: host_of(&target.base_url),
            model: target.model,
            local: target.local,
            content_hash: source_hash,
            authorization_token: token,
            expires_in_seconds: AI_AUTHORIZATION_TTL.as_secs(),
            from_cache,
        })
    }

    /// 预览阶段只读本地正文；返回内容哈希与是否命中缓存。
    fn authorization_source(
        &self,
        function: AiFunction,
        message_id: Option<i64>,
        target_language: &str,
        text: Option<&str>,
        target: &AiTarget,
    ) -> Result<(String, bool), EngineError> {
        let source = match function {
            AiFunction::Translate | AiFunction::Summary => {
                let message_id = message_id
                    .ok_or_else(|| EngineError::BadRequest("这个功能需要先选择一封邮件".to_string()))?;
                let body = self.store().get_message_body(message_id)?.ok_or_else(|| {
                    EngineError::BadRequest("邮件正文还没下载，请先打开一次这封邮件".to_string())
                })?;
                match function {
                    AiFunction::Translate => match (&body.html_sanitized, &body.text_plain) {
                        (Some(html), _) => split_html(html),
                        (None, Some(plain)) => split_text(plain),
                        (None, None) => Vec::new(),
                    }
                    .into_iter()
                    .map(|segment| segment.text)
                    .collect::<Vec<_>>()
                    .join("\n"),
                    AiFunction::Summary => body
                        .text_plain
                        .clone()
                        .or_else(|| {
                            body.html_sanitized.as_deref().map(|html| {
                                split_html(html)
                                    .into_iter()
                                    .map(|segment| segment.text)
                                    .collect::<Vec<_>>()
                                    .join("\n\n")
                            })
                        })
                        .unwrap_or_default(),
                    _ => unreachable!(),
                }
            }
            AiFunction::Polish | AiFunction::Draft => text.unwrap_or_default().to_string(),
            AiFunction::NotificationVerify => {
                return Err(EngineError::BadRequest(
                    "通知智能识别是后台自动功能，不走逐次授权".to_string(),
                ));
            }
        };
        if source.trim().is_empty() {
            return Err(EngineError::BadRequest(
                "没有可供 AI 处理的正文或文字".to_string(),
            ));
        }
        let source_hash = hash_text(&source);
        let key = cache_key(function, target, target_language, &source_hash);
        let from_cache = self
            .store()
            .get_ai_cache(&key)?
            .is_some_and(|cached| !cached.segments_json.is_empty());
        Ok((source_hash, from_cache))
    }

    /// 生成一次性外发授权令牌。
    fn issue_authorization(
        &self,
        function: AiFunction,
        target: &AiTarget,
        target_language: &str,
        content_hash: &str,
    ) -> Result<String, EngineError> {
        let mut bytes = [0_u8; 24];
        getrandom::fill(&mut bytes)
            .map_err(|_| EngineError::BadRequest("无法生成外发授权，请重试".to_string()))?;
        let token = bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let pending = PendingAiAuthorization {
            token: token.clone(),
            function,
            provider_id: target.provider_id,
            provider_label: target.provider_label.clone(),
            host: host_of(&target.base_url),
            model: target.model.clone(),
            thinking_level: target.thinking_level,
            local: target.local,
            target_language: target_language.to_string(),
            content_hash: content_hash.to_string(),
            expires_at: Instant::now() + AI_AUTHORIZATION_TTL,
        };
        self.ai_authorizations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(token.clone(), pending);
        Ok(token)
    }

    /// 校验并一次性消费授权令牌。缓存命中不调用这里，也不需要令牌。
    #[allow(clippy::too_many_arguments)]
    fn consume_authorization(
        &self,
        token: &str,
        function: AiFunction,
        provider_id: i64,
        model: &str,
        thinking_level: AiThinkingLevel,
        target_language: &str,
        content_hash: &str,
    ) -> Result<(), EngineError> {
        if token.trim().is_empty() {
            return Err(EngineError::AiAuthorizationRequired);
        }
        let now = Instant::now();
        let mut guard = self
            .ai_authorizations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(pending) = guard.get(token).cloned() else {
            return Err(EngineError::AiAuthorizationInvalid);
        };
        guard.remove(token);
        let expires_at = pending.expires_at;
        drop(guard);
        if expires_at <= now
            || pending.function != function
            || pending.provider_id != provider_id
            || pending.model != model
            || pending.thinking_level != thinking_level
            || pending.target_language != target_language
            || pending.content_hash != content_hash
        {
            return Err(EngineError::AiAuthorizationInvalid);
        }
        Ok(())
    }

    /// 清掉所有待授权令牌；一键关闭和删除站点时调用。
    pub(crate) fn clear_ai_authorizations(&self) {
        self.ai_authorizations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    /// 翻译一封邮件；三种显示模式共用这里返回的段落对齐译文。
    pub async fn translate_message(
        &self,
        message_id: i64,
        target_language: &str,
        authorization_token: &str,
    ) -> Result<AiTranslation, EngineError> {
        let target = self.resolve_ai_target(AiFunction::Translate)?;
        let body = self.get_message_body(message_id, false).await?;
        let segments = match (&body.html, &body.text_plain) {
            (Some(html), _) => split_html(html),
            (None, Some(text)) => split_text(text),
            (None, None) => Vec::new(),
        };
        if segments.is_empty() {
            return Ok(AiTranslation {
                original: Vec::new(),
                translated: Vec::new(),
                thinking_downgraded: false,
                from_cache: true,
            });
        }

        let texts: Vec<String> = segments.iter().map(|segment| segment.text.clone()).collect();
        let source_hash = hash_text(&texts.join("\n"));
        let cache_key = cache_key(AiFunction::Translate, &target, target_language, &source_hash);
        if let Some(cached) = self.store().get_ai_cache(&cache_key)? {
            if let Ok(translated) = serde_json::from_str::<Vec<String>>(&cached.segments_json) {
                if translated.len() == segments.len() {
                    return Ok(AiTranslation {
                        original: texts.clone(),
                        translated,
                        thinking_downgraded: cached.thinking_downgraded,
                        from_cache: true,
                    });
                }
            }
        }

        self.consume_authorization(
            authorization_token,
            AiFunction::Translate,
            target.provider_id,
            &target.model,
            target.thinking_level,
            target_language,
            &source_hash,
        )?;
        let secret = self.provider_secret(&target)?;
        let route = self.resolve_route(AccountProxyMode::InheritGlobal)?;
        let endpoint = Endpoint {
            kind: provider_kind(target.kind),
            base_url: &target.base_url,
            api_key: secret.as_ref(),
        };
        let outcome = translate(
            route.as_ref(),
            &endpoint,
            &target.model,
            thinking_level(target.thinking_level),
            &texts,
            target_language,
            AI_TIMEOUT,
        )
        .await
        .map_err(|err| self.audit_ai_error(AiFunction::Translate, &target, err))?;
        let TranslationOutcome {
            segments: translated,
            thinking_downgraded,
        } = outcome;

        let payload = serde_json::to_string(&translated)
            .map_err(|_| EngineError::BadRequest("翻译结果无法保存".to_string()))?;
        self.store().upsert_ai_cache(
            &cache_key,
            Some(message_id),
            AiFunction::Translate.as_str(),
            &target.model,
            target_language,
            &source_hash,
            &payload,
            thinking_downgraded,
        )?;
        self.audit_ai_success(AiFunction::Translate, &target, thinking_downgraded)?;

        Ok(AiTranslation {
            original: texts,
            translated,
            thinking_downgraded,
            from_cache: false,
        })
    }

    /// 摘要一封邮件。
    pub async fn summarize_message(
        &self,
        message_id: i64,
        authorization_token: &str,
    ) -> Result<AiTextOutcome, EngineError> {
        let target = self.resolve_ai_target(AiFunction::Summary)?;
        let body = self.get_message_body(message_id, false).await?;
        let plain = match body.text_plain.as_deref() {
            // 纯文本正文里链接本就是明文，直接送。
            Some(text) => text.to_string(),
            // HTML 正文只切出可见文字，超链接地址会丢；补上 href 才能识别链接式验证。
            None => match body.html.as_deref() {
                Some(html) => build_notification_body(html),
                None => String::new(),
            },
        };
        if plain.trim().is_empty() {
            return Err(EngineError::BadRequest("这封邮件没有可摘要的正文".to_string()));
        }

        let source_hash = hash_text(&plain);
        let cache_key = cache_key(AiFunction::Summary, &target, "", &source_hash);
        if let Some(cached) = self.store().get_ai_cache(&cache_key)? {
            if let Ok(segments) = serde_json::from_str::<Vec<String>>(&cached.segments_json) {
                if let Some(text) = segments.into_iter().next() {
                    return Ok(AiTextOutcome {
                        text,
                        thinking_downgraded: cached.thinking_downgraded,
                        from_cache: true,
                    });
                }
            }
        }

        self.consume_authorization(
            authorization_token,
            AiFunction::Summary,
            target.provider_id,
            &target.model,
            target.thinking_level,
            "",
            &source_hash,
        )?;
        let secret = self.provider_secret(&target)?;
        let route = self.resolve_route(AccountProxyMode::InheritGlobal)?;
        let endpoint = Endpoint {
            kind: provider_kind(target.kind),
            base_url: &target.base_url,
            api_key: secret.as_ref(),
        };
        let subject = String::new();
        let from = String::new();
        let outcome = chat(
            route.as_ref(),
            &endpoint,
            &target.model,
            thinking_level(target.thinking_level),
            &prompt::summary_system(),
            &prompt::summary_user(&subject, &from, &plain),
            AI_TIMEOUT,
        )
        .await
        .map_err(|err| self.audit_ai_error(AiFunction::Summary, &target, err))?;
        self.finish_text(
            AiFunction::Summary,
            &target,
            Some(message_id),
            "",
            &source_hash,
            outcome,
        )
    }

    /// 润色一段文字（通常是用户正在写的草稿）。
    pub async fn polish_text(
        &self,
        text: &str,
        authorization_token: &str,
    ) -> Result<AiTextOutcome, EngineError> {
        let target = self.resolve_ai_target(AiFunction::Polish)?;
        if text.trim().is_empty() {
            return Err(EngineError::BadRequest("请先输入要润色的文字".to_string()));
        }
        let source_hash = hash_text(text);
        let cache_key = cache_key(AiFunction::Polish, &target, "", &source_hash);
        if let Some(cached) = self.store().get_ai_cache(&cache_key)? {
            if let Ok(segments) = serde_json::from_str::<Vec<String>>(&cached.segments_json) {
                if let Some(text) = segments.into_iter().next() {
                    return Ok(AiTextOutcome {
                        text,
                        thinking_downgraded: cached.thinking_downgraded,
                        from_cache: true,
                    });
                }
            }
        }

        self.consume_authorization(
            authorization_token,
            AiFunction::Polish,
            target.provider_id,
            &target.model,
            target.thinking_level,
            "",
            &source_hash,
        )?;
        let secret = self.provider_secret(&target)?;
        let route = self.resolve_route(AccountProxyMode::InheritGlobal)?;
        let endpoint = Endpoint {
            kind: provider_kind(target.kind),
            base_url: &target.base_url,
            api_key: secret.as_ref(),
        };
        let outcome = chat(
            route.as_ref(),
            &endpoint,
            &target.model,
            thinking_level(target.thinking_level),
            &prompt::polish_system(),
            &prompt::polish_user(text),
            AI_TIMEOUT,
        )
        .await
        .map_err(|err| self.audit_ai_error(AiFunction::Polish, &target, err))?;
        self.finish_text(AiFunction::Polish, &target, None, "", &source_hash, outcome)
    }

    /// 按一句要求起草正文。
    pub async fn draft_text(
        &self,
        instruction: &str,
        authorization_token: &str,
    ) -> Result<AiTextOutcome, EngineError> {
        let target = self.resolve_ai_target(AiFunction::Draft)?;
        if instruction.trim().is_empty() {
            return Err(EngineError::BadRequest("请先写下起草要求".to_string()));
        }
        let source_hash = hash_text(instruction);
        let cache_key = cache_key(AiFunction::Draft, &target, "", &source_hash);
        if let Some(cached) = self.store().get_ai_cache(&cache_key)? {
            if let Ok(segments) = serde_json::from_str::<Vec<String>>(&cached.segments_json) {
                if let Some(text) = segments.into_iter().next() {
                    return Ok(AiTextOutcome {
                        text,
                        thinking_downgraded: cached.thinking_downgraded,
                        from_cache: true,
                    });
                }
            }
        }

        self.consume_authorization(
            authorization_token,
            AiFunction::Draft,
            target.provider_id,
            &target.model,
            target.thinking_level,
            "",
            &source_hash,
        )?;
        let secret = self.provider_secret(&target)?;
        let route = self.resolve_route(AccountProxyMode::InheritGlobal)?;
        let endpoint = Endpoint {
            kind: provider_kind(target.kind),
            base_url: &target.base_url,
            api_key: secret.as_ref(),
        };
        let outcome = chat(
            route.as_ref(),
            &endpoint,
            &target.model,
            thinking_level(target.thinking_level),
            &prompt::draft_system(),
            &prompt::draft_user(instruction),
            AI_TIMEOUT,
        )
        .await
        .map_err(|err| self.audit_ai_error(AiFunction::Draft, &target, err))?;
        self.finish_text(AiFunction::Draft, &target, None, "", &source_hash, outcome)
    }

    /// 识别一封新邮件里的验证码和验证链接（通知智能识别专用）。
    ///
    /// 这条路径是规格批准的长期外发例外：开关打开并确认风险后，不再逐封要一次性授权令牌。
    /// 例外只作用于本功能，其它 AI 功能仍要逐次授权。
    ///
    /// AI 输出始终当不可信内容：解析出来的候选值再走一遍本地白名单校验，只有通过的值才会返回。
    /// 正文、验证码和链接都不落库、不写日志；审计只记功能、站点、模型与结果分类。
    pub async fn identify_notification_verification(
        &self,
        message_id: i64,
        from: &str,
        subject: &str,
    ) -> Result<VerificationFinding, EngineError> {
        let target = self.resolve_ai_target(AiFunction::NotificationVerify)?;
        let body = self.get_message_body(message_id, false).await?;
        let plain = match body.text_plain.as_deref() {
            // 纯文本正文里链接本就是明文，直接送。
            Some(text) => text.to_string(),
            // HTML 正文只切出可见文字，超链接地址会丢；补上 href 才能识别链接式验证。
            None => match body.html.as_deref() {
                Some(html) => build_notification_body(html),
                None => String::new(),
            },
        };
        if plain.trim().is_empty() {
            return Err(EngineError::BadRequest("这封邮件没有可识别的正文".to_string()));
        }

        let secret = self.provider_secret(&target)?;
        let route = self.resolve_route(AccountProxyMode::InheritGlobal)?;
        let endpoint = Endpoint {
            kind: provider_kind(target.kind),
            base_url: &target.base_url,
            api_key: secret.as_ref(),
        };
        let outcome = chat(
            route.as_ref(),
            &endpoint,
            &target.model,
            thinking_level(target.thinking_level),
            &prompt::notification_verify_system(),
            &prompt::notification_verify_user(from, subject, &plain),
            NOTIFICATION_VERIFY_TIMEOUT,
        )
        .await
        .map_err(|err| self.audit_ai_error(AiFunction::NotificationVerify, &target, err))?;
        self.audit_ai_success(
            AiFunction::NotificationVerify,
            &target,
            outcome.thinking_downgraded,
        )?;
        Ok(parse_verification(&outcome.text))
    }

    /// 把一次对话结果写缓存、写审计，并转成界面结果。
    fn finish_text(
        &self,
        function: AiFunction,
        target: &AiTarget,
        message_id: Option<i64>,
        target_lang: &str,
        source_hash: &str,
        outcome: ChatOutcome,
    ) -> Result<AiTextOutcome, EngineError> {
        let key = cache_key(function, target, target_lang, source_hash);
        let payload = serde_json::to_string(std::slice::from_ref(&outcome.text))
            .map_err(|_| EngineError::BadRequest("AI 结果无法保存".to_string()))?;
        self.store().upsert_ai_cache(
            &key,
            message_id,
            function.as_str(),
            &target.model,
            target_lang,
            source_hash,
            &payload,
            outcome.thinking_downgraded,
        )?;
        self.audit_ai_success(function, target, outcome.thinking_downgraded)?;
        Ok(AiTextOutcome {
            text: outcome.text,
            thinking_downgraded: outcome.thinking_downgraded,
            from_cache: false,
        })
    }

    /// 按功能选出站点、模型与思考程度。
    fn resolve_ai_target(&self, function: AiFunction) -> Result<AiTarget, EngineError> {
        let map = self.store().get_ai_model_map(function)?;
        let provider = match map.as_ref() {
            Some(entry) => self
                .store()
                .get_ai_provider(entry.provider_id)?
                .filter(|provider| provider.enabled)
                .ok_or(EngineError::AiProviderNotFound(entry.provider_id))?,
            None => self
                .store()
                .list_ai_providers()?
                .into_iter()
                .find(|provider| provider.enabled)
                .ok_or(EngineError::AiDisabled)?,
        };
        let model = map
            .as_ref()
            .map(|entry| entry.model.trim())
            .filter(|model| !model.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| provider.default_model.trim().to_string());
        if model.is_empty() {
            return Err(EngineError::BadRequest(
                "这个功能还没有选择模型，请先到设置里选择或手工填写".to_string(),
            ));
        }
        let thinking = map
            .as_ref()
            .and_then(|entry| entry.thinking_level)
            .unwrap_or(provider.thinking_level);
        Ok(AiTarget {
            provider_id: provider.id,
            provider_label: provider.label.clone(),
            base_url: provider.base_url.clone(),
            kind: provider.kind,
            model,
            thinking_level: thinking,
            local: is_local_host(&host_of(&provider.base_url)),
            api_key_ref: provider.api_key_ref.clone(),
        })
    }

    /// 取出站点密钥；没有密钥的非本机站点会在这里被拦住。
    fn provider_secret(&self, target: &AiTarget) -> Result<Option<Secret>, EngineError> {
        let secret = match target.api_key_ref.as_deref() {
            Some(key) => self.secrets().get(key)?,
            None => None,
        };
        if secret.is_none() && !target.local && target.kind != AiProviderKind::Ollama {
            return Err(AiError::MissingKey.into());
        }
        Ok(secret)
    }

    fn audit_ai_success(
        &self,
        function: AiFunction,
        target: &AiTarget,
        thinking_downgraded: bool,
    ) -> Result<(), EngineError> {
        self.store().insert_ai_audit(&NewAiAudit {
            function: function.as_str().to_string(),
            provider_id: Some(target.provider_id),
            provider_label: target.provider_label.clone(),
            model: target.model.clone(),
            target_host: host_of(&target.base_url),
            local: target.local,
            outbound: !target.local,
            outcome: if thinking_downgraded { "downgraded" } else { "ok" }.to_string(),
            detail: String::new(),
        })?;
        Ok(())
    }

    /// 失败也要留一条审计；只记错误大类，不记正文与密钥。
    fn audit_ai_error(&self, function: AiFunction, target: &AiTarget, err: AiError) -> EngineError {
        let detail = match &err {
            AiError::Timeout => "超时",
            AiError::Protocol => "响应无法识别",
            AiError::Status { .. } => "站点返回错误",
            AiError::Network(_) => "网络失败",
            _ => "调用失败",
        };
        if let Err(logged) = self.store().insert_ai_audit(&NewAiAudit {
            function: function.as_str().to_string(),
            provider_id: Some(target.provider_id),
            provider_label: target.provider_label.clone(),
            model: target.model.clone(),
            target_host: host_of(&target.base_url),
            local: target.local,
            outbound: !target.local,
            outcome: "failed".to_string(),
            detail: detail.to_string(),
        }) {
            tracing::warn!(error = %logged, "写 AI 审计失败");
        }
        err.into()
    }

    fn get_ai_provider_view(&self, id: i64) -> Result<AiProviderView, EngineError> {
        let provider = self
            .store()
            .get_ai_provider(id)?
            .ok_or(EngineError::AiProviderNotFound(id))?;
        Ok(provider_view(provider))
    }
}

/// 把存储行转成界面视图。
fn provider_view(provider: StoredAiProvider) -> AiProviderView {
    AiProviderView {
        id: provider.id,
        label: provider.label,
        kind: provider.kind,
        base_url: provider.base_url,
        default_model: provider.default_model,
        models: provider.models,
        thinking_level: provider.thinking_level,
        enabled: provider.enabled,
        has_key: provider.api_key_ref.is_some(),
    }
}

/// 计算缓存键：功能 + 站点 + 模型 + 语言 + 原文哈希。
fn cache_key(function: AiFunction, target: &AiTarget, target_lang: &str, source_hash: &str) -> String {
    hash_text(&format!(
        "{}|{}|{}|{}|{}",
        function.as_str(),
        target.provider_id,
        target.model,
        target_lang,
        source_hash
    ))
}

/// 文本哈希（十六进制小写）。
fn hash_text(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 主机名提取支持端口与路径() {
        assert_eq!(host_of("https://api.example.com/v1"), "api.example.com");
        assert_eq!(host_of("http://127.0.0.1:11434/v1"), "127.0.0.1");
        assert_eq!(host_of("https://user@example.com:8443/x"), "example.com");
    }

    #[test]
    fn 缓存键区分模型与语言() {
        let target = AiTarget {
            provider_id: 1,
            provider_label: "本机".to_string(),
            base_url: "http://127.0.0.1:11434/v1".to_string(),
            kind: AiProviderKind::Ollama,
            model: "qwen".to_string(),
            thinking_level: AiThinkingLevel::Off,
            local: true,
            api_key_ref: None,
        };
        let a = cache_key(AiFunction::Translate, &target, "zh", "hash");
        let b = cache_key(AiFunction::Translate, &target, "en", "hash");
        let mut other = target.clone();
        other.model = "qwen2".to_string();
        let c = cache_key(AiFunction::Translate, &other, "zh", "hash");
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn 哈希稳定且不携带原文() {
        let hash = hash_text("邮件正文");
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, hash_text("邮件正文"));
        assert!(!hash.contains("邮件"));
    }

    #[test]
    fn 送检正文保留可见文字并补上超链接地址() {
        let html =
            r#"<p>验证码 123456</p><p>或点<a href="https://example.com/verify?token=abc">这里</a>验证</p>"#;
        let text = build_notification_body(html);
        assert!(text.contains("验证码 123456"), "应保留可见文字：{text}");
        assert!(text.contains("这里"), "应保留链接可见文字：{text}");
        assert!(
            text.contains("链接：https://example.com/verify?token=abc"),
            "应补上超链接地址：{text}"
        );
    }
    fn engine(dir: &std::path::Path) -> MailEngine {
        MailEngine::initialize_with_secrets(
            dir,
            std::sync::Arc::new(crate::secrets::MemorySecretStore::new()),
        )
        .expect("初始化引擎")
    }

    fn add_ollama(engine: &MailEngine) -> i64 {
        engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "本机".to_string(),
                    kind: AiProviderKind::Ollama,
                    base_url: "http://127.0.0.1:11434/v1".to_string(),
                    default_model: "qwen".to_string(),
                    models: vec!["qwen".to_string()],
                    thinking_level: AiThinkingLevel::Off,
                    enabled: true,
                },
                None,
            )
            .expect("保存站点")
            .id
    }

    #[test]
    fn 默认关闭时拒绝预览且不给令牌() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let err = engine
            .ai_authorization_preview(AiFunction::Summary, None, "", Some("正文"))
            .expect_err("默认关闭应拒绝");
        assert!(matches!(err, EngineError::AiDisabled));
    }

    #[test]
    fn 非本机明文地址保存时就拒绝() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let err = engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "远程".to_string(),
                    kind: AiProviderKind::OpenAiCompatible,
                    base_url: "http://api.example.com/v1".to_string(),
                    default_model: "m".to_string(),
                    models: Vec::new(),
                    thinking_level: AiThinkingLevel::Off,
                    enabled: true,
                },
                None,
            )
            .expect_err("非本机明文应拒绝");
        assert!(matches!(err, EngineError::Ai(AiError::InsecureEndpoint(_))));
    }

    #[test]
    fn 本机地址允许明文并可以预览() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        add_ollama(&engine);
        let preview = engine
            .ai_authorization_preview(AiFunction::Polish, None, "", Some("正文"))
            .expect("本机应可预览");
        assert!(preview.local);
        assert_eq!(preview.host, "127.0.0.1");
        assert_eq!(preview.model, "qwen");
        assert!(!preview.authorization_token.is_empty());
        assert_eq!(preview.expires_in_seconds, AI_AUTHORIZATION_TTL.as_secs());
    }

    #[test]
    fn 站点密钥只进保险箱且出参不带明文() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = std::sync::Arc::new(crate::secrets::MemorySecretStore::new());
        let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("初始化引擎");
        let view = engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "中转".to_string(),
                    kind: AiProviderKind::OpenAiCompatible,
                    base_url: "https://api.example.com/v1".to_string(),
                    default_model: "m".to_string(),
                    models: Vec::new(),
                    thinking_level: AiThinkingLevel::Off,
                    enabled: true,
                },
                Some(&Secret::new("super-secret-key")),
            )
            .expect("保存站点");
        assert!(view.has_key);
        assert_eq!(secrets.len(), 1);
        let raw = engine
            .store()
            .get_ai_provider(view.id)
            .expect("查询")
            .expect("存在");
        assert!(raw.api_key_ref.is_some());
        assert!(!raw
            .api_key_ref
            .as_deref()
            .unwrap_or_default()
            .contains("super-secret-key"));
        assert!(!format!("{view:?}").contains("super-secret-key"));
    }

    #[test]
    fn 编辑站点时留空密钥会保留旧密钥() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = std::sync::Arc::new(crate::secrets::MemorySecretStore::new());
        let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("初始化引擎");
        let input = AiProviderInput {
            id: None,
            label: "中转".to_string(),
            kind: AiProviderKind::OpenAiCompatible,
            base_url: "https://api.example.com/v1".to_string(),
            default_model: "m".to_string(),
            models: Vec::new(),
            thinking_level: AiThinkingLevel::Off,
            enabled: true,
        };
        let created = engine
            .save_ai_provider(&input, Some(&Secret::new("keep-me")))
            .expect("新建");
        let old_ref = engine
            .store()
            .get_ai_provider(created.id)
            .expect("查询")
            .expect("存在")
            .api_key_ref;

        // 编辑时留空（命令层传 None）不能把已存密钥清掉。
        let edited = engine
            .save_ai_provider(
                &AiProviderInput {
                    id: Some(created.id),
                    label: "改过名字".to_string(),
                    ..input.clone()
                },
                None,
            )
            .expect("编辑");
        assert!(edited.has_key, "留空编辑后仍应有密钥");
        assert_eq!(secrets.len(), 1, "旧密钥不应被删");
        let new_ref = engine
            .store()
            .get_ai_provider(created.id)
            .expect("查询")
            .expect("存在")
            .api_key_ref;
        assert_eq!(new_ref, old_ref, "留空编辑不应换密钥引用");

        // 明确传空串才表示清空。
        let cleared = engine
            .save_ai_provider(
                &AiProviderInput {
                    id: Some(created.id),
                    label: "改过名字".to_string(),
                    ..input.clone()
                },
                Some(&Secret::new("")),
            )
            .expect("清空");
        assert!(!cleared.has_key, "显式清空后不应再有密钥");
        assert_eq!(secrets.len(), 0, "显式清空应删掉保险箱条目");
    }

    #[test]
    fn 编辑站点测试连接能回退到已存密钥() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = std::sync::Arc::new(crate::secrets::MemorySecretStore::new());
        let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("初始化引擎");
        let created = engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "中转".to_string(),
                    kind: AiProviderKind::OpenAiCompatible,
                    base_url: "https://api.example.com/v1".to_string(),
                    default_model: "m".to_string(),
                    models: Vec::new(),
                    thinking_level: AiThinkingLevel::Off,
                    enabled: true,
                },
                Some(&Secret::new("stored-key")),
            )
            .expect("新建");

        let stored = engine
            .stored_ai_provider_secret(created.id)
            .expect("读取已存密钥")
            .expect("应有密钥");
        assert_eq!(stored.expose(), "stored-key");
    }

    #[test]
    fn 密钥明文不会落到任何数据文件() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = std::sync::Arc::new(crate::secrets::MemorySecretStore::new());
        let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("初始化引擎");
        let exposed = "super-secret-key-9527";
        engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "中转".to_string(),
                    kind: AiProviderKind::OpenAiCompatible,
                    base_url: "https://api.example.com/v1".to_string(),
                    default_model: "m".to_string(),
                    models: Vec::new(),
                    thinking_level: AiThinkingLevel::Off,
                    enabled: true,
                },
                Some(&Secret::new(exposed)),
            )
            .expect("保存站点");
        drop(engine);
        drop(secrets);

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
        assert!(!found, "任何数据文件里都不应出现明文 AI 密钥");
    }

    #[test]
    fn 授权令牌只能消费一次() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let provider_id = add_ollama(&engine);
        let preview = engine
            .ai_authorization_preview(AiFunction::Polish, None, "", Some("要润色的字"))
            .expect("预览");
        assert!(!preview.from_cache);
        let hash = hash_text("要润色的字");
        engine
            .consume_authorization(
                &preview.authorization_token,
                AiFunction::Polish,
                provider_id,
                "qwen",
                AiThinkingLevel::Off,
                "",
                &hash,
            )
            .expect("第一次应通过");
        let second = engine
            .consume_authorization(
                &preview.authorization_token,
                AiFunction::Polish,
                provider_id,
                "qwen",
                AiThinkingLevel::Off,
                "",
                &hash,
            )
            .expect_err("第二次应失败");
        assert!(matches!(second, EngineError::AiAuthorizationInvalid));
    }

    #[test]
    fn 缓存命中时预览不发令牌() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        add_ollama(&engine);
        let text = "已经缓存过的文字";
        let hash = hash_text(text);
        let target = engine.resolve_ai_target(AiFunction::Polish).expect("目标");
        let key = cache_key(AiFunction::Polish, &target, "", &hash);
        engine
            .store()
            .upsert_ai_cache(
                &key,
                None,
                AiFunction::Polish.as_str(),
                "qwen",
                "",
                &hash,
                r#"["润色结果"]"#,
                false,
            )
            .expect("写缓存");
        let preview = engine
            .ai_authorization_preview(AiFunction::Polish, None, "", Some(text))
            .expect("预览");
        assert!(preview.from_cache);
        assert!(preview.authorization_token.is_empty());
    }
}
