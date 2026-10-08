//! Gist 读写：找出 / 创建 / 读取 / 更新 / 删除设置同步用的那个私密 Gist。
//!
//! 约定（规格 3.5）：Gist 里只放一个文件，文件名固定 [`SYNC_FILENAME`]；
//! Gist 说明固定 [`GIST_DESCRIPTION`]，用来在账号里认出它；Gist 必须是私密的。
//!
//! 规矩：这里只搬字节，不解密、不认识业务表；授权令牌只进请求头，
//! 不进日志、不进错误提示。
//!
//! 找出目标时**只看私密 Gist**：同名同描述但公开的那种不是我们的，宁可当没找到。

use std::collections::HashMap;
use std::time::Duration;

use mail_domain::proxy::ProxyRoute;

use crate::error::SyncError;
use crate::http::{self, HttpRequest, HttpResponse};

/// Gist 说明，固定不变，用来在账号里认出同步用的那一个。
pub const GIST_DESCRIPTION: &str = "Y-Mail 设置同步";

/// Gist 里同步文件的固定文件名。
pub const SYNC_FILENAME: &str = "ymail-sync.json";

/// GitHub 要求自报家门；跟登录那边保持一致。
const USER_AGENT: &str = "Y-Mail";

/// 列 Gist 时一页要多少条（GitHub 上限 100），省得翻页漏掉目标。
const LIST_PER_PAGE: usize = 100;

/// Gist 接口的地址前缀；做成结构体是为了测试能指向本机假服务器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GistEndpoints {
    /// 接口根地址，生产是 `https://api.github.com`。
    pub base: String,
}

impl GistEndpoints {
    /// GitHub 的正式地址。
    pub fn github() -> Self {
        Self {
            base: "https://api.github.com".to_string(),
        }
    }
}

/// 账号里一个 Gist 的元数据（只读，不含内容）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GistSummary {
    /// Gist 编号。
    pub id: String,
    /// 说明。
    pub description: String,
    /// 最后更新时间（RFC 3339 UTC 文本）。
    pub updated_at: String,
    /// 是不是公开的；同步用的那个必须为假。
    pub public: bool,
}

/// 接口返回的 Gist 对象；只挑我们要用的字段，其余忽略。
#[derive(Debug, serde::Deserialize)]
struct GistPayload {
    id: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    public: bool,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    files: HashMap<String, GistFile>,
}

/// 接口返回的文件条目；大文件会被截断，正文可能不给。
#[derive(Debug, serde::Deserialize)]
struct GistFile {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    truncated: bool,
}

impl GistPayload {
    /// 转成对外的摘要。
    fn summary(&self) -> GistSummary {
        GistSummary {
            id: self.id.clone(),
            description: self.description.clone().unwrap_or_default(),
            updated_at: self.updated_at.clone(),
            public: self.public,
        }
    }

    /// 是不是设置同步用的那一个：说明对得上、有同步文件、而且是私密的。
    fn is_sync_target(&self) -> bool {
        !self.public
            && self.description.as_deref().map(str::trim) == Some(GIST_DESCRIPTION)
            && self.files.contains_key(SYNC_FILENAME)
    }
}

/// 找出这个账号里用于设置同步的那个 Gist。
///
/// 有多个（比如以前留下的）时取最后更新时间的最大值；时间格式统一，
/// 直接按文本比大小即可。一个都没有返回 `None`，不当失败。
pub async fn find_sync_gist(
    route: Option<&ProxyRoute>,
    endpoints: &GistEndpoints,
    token: &str,
    timeout: Duration,
) -> Result<Option<GistSummary>, SyncError> {
    let request = HttpRequest::get(format!("{}/gists?per_page={LIST_PER_PAGE}", endpoints.base))
        .with_header("Accept", "application/vnd.github+json")
        .with_header("User-Agent", USER_AGENT)
        .with_bearer(token);
    let response = send_checked(route, &request, timeout).await?;
    let list: Vec<GistPayload> = response.json()?;
    Ok(pick_sync_gist(list))
}

