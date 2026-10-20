//! 站点调用：拉模型列表、对话、逐段翻译。
//!
//! 这一层只负责「按站点的协议把请求发出去、把结果解析回来」，不管缓存、不管授权、
//! 不管是否外发——那些都在 mail-core 的编排层决定。

use std::time::Duration;

use mail_domain::proxy::{ProxyRoute, Secret};
use serde_json::{json, Value};

use crate::error::AiError;
use crate::http::request;
use crate::prompt;
use crate::provider::{join_url, ProviderKind, ThinkingLevel};

/// 一个站点的调用参数。
#[derive(Debug, Clone)]
pub struct Endpoint<'a> {
    /// 站点类型。
    pub kind: ProviderKind,
    /// 站点地址；通用决策站点填写完整请求地址，已经过 `normalize_base_url` 校验。
    pub base_url: &'a str,
    /// CDKey / API Key；本机 Ollama 可以不填。
    pub api_key: Option<&'a Secret>,
}

/// 一次对话调用的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatOutcome {
    /// 模型输出的纯文本。
    pub text: String,
    /// 是不是因为模型不支持而自动降级了思考程度。
    pub thinking_downgraded: bool,
}

/// 一次翻译调用的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationOutcome {
    /// 逐段对齐的译文，顺序与输入完全一致。
    pub segments: Vec<String>,
    /// 是不是因为模型不支持而自动降级了思考程度。
    pub thinking_downgraded: bool,
}

/// 拉取站点支持的模型编号。
pub async fn list_models(
    route: Option<&ProxyRoute>,
    endpoint: &Endpoint<'_>,
    timeout: Duration,
) -> Result<Vec<String>, AiError> {
    if endpoint.kind.translation_only() || endpoint.kind.is_decision() {
        return Err(AiError::Unsupported("拉取模型列表"));
    }
    let url = join_url(endpoint.base_url, "models");
    let headers = auth_headers(endpoint)?;
    let response = request(route, "GET", &url, &headers, None, timeout).await?;
    if response.status != 200 {
        return Err(status_error(response.status, endpoint, &response.body));
    }
    let value: Value = serde_json::from_str(&response.body).map_err(|_| AiError::Protocol)?;
    let mut models = Vec::new();
    if let Some(items) = value.get("data").and_then(|item| item.as_array()) {
        for item in items {
            if let Some(id) = item.get("id").and_then(|id| id.as_str()) {
                if !id.trim().is_empty() {
                    models.push(id.to_string());
                }
            }
        }
    }
    // 有些站点直接返回字符串数组。
    if models.is_empty() {
        if let Some(items) = value.get("models").and_then(|item| item.as_array()) {
            for item in items {
                if let Some(id) = item.as_str() {
                    models.push(id.to_string());
                }
            }
        }
    }
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err(AiError::Protocol);
    }
    Ok(models)
}

/// 决策请求地址；通用站点用完整地址，旧 TypeSafe 地址按原规则补路径。
fn decision_endpoint_url(base_url: &str, kind: ProviderKind) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    match kind {
        ProviderKind::TypeSafe => {
            if trimmed.to_ascii_lowercase().ends_with("/systemone") {
                trimmed.to_string()
            } else {
                join_url(trimmed, "systemone")
            }
        }
        ProviderKind::DecisionApi => trimmed.to_string(),
        _ => trimmed.to_string(),
    }
}
/// 一次邮件分类的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifyOutcome {
    /// 模型选中的标签名。
    pub choice: String,
    /// 把握程度，0 到 1。
    pub confidence: f64,
}

/// 调决策模型站点做一次邮件分类。
///
/// `state` 是邮件正文纯文本（调用方负责截断与安全处理），`choices` 是当前
/// 本地标签名。返回模型选中的标签与把握程度；结果不合法一律按协议错误处理。
pub async fn classify_mail(
    route: Option<&ProxyRoute>,
    endpoint: &Endpoint<'_>,
    model: &str,
    state: &str,
    choices: &[String],
    timeout: Duration,
) -> Result<ClassifyOutcome, AiError> {
    if !endpoint.kind.is_decision() {
        return Err(AiError::Unsupported("邮件分类"));
    }
    if model.trim().is_empty() {
        return Err(AiError::MissingModel);
    }
    if state.trim().is_empty() || choices.is_empty() {
        return Err(AiError::Protocol);
    }
    let url = decision_endpoint_url(endpoint.base_url, endpoint.kind);
    let headers = auth_headers(endpoint)?;
    // 官方形状（docs.typesafe.ai/api）：
    // - questions 是「问题编号 → 问题对象」的对象，不是数组；
    // - choice 的选项放在 criteria 里，键是选项名、值是说明；
    //   我们只有标签名，说明给 null（官方明确说选项不需要额外说明时用 null）。
    let mut criteria = serde_json::Map::new();
    for choice in choices {
        criteria.insert(choice.clone(), Value::Null);
    }
    let body = json!({
        "state": state,
        "model": model,
        "questions": {
            "category": {
                "type": "choice",
                "instructions": "Classify this email: pick exactly one of the categories listed in criteria.",
                "criteria": criteria
            }
        }
    });
    let payload = body.to_string();
    let response = request(route, "POST", &url, &headers, Some(&payload), timeout).await?;
    if response.status != 200 {
        return Err(status_error(response.status, endpoint, &response.body));
    }
    let value: Value = serde_json::from_str(&response.body).map_err(|_| AiError::Protocol)?;
    let answer = value
        .get("answers")
        .and_then(|answers| answers.get("category"))
        .ok_or(AiError::Protocol)?;
    let choice = answer
        .get("choice")
        .and_then(|choice| choice.as_str())
        .map(str::trim)
        .filter(|choice| !choice.is_empty())
        .ok_or(AiError::Protocol)?
        .to_string();
    if !choices.iter().any(|candidate| candidate == &choice) {
        return Err(AiError::Protocol);
    }
    let confidence = answer_confidence(answer).ok_or(AiError::Protocol)?;
    Ok(ClassifyOutcome { choice, confidence })
}

/// 标签组的题型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupQuestionKind {
    /// 选择题：从标签里选一个。
    Choice,
    /// 是否题：返回 0 到 1 的概率。
    Boolean,
    /// 打分题：在有序档位里选一个。
    Score,
}

