//! AI 站点、模型映射、段落译文缓存与审计的本地存储（Wave 7）。
//!
//! 安全约定：
//! - 数据库只保存密钥引用键（`api_key_ref`），绝不保存 CDKey / API Key 明文；
//! - 审计只保存调用元数据，不保存邮件正文；
//! - 邮件正文与模型输出都只作为不可信字符串存取，绝不在这里解释或执行。

use rusqlite::OptionalExtension;

use crate::connection::Store;
use crate::error::StoreError;

/// AI 站点类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderKind {
    /// OpenAI 兼容站点。
    OpenAiCompatible,
    /// DeepL 翻译站点。
    DeepL,
    /// 本机 Ollama。
    Ollama,
}

impl AiProviderKind {
    /// 存库用的稳定标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai_compatible",
            Self::DeepL => "deepl",
            Self::Ollama => "ollama",
        }
    }

    /// 从存库文本还原。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai_compatible" | "openai" | "compatible" => Some(Self::OpenAiCompatible),
            "deepl" => Some(Self::DeepL),
            "ollama" => Some(Self::Ollama),
            _ => None,
        }
    }
}

/// 思考程度四档。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AiThinkingLevel {
    /// 关闭。
    #[default]
    Off,
    /// 低。
    Low,
    /// 中。
    Medium,
    /// 高。
    High,
}

impl AiThinkingLevel {
    /// 存库用的稳定标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// 从存库文本还原；未知值按关闭处理。
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Self::Low,
            "medium" | "mid" => Self::Medium,
            "high" => Self::High,
            _ => Self::Off,
        }
    }
}

/// 可以使用 AI 的功能。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiFunction {
    /// 翻译。
    Translate,
    /// 摘要。
    Summary,
    /// 润色。
    Polish,
    /// 起草。
    Draft,
    /// 通知识别：判断邮件正文里有没有验证码或验证链接。
    NotificationVerify,
}

impl AiFunction {
    /// 存库用的稳定标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Translate => "translate",
            Self::Summary => "summary",
            Self::Polish => "polish",
            Self::Draft => "draft",
            Self::NotificationVerify => "notification_verify",
        }
    }

    /// 从存库文本还原。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "translate" => Some(Self::Translate),
            "summary" => Some(Self::Summary),
            "polish" => Some(Self::Polish),
            "draft" => Some(Self::Draft),
            "notification_verify" => Some(Self::NotificationVerify),
            _ => None,
        }
    }
}

/// 库里的一条 AI 站点配置（不含密钥明文）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAiProvider {
    /// 主键。
    pub id: i64,
    /// 界面显示名。
    pub label: String,
    /// 站点类型。
    pub kind: AiProviderKind,
    /// 站点根地址。
    pub base_url: String,
    /// 站点默认模型。
    pub default_model: String,
    /// 拉取或手工填写的模型列表。
    pub models: Vec<String>,
    /// 站点默认思考程度。
    pub thinking_level: AiThinkingLevel,
    /// 是否启用。
    pub enabled: bool,
    /// 系统凭据库引用键；不含密钥明文。
    pub api_key_ref: Option<String>,
    /// 创建时间。
    pub created_at: String,
    /// 更新时间。
    pub updated_at: String,
}

/// 新建或替换 AI 站点时写入的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAiProvider {
    /// 界面显示名。
    pub label: String,
    /// 站点类型。
    pub kind: AiProviderKind,
    /// 站点根地址。
    pub base_url: String,
    /// 站点默认模型。
    pub default_model: String,
    /// 拉取或手工填写的模型列表。
    pub models: Vec<String>,
    /// 站点默认思考程度。
    pub thinking_level: AiThinkingLevel,
    /// 是否启用。
    pub enabled: bool,
}

/// 功能级模型映射；`thinking_level` 为空时回退站点默认。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiModelMapEntry {
    /// 功能。
    pub function: AiFunction,
    /// 使用的站点主键。
    pub provider_id: i64,
    /// 功能级模型；空串表示回退站点默认模型。
    pub model: String,
    /// 功能级思考程度；None 表示回退站点默认。
    pub thinking_level: Option<AiThinkingLevel>,
    /// 更新时间。
    pub updated_at: String,
}