/// 从列表里挑出同步用的那一个；纯函数，方便单独测。
fn pick_sync_gist(list: Vec<GistPayload>) -> Option<GistSummary> {
    list.into_iter()
        .filter(GistPayload::is_sync_target)
        .map(|gist| gist.summary())
        .max_by(|left, right| left.updated_at.cmp(&right.updated_at))
}

/// 新建一个私密 Gist，写入同步文件。
///
/// 代码里没有建公开 Gist 的分支——规格要求只能是私密。
pub async fn create_sync_gist(
    route: Option<&ProxyRoute>,
    endpoints: &GistEndpoints,
    token: &str,
    content: &[u8],
    timeout: Duration,
) -> Result<GistSummary, SyncError> {
    let body = serde_json::json!({
        "description": GIST_DESCRIPTION,
        "public": false,
        "files": files_body(content)?,
    });
    let request = HttpRequest::post_json(format!("{}/gists", endpoints.base), &body)?
        .with_header("Accept", "application/vnd.github+json")
        .with_header("User-Agent", USER_AGENT)
        .with_bearer(token);
    let response = send_checked(route, &request, timeout).await?;
    let payload: GistPayload = response.json()?;
    Ok(payload.summary())
}

/// 读某个 Gist 里同步文件的正文。
///
/// Gist 被删或没有这个文件都报「没找到」；内容被截断（超过 GitHub 单文件
/// 直出上限）时明确报错，不返回半截内容。
pub async fn read_sync_file(
    route: Option<&ProxyRoute>,
    endpoints: &GistEndpoints,
    token: &str,
    gist_id: &str,
    timeout: Duration,
) -> Result<Vec<u8>, SyncError> {
    let request = HttpRequest::get(gist_url(endpoints, gist_id)?)
        .with_header("Accept", "application/vnd.github+json")
        .with_header("User-Agent", USER_AGENT)
        .with_bearer(token);
    let response = send_checked(route, &request, timeout).await?;
    let payload: GistPayload = response.json()?;
    let file = payload.files.get(SYNC_FILENAME).ok_or(SyncError::NotFound)?;
    if file.truncated {
        return Err(SyncError::Config(
            "云端数据太大，一次读不完整；请稍后再试".to_string(),
        ));
    }
    let text = file.content.as_deref().ok_or(SyncError::Malformed)?;
    Ok(text.as_bytes().to_vec())
}

/// 更新已有 Gist 里的同步文件。
pub async fn update_sync_gist(
    route: Option<&ProxyRoute>,
    endpoints: &GistEndpoints,
    token: &str,
    gist_id: &str,
    content: &[u8],
    timeout: Duration,
) -> Result<(), SyncError> {
    let body = serde_json::json!({
        "files": files_body(content)?,
    });
    let request = HttpRequest::patch_json(gist_url(endpoints, gist_id)?, &body)?
        .with_header("Accept", "application/vnd.github+json")
        .with_header("User-Agent", USER_AGENT)
        .with_bearer(token);
    send_checked(route, &request, timeout).await?;
    Ok(())
}

/// 删除一个 Gist。
///
/// 只由用户明确动作触发（关闭同步时勾选、或重设密码），代码里没有自动删的分支。
pub async fn delete_gist(
    route: Option<&ProxyRoute>,
    endpoints: &GistEndpoints,
    token: &str,
    gist_id: &str,
    timeout: Duration,
) -> Result<(), SyncError> {
    let request = HttpRequest::delete(gist_url(endpoints, gist_id)?)
        .with_header("Accept", "application/vnd.github+json")
        .with_header("User-Agent", USER_AGENT)
        .with_bearer(token);
    send_checked(route, &request, timeout).await?;
    Ok(())
}

/// Gist 的网页地址，给界面显示用。
pub fn gist_html_url(gist_id: &str) -> String {
    format!("https://gist.github.com/{gist_id}")
}

/// 拼请求体里「固定文件名 → 正文」的那一段。
///
/// 文件名是常量，不能直接写进 `json!` 的键位（那会被当成字面名字），
/// 所以显式建一行映射。
fn files_body(content: &[u8]) -> Result<serde_json::Value, SyncError> {
    let mut files = serde_json::Map::new();
    files.insert(
        SYNC_FILENAME.to_string(),
        serde_json::json!({ "content": text_content(content)? }),
    );
    Ok(serde_json::Value::Object(files))
}

