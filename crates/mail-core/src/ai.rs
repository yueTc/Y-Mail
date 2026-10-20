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
    anchor_targets, chat, classify_mail_groups, is_local_host, list_models, normalize_base_url,
    parse_verification, prompt, split_html, split_text, translate, AiError, ChatOutcome, Endpoint,
    GroupAnswer, GroupQuestion, GroupQuestionKind, ProviderFunction, ProviderKind, ThinkingLevel,
    TranslationOutcome, VerificationFinding,
};
use mail_domain::account::AccountProxyMode;
use mail_domain::proxy::Secret;
use mail_store::{
    AiFunction, AiModelMapEntry, AiProviderKind, AiThinkingLevel, ClassificationGroupWrite, LabelGroup,
    LabelGroupKind, MailLabel, MailLabelSource, MessageGroupResult, NewAiAudit, NewAiProvider, StoredAiAudit,
    StoredAiProvider,
};

use crate::engine::{EngineError, MailEngine};
use crate::proxies::new_credential_key;

/// AI 请求超时：本地模型可能较慢，给到 60 秒。
pub const AI_TIMEOUT: Duration = Duration::from_secs(60);

/// 通知识别的单封等待上限；超了就退回普通通知。
pub const NOTIFICATION_VERIFY_TIMEOUT: Duration = Duration::from_secs(15);

/// 分类外发的正文上限：只发前 2000 个字符。
pub const CLASSIFY_BODY_MAX_CHARS: usize = 2000;

/// 一次外发授权令牌的有效期。
pub const AI_AUTHORIZATION_TTL: Duration = Duration::from_secs(300);

/// 站点类型在存储层与 AI 层之间的映射。
pub(crate) fn provider_kind(kind: AiProviderKind) -> ProviderKind {
    match kind {
        AiProviderKind::OpenAiCompatible => ProviderKind::OpenAiCompatible,
        AiProviderKind::DeepL => ProviderKind::DeepL,
        AiProviderKind::Ollama => ProviderKind::Ollama,
        AiProviderKind::TypeSafe => ProviderKind::TypeSafe,
        AiProviderKind::DecisionApi => ProviderKind::DecisionApi,
    }
}

/// 存储层功能与 AI 层功能的一对一映射。
pub fn provider_function(function: AiFunction) -> ProviderFunction {
    match function {
        AiFunction::Translate => ProviderFunction::Translation,
        AiFunction::Summary => ProviderFunction::Summary,
        AiFunction::Polish => ProviderFunction::Polish,
        AiFunction::Draft => ProviderFunction::Draft,
        AiFunction::NotificationVerify => ProviderFunction::NotificationVerify,
        AiFunction::Classify => ProviderFunction::Classification,
    }
}

/// 站点类型是否支持某个功能；命令层与界面用它过滤功能级站点下拉。
pub fn ai_provider_supports(kind: AiProviderKind, function: AiFunction) -> bool {
    provider_kind(kind).supports(provider_function(function))
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
    /// 站点级代理策略。
    pub proxy: AccountProxyMode,
    /// 是否本机服务。
    pub local: bool,
    /// 保密的系统凭据引用键；不对外展示。
    pub api_key_ref: Option<String>,
}

/// 「检查延迟」的结果：一次测试外发花了多久、认出了什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationLatency {
    /// 一次外发往返的毫秒数。
    pub elapsed_ms: u64,
    /// 有没有认出验证码。
    pub found_code: bool,
    /// 有没有认出验证链接。
    pub found_link: bool,
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
    /// 站点级代理策略。
    pub proxy: AccountProxyMode,
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
    /// 站点级代理策略。
    pub proxy: AccountProxyMode,
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

/// 一批邮件的分类外发确认预览。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifyPreview {
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
    /// 一次性授权令牌。
    pub authorization_token: String,
    /// 令牌有效期（秒）。
    pub expires_in_seconds: u64,
    /// 本次要分类的邮件与各自内容哈希。
    pub targets: Vec<ClassifyPreviewTarget>,
    /// 本次将开启的标签组。
    pub enabled_groups: Vec<ClassifyPreviewGroup>,
}

/// 分类预览里的一个「本次将开启的标签组」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifyPreviewGroup {
    /// 组名。
    pub name: String,
    /// 题型标识：choice / boolean / score。
    pub kind: String,
}

/// 分类预览里的一封目标邮件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifyPreviewTarget {
    /// 邮件主键。
    pub message_id: i64,
    /// 外发正文的内容哈希；只用于校验，不包含正文。
    pub content_hash: String,
}