/// 一次多问题分类请求里的一个标签组问题。
///
/// mail-ai 不反向依赖 mail-store，所以这里用本地形状；编排层负责把组数据搬进来。
#[derive(Debug, Clone, PartialEq)]
pub struct GroupQuestion {
    /// 组编号；请求键和应答键都用 `g<编号>`。
    pub group_id: i64,
    /// 题型。
    pub kind: GroupQuestionKind,
    /// 给模型的完整指令。
    pub instructions: String,
    /// 选择题的候选标签名；打分题的有序档位描述；是否题留空。
    pub criteria: Vec<String>,
    /// 打分题下限；其它题型为空。
    pub score_min: Option<i64>,
    /// 打分题上限；其它题型为空。
    pub score_max: Option<i64>,
}

/// 一个标签组的答案。
#[derive(Debug, Clone, PartialEq)]
pub enum GroupAnswer {
    /// 选择题：选中的标签与把握程度。
    Choice {
        /// 选中的标签名。
        label: String,
        /// 把握程度，0 到 1。
        confidence: f64,
    },
    /// 是否题：回答「是」的概率，0 到 1。
    Boolean {
        /// 「是」的概率。
        probability: f64,
    },
    /// 打分题：落在哪一档与把握程度。
    Score {
        /// 档位数字，落在组的上下限之间。
        level: i64,
        /// 把握程度，0 到 1。
        confidence: f64,
    },
}

/// 一次请求问完所有已开启的标签组。
///
/// 请求形状按规格 3.2；应答按 `answers.g<编号>` 逐个解析，某一组不合法只跳过该组，
/// 不影响其它组。整批请求失败（网络、超时、非 200、应答不是 JSON）才返回错误。
///
/// 注意：`noul` 与 `score` 的应答字段名**还没和真机对过**。这里按官方文档
/// （docs.typesafe.ai/primitives）选形状：是否题读 `noul`，打分题读 `probabilities`
/// 并取概率最高的一档；同时兼容未验证的备用字段 `probability` / `level`，
/// 真机接入后再按实测结果收窄。
pub async fn classify_mail_groups(
    route: Option<&ProxyRoute>,
    endpoint: &Endpoint<'_>,
    model: &str,
    state: &str,
    questions: &[GroupQuestion],
    timeout: Duration,
) -> Result<Vec<(i64, GroupAnswer)>, AiError> {
    if !endpoint.kind.is_decision() {
        return Err(AiError::Unsupported("邮件分类"));
    }
    if model.trim().is_empty() {
        return Err(AiError::MissingModel);
    }
    if state.trim().is_empty() || questions.is_empty() {
        return Err(AiError::Protocol);
    }
    let url = decision_endpoint_url(endpoint.base_url, endpoint.kind);
    let headers = auth_headers(endpoint)?;
    let mut question_map = serde_json::Map::new();
    for question in questions {
        let key = format!("g{}", question.group_id);
        let value = match question.kind {
            GroupQuestionKind::Choice => {
                // 官方形状：criteria 是「选项名 → 说明」，不需要说明时给 null。
                let mut criteria = serde_json::Map::new();
                for label in &question.criteria {
                    criteria.insert(label.clone(), Value::Null);
                }
                json!({
                    "type": "choice",
                    "instructions": question.instructions,
                    "criteria": criteria
                })
            }
            GroupQuestionKind::Boolean => json!({
                "type": "noul",
                "instructions": question.instructions
            }),
            GroupQuestionKind::Score => json!({
                "type": "score",
                "instructions": question.instructions,
                "criteria": question.criteria
            }),
        };
        question_map.insert(key, value);
    }
    let body = json!({
        "state": state,
        "model": model,
        "questions": question_map
    });
    let payload = body.to_string();
    let response = request(route, "POST", &url, &headers, Some(&payload), timeout).await?;
    if response.status != 200 {
        return Err(status_error(response.status, endpoint, &response.body));
    }
    let value: Value = serde_json::from_str(&response.body).map_err(|_| AiError::Protocol)?;
    let answers = value
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(AiError::Protocol)?;
    let mut parsed = Vec::new();
    for question in questions {
        let key = format!("g{}", question.group_id);
        let Some(answer) = answers.get(&key) else {
            continue;
        };
        // 缺省类型可以按题型解析；显式给了非法类型则只跳过这一组。
        if answer.get("type").is_some_and(|kind| !kind.is_string()) {
            continue;
        }
        let answer_type = answer.get("type").and_then(Value::as_str);
        let one = match question.kind {
            GroupQuestionKind::Choice => parse_choice_answer(answer, answer_type, question),
            GroupQuestionKind::Boolean => parse_boolean_answer(answer, answer_type),
            GroupQuestionKind::Score => parse_score_answer(answer, answer_type, question),
        };
        if let Some(answer) = one {
            parsed.push((question.group_id, answer));
        }
    }
    Ok(parsed)
}

/// 解析选择题答案；类型不符、选项不在候选里或把握程度越界都返回 `None`。
fn parse_choice_answer(
    answer: &Value,
    answer_type: Option<&str>,
    question: &GroupQuestion,
) -> Option<GroupAnswer> {
    if answer_type.is_some_and(|kind| kind != "choice") {
        return None;
    }
    let label = answer
        .get("choice")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty())?
        .to_string();
    if !question.criteria.iter().any(|candidate| candidate == &label) {
        return None;
    }
    if let Some(probabilities) = answer.get("probabilities") {
        if probabilities
            .as_object()?
            .keys()
            .any(|label| !question.criteria.contains(label))
        {
            return None;
        }
    }
    let confidence = answer_confidence(answer)?;
    Some(GroupAnswer::Choice { label, confidence })
}

/// 解析是否题答案；类型不符或概率越界都返回 `None`。
fn parse_boolean_answer(answer: &Value, answer_type: Option<&str>) -> Option<GroupAnswer> {
    if answer_type.is_some_and(|kind| !matches!(kind, "noul" | "bool")) {
        return None;
    }
    // 官方文档字段是 `noul`；`probability` 是未真机验证的备用形状。
    let probability = answer
        .get("noul")
        .or_else(|| answer.get("probability"))
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_bool().map(|flag| if flag { 1.0 } else { 0.0 }))
        })?;
    if !valid_confidence(probability) {
        return None;
    }
    Some(GroupAnswer::Boolean { probability })
}