/// 把正文转成文本；同步包是 JSON，本就该是 UTF-8。
fn text_content(content: &[u8]) -> Result<String, SyncError> {
    String::from_utf8(content.to_vec()).map_err(|_| SyncError::Malformed)
}

/// 拼单个 Gist 的接口地址；编号只允许十六进制，挡住往地址里塞东西。
fn gist_url(endpoints: &GistEndpoints, gist_id: &str) -> Result<String, SyncError> {
    if gist_id.is_empty() || !gist_id.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(SyncError::Config("Gist 编号不合法".to_string()));
    }
    Ok(format!("{}/gists/{gist_id}", endpoints.base))
}

/// 发请求；非 2xx 按状态码分类报错。
async fn send_checked(
    route: Option<&ProxyRoute>,
    request: &HttpRequest,
    timeout: Duration,
) -> Result<HttpResponse, SyncError> {
    let response = http::send(route, request, timeout).await?;
    if !response.is_success() {
        return Err(map_status(response.status, &response.body));
    }
    Ok(response)
}

/// 状态码到错误的映射。
///
/// GitHub 对「权限不足」和「调用超限」都用 403，所以先看响应正文里有没有
/// 限流字样；是限流就只报状态码，不是才当权限不足。
fn map_status(status: u16, body: &[u8]) -> SyncError {
    match status {
        404 => SyncError::NotFound,
        409 => SyncError::Conflict,
        403 if is_rate_limited(body) => SyncError::Http { status: 403 },
        403 => SyncError::MissingGistScope,
        other => SyncError::Http { status: other },
    }
}