/// 一次分类里，一封邮件本次写下的各组结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifiedMessageGroups {
    /// 邮件主键。
    pub message_id: i64,
    /// 这封邮件本次写下的各组结果，按组显示顺序。
    pub results: Vec<MessageGroupResult>,
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
    /// 目标邮件编号；普通单封功能为空。
    pub message_ids: Vec<i64>,
    /// 每封邮件外发正文的哈希；普通单封功能只放一条。
    pub content_hashes: Vec<String>,
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
        let view = self.save_ai_provider_inner(input, api_key)?;
        self.mark_settings_changed();
        Ok(view)
    }

    /// save_ai_provider 的真身；写成功后打同步标记由外层统一负责。
    fn save_ai_provider_inner(
        &self,
        input: &AiProviderInput,
        api_key: Option<&Secret>,
    ) -> Result<AiProviderView, EngineError> {
        let label = input.label.trim();
        if label.is_empty() {
            return Err(EngineError::BadRequest("请填写站点名称".to_string()));
        }
        let base_url = normalize_base_url(&input.base_url, provider_kind(input.kind))?;
        let default_model = input.default_model.trim();
        if input.kind == AiProviderKind::DecisionApi && default_model.is_empty() {
            return Err(EngineError::BadRequest("决策模型站点必须填写模型".to_string()));
        }
        if let AccountProxyMode::Custom(id) = input.proxy {
            if self.store().get_proxy(id)?.is_none() {
                return Err(EngineError::ProxyNotFound(id.0));
            }
        }
        let new_provider = NewAiProvider {
            label: label.to_string(),
            kind: input.kind,
            base_url,
            default_model: default_model.to_string(),
            models: input.models.clone(),
            thinking_level: input.thinking_level,
            enabled: input.enabled,
            proxy: input.proxy,
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
        self.delete_ai_provider_inner(id)?;
        self.mark_settings_changed();
        Ok(())
    }

    /// delete_ai_provider 的真身；写成功后打同步标记由外层统一负责。
    fn delete_ai_provider_inner(&self, id: i64) -> Result<(), EngineError> {
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
        model: &str,
        api_key: Option<&Secret>,
        provider_id: Option<i64>,
        proxy: AccountProxyMode,
    ) -> Result<Vec<String>, EngineError> {
        let base_url = normalize_base_url(base_url, provider_kind(kind))?;
        let model = model.trim();
        if provider_kind(kind).is_decision() && model.is_empty() {
            return Err(EngineError::BadRequest("请选择或填写测试模型".to_string()));
        }
        if let AccountProxyMode::Custom(id) = proxy {
            if self.store().get_proxy(id)?.is_none() {
                return Err(EngineError::ProxyNotFound(id.0));
            }
        }
        let fallback = match provider_id {
            Some(id) if api_key.map(|secret| secret.is_empty()).unwrap_or(true) => {
                self.stored_ai_provider_secret(id)?
            }
            _ => None,
        };
        let secret = api_key.filter(|secret| !secret.is_empty()).or(fallback.as_ref());
        let route = self.resolve_route(proxy)?;
        let endpoint = Endpoint {
            kind: provider_kind(kind),
            base_url: &base_url,
            api_key: secret,
        };
        if provider_kind(kind).is_decision() {
            // 决策站点没有「拉模型列表」接口，用固定是否题探针确认协议和答案都能走通。
            let questions = vec![GroupQuestion {
                group_id: 1,
                kind: GroupQuestionKind::Boolean,
                instructions: prompt::TYPESAFE_PROBE_QUESTION.to_string(),
                criteria: Vec::new(),
                score_min: None,
                score_max: None,
            }];
            let answers = classify_mail_groups(
                route.as_ref(),
                &endpoint,
                model,
                prompt::TYPESAFE_PROBE_STATE,
                &questions,
                NOTIFICATION_VERIFY_TIMEOUT,
            )
            .await?;
            if !answers
                .iter()
                .any(|(_, answer)| matches!(answer, GroupAnswer::Boolean { .. }))
            {
                return Err(AiError::Protocol.into());
            }
            return Ok(vec![model.to_string()]);
        }
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
        let route = self.resolve_route(provider.proxy)?;
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
        self.mark_settings_changed();
        Ok(())
    }

    /// 清除一个功能的站点选择，回退到第一个启用的站点。
    pub fn clear_ai_feature(&self, function: AiFunction) -> Result<(), EngineError> {
        self.store().delete_ai_model_map(function)?;
        self.mark_settings_changed();
        Ok(())
    }

    /// 一键关闭全部 AI；返回停掉的站点数。
    pub fn disable_all_ai(&self) -> Result<usize, EngineError> {
        self.clear_ai_authorizations();
        let count = self.store().disable_all_ai_providers()?;
        self.mark_settings_changed();
        Ok(count)
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

    /// 取全部本地标签（按显示顺序）。
    pub fn list_mail_labels(&self) -> Result<Vec<MailLabel>, EngineError> {
        Ok(self.store().list_mail_labels()?)
    }

    /// 新增标签并返回新标签。
    pub fn create_mail_label(&self, name: &str) -> Result<MailLabel, EngineError> {
        let id = self.store().create_mail_label(name)?;
        self.store()
            .get_mail_label(id)?
            .ok_or_else(|| EngineError::BadRequest("标签不存在".to_string()))
    }

    /// 给标签改名。
    pub fn rename_mail_label(&self, label_id: i64, name: &str) -> Result<(), EngineError> {
        Ok(self.store().rename_mail_label(label_id, name)?)
    }

    /// 修改标签颜色；只接受固定色板键。
    pub fn set_mail_label_color(&self, label_id: i64, color: &str) -> Result<(), EngineError> {
        Ok(self.store().set_mail_label_color(label_id, color)?)
    }

    /// 删除标签；兜底标签会被拒绝。
    pub fn delete_mail_label(&self, label_id: i64) -> Result<(), EngineError> {
        Ok(self.store().delete_mail_label(label_id)?)
    }

    /// 按给定顺序重排标签；必须传全部标签编号。
    pub fn reorder_mail_labels(&self, label_ids: &[i64]) -> Result<(), EngineError> {
        Ok(self.store().reorder_mail_labels(label_ids)?)
    }

    /// 取全部标签组，按显示顺序。
    pub fn list_label_groups(&self) -> Result<Vec<LabelGroup>, EngineError> {
        Ok(self.store().list_label_groups()?)
    }

    /// 取某个组内的标签，按显示顺序。
    pub fn list_group_labels(&self, group_id: i64) -> Result<Vec<MailLabel>, EngineError> {
        Ok(self.store().list_group_labels(group_id)?)
    }

    /// 按主键取一个标签组。
    pub fn get_label_group(&self, group_id: i64) -> Result<Option<LabelGroup>, EngineError> {
        Ok(self.store().get_label_group(group_id)?)
    }

    /// 新建标签组并返回；打分题必须同时传上下限。
    pub fn create_label_group(
        &self,
        name: &str,
        kind: &str,
        score_min: Option<i64>,
        score_max: Option<i64>,
    ) -> Result<LabelGroup, EngineError> {
        let kind = LabelGroupKind::parse(kind.trim())
            .ok_or_else(|| EngineError::BadRequest("题型只能是 choice、boolean 或 score".to_string()))?;
        let id = self
            .store()
            .create_label_group(name, kind, score_min, score_max)?;
        self.store()
            .get_label_group(id)?
            .ok_or_else(|| EngineError::BadRequest("标签组不存在".to_string()))
    }

    /// 给标签组改名。
    pub fn rename_label_group(&self, group_id: i64, name: &str) -> Result<(), EngineError> {
        Ok(self.store().rename_label_group(group_id, name)?)
    }

    /// 开或关一个标签组。
    pub fn set_label_group_enabled(&self, group_id: i64, enabled: bool) -> Result<(), EngineError> {
        Ok(self.store().set_label_group_enabled(group_id, enabled)?)
    }

    /// 改一个标签组的把握门槛（0.50 到 0.99）。
    pub fn set_label_group_threshold(&self, group_id: i64, threshold: f64) -> Result<(), EngineError> {
        Ok(self.store().set_label_group_threshold(group_id, threshold)?)
    }

    /// 删除标签组；组内标签与该组所有邮件结果由外键级联清掉。
    pub fn delete_label_group(&self, group_id: i64) -> Result<(), EngineError> {
        Ok(self.store().delete_label_group(group_id)?)
    }

    /// 按给定顺序重排全部标签组；必须传全部组编号。
    pub fn reorder_label_groups(&self, group_ids: &[i64]) -> Result<(), EngineError> {
        Ok(self.store().reorder_label_groups(group_ids)?)
    }

    /// 在某个组里新增标签并返回新标签。
    pub fn create_group_label(&self, group_id: i64, name: &str) -> Result<MailLabel, EngineError> {
        let id = self.store().create_group_label(group_id, name)?;
        self.store()
            .get_mail_label(id)?
            .ok_or_else(|| EngineError::BadRequest("标签不存在".to_string()))
    }

    /// 给某个组内标签改名。
    pub fn rename_group_label(&self, group_id: i64, label_id: i64, name: &str) -> Result<(), EngineError> {
        Ok(self.store().rename_group_label(group_id, label_id, name)?)
    }

    /// 改某个组内标签颜色；只接受固定色板键。
    pub fn set_group_label_color(
        &self,
        group_id: i64,
        label_id: i64,
        color: &str,
    ) -> Result<(), EngineError> {
        Ok(self.store().set_group_label_color(group_id, label_id, color)?)
    }

    /// 指定或取消某个组内标签的兜底；同组最多一个，兜底颜色固定灰色。
    pub fn set_group_label_fallback(
        &self,
        group_id: i64,
        label_id: i64,
        is_fallback: bool,
    ) -> Result<(), EngineError> {
        Ok(self
            .store()
            .set_group_label_fallback(group_id, label_id, is_fallback)?)
    }
    /// 删除某个组内标签；兜底标签会被拒绝。
    pub fn delete_group_label(&self, group_id: i64, label_id: i64) -> Result<(), EngineError> {
        Ok(self.store().delete_group_label(group_id, label_id)?)
    }

    /// 按给定顺序重排某个组内的全部标签。
    pub fn reorder_group_labels(&self, group_id: i64, label_ids: &[i64]) -> Result<(), EngineError> {
        Ok(self.store().reorder_group_labels(group_id, label_ids)?)
    }

    /// 手动写某封邮件某个组的结果；来源记 `manual`，该组从此锁定。
    pub fn set_message_group_value(
        &self,
        message_id: i64,
        group_id: i64,
        label_id: Option<i64>,
        score_value: Option<i64>,
    ) -> Result<(), EngineError> {
        self.store().set_message_group_value(
            message_id,
            group_id,
            label_id,
            score_value,
            MailLabelSource::Manual,
            None,
        )?;
        Ok(())
    }

    /// 读一封邮件的全部组结果，按组显示顺序。
    pub fn message_group_results(&self, message_id: i64) -> Result<Vec<MessageGroupResult>, EngineError> {
        Ok(self.store().list_message_group_results(message_id)?)
    }

    /// 清空某封邮件某组的结果并解锁，后续可以重判。
    pub fn clear_message_group_value(&self, message_id: i64, group_id: i64) -> Result<(), EngineError> {
        self.store().clear_message_group_value(message_id, group_id)?;
        Ok(())
    }

    /// 手动设置或清除某封邮件的标签；来源记 `manual`。
    pub fn set_message_label(&self, message_id: i64, label_id: Option<i64>) -> Result<(), EngineError> {
        match label_id {
            Some(label_id) => {
                self.store()
                    .set_message_label(message_id, label_id, MailLabelSource::Manual, None)?;
            }
            None => {
                self.store().clear_message_label(message_id)?;
            }
        }
        Ok(())
    }

    /// 读某封邮件的标签。
    pub fn message_label(&self, message_id: i64) -> Result<Option<MailLabel>, EngineError> {
        Ok(self
            .store()
            .get_message_label(message_id)?
            .map(|(label, _, _)| label))
    }

    /// 消费一次性分类授权，对目标邮件逐封跑一次多组分类。
    ///
    /// 令牌校验失败整体拒绝；进入分类阶段后，某一封失败不影响其它封（错误已在
    /// 分类里记过审计），返回按邮件分组的本次写下的各组结果。
    pub async fn classify_messages(&self, token: &str) -> Result<Vec<ClassifiedMessageGroups>, EngineError> {
        let ids = self.consume_classify_authorization(token)?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            match self.classify_message_groups(id).await {
                Ok(results) => out.push(ClassifiedMessageGroups {
                    message_id: id,
                    results,
                }),
                Err(error) => {
                    tracing::warn!(message_id = id, error = %error, "单封分类失败，跳过该封");
                }
            }
        }
        Ok(out)
    }

    /// 取所有开启的标签组，按排序升序。
    fn enabled_classify_groups(&self) -> Result<Vec<LabelGroup>, EngineError> {
        Ok(self
            .store()
            .list_label_groups()?
            .into_iter()
            .filter(|group| group.enabled)
            .collect())
    }

    /// 自动分类：对一封邮件跑一次多组分类，返回是否真的发了请求。
    ///
    /// 一个组都没开就跳过，不当错误。正文的下载在 [`Self::classify_message_groups`] 里统一处理。
    pub async fn classify_message_if_untagged(&self, message_id: i64) -> Result<bool, EngineError> {
        if self.enabled_classify_groups()?.is_empty() {
            return Ok(false);
        }
        self.classify_message_groups(message_id).await?;
        Ok(true)
    }

    /// 分类前把正文备齐：本地已有直接返回，没有就联网拉一次并落库。
    ///
    /// 做法跟用户点开这封邮件时一样；拉不到就把错误往外抛，交给上层决定要不要重试。
    async fn ensure_message_body_cached(&self, message_id: i64) -> Result<(), EngineError> {
        let cached = { self.store().get_message_body(message_id)?.is_some() };
        if cached {
            return Ok(());
        }
        self.get_message_body(message_id, false).await?;
        Ok(())
    }

    /// 一次请求问完所有开启的标签组，逐组按门槛落结果，返回该邮件最新的组结果。
    ///
    /// 没有开启任何组、正文缺失或空正文都按参数错误处理；整个请求失败（超时、
    /// 网络、非 200）一组都不写，只记审计；单组结果不合法只跳过该组。
    /// 落库前先看该组是不是用户手改（manual），是就跳过不覆盖。正文绝不进审计。
    pub async fn classify_message_groups(
        &self,
        message_id: i64,
    ) -> Result<Vec<MessageGroupResult>, EngineError> {
        let groups = self.enabled_classify_groups()?;
        if groups.is_empty() {
            return Err(EngineError::BadRequest("没有开启任何标签组".to_string()));
        }
        // 自动分类和手动「立即分类」都汇到这里：正文本地没有就先联网下载一次并落库，
        // 不再要求用户先点开邮件。
        self.ensure_message_body_cached(message_id).await?;
        let body = self
            .store()
            .get_message_body(message_id)?
            .ok_or_else(|| EngineError::BadRequest("邮件正文还没下载，请先打开一次这封邮件".to_string()))?;
        let text = truncate_classify_body(&classify_body_text(&body));
        if text.trim().is_empty() {
            return Err(EngineError::BadRequest("这封邮件没有可以分类的正文".to_string()));
        }

        // 逐组备好问题；任何一组当前出不了结果都按参数错误整体拒绝，一次都不发。
        let mut plans: Vec<(LabelGroup, Vec<MailLabel>, GroupQuestion)> = Vec::with_capacity(groups.len());
        for group in groups {
            let labels = self.store().list_group_labels(group.id)?;
            let question = group_question(&group, &labels)?;
            plans.push((group, labels, question));
        }
        let questions: Vec<GroupQuestion> = plans.iter().map(|(_, _, question)| question.clone()).collect();

        let target = self.resolve_ai_target(AiFunction::Classify)?;
        let secret = self.provider_secret(&target)?;
        let route = self.resolve_route(target.proxy)?;
        let endpoint = Endpoint {
            kind: provider_kind(target.kind),
            base_url: &target.base_url,
            api_key: secret.as_ref(),
        };
        let answers = classify_mail_groups(
            route.as_ref(),
            &endpoint,
            &target.model,
            &text,
            &questions,
            NOTIFICATION_VERIFY_TIMEOUT,
        )
        .await
        .map_err(|err| self.audit_ai_error(AiFunction::Classify, &target, err))?;

        let mut writes = Vec::new();
        for (group, labels, _) in &plans {
            let Some((_, answer)) = answers.iter().find(|(id, _)| *id == group.id) else {
                continue;
            };
            let Some(write) = resolve_group_write(group, labels, answer) else {
                continue;
            };
            writes.push(ClassificationGroupWrite {
                group_id: group.id,
                label_id: write.label_id,
                score_value: write.score_value,
                confidence: write.confidence,
                summary: format!("{}={}", group.name, write.summary),
            });
        }

        self.store().save_classification_batch(
            message_id,
            &writes,
            NewAiAudit {
                function: AiFunction::Classify.as_str().to_string(),
                provider_id: Some(target.provider_id),
                provider_label: target.provider_label.clone(),
                model: target.model.clone(),
                target_host: host_of(&target.base_url),
                local: target.local,
                outbound: !target.local,
                outcome: "ok".to_string(),
                // 存储层按真正提交的组生成摘要；手动锁定的组不会误报。
                detail: String::new(),
            },
        )?;

        Ok(self.store().list_message_group_results(message_id)?)
    }
    /// 分类前的外发确认预览：备齐正文、算哈希并回一次性令牌；不发网络请求。
    ///
    /// 正文本地没有就先联网下载一次（跟用户点开邮件一样）；传入的邮件编号会去重并
    /// 保持顺序；任何一封下载失败或没有可用正文都会整体拒绝。
    pub async fn preview_classify_messages(
        &self,
        message_ids: &[i64],
    ) -> Result<ClassifyPreview, EngineError> {
        if message_ids.is_empty() {
            return Err(EngineError::BadRequest("请先选择要分类的邮件".to_string()));
        }
        let mut unique: Vec<i64> = Vec::new();
        for id in message_ids {
            if !unique.contains(id) {
                unique.push(*id);
            }
        }
        let enabled_groups = self.enabled_classify_groups()?;
        if enabled_groups.is_empty() {
            return Err(EngineError::BadRequest("没有开启任何标签组".to_string()));
        }
        let target = self.resolve_ai_target(AiFunction::Classify)?;
        let mut targets = Vec::with_capacity(unique.len());
        let mut hashes = Vec::with_capacity(unique.len());
        for id in &unique {
            self.ensure_message_body_cached(*id).await?;
            let body = self.store().get_message_body(*id)?.ok_or_else(|| {
                EngineError::BadRequest("邮件正文还没下载，请先打开一次这封邮件".to_string())
            })?;
            let text = truncate_classify_body(&classify_body_text(&body));
            if text.trim().is_empty() {
                return Err(EngineError::BadRequest("这封邮件没有可以分类的正文".to_string()));
            }
            let hash = hash_text(&text);
            hashes.push(hash.clone());
            targets.push(ClassifyPreviewTarget {
                message_id: *id,
                content_hash: hash,
            });
        }
        let token = self.store_pending_authorization(
            AiFunction::Classify,
            &target,
            "",
            hashes.first().cloned().unwrap_or_default(),
            unique,
            hashes,
        )?;
        Ok(ClassifyPreview {
            provider_id: target.provider_id,
            provider_label: target.provider_label.clone(),
            host: host_of(&target.base_url),
            model: target.model.clone(),
            local: target.local,
            authorization_token: token,
            expires_in_seconds: AI_AUTHORIZATION_TTL.as_secs(),
            targets,
            enabled_groups: enabled_groups
                .into_iter()
                .map(|group| ClassifyPreviewGroup {
                    name: group.name,
                    kind: group.kind.as_str().to_string(),
                })
                .collect(),
        })
    }

    /// 校验并一次性消费分类令牌；重新读正文逐封比对哈希，返回邮件编号。
    ///
    /// 令牌无论校验成功与否都只消费一次；哈希、数量、顺序任何不一致都整体拒绝。
    pub fn consume_classify_authorization(&self, token: &str) -> Result<Vec<i64>, EngineError> {
        let pending = self.take_pending_classify(token)?;
        if pending.message_ids.is_empty() || pending.message_ids.len() != pending.content_hashes.len() {
            return Err(EngineError::AiAuthorizationInvalid);
        }
        let mut hashes = Vec::with_capacity(pending.message_ids.len());
        for id in &pending.message_ids {
            let body = self
                .store()
                .get_message_body(*id)?
                .ok_or(EngineError::AiAuthorizationInvalid)?;
            let text = truncate_classify_body(&classify_body_text(&body));
            hashes.push(hash_text(&text));
        }
        if hashes != pending.content_hashes {
            return Err(EngineError::AiAuthorizationInvalid);
        }
        Ok(pending.message_ids)
    }

    fn take_pending_classify(&self, token: &str) -> Result<PendingAiAuthorization, EngineError> {
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
        drop(guard);
        if pending.expires_at <= now || pending.function != AiFunction::Classify {
            return Err(EngineError::AiAuthorizationInvalid);
        }
        Ok(pending)
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
            AiFunction::Classify => {
                return Err(EngineError::BadRequest(
                    "邮件分类请走专门的分类预览接口".to_string(),
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

    /// 生成一次性外发授权令牌（单封功能）。
    fn issue_authorization(
        &self,
        function: AiFunction,
        target: &AiTarget,
        target_language: &str,
        content_hash: &str,
    ) -> Result<String, EngineError> {
        self.store_pending_authorization(
            function,
            target,
            target_language,
            content_hash.to_string(),
            Vec::new(),
            vec![content_hash.to_string()],
        )
    }

    /// 生成一次性外发授权令牌并放进待授权表。
    fn store_pending_authorization(
        &self,
        function: AiFunction,
        target: &AiTarget,
        target_language: &str,
        content_hash: String,
        message_ids: Vec<i64>,
        content_hashes: Vec<String>,
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
            content_hash,
            message_ids,
            content_hashes,
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
        let route = self.resolve_route(target.proxy)?;
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
        let route = self.resolve_route(target.proxy)?;
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
        let route = self.resolve_route(target.proxy)?;
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
        let route = self.resolve_route(target.proxy)?;
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
        let route = self.resolve_route(target.proxy)?;
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

    /// 用指定站点和模型发一封固定的假邮件，量一次通知识别的实际往返耗时。
    ///
    /// 这是用户手动点出来的诊断：内容全部是编的，不含任何真实邮件；
    /// 走和正常识别相同的外发路径，同样写一条 AI 审计。
    pub async fn check_notification_latency(
        &self,
        provider_id: i64,
        model: &str,
        level: Option<AiThinkingLevel>,
    ) -> Result<NotificationLatency, EngineError> {
        let target = self.resolve_probe_target(provider_id, model, level)?;
        let secret = self.provider_secret(&target)?;
        let route = self.resolve_route(target.proxy)?;
        let endpoint = Endpoint {
            kind: provider_kind(target.kind),
            base_url: &target.base_url,
            api_key: secret.as_ref(),
        };
        let (from, subject, body) = prompt::notification_verify_probe_mail();
        let started = Instant::now();
        let outcome = chat(
            route.as_ref(),
            &endpoint,
            &target.model,
            thinking_level(target.thinking_level),
            &prompt::notification_verify_system(),
            &prompt::notification_verify_user(from, subject, body),
            NOTIFICATION_VERIFY_TIMEOUT,
        )
        .await
        .map_err(|err| self.audit_ai_error(AiFunction::NotificationVerify, &target, err))?;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        self.audit_ai_success(
            AiFunction::NotificationVerify,
            &target,
            outcome.thinking_downgraded,
        )?;
        let finding = parse_verification(&outcome.text);
        Ok(NotificationLatency {
            elapsed_ms,
            found_code: finding.code.is_some(),
            found_link: finding.link.is_some(),
        })
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
            None => {
                if function == AiFunction::Classify {
                    // 分类只认显式配好的决策模型站点，绝不回退到第一个启用站点。
                    return Err(EngineError::BadRequest(
                        "邮件分类还没有配置决策模型站点".to_string(),
                    ));
                }
                self.store()
                    .list_ai_providers()?
                    .into_iter()
                    .find(|provider| provider.enabled)
                    .ok_or(EngineError::AiDisabled)?
            }
        };
        // 站点能力校验：分类只认决策模型站点，聊天类站点也做不了分类。
        if !provider_kind(provider.kind).supports(provider_function(function)) {
            return Err(EngineError::BadRequest(format!(
                "站点「{}」不支持这个功能，请到设置里换成支持的站点",
                provider.label
            )));
        }
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
            proxy: provider.proxy,
            local: is_local_host(&host_of(&provider.base_url)),
            api_key_ref: provider.api_key_ref.clone(),
        })
    }

    /// 按界面当前选的站点和模型构造测试目标；站点必须存在且启用。
    fn resolve_probe_target(
        &self,
        provider_id: i64,
        model: &str,
        level: Option<AiThinkingLevel>,
    ) -> Result<AiTarget, EngineError> {
        if provider_id <= 0 {
            return Err(EngineError::BadRequest("请选择 AI 站点".to_string()));
        }
        let provider = self
            .store()
            .get_ai_provider(provider_id)?
            .filter(|provider| provider.enabled)
            .ok_or(EngineError::AiProviderNotFound(provider_id))?;
        let model = if model.trim().is_empty() {
            provider.default_model.trim().to_string()
        } else {
            model.trim().to_string()
        };
        if model.is_empty() {
            return Err(EngineError::BadRequest(
                "这个站点还没有可用的模型，请先填写模型".to_string(),
            ));
        }
        Ok(AiTarget {
            provider_id: provider.id,
            provider_label: provider.label.clone(),
            base_url: provider.base_url.clone(),
            kind: provider.kind,
            model,
            thinking_level: level.unwrap_or(provider.thinking_level),
            proxy: provider.proxy,
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
        proxy: provider.proxy,
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

/// 一组结果换算出来的落库值。
struct GroupWrite {
    /// 选择题或是否题要落的标签；打分题为空。
    label_id: Option<i64>,
    /// 打分题要落的数字；其它题型为空。
    score_value: Option<i64>,
    /// 模型给的把握程度。
    confidence: Option<f64>,
    /// 审计里用的结果摘要（标签名或数字）。
    summary: String,
}

/// 把一个标签组转成 jev 的一次请求问题；组配置出不了结果就返回参数错误。
fn group_question(group: &LabelGroup, labels: &[MailLabel]) -> Result<GroupQuestion, EngineError> {
    let kind = match group.kind {
        LabelGroupKind::Choice => {
            if labels.is_empty() {
                return Err(EngineError::BadRequest(format!(
                    "标签组「{}」还没有标签，先加一个",
                    group.name
                )));
            }
            GroupQuestionKind::Choice
        }
        LabelGroupKind::Boolean => {
            if labels.len() != 2 {
                return Err(EngineError::BadRequest(format!(
                    "是否题组「{}」必须正好两条标签",
                    group.name
                )));
            }
            GroupQuestionKind::Boolean
        }
        LabelGroupKind::Score => {
            if score_range(group).is_none() {
                return Err(EngineError::BadRequest(format!(
                    "打分题组「{}」的档位不合法",
                    group.name
                )));
            }
            GroupQuestionKind::Score
        }
    };
    let instructions = match kind {
        GroupQuestionKind::Choice => format!("请判断这封邮件属于「{}」的哪一类。", group.name),
        GroupQuestionKind::Boolean => format!("请判断这封邮件是否{}。", group.name),
        GroupQuestionKind::Score => format!("请判断这封邮件的「{}」属于哪一档。", group.name),
    };
    let criteria = match kind {
        GroupQuestionKind::Choice => labels.iter().map(|label| label.name.clone()).collect(),
        GroupQuestionKind::Score => {
            let (min, max) = score_range(group).unwrap_or((0, 0));
            prompt::score_levels(min, max)
        }
        GroupQuestionKind::Boolean => Vec::new(),
    };
    Ok(GroupQuestion {
        group_id: group.id,
        kind,
        instructions,
        criteria,
        score_min: group.score_min,
        score_max: group.score_max,
    })
}

/// 取打分题的上下限；档位数不在 2 到 10 之间就返回 `None`。
fn score_range(group: &LabelGroup) -> Option<(i64, i64)> {
    let min = group.score_min?;
    let max = group.score_max?;
    let count = max.checked_sub(min)?.checked_add(1)?;
    ((2..=10).contains(&count) && min < max).then_some((min, max))
}

/// 按组类型把模型答案换算成落库值；该组不需要写就返回 `None`。打分题把握低也照写。
fn resolve_group_write(group: &LabelGroup, labels: &[MailLabel], answer: &GroupAnswer) -> Option<GroupWrite> {
    match (group.kind, answer) {
        (LabelGroupKind::Choice, GroupAnswer::Choice { label, confidence }) => {
            let chosen = if *confidence >= group.confidence_threshold {
                labels.iter().find(|item| &item.name == label)
            } else {
                labels.iter().find(|item| item.is_fallback)
            }?;
            Some(GroupWrite {
                label_id: Some(chosen.id),
                score_value: None,
                confidence: Some(*confidence),
                summary: chosen.name.clone(),
            })
        }
        (LabelGroupKind::Boolean, GroupAnswer::Boolean { probability }) => {
            let index = if *probability >= group.confidence_threshold {
                0
            } else {
                1
            };
            let chosen = labels.get(index)?;
            Some(GroupWrite {
                label_id: Some(chosen.id),
                score_value: None,
                confidence: Some(*probability),
                summary: chosen.name.clone(),
            })
        }
        // 打分题只要模型给出合法档位就落库（越界档位在解析阶段已经丢掉），
        // 把握再低也留个数字，别让界面上这一组空着；把握值原样存进 confidence 列。
        (LabelGroupKind::Score, GroupAnswer::Score { level, confidence }) => Some(GroupWrite {
            label_id: None,
            score_value: Some(*level),
            confidence: Some(*confidence),
            summary: level.to_string(),
        }),
        _ => None,
    }
}

/// 分类要外发的正文纯文本：优先纯文本，没有就按现有切段逻辑取 HTML 文字。
fn classify_body_text(body: &mail_store::StoredMessageBody) -> String {
    body.text_plain
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
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 截取分类正文；最多前 2000 个字符。
fn truncate_classify_body(text: &str) -> String {
    text.chars().take(CLASSIFY_BODY_MAX_CHARS).collect()
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
            proxy: AccountProxyMode::InheritGlobal,
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
    /// 假 HTTP 服务：收一次完整请求，把原始请求文本回传给测试；返回固定 JSON。
    ///
    /// 必须按 Content-Length 读完整包：并行跑测试时一次 read 可能只拿到半个请求体。
    async fn fake_capture_server(payload: &str) -> (u16, std::sync::mpsc::Receiver<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let payload = payload.to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("绑定");
        let port = listener.local_addr().expect("地址").port();
        let (tx, rx) = std::sync::mpsc::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接");
            let mut buf = vec![0u8; 4096];
            let mut request = Vec::new();
            loop {
                let read = socket.read(&mut buf).await.expect("读请求");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..read]);
                if let Some(header_end) = find_header_end(&request) {
                    let content_length = content_length_of(&request[..header_end]);
                    if request.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&request).to_string();
            let _ = tx.send(text);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(response.as_bytes()).await.expect("写响应");
        });
        (port, rx)
    }

    /// 找请求头结束位置（`\r\n\r\n`）。
    fn find_header_end(bytes: &[u8]) -> Option<usize> {
        bytes.windows(4).position(|window| window == b"\r\n\r\n")
    }

    /// 从请求头里取 Content-Length；没有就按 0 处理。
    fn content_length_of(headers: &[u8]) -> usize {
        String::from_utf8_lossy(headers)
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("content-length") {
                    value.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0)
    }
    #[tokio::test]
    async fn 通用决策站点必须填写模型() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let input = AiProviderInput {
            id: None,
            label: "决策".to_string(),
            kind: AiProviderKind::DecisionApi,
            base_url: "https://api.bamboonode.dev/v1/decisions".to_string(),
            default_model: "   ".to_string(),
            models: Vec::new(),
            thinking_level: AiThinkingLevel::Off,
            enabled: true,
            proxy: AccountProxyMode::Direct,
        };
        let error = engine.save_ai_provider(&input, None).expect_err("空模型应被拒绝");
        assert!(format!("{error}").contains("决策模型站点必须填写模型"));
    }

    #[tokio::test]
    async fn 通用决策站点测试连接使用完整地址() {
        let (port, captured) = fake_capture_server(r#"{"answers":{"g1":{"type":"noul","noul":0.99}}}"#).await;
        let base = format!("http://127.0.0.1:{port}/v1/decisions");
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let models = engine
            .test_ai_provider(
                AiProviderKind::DecisionApi,
                &base,
                "startlux-decision-27b-autothink",
                Some(&Secret::new("test-key")),
                None,
                AccountProxyMode::Direct,
            )
            .await
            .expect("测试连接");
        assert_eq!(models, vec!["startlux-decision-27b-autothink"]);
        let request = captured.recv().expect("收到请求");
        assert!(request.starts_with("POST /v1/decisions "), "{request}");
        assert!(request.contains("\"model\":\"startlux-decision-27b-autothink\""));
        assert!(
            request.contains("\"type\":\"noul\""),
            "探针必须是是否题：{request}"
        );
        assert!(request.contains(prompt::TYPESAFE_PROBE_STATE), "{request}");
    }

    #[tokio::test]
    async fn typesafe_test_connection_uses_selected_relay_model() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let (port, captured) = fake_capture_server(r#"{"answers":{"g1":{"type":"noul","noul":0.92}}}"#).await;
        let base = format!("http://127.0.0.1:{port}/v1");

        let models = engine
            .test_ai_provider(
                AiProviderKind::TypeSafe,
                &base,
                " relay/jev-custom ",
                None,
                None,
                AccountProxyMode::InheritGlobal,
            )
            .await
            .expect("TypeSafe 测试连接应成功");
        assert_eq!(models, vec!["relay/jev-custom".to_string()]);

        let request = captured.recv().expect("拿到请求");
        assert!(request.contains("POST /v1/systemone"), "{request}");
        assert!(
            request.contains(prompt::TYPESAFE_PROBE_STATE),
            "请求体里应是固定探针：{request}"
        );
        assert!(
            request.contains("\"type\":\"noul\""),
            "探针必须是是否题：{request}"
        );
        assert!(request.contains("\"model\":\"relay/jev-custom\""), "{request}");
        assert!(engine.list_ai_providers().expect("站点列表").is_empty());
    }

    #[tokio::test]
    async fn 决策站点探针没有合法是否题答案时失败() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let (port, _captured) = fake_capture_server(r#"{"answers":{"g1":{"type":"noul","noul":1.2}}}"#).await;
        let base = format!("http://127.0.0.1:{port}/v1/decisions");
        let error = engine
            .test_ai_provider(
                AiProviderKind::DecisionApi,
                &base,
                "startlux-decision-27b-autothink",
                Some(&Secret::new("test-key")),
                None,
                AccountProxyMode::Direct,
            )
            .await
            .expect_err("没有合法是否题答案不能算测试成功");
        assert!(format!("{error}").contains("响应无法识别"), "{error}");
    }

    #[tokio::test]
    async fn typesafe_test_connection_reuses_stored_key_without_changing_configuration() {
        let dir = tempfile::tempdir().expect("临时目录");
        let secrets = std::sync::Arc::new(crate::secrets::MemorySecretStore::new());
        let engine = MailEngine::initialize_with_secrets(dir.path(), secrets.clone()).expect("初始化");
        let (port, captured) = fake_capture_server(r#"{"answers":{"g1":{"type":"noul","noul":0.92}}}"#).await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let saved = engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "中转".to_string(),
                    kind: AiProviderKind::TypeSafe,
                    base_url: base.clone(),
                    default_model: "relay/default".to_string(),
                    models: vec!["relay/default".to_string(), "relay/fast".to_string()],
                    thinking_level: AiThinkingLevel::Off,
                    enabled: false,
                    proxy: AccountProxyMode::Direct,
                },
                Some(&Secret::new("fake-stored-test-key")),
            )
            .expect("保存站点");
        let models = engine
            .test_ai_provider(
                AiProviderKind::TypeSafe,
                &base,
                "relay/fast",
                None,
                Some(saved.id),
                AccountProxyMode::Direct,
            )
            .await
            .expect("沿用已存密钥测试");
        assert_eq!(models, vec!["relay/fast"]);
        let request = captured.recv().expect("请求");
        assert!(request.contains("Authorization: Bearer fake-stored-test-key"));
        assert!(request.contains(r#""model":"relay/fast""#));
        let after = engine.list_ai_providers().expect("站点").remove(0);
        assert_eq!(after.default_model, saved.default_model);
        assert_eq!(after.models, saved.models);
        assert!(!after.enabled);
        assert_eq!(secrets.len(), 1);
    }

    #[tokio::test]
    async fn typesafe_test_connection_rejects_blank_model_before_sending() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("监听");
        let base = format!("http://{}/v1", listener.local_addr().expect("地址"));
        for model in ["", " \n "] {
            let error = engine
                .test_ai_provider(
                    AiProviderKind::TypeSafe,
                    &base,
                    model,
                    None,
                    None,
                    AccountProxyMode::Direct,
                )
                .await
                .expect_err("空模型必须拦截");
            assert!(matches!(error, EngineError::BadRequest(ref text) if text.contains("测试模型")));
        }
        assert!(tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err());
        assert!(engine.list_ai_providers().expect("站点列表").is_empty());
    }

    #[tokio::test]
    async fn compatible_test_connection_still_lists_models_without_selected_model() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let (port, captured) = fake_capture_server(r#"{"data":[{"id":"relay-model"}]}"#).await;
        let models = engine
            .test_ai_provider(
                AiProviderKind::OpenAiCompatible,
                &format!("http://127.0.0.1:{port}/v1"),
                "",
                None,
                None,
                AccountProxyMode::Direct,
            )
            .await
            .expect("兼容站点仍可拉取模型");
        assert_eq!(models, vec!["relay-model"]);
        let request = captured.recv().expect("请求");
        assert!(request.starts_with("GET /v1/models "), "{request}");
        assert!(!request.contains("systemone"), "{request}");
    }

    fn add_typesafe(engine: &MailEngine) -> i64 {
        add_typesafe_at(engine, "https://api.typesafe.ai/v1".to_string())
    }

    fn add_typesafe_at(engine: &MailEngine, base_url: String) -> i64 {
        engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "jev".to_string(),
                    kind: AiProviderKind::TypeSafe,
                    base_url,
                    default_model: "jev-latest".to_string(),
                    models: Vec::new(),
                    thinking_level: AiThinkingLevel::Off,
                    enabled: true,
                    proxy: AccountProxyMode::InheritGlobal,
                },
                None,
            )
            .expect("保存 TypeSafe 站点")
            .id
    }

    #[test]
    fn classify_never_falls_back_to_a_chat_provider() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        add_ollama(&engine);

        let error = engine
            .resolve_ai_target(AiFunction::Classify)
            .expect_err("没有分类映射时不能回退到聊天站点");
        assert!(matches!(error, EngineError::BadRequest(_)), "{error:?}");
    }

    #[test]
    fn classify_uses_configured_typesafe_provider() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let provider_id = add_typesafe(&engine);
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");

        let target = engine
            .resolve_ai_target(AiFunction::Classify)
            .expect("应能解析分类目标");
        assert_eq!(target.kind, AiProviderKind::TypeSafe);
        assert_eq!(target.model, "jev-latest");
    }

    #[test]
    fn chat_function_rejects_typesafe_provider() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let provider_id = add_typesafe(&engine);
        engine
            .set_ai_feature(AiFunction::Translate, provider_id, "jev-latest", None)
            .expect("写入映射");

        let error = engine
            .resolve_ai_target(AiFunction::Translate)
            .expect_err("jev 做不了翻译，应拒绝");
        assert!(matches!(error, EngineError::BadRequest(_)), "{error:?}");
    }

    fn seed_message_with_body(engine: &MailEngine, uid: u32, body: &str) -> i64 {
        let store = engine.store();
        let draft = mail_domain::AccountDraft {
            display_name: "测试".to_string(),
            email: format!("u{uid}@example.com"),
            auth_type: mail_domain::AuthType::Password,
            username: format!("u{uid}@example.com"),
            imap: mail_domain::ServerConfig {
                host: "imap.example.com".to_string(),
                port: 993,
                security: mail_domain::Security::Tls,
            },
            smtp: mail_domain::ServerConfig {
                host: "smtp.example.com".to_string(),
                port: 465,
                security: mail_domain::Security::Tls,
            },
            proxy: AccountProxyMode::InheritGlobal,
            color: String::new(),
            enabled: true,
            oauth_provider: None,
            oauth_client_id: String::new(),
        };
        let account_id = store.insert_account(&draft, None).expect("插入账号").0;
        let folder_id = store
            .upsert_folder(account_id, "INBOX", "/", mail_domain::FolderKind::Inbox)
            .expect("插入文件夹");
        store
            .insert_messages(&[mail_store::NewMessage {
                account_id,
                folder_id,
                uid,
                message_id_header: format!("<m{uid}@example.com>"),
                thread_key: format!("thread-{uid}"),
                subject: format!("主题 {uid}"),
                from_name: "张三".to_string(),
                from_addr: "zhangsan@example.com".to_string(),
                to_json: "[]".to_string(),
                cc_json: "[]".to_string(),
                date_utc: "2026-10-10T01:00:00Z".to_string(),
                size: 1000,
                has_attachments: false,
                is_read: false,
                is_flagged: false,
                is_answered: false,
                is_draft: false,
            }])
            .expect("插入邮件");
        let id = store
            .list_inbox_messages(&mail_store::InboxQuery {
                account_id: Some(account_id),
                folder_id: Some(folder_id),
                unread_only: false,
                flagged_only: false,
                folder_kind: None,
                offset: 0,
                limit: 10,
            })
            .expect("读回收件箱")
            .into_iter()
            .find(|message| message.uid == uid)
            .expect("应能读回刚插的邮件")
            .id;
        store.save_message_body(id, Some(body), None).expect("写入正文");
        id
    }

    #[tokio::test]
    async fn classify_authorization_is_one_time_and_bound_to_message_hashes() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let provider_id = add_typesafe(&engine);
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");
        enable_group(&engine, "内容分类", 0.75);
        let first = seed_message_with_body(&engine, 1, "账单已出，请查收");
        let second = seed_message_with_body(&engine, 2, "明天上午开会");

        let preview = engine
            .preview_classify_messages(&[first, second])
            .await
            .expect("分类预览");
        assert!(!preview.authorization_token.is_empty());
        assert_eq!(preview.targets.len(), 2);
        assert_eq!(preview.targets[0].message_id, first);
        assert_eq!(preview.targets[1].message_id, second);
        assert_eq!(preview.host, "api.typesafe.ai");
        assert!(!preview.local);
        assert_eq!(preview.model, "jev-latest");

        // 改掉第二封正文后，令牌必须失效，且不允许部分执行。
        engine
            .store()
            .save_message_body(second, Some("改过的正文"), None)
            .expect("改写正文");
        let error = engine
            .consume_classify_authorization(&preview.authorization_token)
            .expect_err("正文变了应拒绝");
        assert!(matches!(error, EngineError::AiAuthorizationInvalid));

        // 重新预览后第一次能消费，第二次必须拒绝（一次性）。
        let preview = engine
            .preview_classify_messages(&[first, second])
            .await
            .expect("再预览");
        let ids = engine
            .consume_classify_authorization(&preview.authorization_token)
            .expect("第一次消费应成功");
        assert_eq!(ids, vec![first, second]);
        let error = engine
            .consume_classify_authorization(&preview.authorization_token)
            .expect_err("令牌只能用一次");
        assert!(matches!(error, EngineError::AiAuthorizationInvalid));
    }

    /// 把某个默认标签组打开并设置门槛，返回组编号。
    fn enable_group(engine: &MailEngine, name: &str, threshold: f64) -> i64 {
        let group = engine
            .store()
            .list_label_groups()
            .expect("列标签组")
            .into_iter()
            .find(|group| group.name == name)
            .expect("应有这个默认组");
        engine
            .store()
            .set_label_group_enabled(group.id, true)
            .expect("开组");
        engine
            .store()
            .set_label_group_threshold(group.id, threshold)
            .expect("设门槛");
        group.id
    }

    #[tokio::test]
    async fn one_request_carries_all_enabled_groups() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let content_id = enable_group(&engine, "内容分类", 0.75);
        let reply_id = enable_group(&engine, "需要我回复", 0.5);
        let payload = format!(
            r#"{{"answers":{{"g{content_id}":{{"type":"choice","choice":"账单","confidence":0.90}},"g{reply_id}":{{"type":"noul","noul":0.90}}}}}}"#
        );
        let (port, captured) = fake_capture_server(&payload).await;
        let provider_id = add_typesafe_at(&engine, format!("http://127.0.0.1:{port}/v1"));
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");
        let message = seed_message_with_body(&engine, 1, "本月账单已出，请尽快回复");

        engine.classify_message_groups(message).await.expect("分类应成功");

        let request = captured.recv().expect("拿到请求");
        assert!(
            request.contains(&format!("\"g{content_id}\"")),
            "一次请求要带上内容分类组：{request}"
        );
        assert!(
            request.contains(&format!("\"g{reply_id}\"")),
            "一次请求要带上需要回复组：{request}"
        );
    }

    #[tokio::test]
    async fn choice_group_below_threshold_falls_back() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let group_id = enable_group(&engine, "内容分类", 0.75);
        let payload = format!(
            r#"{{"answers":{{"g{group_id}":{{"type":"choice","choice":"账单","confidence":0.60}}}}}}"#
        );
        let port = fake_chat_server(&payload).await;
        let provider_id = add_typesafe_at(&engine, format!("http://127.0.0.1:{port}/v1"));
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");
        let message = seed_message_with_body(&engine, 2, "有点说不准的一封邮件");

        let results = engine.classify_message_groups(message).await.expect("分类应成功");
        assert_eq!(results.len(), 1);
        let result = &results[0];
        assert_eq!(result.group.id, group_id);
        let label = result.label.as_ref().expect("应落兜底标签");
        assert_eq!(label.name, "其他");
        assert!(label.is_fallback);
        assert_eq!(result.source, MailLabelSource::Auto);
    }

    #[tokio::test]
    async fn choice_group_below_threshold_without_fallback_writes_nothing() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let group_id = engine
            .store()
            .create_label_group("主题分类", LabelGroupKind::Choice, None, None)
            .expect("建组");
        engine
            .store()
            .create_group_label(group_id, "账单")
            .expect("加标签");
        engine
            .store()
            .create_group_label(group_id, "通知")
            .expect("加标签");
        engine
            .store()
            .set_label_group_enabled(group_id, true)
            .expect("开组");
        let payload = format!(
            r#"{{"answers":{{"g{group_id}":{{"type":"choice","choice":"账单","confidence":0.60}}}}}}"#
        );
        let port = fake_chat_server(&payload).await;
        let provider_id = add_typesafe_at(&engine, format!("http://127.0.0.1:{port}/v1"));
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");
        let message = seed_message_with_body(&engine, 3, "有点说不准的一封邮件");

        let results = engine.classify_message_groups(message).await.expect("分类应成功");
        assert!(results.is_empty(), "没有兜底就不写：{results:?}");
        assert!(engine
            .store()
            .message_group_result(message, group_id)
            .expect("读组结果")
            .is_none());
    }

    #[tokio::test]
    async fn boolean_group_always_writes_one_of_two() {
        // 概率够门槛 → 第一条「需要回复」。
        let dir = tempfile::tempdir().expect("临时目录");
        let first_engine = engine(dir.path());
        let group_id = enable_group(&first_engine, "需要我回复", 0.5);
        let payload = format!(r#"{{"answers":{{"g{group_id}":{{"type":"noul","noul":0.90}}}}}}"#);
        let port = fake_chat_server(&payload).await;
        let provider_id = add_typesafe_at(&first_engine, format!("http://127.0.0.1:{port}/v1"));
        first_engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");
        let message = seed_message_with_body(&first_engine, 4, "请尽快给我答复");
        let results = first_engine
            .classify_message_groups(message)
            .await
            .expect("分类应成功");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].label.as_ref().expect("应落标签").name, "需要回复");
        assert_eq!(results[0].confidence, Some(0.90));

        // 概率不够门槛 → 第二条「无需回复」。
        let dir2 = tempfile::tempdir().expect("临时目录");
        let engine2 = engine(dir2.path());
        let group_id2 = enable_group(&engine2, "需要我回复", 0.5);
        let payload2 = format!(r#"{{"answers":{{"g{group_id2}":{{"type":"noul","noul":0.20}}}}}}"#);
        let port2 = fake_chat_server(&payload2).await;
        let provider_id2 = add_typesafe_at(&engine2, format!("http://127.0.0.1:{port2}/v1"));
        engine2
            .set_ai_feature(AiFunction::Classify, provider_id2, "jev-latest", None)
            .expect("配置分类");
        let message2 = seed_message_with_body(&engine2, 5, "一封信");
        let results2 = engine2
            .classify_message_groups(message2)
            .await
            .expect("分类应成功");
        assert_eq!(results2.len(), 1);
        assert_eq!(results2[0].label.as_ref().expect("应落标签").name, "无需回复");
    }

    #[tokio::test]
    async fn score_group_takes_highest_probability_level() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let group_id = enable_group(&engine, "紧急程度", 0.75);
        // 5 档里最高概率是第 3 档（下标 2），换算成数字 1 + 2 = 3。
        let payload = format!(
            r#"{{"answers":{{"g{group_id}":{{"type":"score","probabilities":{{"0":0.05,"1":0.10,"2":0.60,"3":0.15,"4":0.10}}}}}}}}"#
        );
        let port = fake_chat_server(&payload).await;
        let provider_id = add_typesafe_at(&engine, format!("http://127.0.0.1:{port}/v1"));
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");
        let message = seed_message_with_body(&engine, 6, "这事很急，今天就要");
        let results = engine.classify_message_groups(message).await.expect("分类应成功");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].score_value, Some(3));
        assert_eq!(results[0].confidence, Some(0.60), "把握程度应取最高概率");
        assert!(results[0].label.is_none());
        assert_eq!(results[0].source, MailLabelSource::Auto);
    }

    #[tokio::test]
    async fn score_group_below_threshold_still_writes_a_number() {
        // 门槛 0.9，模型只给 0.3 的把握：旧行为是整组留白，现在要照常落分数。
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let group_id = enable_group(&engine, "紧急程度", 0.9);
        let payload =
            format!(r#"{{"answers":{{"g{group_id}":{{"type":"score","level":4,"confidence":0.30}}}}}}"#);
        let port = fake_chat_server(&payload).await;
        let provider_id = add_typesafe_at(&engine, format!("http://127.0.0.1:{port}/v1"));
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");
        let message = seed_message_with_body(&engine, 61, "说不准急不急的一封邮件");

        let results = engine.classify_message_groups(message).await.expect("分类应成功");
        assert_eq!(results.len(), 1, "低把握也要落分数：{results:?}");
        assert_eq!(results[0].score_value, Some(4));
        assert_eq!(results[0].confidence, Some(0.30));
        assert_eq!(results[0].source, MailLabelSource::Auto);
    }

    #[tokio::test]
    async fn manual_group_is_not_overwritten() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let content_id = enable_group(&engine, "内容分类", 0.75);
        let reply_id = enable_group(&engine, "需要我回复", 0.5);
        let message = seed_message_with_body(&engine, 7, "本月账单已出，请查收");
        let notice = engine
            .store()
            .list_group_labels(content_id)
            .expect("列组内标签")
            .into_iter()
            .find(|label| label.name == "通知")
            .expect("应有通知标签");
        engine
            .store()
            .set_message_group_value(
                message,
                content_id,
                Some(notice.id),
                None,
                MailLabelSource::Manual,
                None,
            )
            .expect("手改内容分类");

        let payload = format!(
            r#"{{"answers":{{"g{content_id}":{{"type":"choice","choice":"账单","confidence":0.95}},"g{reply_id}":{{"type":"noul","noul":0.90}}}}}}"#
        );
        let port = fake_chat_server(&payload).await;
        let provider_id = add_typesafe_at(&engine, format!("http://127.0.0.1:{port}/v1"));
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");

        let results = engine.classify_message_groups(message).await.expect("分类应成功");
        let content = results
            .iter()
            .find(|result| result.group.id == content_id)
            .expect("内容分类组应有结果");
        assert_eq!(content.label.as_ref().expect("应保留手改标签").name, "通知");
        assert_eq!(content.source, MailLabelSource::Manual);
        let reply = results
            .iter()
            .find(|result| result.group.id == reply_id)
            .expect("需要回复组应照写");
        assert_eq!(reply.label.as_ref().expect("应落标签").name, "需要回复");
        assert_eq!(reply.source, MailLabelSource::Auto);
    }

    #[tokio::test]
    async fn request_failure_writes_nothing() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let group_id = enable_group(&engine, "内容分类", 0.75);
        let provider_id = add_typesafe_at(&engine, "http://127.0.0.1:1/v1".to_string());
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");
        let message = seed_message_with_body(&engine, 8, "一封发不出去的邮件");

        let error = engine
            .classify_message_groups(message)
            .await
            .expect_err("网络失败应报错");
        assert!(matches!(error, EngineError::Ai(_)), "{error:?}");
        assert!(engine
            .store()
            .message_group_result(message, group_id)
            .expect("读组结果")
            .is_none());
        let audits = engine.list_ai_audit(10).expect("审计");
        assert!(
            audits.iter().any(|audit| audit.outcome == "failed"),
            "失败要记审计"
        );
        for audit in audits {
            assert!(!audit.detail.contains("一封发不出去的邮件"), "审计不能记正文");
        }
    }

    /// 假 HTTP 服务：收一次请求，返回指定状态与正文；用来测错误脱敏。
    async fn fake_status_server(status: &'static str, payload: &'static str) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("绑定");
        let port = listener.local_addr().expect("地址").port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接");
            let mut buf = vec![0u8; 8192];
            let _ = socket.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(response.as_bytes()).await.expect("写响应");
        });
        port
    }

    #[tokio::test]
    async fn classify_error_never_leaks_the_api_key_or_body() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let port = fake_status_server(
            "401 Unauthorized",
            r#"{"error":{"message":"Incorrect API key provided: sk-live-secret-123"}}"#,
        )
        .await;
        let secret = Secret::new("sk-live-secret-123");
        let provider_id = engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "jev".to_string(),
                    kind: AiProviderKind::TypeSafe,
                    base_url: format!("http://127.0.0.1:{port}/v1"),
                    default_model: "jev-latest".to_string(),
                    models: Vec::new(),
                    thinking_level: AiThinkingLevel::Off,
                    enabled: true,
                    proxy: AccountProxyMode::InheritGlobal,
                },
                Some(&secret),
            )
            .expect("保存站点")
            .id;
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");
        enable_group(&engine, "内容分类", 0.75);
        let message = seed_message_with_body(&engine, 91, "正文里有敏感内容，不该进审计");

        let error = engine
            .classify_message_groups(message)
            .await
            .expect_err("站点报错时应返回错误");
        let text = error.to_string();
        assert!(!text.contains("sk-live-secret-123"), "错误里不能出现密钥：{text}");
        assert!(
            engine
                .store()
                .list_message_group_results(message)
                .expect("读组结果")
                .is_empty(),
            "报错时不能写结果"
        );

        let audits = engine.list_ai_audit(10).expect("审计");
        for audit in audits {
            assert!(!audit.detail.contains("sk-live-secret-123"), "审计不能记密钥");
            assert!(!audit.detail.contains("正文里有敏感内容"), "审计不能记正文");
            assert!(!audit.provider_label.contains("正文里有敏感内容"));
        }
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
                    proxy: AccountProxyMode::InheritGlobal,
                },
                None,
            )
            .expect("保存站点")
            .id
    }

    /// 假 HTTP 服务：收一次请求，回固定 JSON；用来测协议往返。
    ///
    /// 会先把请求头和按 Content-Length 声明的正文读完再回包，避免请求被拆成
    /// 多个 TCP 包时服务端提前关连接、让客户端偶发报协议错。
    async fn fake_chat_server(payload: &str) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let payload = payload.to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("绑定");
        let port = listener.local_addr().expect("地址").port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接");
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            // 要读到的总字节数；读完请求头才能按 Content-Length 算出来。
            let mut want = usize::MAX;
            loop {
                let read = match socket.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                request.extend_from_slice(&buf[..read]);
                if want == usize::MAX {
                    if let Some(head_end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        want = head_end + 4 + request_content_length(&request[..head_end]);
                    }
                }
                if request.len() >= want {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(response.as_bytes()).await.expect("写响应");
        });
        port
    }

    /// 从请求头里取 Content-Length；没有就按 0 算（无正文的请求）。
    fn request_content_length(head: &[u8]) -> usize {
        String::from_utf8_lossy(head)
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().ok())
                    .flatten()
            })
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn 检查延迟能量出耗时并解析识别结果() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let port = fake_chat_server(
            r#"{"choices":[{"message":{"role":"assistant","content":"{\"code\":\"482913\",\"link\":\"\"}"}}]}"#,
        )
        .await;
        let provider_id = engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "本机测速".to_string(),
                    kind: AiProviderKind::Ollama,
                    base_url: format!("http://127.0.0.1:{port}/v1"),
                    default_model: "qwen".to_string(),
                    models: Vec::new(),
                    thinking_level: AiThinkingLevel::Off,
                    enabled: true,
                    proxy: AccountProxyMode::InheritGlobal,
                },
                None,
            )
            .expect("保存站点")
            .id;
        let result = engine
            .check_notification_latency(provider_id, "", None)
            .await
            .expect("应能测出结果");
        assert!(result.found_code, "应认出验证码");
        assert!(!result.found_link, "邮件里没有链接就不该报有");
        assert!(result.elapsed_ms < 10_000, "本机假服务不该慢过十秒");
    }

    #[tokio::test]
    async fn 检查延迟拒绝没选的站点() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let err = engine
            .check_notification_latency(0, "", None)
            .await
            .expect_err("编号不合法应拒绝");
        assert!(matches!(err, EngineError::BadRequest(_)));
    }

    #[test]
    fn 检查延迟拒绝已停用的站点() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let provider_id = engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "停用站点".to_string(),
                    kind: AiProviderKind::Ollama,
                    base_url: "http://127.0.0.1:11434/v1".to_string(),
                    default_model: "qwen".to_string(),
                    models: Vec::new(),
                    thinking_level: AiThinkingLevel::Off,
                    enabled: false,
                    proxy: AccountProxyMode::InheritGlobal,
                },
                None,
            )
            .expect("保存站点")
            .id;
        let err = engine
            .resolve_probe_target(provider_id, "", None)
            .expect_err("停用站点应拒绝");
        assert!(matches!(err, EngineError::AiProviderNotFound(_)));
    }

    #[test]
    fn 检查延迟留空模型时回退站点默认() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let provider_id = add_ollama(&engine);
        let fallback = engine
            .resolve_probe_target(provider_id, "   ", None)
            .expect("应能构造目标");
        assert_eq!(fallback.model, "qwen");
        assert_eq!(fallback.thinking_level, AiThinkingLevel::Off);
        let explicit = engine
            .resolve_probe_target(provider_id, " qwen2 ", Some(AiThinkingLevel::High))
            .expect("应能构造目标");
        assert_eq!(explicit.model, "qwen2");
        assert_eq!(explicit.thinking_level, AiThinkingLevel::High);
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
                    proxy: AccountProxyMode::InheritGlobal,
                },
                None,
            )
            .expect_err("非本机明文应拒绝");
        assert!(matches!(err, EngineError::Ai(AiError::InsecureEndpoint(_))));
    }

    #[test]
    fn 保存站点拒绝敏感查询参数变体且保留普通参数() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let fake_value = "fake-query-credential";
        for name in ["x-api-key", "client_secret", "access_key", "api-key"] {
            let input = AiProviderInput {
                id: None,
                label: format!("变形{name}"),
                kind: AiProviderKind::OpenAiCompatible,
                base_url: format!("https://api.example.com/v1?{name}={fake_value}"),
                default_model: "m".to_string(),
                models: Vec::new(),
                thinking_level: AiThinkingLevel::Off,
                enabled: true,
                proxy: AccountProxyMode::InheritGlobal,
            };
            let error = engine
                .save_ai_provider(&input, None)
                .expect_err("变形敏感参数必须拒绝");
            let text = error.to_string();
            assert!(text.contains("密钥框"), "{text}");
            assert!(!text.contains(fake_value), "错误不能回显虚构值：{text}");
        }
        assert!(
            engine.list_ai_providers().expect("列站点").is_empty(),
            "被拒站点不能写入数据库"
        );

        let base_url = "https://api.example.com/v1?region=cn&version=2";
        let saved = engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "普通参数".to_string(),
                    kind: AiProviderKind::OpenAiCompatible,
                    base_url: base_url.to_string(),
                    default_model: "m".to_string(),
                    models: Vec::new(),
                    thinking_level: AiThinkingLevel::Off,
                    enabled: true,
                    proxy: AccountProxyMode::InheritGlobal,
                },
                None,
            )
            .expect("普通查询参数应能保存");
        assert_eq!(saved.base_url, base_url);
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
                    proxy: AccountProxyMode::InheritGlobal,
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
            proxy: AccountProxyMode::InheritGlobal,
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
                    proxy: AccountProxyMode::InheritGlobal,
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
                    proxy: AccountProxyMode::InheritGlobal,
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
    #[tokio::test]
    async fn 一键关闭后不外发也不写结果且预览不读正文() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let group_id = enable_group(&engine, "内容分类", 0.75);
        let payload = format!(
            r#"{{"answers":{{"g{group_id}":{{"type":"choice","choice":"账单","confidence":0.90}}}}}}"#
        );
        let (port, captured) = fake_capture_server(&payload).await;
        let provider_id = add_typesafe_at(&engine, format!("http://127.0.0.1:{port}/v1"));
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");
        let message = seed_message_with_body(&engine, 41, "这封邮件的正文绝不该在关闭后被外发");

        // 一键关闭：停掉所有站点，并让待授权令牌失效。
        engine.disable_all_ai().expect("一键关闭");

        // 预览阶段应先被外发开关拦住、不会去读正文：这里故意传一个不存在的邮件编号，
        // 若先去读正文就会报「正文」相关错误。
        let preview_error = engine
            .preview_classify_messages(&[999_999])
            .await
            .expect_err("关闭后预览应被拒");
        assert!(
            !preview_error.to_string().contains("正文"),
            "关闭后预览不该去碰正文：{preview_error}"
        );

        // 直接分类也必须被拦：一个字节都不外发、一条结果都不写。
        let error = engine
            .classify_message_groups(message)
            .await
            .expect_err("关闭后分类应被拒");
        assert!(
            !error.to_string().contains("这封邮件的正文"),
            "错误不能回显正文：{error}"
        );
        assert!(captured.try_recv().is_err(), "关闭总开关后不能有任何请求发出去");
        assert!(
            engine
                .store()
                .message_group_result(message, group_id)
                .expect("读组结果")
                .is_none(),
            "关闭后不能写结果"
        );
    }

    #[tokio::test]
    async fn 成功分类的审计只记摘要不记正文密钥颜色门槛() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let group_id = enable_group(&engine, "内容分类", 0.75);
        let payload = format!(
            r#"{{"answers":{{"g{group_id}":{{"type":"choice","choice":"账单","confidence":0.90}}}}}}"#
        );
        let port = fake_chat_server(&payload).await;
        let secret = Secret::new("sk-audit-should-not-appear");
        let provider_id = engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "jev".to_string(),
                    kind: AiProviderKind::TypeSafe,
                    base_url: format!("http://127.0.0.1:{port}/v1"),
                    default_model: "jev-latest".to_string(),
                    models: Vec::new(),
                    thinking_level: AiThinkingLevel::Off,
                    enabled: true,
                    proxy: AccountProxyMode::InheritGlobal,
                },
                Some(&secret),
            )
            .expect("保存站点")
            .id;
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");
        let message = seed_message_with_body(&engine, 42, "正文里写着绝密正文，不该进审计");

        engine.classify_message_groups(message).await.expect("分类应成功");

        let audits = engine.list_ai_audit(10).expect("审计");
        let ok = audits
            .iter()
            .find(|audit| audit.outcome == "ok")
            .expect("应有成功审计");
        // 摘要里要有组名和标签名，方便回溯。
        assert!(ok.detail.contains("内容分类"), "审计要有组名：{}", ok.detail);
        assert!(ok.detail.contains("账单"), "审计要有标签名：{}", ok.detail);
        // 正文、密钥、颜色键、把握门槛都不能出现。
        assert!(!ok.detail.contains("绝密正文"), "审计不能记正文：{}", ok.detail);
        assert!(
            !ok.detail.contains("sk-audit-should-not-appear"),
            "审计不能记密钥"
        );
        assert!(!ok.detail.contains("orange"), "审计不能记颜色键：{}", ok.detail);
        assert!(!ok.detail.contains("0.75"), "审计不能记把握门槛：{}", ok.detail);
        assert!(!ok.provider_label.contains("绝密正文"));
        assert!(!ok.model.contains("绝密正文"));
        assert!(!ok.target_host.contains("绝密正文"));
    }

    /// 造一个无认证的本地代理配置。
    fn proxy_config(label: &str) -> mail_domain::proxy::ProxyConfig {
        mail_domain::proxy::ProxyConfig {
            id: None,
            label: label.to_string(),
            kind: mail_domain::proxy::ProxyKind::Socks5,
            host: "127.0.0.1".to_string(),
            port: 1080,
            username: String::new(),
        }
    }

    /// 造一个带指定代理策略的本机站点。
    fn add_local_provider_with_proxy(engine: &MailEngine, proxy: AccountProxyMode) -> i64 {
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
                    proxy,
                },
                None,
            )
            .expect("保存站点")
            .id
    }

    #[test]
    fn 站点跟随全局时ai外发走全局代理() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let stored = engine.save_proxy(&proxy_config("全局"), None).expect("保存代理");
        let proxy_id = stored.config.id.expect("代理编号");
        engine
            .set_global_proxy_mode(mail_domain::proxy::GlobalProxyMode::Custom(proxy_id))
            .expect("设为全局");

        let provider_id = add_local_provider_with_proxy(&engine, AccountProxyMode::InheritGlobal);
        engine
            .set_ai_feature(AiFunction::Translate, provider_id, "qwen", None)
            .expect("配置翻译");

        let target = engine.resolve_ai_target(AiFunction::Translate).expect("解析目标");
        assert_eq!(target.proxy, AccountProxyMode::InheritGlobal);
        let route = engine
            .resolve_route(target.proxy)
            .expect("决议应成功")
            .expect("跟随全局应走全局代理");
        assert_eq!(route.config.id, Some(proxy_id));
    }

    #[test]
    fn 站点直连时ai外发不走代理() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let stored = engine.save_proxy(&proxy_config("全局"), None).expect("保存代理");
        engine
            .set_global_proxy_mode(mail_domain::proxy::GlobalProxyMode::Custom(
                stored.config.id.expect("代理编号"),
            ))
            .expect("设为全局");

        let provider_id = add_local_provider_with_proxy(&engine, AccountProxyMode::Direct);
        engine
            .set_ai_feature(AiFunction::Translate, provider_id, "qwen", None)
            .expect("配置翻译");

        let target = engine.resolve_ai_target(AiFunction::Translate).expect("解析目标");
        assert_eq!(target.proxy, AccountProxyMode::Direct);
        assert!(
            engine.resolve_route(target.proxy).expect("决议应成功").is_none(),
            "站点选直连时不该走任何代理"
        );
    }

    #[test]
    fn 站点指定代理时ai外发走指定代理() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let global = engine.save_proxy(&proxy_config("全局"), None).expect("保存代理");
        let chosen = engine.save_proxy(&proxy_config("专用"), None).expect("保存代理");
        let chosen_id = chosen.config.id.expect("代理编号");
        engine
            .set_global_proxy_mode(mail_domain::proxy::GlobalProxyMode::Custom(
                global.config.id.expect("代理编号"),
            ))
            .expect("设为全局");

        let provider_id = add_local_provider_with_proxy(&engine, AccountProxyMode::Custom(chosen_id));
        engine
            .set_ai_feature(AiFunction::Translate, provider_id, "qwen", None)
            .expect("配置翻译");

        let target = engine.resolve_ai_target(AiFunction::Translate).expect("解析目标");
        assert_eq!(target.proxy, AccountProxyMode::Custom(chosen_id));
        let route = engine
            .resolve_route(target.proxy)
            .expect("决议应成功")
            .expect("指定代理应生效");
        assert_eq!(
            route.config.id,
            Some(chosen_id),
            "应走站点指定的代理而不是全局代理"
        );
    }

    #[test]
    fn 保存站点时指定不存在的代理会被拦住() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let err = engine
            .save_ai_provider(
                &AiProviderInput {
                    id: None,
                    label: "本机".to_string(),
                    kind: AiProviderKind::Ollama,
                    base_url: "http://127.0.0.1:11434/v1".to_string(),
                    default_model: "qwen".to_string(),
                    models: Vec::new(),
                    thinking_level: AiThinkingLevel::Off,
                    enabled: true,
                    proxy: AccountProxyMode::Custom(mail_domain::proxy::ProxyId(9999)),
                },
                None,
            )
            .expect_err("不存在的代理应被拦住");
        assert!(
            matches!(err, EngineError::ProxyNotFound(9999)),
            "错误应是代理不存在：{err}"
        );
    }
    #[tokio::test]
    async fn 模型给出不属于该组的标签或越界分数时跳过该组() {
        let dir = tempfile::tempdir().expect("临时目录");
        let engine = engine(dir.path());
        let choice_id = enable_group(&engine, "内容分类", 0.75);
        let score_id = enable_group(&engine, "紧急程度", 0.5);
        // 一个给不存在的标签名，一个给越界的分数，两组都不该落库。
        let payload = format!(
            r#"{{"answers":{{"g{choice_id}":{{"type":"choice","choice":"不存在的标签","confidence":0.90}},"g{score_id}":{{"type":"score","level":9,"confidence":0.80}}}}}}"#
        );
        let port = fake_chat_server(&payload).await;
        let provider_id = add_typesafe_at(&engine, format!("http://127.0.0.1:{port}/v1"));
        engine
            .set_ai_feature(AiFunction::Classify, provider_id, "jev-latest", None)
            .expect("配置分类");
        let message = seed_message_with_body(&engine, 43, "模型乱答也不能写脏数据");

        let results = engine
            .classify_message_groups(message)
            .await
            .expect("分类本身要成功");
        assert!(results.is_empty(), "越界与跨组的答案都应被丢弃：{results:?}");
        assert!(engine
            .store()
            .message_group_result(message, choice_id)
            .expect("读")
            .is_none());
        assert!(engine
            .store()
            .message_group_result(message, score_id)
            .expect("读")
            .is_none());
    }
}