/// 解析打分题答案；类型不符、档位越界或把握程度越界都返回 `None`。
fn parse_score_answer(
    answer: &Value,
    answer_type: Option<&str>,
    question: &GroupQuestion,
) -> Option<GroupAnswer> {
    if answer_type.is_some_and(|kind| kind != "score") {
        return None;
    }
    let (min, max) = (question.score_min?, question.score_max?);
    let level = score_level_from_answer(answer, min, max)?;
    if !(min..=max).contains(&level) {
        return None;
    }
    let confidence = answer.get("confidence").and_then(Value::as_f64)?;
    if !valid_confidence(confidence) {
        return None;
    }
    Some(GroupAnswer::Score { level, confidence })
}

/// 从打分题答案里取整数档位。
///
/// 官方形状（未真机验证）是 `probabilities: {"0": 0.0, "1": 0.57, "2": 0.43}`，
/// 按规格 3.4 取概率最高的一档，换算成数字：`min + 下标`。
/// 找不到 `probabilities` 时读整数 `score`，也兼容旧 `level` 字段。
fn score_level_from_answer(answer: &Value, min: i64, max: i64) -> Option<i64> {
    let count = max.checked_sub(min).and_then(|diff| diff.checked_add(1))?;
    if !(2..=10).contains(&count) {
        return None;
    }
    if let Some(probabilities) = answer.get("probabilities") {
        let probabilities = probabilities.as_object()?;
        let mut best: Option<(i64, f64)> = None;
        for (level, probability) in probabilities {
            let index: i64 = level.parse().ok()?;
            let value = probability.as_f64()?;
            if !valid_confidence(value) || !(0..count).contains(&index) {
                return None;
            }
            if best.is_none_or(|(_, top)| value > top) {
                best = Some((index, value));
            }
        }
        let (index, _) = best?;
        return min.checked_add(index);
    }
    answer
        .get("score")
        .and_then(Value::as_i64)
        .or_else(|| answer.get("level").and_then(Value::as_i64))
}

/// 选择题优先读把握程度；缺省时从概率中取最大值。
fn answer_confidence(answer: &Value) -> Option<f64> {
    // 有概率表时逐项校验，不能丢掉坏项后继续使用其它值。
    let mut maximum: Option<f64> = None;
    if let Some(probabilities) = answer.get("probabilities") {
        let probabilities = probabilities.as_object()?;
        if probabilities.is_empty() {
            return None;
        }
        for probability in probabilities.values() {
            let probability = probability.as_f64()?;
            if !valid_confidence(probability) {
                return None;
            }
            maximum = Some(maximum.map_or(probability, |top| top.max(probability)));
        }
    }
    if let Some(confidence) = answer.get("confidence") {
        let confidence = confidence.as_f64()?;
        return valid_confidence(confidence).then_some(confidence);
    }
    maximum
}
/// 把握程度 / 概率必须在 0 到 1 之间。
fn valid_confidence(value: f64) -> bool {
    (0.0..=1.0).contains(&value)
}
/// 发一次对话，返回模型输出的纯文本。
#[allow(clippy::too_many_arguments)]
pub async fn chat(
    route: Option<&ProxyRoute>,
    endpoint: &Endpoint<'_>,
    model: &str,
    thinking: ThinkingLevel,
    system: &str,
    user: &str,
    timeout: Duration,
) -> Result<ChatOutcome, AiError> {
    if endpoint.kind.translation_only() || endpoint.kind.is_decision() {
        return Err(AiError::Unsupported("摘要与润色"));
    }
    if model.trim().is_empty() {
        return Err(AiError::MissingModel);
    }
    let url = join_url(endpoint.base_url, "chat/completions");
    let headers = auth_headers(endpoint)?;

    let mut downgraded = false;
    let mut effort = thinking.reasoning_effort();
    loop {
        let mut body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "stream": false
        });
        if let Some(level) = effort {
            body["reasoning_effort"] = json!(level);
        }
        let payload = body.to_string();
        let response = request(route, "POST", &url, &headers, Some(&payload), timeout).await?;
        if response.status == 200 {
            let text = parse_chat_text(&response.body)?;
            return Ok(ChatOutcome {
                text,
                thinking_downgraded: downgraded,
            });
        }
        // 模型不认识思考程度参数时降级重试一次，绝不让调用直接失败。
        if effort.is_some() && AiError::looks_like_thinking_unsupported(response.status, &response.body) {
            tracing::debug!(
                status = response.status,
                "该模型不支持思考程度参数，改成不带该参数重试"
            );
            effort = None;
            downgraded = true;
            continue;
        }
        return Err(status_error(response.status, endpoint, &response.body));
    }
}

/// 逐段翻译；输入与输出段数必须一致。
#[allow(clippy::too_many_arguments)]
pub async fn translate(
    route: Option<&ProxyRoute>,
    endpoint: &Endpoint<'_>,
    model: &str,
    thinking: ThinkingLevel,
    texts: &[String],
    target_language: &str,
    timeout: Duration,
) -> Result<TranslationOutcome, AiError> {
    if texts.is_empty() {
        return Ok(TranslationOutcome {
            segments: Vec::new(),
            thinking_downgraded: false,
        });
    }
    if endpoint.kind == ProviderKind::DeepL {
        return translate_deepl(route, endpoint, texts, target_language, timeout).await;
    }

    let outcome = chat(
        route,
        endpoint,
        model,
        thinking,
        &prompt::translate_system(),
        &prompt::translate_user(texts, target_language),
        timeout,
    )
    .await?;
    let segments = parse_string_array(&outcome.text).ok_or(AiError::Protocol)?;
    if segments.len() != texts.len() {
        return Err(AiError::SegmentMismatch);
    }
    Ok(TranslationOutcome {
        segments,
        thinking_downgraded: outcome.thinking_downgraded,
    })
}