/// 响应正文里是不是在说「调用超限」。
fn is_rate_limited(body: &[u8]) -> bool {
    String::from_utf8_lossy(body)
        .to_ascii_lowercase()
        .contains("rate limit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// 起一个按剧本逐个回应的假服务器；返回端口与「收到的请求」任务句柄。
    async fn spawn_script(responses: Vec<Vec<u8>>) -> (u16, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定成功");
        let port = listener.local_addr().expect("取地址").port();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut socket, _) = listener.accept().await.expect("接受连接");
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let read = socket.read(&mut chunk).await.expect("读取请求");
                    if read == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..read]);
                    if request_complete(&buf) {
                        break;
                    }
                }
                socket.write_all(&response).await.expect("写响应");
                let _ = socket.shutdown().await;
                requests.push(String::from_utf8_lossy(&buf).to_string());
            }
            requests
        });
        (port, handle)
    }

    fn request_complete(buf: &[u8]) -> bool {
        let Some(split) = buf.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let head = String::from_utf8_lossy(&buf[..split]);
        let length = head
            .lines()
            .find_map(|line| {
                let lower = line.to_ascii_lowercase();
                lower
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap_or(0))
            })
            .unwrap_or(0);
        buf.len() >= split + 4 + length
    }

    fn json_response(body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn status_response(status: u16, reason: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn no_content_response() -> Vec<u8> {
        b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec()
    }

    fn endpoints(port: u16) -> GistEndpoints {
        GistEndpoints {
            base: format!("http://127.0.0.1:{port}"),
        }
    }

    fn timeout() -> Duration {
        Duration::from_secs(5)
    }

    #[tokio::test]
    async fn 列出多个时挑最近的私密同步条目() {
        let body = r#"[
            {"id":"aaa1","description":"别的说明","public":false,"updated_at":"2026-10-08T09:00:00Z","files":{"ymail-sync.json":{"content":"{}"}}},
            {"id":"bbb2","description":"Y-Mail 设置同步","public":true,"updated_at":"2026-10-08T20:00:00Z","files":{"ymail-sync.json":{"content":"{}"}}},
            {"id":"ccc3","description":"Y-Mail 设置同步","public":false,"updated_at":"2026-10-08T10:00:00Z","files":{"ymail-sync.json":{"content":"{}"}}},
            {"id":"ddd4","description":"Y-Mail 设置同步","public":false,"updated_at":"2026-10-08T12:00:00Z","files":{"ymail-sync.json":{"content":"{}"}}}
        ]"#;
        let (port, server) = spawn_script(vec![json_response(body)]).await;
        let found = find_sync_gist(None, &endpoints(port), "gho_token", timeout())
            .await
            .expect("列出应成功")
            .expect("应找到目标");
        assert_eq!(found.id, "ddd4", "该取最近更新的那个私密 Gist");
        assert!(!found.public);

        let requests = server.await.expect("服务器任务");
        let raw = &requests[0];
        assert!(
            raw.starts_with("GET /gists?per_page=100 HTTP/1.1\r\n"),
            "实际：{raw}"
        );
        assert!(raw.contains("Authorization: Bearer gho_token\r\n"));
        assert!(raw.contains("User-Agent: Y-Mail\r\n"));
    }

    #[tokio::test]
    async fn 一个都没有时返回没找到() {
        let (port, _server) = spawn_script(vec![json_response("[]")]).await;
        let found = find_sync_gist(None, &endpoints(port), "gho_token", timeout())
            .await
            .expect("空列表不算失败");
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn 只有公开的同名条目时算没找到() {
        let body = r#"[{"id":"aaa1","description":"Y-Mail 设置同步","public":true,"updated_at":"2026-10-08T09:00:00Z","files":{"ymail-sync.json":{"content":"{}"}}}]"#;
        let (port, _server) = spawn_script(vec![json_response(body)]).await;
        let found = find_sync_gist(None, &endpoints(port), "gho_token", timeout())
            .await
            .expect("列出应成功");
        assert!(found.is_none(), "公开的那个不是我们的，不能动它");
    }

    #[tokio::test]
    async fn 创建的是私密条目且文件名固定() {
        let created = r#"{"id":"eee5","description":"Y-Mail 设置同步","public":false,"updated_at":"2026-10-08T12:00:00Z","files":{"ymail-sync.json":{"content":"{\"format\":\"ymail-settings-sync\"}"}}}"#;
        let (port, server) = spawn_script(vec![json_response(created)]).await;
        let summary = create_sync_gist(
            None,
            &endpoints(port),
            "gho_token",
            br#"{"format":"ymail-settings-sync"}"#,
            timeout(),
        )
        .await
        .expect("创建应成功");
        assert_eq!(summary.id, "eee5");
        assert!(!summary.public);

        let requests = server.await.expect("服务器任务");
        let raw = &requests[0];
        assert!(raw.starts_with("POST /gists HTTP/1.1\r\n"), "实际：{raw}");
        assert!(raw.contains("\"public\":false"), "必须私密：{raw}");
        assert!(raw.contains("\"ymail-sync.json\""), "文件名要固定：{raw}");
        assert!(raw.contains("Y-Mail 设置同步"));
    }

    #[tokio::test]
    async fn 读回同步文件的正文() {
        let body = r#"{"id":"fff6","description":"Y-Mail 设置同步","public":false,"updated_at":"2026-10-08T12:00:00Z","files":{"ymail-sync.json":{"content":"{\"revision\":3}","truncated":false}}}"#;
        let (port, server) = spawn_script(vec![json_response(body)]).await;
        let content = read_sync_file(None, &endpoints(port), "gho_token", "fff6", timeout())
            .await
            .expect("读取应成功");
        assert_eq!(String::from_utf8_lossy(&content), r#"{"revision":3}"#);

        let requests = server.await.expect("服务器任务");
        assert!(requests[0].starts_with("GET /gists/fff6 HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn 内容被截断时直接报错() {
        let body = r#"{"id":"fff6","description":"Y-Mail 设置同步","public":false,"updated_at":"2026-10-08T12:00:00Z","files":{"ymail-sync.json":{"truncated":true}}}"#;
        let (port, _server) = spawn_script(vec![json_response(body)]).await;
        let error = read_sync_file(None, &endpoints(port), "gho_token", "fff6", timeout())
            .await
            .expect_err("截断内容必须报错");
        assert!(matches!(error, SyncError::Config(_)), "实际：{error:?}");
    }

    #[tokio::test]
    async fn gist被删了报没找到() {
        let (port, _server) = spawn_script(vec![status_response(
            404,
            "Not Found",
            r#"{"message":"Not Found"}"#,
        )])
        .await;
        let error = read_sync_file(None, &endpoints(port), "gho_token", "fff6", timeout())
            .await
            .expect_err("404 应报没找到");
        assert!(matches!(error, SyncError::NotFound));
    }

    #[tokio::test]
    async fn 更新走补丁请求且带上新内容() {
        let body = r#"{"id":"fff6","description":"Y-Mail 设置同步","public":false,"updated_at":"2026-10-08T13:00:00Z","files":{"ymail-sync.json":{"content":"{\"revision\":4}"}}}"#;
        let (port, server) = spawn_script(vec![json_response(body)]).await;
        update_sync_gist(
            None,
            &endpoints(port),
            "gho_token",
            "fff6",
            br#"{"revision":4}"#,
            timeout(),
        )
        .await
        .expect("更新应成功");

        let requests = server.await.expect("服务器任务");
        let raw = &requests[0];
        assert!(raw.starts_with("PATCH /gists/fff6 HTTP/1.1\r\n"), "实际：{raw}");
        assert!(raw.contains("\"ymail-sync.json\""));
        assert!(raw.contains("revision"), "新内容该在正文里：{raw}");
    }

    #[tokio::test]
    async fn 权限没了报缺权限() {
        let (port, _server) = spawn_script(vec![status_response(
            403,
            "Forbidden",
            r#"{"message":"You do not have permission"}"#,
        )])
        .await;
        let error = update_sync_gist(None, &endpoints(port), "gho_token", "fff6", b"{}", timeout())
            .await
            .expect_err("403 应报错");
        assert!(matches!(error, SyncError::MissingGistScope), "实际：{error:?}");
    }

    #[tokio::test]
    async fn 调用超限只报状态码不误报缺权限() {
        let (port, _server) = spawn_script(vec![status_response(
            403,
            "Forbidden",
            r#"{"message":"API rate limit exceeded"}"#,
        )])
        .await;
        let error = update_sync_gist(None, &endpoints(port), "gho_token", "fff6", b"{}", timeout())
            .await
            .expect_err("403 应报错");
        assert!(
            matches!(error, SyncError::Http { status: 403 }),
            "实际：{error:?}"
        );
    }

    #[tokio::test]
    async fn 版本对不上报冲突() {
        let (port, _server) = spawn_script(vec![status_response(
            409,
            "Conflict",
            r#"{"message":"Conflict"}"#,
        )])
        .await;
        let error = update_sync_gist(None, &endpoints(port), "gho_token", "fff6", b"{}", timeout())
            .await
            .expect_err("409 应报冲突");
        assert!(matches!(error, SyncError::Conflict));
    }

    #[tokio::test]
    async fn 删除走删除请求() {
        let (port, server) = spawn_script(vec![no_content_response()]).await;
        delete_gist(None, &endpoints(port), "gho_token", "fff6", timeout())
            .await
            .expect("删除应成功");

        let requests = server.await.expect("服务器任务");
        assert!(
            requests[0].starts_with("DELETE /gists/fff6 HTTP/1.1\r\n"),
            "实际：{}",
            requests[0]
        );
    }

    #[test]
    fn 编号不合法时不拼地址() {
        let endpoints = GistEndpoints::github();
        assert!(gist_url(&endpoints, "fff6").is_ok());
        assert!(gist_url(&endpoints, "").is_err());
        assert!(gist_url(&endpoints, "ff/../x").is_err());
        assert!(gist_url(&endpoints, "ff ff").is_err());
    }

    #[test]
    fn 网页地址按编号拼() {
        assert_eq!(gist_html_url("fff6"), "https://gist.github.com/fff6");
    }

    #[test]
    fn 令牌不出现在错误里() {
        let error = SyncError::Http { status: 403 };
        let text = format!("{error} {error:?}");
        assert!(!text.contains("gho_token"));
    }
}
