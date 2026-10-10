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
    /// 站点根地址；已经过 `normalize_base_url` 校验。
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
    if endpoint.kind.translation_only() || endpoint.kind == ProviderKind::TypeSafe {
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

/// TypeSafe 分类接口的地址。
///
/// 站点地址填到 `/v1` 就行；如果用户直接把完整地址 `.../systemone` 填了进来，
/// 也不能再拼一次，否则会变成 `/v1/systemone/systemone` 这种 404 路径。
fn systemone_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.to_ascii_lowercase().ends_with("/systemone") {
        trimmed.to_string()
    } else {
        join_url(trimmed, "systemone")
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

/// 调 TypeSafe jev 做一次邮件分类。
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
    if endpoint.kind != ProviderKind::TypeSafe {
        return Err(AiError::Unsupported("邮件分类"));
    }
    if model.trim().is_empty() {
        return Err(AiError::MissingModel);
    }
    if state.trim().is_empty() || choices.is_empty() {
        return Err(AiError::Protocol);
    }
    let url = systemone_url(endpoint.base_url);
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
    let confidence = answer
        .get("confidence")
        .and_then(Value::as_f64)
        .ok_or(AiError::Protocol)?;
    if !(0.0..=1.0).contains(&confidence) {
        return Err(AiError::Protocol);
    }
    Ok(ClassifyOutcome { choice, confidence })
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
    if endpoint.kind.translation_only() {
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
    fn systemone_地址不会重复拼() {
        assert_eq!(
            systemone_url("https://pool.futureppo.top/v1"),
            "https://pool.futureppo.top/v1/systemone"
        );
        assert_eq!(
            systemone_url("https://pool.futureppo.top/v1/systemone"),
            "https://pool.futureppo.top/v1/systemone"
        );
        assert_eq!(
            systemone_url("https://pool.futureppo.top/v1/systemone/"),
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
}