/// DeepL 走自家的翻译接口。
async fn translate_deepl(
    route: Option<&ProxyRoute>,
    endpoint: &Endpoint<'_>,
    texts: &[String],
    target_language: &str,
    timeout: Duration,
) -> Result<TranslationOutcome, AiError> {
    let url = join_url(endpoint.base_url, "v2/translate");
    let headers = deepl_headers(endpoint)?;
    let body = json!({
        "text": texts,
        "target_lang": deepl_target_lang(target_language)
    })
    .to_string();
    let response = request(route, "POST", &url, &headers, Some(&body), timeout).await?;
    if response.status != 200 {
        return Err(status_error(response.status, endpoint, &response.body));
    }
    let value: Value = serde_json::from_str(&response.body).map_err(|_| AiError::Protocol)?;
    let items = value
        .get("translations")
        .and_then(|item| item.as_array())
        .ok_or(AiError::Protocol)?;
    let mut segments = Vec::with_capacity(items.len());
    for item in items {
        let text = item
            .get("text")
            .and_then(|value| value.as_str())
            .ok_or(AiError::Protocol)?;
        segments.push(text.to_string());
    }
    if segments.len() != texts.len() {
        return Err(AiError::SegmentMismatch);
    }
    Ok(TranslationOutcome {
        segments,
        thinking_downgraded: false,
    })
}

/// 拼认证头；站点配了密钥就带 Bearer，本机 Ollama 允许不带。
fn auth_headers(endpoint: &Endpoint<'_>) -> Result<Vec<(&'static str, String)>, AiError> {
    let mut headers: Vec<(&'static str, String)> = vec![("Content-Type", "application/json".to_string())];
    if let Some(key) = endpoint.api_key {
        if !key.is_empty() {
            headers.push(("Authorization", format!("Bearer {}", key.expose())));
            return Ok(headers);
        }
    }
    if endpoint.kind != ProviderKind::Ollama {
        let host = url::Url::parse(endpoint.base_url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_string))
            .ok_or(AiError::Config("站点地址缺少主机名"))?;
        if !crate::provider::is_local_host(&host) {
            return Err(AiError::MissingKey);
        }
    }
    Ok(headers)
}

/// DeepL 的认证头格式与 OpenAI 兼容站点不一样。
fn deepl_headers(endpoint: &Endpoint<'_>) -> Result<Vec<(&'static str, String)>, AiError> {
    let key = endpoint
        .api_key
        .filter(|secret| !secret.is_empty())
        .ok_or(AiError::MissingKey)?;
    Ok(vec![
        ("Content-Type", "application/json".to_string()),
        ("Authorization", format!("DeepL-Auth-Key {}", key.expose())),
    ])
}

/// DeepL 的目标语言代码要求大写。
fn deepl_target_lang(code: &str) -> &'static str {
    match code.trim().to_ascii_lowercase().as_str() {
        "zh" | "zh-cn" | "zh-hans" | "中文" => "ZH",
        "zh-tw" | "zh-hant" => "ZH-HANT",
        "en" | "english" => "EN-US",
        "ja" | "japanese" => "JA",
        "ko" | "korean" => "KO",
        "fr" | "french" => "FR",
        "de" | "german" => "DE",
        "es" | "spanish" => "ES",
        "ru" | "russian" => "RU",
        _ => "ZH",
    }
}

/// 从对话响应里取第一段回答的正文。
fn parse_chat_text(body: &str) -> Result<String, AiError> {
    let value: Value = serde_json::from_str(body).map_err(|_| AiError::Protocol)?;
    let text = value
        .get("choices")
        .and_then(|item| item.as_array())
        .and_then(|items| items.first())
        .and_then(|item| item.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .ok_or(AiError::Protocol)?;
    Ok(text.trim().to_string())
}

/// 从模型输出里抠出一个 JSON 字符串数组；容忍外面裹了代码块围栏或说明文字。
pub fn parse_string_array(raw: &str) -> Option<Vec<String>> {
    let start = raw.find('[')?;
    let end = raw.rfind(']')?;
    if end <= start {
        return None;
    }
    let slice = &raw[start..=end];
    let value: Value = serde_json::from_str(slice).ok()?;
    let items = value.as_array()?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(item.as_str()?.to_string());
    }
    Some(out)
}