/// 段落对齐译文缓存的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAiCache {
    /// 缓存键。
    pub cache_key: String,
    /// 关联邮件；摘要等非邮件调用可为空。
    pub message_id: Option<i64>,
    /// 功能标识。
    pub function: String,
    /// 调用模型。
    pub model: String,
    /// 目标语言。
    pub target_lang: String,
    /// 原文内容哈希。
    pub source_hash: String,
    /// 段落对齐译文 JSON 数组。
    pub segments_json: String,
    /// 是否因为模型不支持思考程度而降级。
    pub thinking_downgraded: bool,
    /// 创建时间。
    pub created_at: String,
    /// 更新时间。
    pub updated_at: String,
}

/// 写入一条 AI 调用审计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAiAudit {
    /// 功能标识。
    pub function: String,
    /// 站点主键；站点已删除时可为空。
    pub provider_id: Option<i64>,
    /// 站点显示名。
    pub provider_label: String,
    /// 模型。
    pub model: String,
    /// 外发目标域名。
    pub target_host: String,
    /// 是否本机服务。
    pub local: bool,
    /// 是否发生外发。
    pub outbound: bool,
    /// 结果分类，如 `ok` / `failed` / `downgraded`。
    pub outcome: String,
    /// 可读但已脱敏的补充说明。
    pub detail: String,
}

/// 库里的一条 AI 审计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAiAudit {
    /// 主键。
    pub id: i64,
    /// 创建时间。
    pub created_at: String,
    /// 功能标识。
    pub function: String,
    /// 站点主键。
    pub provider_id: Option<i64>,
    /// 站点显示名。
    pub provider_label: String,
    /// 模型。
    pub model: String,
    /// 外发目标域名。
    pub target_host: String,
    /// 是否本机服务。
    pub local: bool,
    /// 是否发生外发。
    pub outbound: bool,
    /// 结果分类。
    pub outcome: String,
    /// 补充说明。
    pub detail: String,
}

const SELECT_PROVIDER: &str = "SELECT id, label, kind, base_url, default_model, models_json, \
    thinking_level, enabled, api_key_ref, created_at, updated_at FROM ai_provider";
const SELECT_MODEL_MAP: &str = "SELECT function, provider_id, model, thinking_level, updated_at \
    FROM ai_model_map";
const SELECT_CACHE: &str = "SELECT cache_key, message_id, function, model, target_lang, source_hash, \
    segments_json, thinking_downgraded, created_at, updated_at FROM ai_cache";
const SELECT_AUDIT: &str = "SELECT id, created_at, function, provider_id, provider_label, model, \
    target_host, local, outbound, outcome, detail FROM ai_audit";