/// 把非 200 的响应转成可读错误。
///
/// 站点有时会把密钥回显在错误正文里；这里先按密钥形态清洗，再用当前密钥
/// 精确替换一次，保证完整密钥不会进界面或日志。
fn status_error(status: u16, endpoint: &Endpoint<'_>, body: &str) -> AiError {
    let detail = AiError::status_detail(status, body);
    let detail = match endpoint.api_key {
        Some(key) if !key.is_empty() => mail_net::error::redact(&detail, &[key.expose()]),
        _ => detail,
    };
    AiError::Status { status, detail }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// 假服务器：收一次请求，返回固定状态与正文。
    async fn fake_server(status: &'static str, payload: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定成功");
        let port = listener.local_addr().expect("取地址").port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("接受连接");
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await.expect("读请求");
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(response.as_bytes()).await.expect("写响应");
        });
        port
    }

    /// 假服务器：按顺序接多次请求，用来测「先失败后降级重试」。
    async fn fake_sequence(responses: Vec<(&'static str, &'static str)>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定成功");
        let port = listener.local_addr().expect("取地址").port();
        tokio::spawn(async move {
            for (status, payload) in responses {
                let (mut socket, _) = listener.accept().await.expect("接受连接");
                let mut buf = vec![0u8; 8192];
                let _ = socket.read(&mut buf).await.expect("读请求");
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                socket.write_all(response.as_bytes()).await.expect("写响应");
            }
        });
        port
    }

    fn endpoint(kind: ProviderKind, base_url: &str) -> Endpoint<'_> {
        Endpoint {
            kind,
            base_url,
            api_key: None,
        }
    }

    /// 假 HTTP 服务：收一次完整请求，把原始请求文本回传给测试；返回固定 JSON。
    ///
    /// 必须按 Content-Length 读完整包：并行跑测试时一次 read 可能只拿到半个请求体。
    async fn fake_capture_server(payload: &'static str) -> (u16, std::sync::mpsc::Receiver<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
    #[test]
    fn 决策站点选择题拒绝非法概率和把握程度() {
        let question = group_question(
            1,
            GroupQuestionKind::Choice,
            "分类",
            vec!["账单".to_string(), "其他".to_string()],
            None,
            None,
        );
        for answer in [
            json!({"choice":"账单","probabilities":{"账单":0.9,"其他":-0.1}}),
            json!({"choice":"账单","probabilities":{"账单":0.9,"其他":"无效"}}),
            json!({"choice":"账单","confidence":"无效","probabilities":{"账单":0.9}}),
            json!({"choice":"账单","confidence":0.9,"probabilities":{"账单":1.1}}),
            json!({"choice":"账单","probabilities":{"未知选项":0.9}}),
            json!({"choice":"账单","probabilities":{}}),
        ] {
            assert!(
                parse_choice_answer(&answer, None, &question).is_none(),
                "{answer}"
            );
        }
    }

    #[test]
    fn 决策站点是否题和打分题严格校验() {
        assert_eq!(
            parse_boolean_answer(&json!({"probability":false}), None),
            Some(GroupAnswer::Boolean { probability: 0.0 })
        );
        for probability in [json!(-0.1), json!(1.1), json!("true"), Value::Null] {
            assert!(parse_boolean_answer(&json!({"noul":probability}), None).is_none());
        }
        let question = group_question(
            1,
            GroupQuestionKind::Score,
            "打分",
            prompt::score_levels(-1, 1),
            Some(-1),
            Some(1),
        );
        assert_eq!(
            parse_score_answer(&json!({"score":0,"confidence":0.8}), None, &question),
            Some(GroupAnswer::Score {
                level: 0,
                confidence: 0.8
            })
        );
        for answer in [
            json!({"score":2,"confidence":0.8}),
            json!({"score":0.5,"confidence":0.8}),
            json!({"score":0,"confidence":1.1}),
            json!({"score":0}),
            json!({"probabilities":{"0":0.9,"1":-0.1},"confidence":0.8}),
            json!({"probabilities":{"3":0.9},"confidence":0.8}),
        ] {
            assert!(parse_score_answer(&answer, None, &question).is_none(), "{answer}");
        }
    }

    #[tokio::test]
    async fn 决策站点两种分类调用均保留完整路径和查询参数() {
        for grouped in [false, true] {
            let (port, captured) = fake_capture_server(
                r#"{"answers":{"category":{"choice":"账单","probabilities":{"账单":0.9,"其他":0.1}},"g1":{"choice":"账单","probabilities":{"账单":0.9,"其他":0.1}}}}"#
            ).await;
            let base = format!("http://127.0.0.1:{port}/custom/decision?version=2");
            let endpoint = endpoint(ProviderKind::DecisionApi, &base);
            let choices = vec!["账单".to_string(), "其他".to_string()];
            if grouped {
                let questions = vec![group_question(
                    1,
                    GroupQuestionKind::Choice,
                    "分类",
                    choices,
                    None,
                    None,
                )];
                let answers = classify_mail_groups(
                    None,
                    &endpoint,
                    "测试模型",
                    "虚构测试正文",
                    &questions,
                    Duration::from_secs(5),
                )
                .await
                .expect("应能分类");
                assert_eq!(
                    answers,
                    vec![(
                        1,
                        GroupAnswer::Choice {
                            label: "账单".to_string(),
                            confidence: 0.9,
                        }
                    )]
                );
            } else {
                let answer = classify_mail(
                    None,
                    &endpoint,
                    "测试模型",
                    "虚构测试正文",
                    &choices,
                    Duration::from_secs(5),
                )
                .await
                .expect("应能分类");
                assert_eq!(
                    answer,
                    ClassifyOutcome {
                        choice: "账单".to_string(),
                        confidence: 0.9
                    }
                );
            }
            let raw = captured.recv().expect("收到请求");
            assert!(
                raw.starts_with("POST /custom/decision?version=2 HTTP/1.1\r\n"),
                "{raw}"
            );
            let body = request_body(&raw);
            assert_eq!(body["model"], "测试模型");
            assert_eq!(body["state"], "虚构测试正文");
        }
    }

    #[tokio::test]
    async fn 决策站点非法分组不影响其他答案() {
        let (port, captured) = fake_capture_server(
            r#"{"answers":{
            "g1":{"type":123,"choice":"账单","confidence":0.9},
            "g2":{"type":"bool","noul":true},
            "g3":{"score":0,"confidence":0.8},
            "g4":{"choice":"账单","probabilities":{"账单":0.9,"其他":1.1}},
            "g5":{"choice":"其他","confidence":0.7}
        }}"#,
        )
        .await;
        let base = format!("http://127.0.0.1:{port}/v1/decisions");
        let endpoint = endpoint(ProviderKind::DecisionApi, &base);
        let choice = |id| {
            group_question(
                id,
                GroupQuestionKind::Choice,
                "分类",
                vec!["账单".to_string(), "其他".to_string()],
                None,
                None,
            )
        };
        let questions = vec![
            choice(1),
            group_question(2, GroupQuestionKind::Boolean, "是否", vec![], None, None),
            group_question(
                3,
                GroupQuestionKind::Score,
                "打分",
                prompt::score_levels(-1, 1),
                Some(-1),
                Some(1),
            ),
            choice(4),
            choice(5),
        ];
        let answers = classify_mail_groups(
            None,
            &endpoint,
            "测试模型",
            "虚构测试正文",
            &questions,
            Duration::from_secs(5),
        )
        .await
        .expect("整批成功");
        assert_eq!(
            answers,
            vec![
                (2, GroupAnswer::Boolean { probability: 1.0 }),
                (
                    3,
                    GroupAnswer::Score {
                        level: 0,
                        confidence: 0.8
                    }
                ),
                (
                    5,
                    GroupAnswer::Choice {
                        label: "其他".to_string(),
                        confidence: 0.7
                    }
                )
            ]
        );
        assert!(captured
            .recv()
            .expect("收到请求")
            .starts_with("POST /v1/decisions HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn 决策站点不走聊天和模型列表() {
        for kind in [ProviderKind::TypeSafe, ProviderKind::DecisionApi] {
            let endpoint = endpoint(kind, "http://127.0.0.1:1/unreachable");
            assert!(matches!(
                list_models(None, &endpoint, Duration::from_secs(1)).await,
                Err(AiError::Unsupported("拉取模型列表"))
            ));
            assert!(matches!(
                chat(
                    None,
                    &endpoint,
                    "测试模型",
                    ThinkingLevel::Off,
                    "测试指令",
                    "虚构测试正文",
                    Duration::from_secs(1)
                )
                .await,
                Err(AiError::Unsupported("摘要与润色"))
            ));
        }
    }
    #[test]
    fn 决策站点是否题兼容布尔值和类型别名() {
        let answer = serde_json::json!({"type":"bool","noul":true});
        assert_eq!(
            parse_boolean_answer(&answer, Some("bool")),
            Some(GroupAnswer::Boolean { probability: 1.0 })
        );
        assert!(parse_boolean_answer(&serde_json::json!({"type":"noul","value":2}), Some("noul")).is_none());
    }

    #[test]
    fn 决策站点选择题可从概率推导把握程度() {
        let question = GroupQuestion {
            group_id: 1,
            kind: GroupQuestionKind::Choice,
            instructions: "分类".to_string(),
            criteria: vec!["账单".to_string(), "其他".to_string()],
            score_min: None,
            score_max: None,
        };
        let answer = serde_json::json!({
            "choice":"账单",
            "probabilities":{"账单":0.9,"其他":0.1}
        });
        assert_eq!(
            parse_choice_answer(&answer, None, &question),
            Some(GroupAnswer::Choice {
                label: "账单".to_string(),
                confidence: 0.9,
            })
        );
    }
    #[test]
    fn 通用决策站点完整地址原样使用() {
        assert_eq!(
            decision_endpoint_url(
                "https://api.bamboonode.dev/v1/decisions",
                ProviderKind::DecisionApi,
            ),
            "https://api.bamboonode.dev/v1/decisions"
        );
        assert_eq!(
            decision_endpoint_url(
                " https://example.test/custom/path?version=2 ",
                ProviderKind::DecisionApi
            ),
            "https://example.test/custom/path?version=2"
        );
        assert_eq!(
            decision_endpoint_url("https://example.test/custom/", ProviderKind::DecisionApi),
            "https://example.test/custom"
        );
        assert_eq!(
            decision_endpoint_url(" https://example.test/v1/SYSTEMONE/ ", ProviderKind::TypeSafe),
            "https://example.test/v1/SYSTEMONE"
        );
        assert_eq!(
            decision_endpoint_url("https://api.typesafe.ai/v1", ProviderKind::TypeSafe),
            "https://api.typesafe.ai/v1/systemone"
        );
    }
    #[test]
    fn systemone_地址不会重复拼() {
        assert_eq!(
            decision_endpoint_url("https://pool.futureppo.top/v1", ProviderKind::TypeSafe),
            "https://pool.futureppo.top/v1/systemone"
        );
        assert_eq!(
            decision_endpoint_url("https://pool.futureppo.top/v1/systemone", ProviderKind::TypeSafe),
            "https://pool.futureppo.top/v1/systemone"
        );
        assert_eq!(
            decision_endpoint_url("https://pool.futureppo.top/v1/systemone/", ProviderKind::TypeSafe),
            "https://pool.futureppo.top/v1/systemone"
        );
    }

    #[tokio::test]
    async fn typesafe_classify_sends_systemone_request_and_parses_answer() {
        let (port, captured) =
            fake_capture_server(r#"{"answers":{"category":{"choice":"账单","confidence":0.91}}}"#).await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let endpoint = endpoint(ProviderKind::TypeSafe, &base);
        let choices = vec!["账单".to_string(), "其他".to_string()];
        let outcome = classify_mail(
            None,
            &endpoint,
            "jev-latest",
            "本月账单已出",
            &choices,
            Duration::from_secs(15),
        )
        .await
        .expect("应能分类");
        assert_eq!(outcome.choice, "账单");
        assert!((outcome.confidence - 0.91).abs() < 0.0001);

        let request = captured.recv().expect("拿到请求");
        assert!(request.contains("POST /v1/systemone"), "{request}");
        assert!(request.contains("\"state\":\"本月账单已出\""), "{request}");
        assert!(request.contains("\"model\":\"jev-latest\""), "{request}");
        assert!(request.contains("\"category\""), "{request}");
        assert!(request.contains("\"type\":\"choice\""), "{request}");
        assert!(request.contains("\"instructions\""), "{request}");
        assert!(request.contains("\"criteria\""), "{request}");
        assert!(request.contains("\"账单\":null"), "{request}");
        assert!(request.contains("\"其他\":null"), "{request}");
    }

    #[tokio::test]
    async fn typesafe_classify_rejects_out_of_range_confidence() {
        let port = fake_server(
            "200 OK",
            r#"{"answers":{"category":{"choice":"账单","confidence":1.5}}}"#,
        )
        .await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let error = classify_mail(
            None,
            &endpoint(ProviderKind::TypeSafe, &base),
            "jev-latest",
            "本月账单已出",
            &["账单".to_string(), "其他".to_string()],
            Duration::from_secs(15),
        )
        .await
        .expect_err("把握程度超范围应报错");
        assert!(matches!(error, AiError::Protocol));
    }

    #[tokio::test]
    async fn typesafe_classify_rejects_choice_outside_candidates() {
        let port = fake_server(
            "200 OK",
            r#"{"answers":{"category":{"choice":"未知类别","confidence":0.9}}}"#,
        )
        .await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let error = classify_mail(
            None,
            &endpoint(ProviderKind::TypeSafe, &base),
            "jev-latest",
            "本月账单已出",
            &["账单".to_string(), "其他".to_string()],
            Duration::from_secs(15),
        )
        .await
        .expect_err("候选外的选择应报错");
        assert!(matches!(error, AiError::Protocol));
    }

    #[tokio::test]
    async fn 拉模型列表会排序去重() {
        let port = fake_server(
            "200 OK",
            r#"{"data":[{"id":"b-model"},{"id":"a-model"},{"id":"a-model"}]}"#,
        )
        .await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let models = list_models(
            None,
            &endpoint(ProviderKind::OpenAiCompatible, &base),
            Duration::from_secs(5),
        )
        .await
        .expect("应能拉到模型");
        assert_eq!(models, vec!["a-model", "b-model"]);
    }

    #[tokio::test]
    async fn 对话能取到回答正文() {
        let port = fake_server(
            "200 OK",
            r#"{"choices":[{"message":{"role":"assistant","content":"  你好  "}}]}"#,
        )
        .await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let outcome = chat(
            None,
            &endpoint(ProviderKind::Ollama, &base),
            "qwen",
            ThinkingLevel::Off,
            "系统",
            "用户",
            Duration::from_secs(5),
        )
        .await
        .expect("应能对话");
        assert_eq!(outcome.text, "你好");
        assert!(!outcome.thinking_downgraded);
    }

    #[tokio::test]
    async fn 思考程度不被支持时自动降级重试() {
        let port = fake_sequence(vec![
            (
                "400 Bad Request",
                r#"{"error":{"message":"Unsupported parameter: reasoning_effort"}}"#,
            ),
            ("200 OK", r#"{"choices":[{"message":{"content":"降级后成功"}}]}"#),
        ])
        .await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let outcome = chat(
            None,
            &endpoint(ProviderKind::Ollama, &base),
            "qwen",
            ThinkingLevel::High,
            "系统",
            "用户",
            Duration::from_secs(5),
        )
        .await
        .expect("降级后应成功");
        assert_eq!(outcome.text, "降级后成功");
        assert!(outcome.thinking_downgraded, "应该标记成已降级");
    }

    #[tokio::test]
    async fn 站点回显密钥时会被精确替换() {
        let port = fake_server(
            "401 Unauthorized",
            r#"{"error":{"message":"Incorrect API key provided: shortkey123"}}"#,
        )
        .await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let secret = Secret::new("shortkey123");
        let endpoint = Endpoint {
            kind: ProviderKind::OpenAiCompatible,
            base_url: &base,
            api_key: Some(&secret),
        };
        let error = list_models(None, &endpoint, Duration::from_secs(5))
            .await
            .expect_err("应返回错误");
        let text = error.to_string();
        assert!(!text.contains("shortkey123"), "{text}");
    }

    #[tokio::test]
    async fn deepl拒绝摘要与润色() {
        let base = "http://127.0.0.1:1";
        let error = chat(
            None,
            &endpoint(ProviderKind::DeepL, base),
            "m",
            ThinkingLevel::Off,
            "系统",
            "用户",
            Duration::from_secs(1),
        )
        .await
        .expect_err("DeepL 不该支持摘要");
        assert!(matches!(error, AiError::Unsupported(_)));
    }

    #[tokio::test]
    async fn 没有密钥的非本机站点会被拦住() {
        let base = "https://api.example.com/v1";
        let error = list_models(
            None,
            &endpoint(ProviderKind::OpenAiCompatible, base),
            Duration::from_secs(1),
        )
        .await
        .expect_err("没密钥应被拦住");
        assert!(matches!(error, AiError::MissingKey));
    }

    #[tokio::test]
    async fn 翻译要求段数一致() {
        let port = fake_server(
            "200 OK",
            r#"{"choices":[{"message":{"content":"[\"只有一段\"]"}}]}"#,
        )
        .await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let texts = vec!["甲".to_string(), "乙".to_string()];
        let error = translate(
            None,
            &endpoint(ProviderKind::Ollama, &base),
            "qwen",
            ThinkingLevel::Off,
            &texts,
            "英语",
            Duration::from_secs(5),
        )
        .await
        .expect_err("段数对不上应报错");
        assert!(matches!(error, AiError::SegmentMismatch));
    }

    #[tokio::test]
    async fn 翻译能解析成段数组() {
        let port = fake_server(
            "200 OK",
            r#"{"choices":[{"message":{"content":"[\"one\",\"two\"]"}}]}"#,
        )
        .await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let texts = vec!["甲".to_string(), "乙".to_string()];
        let outcome = translate(
            None,
            &endpoint(ProviderKind::Ollama, &base),
            "qwen",
            ThinkingLevel::Off,
            &texts,
            "英语",
            Duration::from_secs(5),
        )
        .await
        .expect("应能翻译");
        assert_eq!(outcome.segments, vec!["one", "two"]);
    }

    #[test]
    fn 解析数组容忍代码块围栏() {
        let raw = "```json\n[\"a\",\"b\"]\n```";
        assert_eq!(
            parse_string_array(raw),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(parse_string_array("没有数组"), None);
        assert_eq!(parse_string_array("[1,2]"), None);
    }

    #[test]
    fn deepl目标语言代码是大写() {
        assert_eq!(deepl_target_lang("zh"), "ZH");
        assert_eq!(deepl_target_lang("en"), "EN-US");
        assert_eq!(deepl_target_lang("看不懂"), "ZH");
    }
    #[test]
    fn score_levels_are_generated_by_range() {
        assert_eq!(prompt::score_levels(1, 5).len(), 5);
        assert_eq!(prompt::score_levels(1, 10).len(), 10);
        assert!(prompt::score_levels(1, 11).is_empty());
        assert!(prompt::score_levels(1, 1).is_empty());
        assert!(prompt::score_levels(5, 4).is_empty());
        for count in 2..=10 {
            assert_eq!(prompt::score_levels(0, count - 1).len(), count as usize);
        }
        // 同一组的档位描述必须稳定，不能每次调用都漂移。
        assert_eq!(prompt::score_levels(1, 5), prompt::score_levels(1, 5));
        assert_eq!(
            prompt::score_levels(1, 5),
            vec![
                "完全不急".to_string(),
                "不太急".to_string(),
                "一般".to_string(),
                "比较急".to_string(),
                "非常急".to_string()
            ]
        );
    }

    fn group_question(
        group_id: i64,
        kind: GroupQuestionKind,
        instructions: &str,
        criteria: Vec<String>,
        score_min: Option<i64>,
        score_max: Option<i64>,
    ) -> GroupQuestion {
        GroupQuestion {
            group_id,
            kind,
            instructions: instructions.to_string(),
            criteria,
            score_min,
            score_max,
        }
    }

    /// 从假服务端抓到的原始请求里取出 JSON 请求正文。
    fn request_body(raw: &str) -> Value {
        let (_, body) = raw.split_once("\r\n\r\n").expect("请求应带正文");
        serde_json::from_str(body).expect("请求正文应是 JSON")
    }

    #[tokio::test]
    async fn multi_question_request_sends_all_groups_and_parses_three_types() {
        let (port, captured) = fake_capture_server(
            r#"{"answers":{
                "g1":{"type":"choice","choice":"账单","confidence":0.91},
                "g2":{"type":"noul","noul":0.82},
                "g3":{"type":"score","score":3.4,"confidence":0.7,"probabilities":{"0":0.05,"1":0.1,"2":0.6,"3":0.2,"4":0.05}}
            }}"#,
        )
        .await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let endpoint = endpoint(ProviderKind::TypeSafe, &base);
        let questions = vec![
            group_question(
                1,
                GroupQuestionKind::Choice,
                "选一个类别",
                vec!["账单".to_string(), "其他".to_string()],
                None,
                None,
            ),
            group_question(
                2,
                GroupQuestionKind::Boolean,
                "需要收件人亲自回复吗？",
                Vec::new(),
                None,
                None,
            ),
            group_question(
                3,
                GroupQuestionKind::Score,
                "紧急程度是多少？",
                prompt::score_levels(1, 5),
                Some(1),
                Some(5),
            ),
        ];
        let answers = classify_mail_groups(
            None,
            &endpoint,
            "jev-latest",
            "本月账单已出",
            &questions,
            Duration::from_secs(15),
        )
        .await
        .expect("多问题应能解析");

        assert_eq!(answers.len(), 3);
        match &answers[0] {
            (1, GroupAnswer::Choice { label, confidence }) => {
                assert_eq!(label, "账单");
                assert!((confidence - 0.91).abs() < 0.0001);
            }
            other => panic!("g1 应是选择题答案：{other:?}"),
        }
        match &answers[1] {
            (2, GroupAnswer::Boolean { probability }) => {
                assert!((probability - 0.82).abs() < 0.0001);
            }
            other => panic!("g2 应是是否题答案：{other:?}"),
        }
        match &answers[2] {
            (3, GroupAnswer::Score { level, confidence }) => {
                assert_eq!(*level, 3);
                assert!((confidence - 0.7).abs() < 0.0001);
            }
            other => panic!("g3 应是打分题答案：{other:?}"),
        }

        let request = captured.recv().expect("拿到请求");
        assert!(request.contains("POST /v1/systemone"), "{request}");
        let body = request_body(&request);
        assert_eq!(body["state"], "本月账单已出");
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["questions"]["g1"]["type"], "choice");
        assert_eq!(body["questions"]["g1"]["criteria"]["账单"], Value::Null);
        assert_eq!(body["questions"]["g2"]["type"], "noul");
        assert!(body["questions"]["g2"].get("criteria").is_none());
        assert_eq!(body["questions"]["g3"]["type"], "score");
        assert_eq!(body["questions"]["g3"]["criteria"][0], "完全不急");
        assert_eq!(body["questions"]["g3"]["criteria"][4], "非常急");
    }

    #[tokio::test]
    async fn multi_question_skips_group_with_wrong_type() {
        let port = fake_server(
            "200 OK",
            r#"{"answers":{
                "g1":{"type":"choice","choice":"账单","confidence":0.91},
                "g2":{"type":"choice","choice":"需要回复","confidence":0.9},
                "g3":{"type":"score","score":2.0,"confidence":0.8,"probabilities":{"0":0.1,"1":0.2,"2":0.6,"3":0.1}}
            }}"#,
        )
        .await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let endpoint = endpoint(ProviderKind::TypeSafe, &base);
        let questions = vec![
            group_question(
                1,
                GroupQuestionKind::Choice,
                "选一个类别",
                vec!["账单".to_string(), "其他".to_string()],
                None,
                None,
            ),
            group_question(
                2,
                GroupQuestionKind::Boolean,
                "需要亲自回复吗？",
                Vec::new(),
                None,
                None,
            ),
            group_question(
                3,
                GroupQuestionKind::Score,
                "紧急程度？",
                prompt::score_levels(1, 5),
                Some(1),
                Some(5),
            ),
        ];
        let answers = classify_mail_groups(
            None,
            &endpoint,
            "jev-latest",
            "正文",
            &questions,
            Duration::from_secs(15),
        )
        .await
        .expect("整批应成功，只是跳过坏组");
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0].0, 1);
        assert_eq!(answers[1].0, 3);
    }

    #[tokio::test]
    async fn multi_question_skips_invalid_answers() {
        let port = fake_server(
            "200 OK",
            r#"{"answers":{
                "g1":{"type":"choice","choice":"不在候选里","confidence":0.91},
                "g2":{"type":"noul","noul":1.2},
                "g3":{"type":"score","level":9,"confidence":0.8}
            }}"#,
        )
        .await;
        let base = format!("http://127.0.0.1:{port}/v1");
        let endpoint = endpoint(ProviderKind::TypeSafe, &base);
        let questions = vec![
            group_question(
                1,
                GroupQuestionKind::Choice,
                "选一个类别",
                vec!["账单".to_string(), "其他".to_string()],
                None,
                None,
            ),
            group_question(
                2,
                GroupQuestionKind::Boolean,
                "需要亲自回复吗？",
                Vec::new(),
                None,
                None,
            ),
            group_question(
                3,
                GroupQuestionKind::Score,
                "紧急程度？",
                prompt::score_levels(1, 5),
                Some(1),
                Some(5),
            ),
        ];
        let answers = classify_mail_groups(
            None,
            &endpoint,
            "jev-latest",
            "正文",
            &questions,
            Duration::from_secs(15),
        )
        .await
        .expect("整批应成功，非法组被跳过");
        assert!(answers.is_empty(), "三组都应跳过：{answers:?}");
    }
    #[tokio::test]
    async fn multi_question_rejects_bad_parameters_without_request() {
        let questions = vec![group_question(
            1,
            GroupQuestionKind::Choice,
            "选一个类别",
            vec!["账单".to_string(), "其他".to_string()],
            None,
            None,
        )];
        let bad_endpoint = endpoint(ProviderKind::OpenAiCompatible, "http://127.0.0.1:1/v1");
        let error = classify_mail_groups(
            None,
            &bad_endpoint,
            "jev-latest",
            "正文",
            &questions,
            Duration::from_secs(1),
        )
        .await
        .expect_err("非 TypeSafe 站点应被拦住");
        assert!(matches!(error, AiError::Unsupported(_)));

        let unreachable = endpoint(ProviderKind::TypeSafe, "http://127.0.0.1:1/v1");
        let error = classify_mail_groups(None, &unreachable, "", "正文", &questions, Duration::from_secs(1))
            .await
            .expect_err("空模型应被拦住");
        assert!(matches!(error, AiError::MissingModel));

        let error = classify_mail_groups(
            None,
            &unreachable,
            "jev-latest",
            "   ",
            &questions,
            Duration::from_secs(1),
        )
        .await
        .expect_err("空正文应被拦住");
        assert!(matches!(error, AiError::Protocol));

        let error = classify_mail_groups(
            None,
            &unreachable,
            "jev-latest",
            "正文",
            &[],
            Duration::from_secs(1),
        )
        .await
        .expect_err("没有任何组应被拦住");
        assert!(matches!(error, AiError::Protocol));
    }
}