fn parse_models(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn row_to_provider(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredAiProvider> {
    let kind: String = row.get(2)?;
    let models_json: String = row.get(5)?;
    let thinking: String = row.get(6)?;
    Ok(StoredAiProvider {
        id: row.get(0)?,
        label: row.get(1)?,
        kind: AiProviderKind::parse(&kind).unwrap_or(AiProviderKind::OpenAiCompatible),
        base_url: row.get(3)?,
        default_model: row.get(4)?,
        models: parse_models(&models_json),
        thinking_level: AiThinkingLevel::parse(&thinking),
        enabled: row.get::<_, i64>(7)? != 0,
        api_key_ref: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn row_to_model_map(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiModelMapEntry> {
    let function: String = row.get(0)?;
    let thinking: Option<String> = row.get(3)?;
    Ok(AiModelMapEntry {
        function: AiFunction::parse(&function).unwrap_or(AiFunction::Translate),
        provider_id: row.get(1)?,
        model: row.get(2)?,
        thinking_level: thinking.as_deref().map(AiThinkingLevel::parse),
        updated_at: row.get(4)?,
    })
}

fn row_to_cache(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredAiCache> {
    Ok(StoredAiCache {
        cache_key: row.get(0)?,
        message_id: row.get(1)?,
        function: row.get(2)?,
        model: row.get(3)?,
        target_lang: row.get(4)?,
        source_hash: row.get(5)?,
        segments_json: row.get(6)?,
        thinking_downgraded: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn row_to_audit(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredAiAudit> {
    Ok(StoredAiAudit {
        id: row.get(0)?,
        created_at: row.get(1)?,
        function: row.get(2)?,
        provider_id: row.get(3)?,
        provider_label: row.get(4)?,
        model: row.get(5)?,
        target_host: row.get(6)?,
        local: row.get::<_, i64>(7)? != 0,
        outbound: row.get::<_, i64>(8)? != 0,
        outcome: row.get(9)?,
        detail: row.get(10)?,
    })
}

impl Store {
    /// 列出全部 AI 站点。
    pub fn list_ai_providers(&self) -> Result<Vec<StoredAiProvider>, StoreError> {
        let sql = format!("{SELECT_PROVIDER} ORDER BY id ASC");
        let mut stmt = self.conn().prepare(&sql)?;
        let rows = stmt.query_map([], row_to_provider)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 按主键取一个 AI 站点。
    pub fn get_ai_provider(&self, id: i64) -> Result<Option<StoredAiProvider>, StoreError> {
        let sql = format!("{SELECT_PROVIDER} WHERE id = ?1");
        self.conn()
            .query_row(&sql, [id], row_to_provider)
            .optional()
            .map_err(StoreError::from)
    }

    /// 新建 AI 站点，返回主键；只接受密钥引用键。
    pub fn insert_ai_provider(
        &self,
        provider: &NewAiProvider,
        api_key_ref: Option<&str>,
    ) -> Result<i64, StoreError> {
        let models = serde_json::to_string(&provider.models)
            .map_err(|_| StoreError::InvalidData("AI 模型列表无法序列化".to_string()))?;
        self.conn().execute(
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
        Ok(self.conn().last_insert_rowid())
    }

    /// 更新 AI 站点；不存在时返回 `false`。`api_key_ref` 直接替换引用键。
    pub fn update_ai_provider(
        &self,
        id: i64,
        provider: &NewAiProvider,
        api_key_ref: Option<&str>,
    ) -> Result<bool, StoreError> {
        let models = serde_json::to_string(&provider.models)
            .map_err(|_| StoreError::InvalidData("AI 模型列表无法序列化".to_string()))?;
        let changed = self.conn().execute(
            "UPDATE ai_provider
             SET label = ?2, kind = ?3, base_url = ?4, default_model = ?5,
                 models_json = ?6, thinking_level = ?7, enabled = ?8, api_key_ref = ?9,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            rusqlite::params![
                id,
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
        Ok(changed > 0)
    }

    /// 只更新站点模型列表（测试连接 / 拉取模型后使用）。
    pub fn update_ai_provider_models(&self, id: i64, models: &[String]) -> Result<bool, StoreError> {
        let models = serde_json::to_string(models)
            .map_err(|_| StoreError::InvalidData("AI 模型列表无法序列化".to_string()))?;
        let changed = self.conn().execute(
            "UPDATE ai_provider SET models_json = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
            rusqlite::params![id, models],
        )?;
        Ok(changed > 0)
    }

    /// 删除 AI 站点；功能映射由外键级联删除。
    pub fn delete_ai_provider(&self, id: i64) -> Result<bool, StoreError> {
        Ok(self
            .conn()
            .execute("DELETE FROM ai_provider WHERE id = ?1", [id])?
            > 0)
    }

    /// 一键关闭全部 AI 站点；返回受影响条数。
    pub fn disable_all_ai_providers(&self) -> Result<usize, StoreError> {
        Ok(self.conn().execute(
            "UPDATE ai_provider SET enabled = 0,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE enabled != 0",
            [],
        )?)
    }

    /// 列出全部功能级模型映射。
    pub fn list_ai_model_maps(&self) -> Result<Vec<AiModelMapEntry>, StoreError> {
        let sql = format!("{SELECT_MODEL_MAP} ORDER BY function ASC");
        let mut stmt = self.conn().prepare(&sql)?;
        let rows = stmt.query_map([], row_to_model_map)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 取一个功能的模型映射。
    pub fn get_ai_model_map(&self, function: AiFunction) -> Result<Option<AiModelMapEntry>, StoreError> {
        let sql = format!("{SELECT_MODEL_MAP} WHERE function = ?1");
        self.conn()
            .query_row(&sql, [function.as_str()], row_to_model_map)
            .optional()
            .map_err(StoreError::from)
    }

    /// 新建或替换一个功能的模型映射。
    pub fn upsert_ai_model_map(
        &self,
        function: AiFunction,
        provider_id: i64,
        model: &str,
        thinking_level: Option<AiThinkingLevel>,
    ) -> Result<(), StoreError> {
        self.conn().execute(
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

    /// 删除一个功能的模型映射。
    pub fn delete_ai_model_map(&self, function: AiFunction) -> Result<bool, StoreError> {
        Ok(self.conn().execute(
            "DELETE FROM ai_model_map WHERE function = ?1",
            [function.as_str()],
        )? > 0)
    }

    /// 按缓存键读取缓存。
    pub fn get_ai_cache(&self, cache_key: &str) -> Result<Option<StoredAiCache>, StoreError> {
        let sql = format!("{SELECT_CACHE} WHERE cache_key = ?1");
        self.conn()
            .query_row(&sql, [cache_key], row_to_cache)
            .optional()
            .map_err(StoreError::from)
    }

    /// 新建或替换缓存。
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_ai_cache(
        &self,
        cache_key: &str,
        message_id: Option<i64>,
        function: &str,
        model: &str,
        target_lang: &str,
        source_hash: &str,
        segments_json: &str,
        thinking_downgraded: bool,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO ai_cache
                 (cache_key, message_id, function, model, target_lang, source_hash,
                  segments_json, thinking_downgraded)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(cache_key) DO UPDATE SET
                 message_id = excluded.message_id,
                 function = excluded.function,
                 model = excluded.model,
                 target_lang = excluded.target_lang,
                 source_hash = excluded.source_hash,
                 segments_json = excluded.segments_json,
                 thinking_downgraded = excluded.thinking_downgraded,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            rusqlite::params![
                cache_key,
                message_id,
                function,
                model,
                target_lang,
                source_hash,
                segments_json,
                i64::from(thinking_downgraded),
            ],
        )?;
        Ok(())
    }

    /// 清空全部 AI 缓存；返回删除条数。
    pub fn clear_ai_cache(&self) -> Result<usize, StoreError> {
        Ok(self.conn().execute("DELETE FROM ai_cache", [])?)
    }

    /// 清掉一封邮件的 AI 缓存；返回删除条数。
    pub fn clear_ai_cache_for_message(&self, message_id: i64) -> Result<usize, StoreError> {
        Ok(self
            .conn()
            .execute("DELETE FROM ai_cache WHERE message_id = ?1", [message_id])?)
    }

    /// 写入一条 AI 审计。
    pub fn insert_ai_audit(&self, audit: &NewAiAudit) -> Result<i64, StoreError> {
        self.conn().execute(
            "INSERT INTO ai_audit
                 (function, provider_id, provider_label, model, target_host, local, outbound, outcome, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                audit.function,
                audit.provider_id,
                audit.provider_label,
                audit.model,
                audit.target_host,
                i64::from(audit.local),
                i64::from(audit.outbound),
                audit.outcome,
                audit.detail,
            ],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// 读取最近 AI 审计，按时间倒序。
    pub fn list_ai_audit(&self, limit: usize) -> Result<Vec<StoredAiAudit>, StoreError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let sql = format!("{SELECT_AUDIT} ORDER BY id DESC LIMIT ?1");
        let mut stmt = self.conn().prepare(&sql)?;
        let rows = stmt.query_map([limit], row_to_audit)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 审计总条数。
    pub fn ai_audit_count(&self) -> Result<i64, StoreError> {
        Ok(self
            .conn()
            .query_row("SELECT COUNT(*) FROM ai_audit", [], |row| row.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let mut store = Store::open_in_memory().expect("打开内存库");
        store.run_migrations().expect("迁移");
        store
    }

    fn provider() -> NewAiProvider {
        NewAiProvider {
            label: "本机".to_string(),
            kind: AiProviderKind::Ollama,
            base_url: "http://127.0.0.1:11434/v1".to_string(),
            default_model: "qwen".to_string(),
            models: vec!["qwen".to_string()],
            thinking_level: AiThinkingLevel::Low,
            enabled: true,
        }
    }

    #[test]
    fn 站点可以增删改查且只保存密钥引用() {
        let store = store();
        let id = store
            .insert_ai_provider(&provider(), Some("ai/ref-only"))
            .expect("新建");
        let saved = store.get_ai_provider(id).expect("查询").expect("存在");
        assert_eq!(saved.kind, AiProviderKind::Ollama);
        assert_eq!(saved.api_key_ref.as_deref(), Some("ai/ref-only"));
        assert_eq!(saved.models, vec!["qwen".to_string()]);
        assert!(!saved.base_url.contains("ref-only"));

        let mut edited = provider();
        edited.label = "改过".to_string();
        assert!(store
            .update_ai_provider(id, &edited, Some("ai/ref-only"))
            .expect("更新"));
        assert_eq!(
            store.get_ai_provider(id).expect("查询").expect("存在").label,
            "改过"
        );
        assert!(store.delete_ai_provider(id).expect("删除"));
        assert!(store.get_ai_provider(id).expect("查询").is_none());
    }

    #[test]
    fn 功能映射和缓存都能覆盖和清空() {
        let store = store();
        let id = store.insert_ai_provider(&provider(), None).expect("新建");
        store
            .upsert_ai_model_map(AiFunction::Translate, id, "qwen", Some(AiThinkingLevel::High))
            .expect("写映射");
        store
            .upsert_ai_model_map(AiFunction::Translate, id, "qwen2", None)
            .expect("覆盖映射");
        let map = store
            .get_ai_model_map(AiFunction::Translate)
            .expect("查询")
            .expect("存在");
        assert_eq!(map.model, "qwen2");
        assert_eq!(map.thinking_level, None);

        store
            .upsert_ai_cache(
                "k",
                None,
                "translate",
                "qwen",
                "zh",
                "hash",
                r#"["甲","乙"]"#,
                true,
            )
            .expect("写缓存");
        let cached = store.get_ai_cache("k").expect("查询").expect("存在");
        assert_eq!(cached.segments_json, r#"["甲","乙"]"#);
        assert!(cached.thinking_downgraded);
        assert_eq!(store.clear_ai_cache().expect("清缓存"), 1);
        assert!(store.get_ai_cache("k").expect("查询").is_none());
    }

    #[test]
    fn 通知智能识别功能映射可以写读() {
        // 回归：0008 的 CHECK 只允许 translate/summary/polish/draft，
        // notification_verify 会被拒；0012 迁移放开后这里要能写能读。
        let store = store();
        let id = store.insert_ai_provider(&provider(), None).expect("新建");
        store
            .upsert_ai_model_map(AiFunction::NotificationVerify, id, "deepseek-v4.1-flash", None)
            .expect("写通知智能识别映射");
        let map = store
            .get_ai_model_map(AiFunction::NotificationVerify)
            .expect("查询")
            .expect("存在");
        assert_eq!(map.model, "deepseek-v4.1-flash");
    }

    #[test]
    fn 审计只记元数据且可以按时间读取() {
        let store = store();
        let id = store.insert_ai_provider(&provider(), None).expect("新建");
        store
            .insert_ai_audit(&NewAiAudit {
                function: "summary".to_string(),
                provider_id: Some(id),
                provider_label: "本机".to_string(),
                model: "qwen".to_string(),
                target_host: "127.0.0.1".to_string(),
                local: true,
                outbound: false,
                outcome: "ok".to_string(),
                detail: "已调用".to_string(),
            })
            .expect("写审计");
        assert_eq!(store.ai_audit_count().expect("计数"), 1);
        let rows = store.list_ai_audit(10).expect("读取");
        assert_eq!(rows[0].function, "summary");
        assert!(!rows[0].detail.contains("邮件正文"));
    }

    #[test]
    fn 一键关闭会停掉所有站点() {
        let store = store();
        store.insert_ai_provider(&provider(), None).expect("新建");
        assert_eq!(store.disable_all_ai_providers().expect("关闭"), 1);
        assert!(!store.list_ai_providers().expect("读取")[0].enabled);
    }
}
