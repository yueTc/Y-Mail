//! 外部大附件下载（网易 163 / 126 的超大附件）。
//!
//! 邮件正文里只有一条下载页链接，文件本体在网易服务器上，BODYSTRUCTURE 看不到它。
//! 下载流程：
//! 1. 从链接里取出 `file` 参数（只认网易域名，别的一律拒绝）；
//! 2. `GET /filehub/bg/link/info/get?key=<file>` 拿文件名与大小；
//! 3. `POST /filehub/bg/dl/prepare` 拿真实下载地址；
//! 4. 下载到本地下载目录，文件名先消毒。
//!
//! 安全口径：只有用户点了「下载」才会联网；只认网易域名；正文本身不触发任何动作。
//! 过期、被删除这类情况按网易返回的错误码翻成中文，界面直接显示。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use mail_domain::account::AccountProxyMode;
use mail_domain::proxy::ProxyRoute;
use mail_net::Stream;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use crate::engine::{EngineError, MailEngine};
use crate::reading::clean_file_name;

/// 一次请求最多等多久。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// 整文件下载最多等多久（超大附件动辄几百兆）。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(900);
/// 跟随跳转的上限。
const MAX_REDIRECTS: usize = 5;
/// 单次响应字节上限，避免被超大文件拖垮内存。
const MAX_RESPONSE_BYTES: usize = 1024 * 1024 * 1024;
/// 认得出的网易域名后缀。
const ALLOWED_HOST_SUFFIXES: [&str; 4] = ["163.com", "126.com", "yeah.net", "188.com"];

/// 下载完的落盘结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedExternalAttachment {
    /// 本地绝对路径。
    pub path: String,
    /// 落盘用的文件名。
    pub filename: String,
    /// 字节数。
    pub size: u64,
}

/// 正文里认得出的一条外部大附件链接。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExternalLink {
    /// 链接所在主机；网易的 filehub 接口也挂在这台主机上。
    host: String,
    /// 链接里的 `file` 参数，网易拿它当 linkKey。
    file_key: String,
}

/// 网易给的附件信息。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExternalFileInfo {
    filename: String,
}

/// 一次 HTTP 往返：状态码、头部与原始正文。
struct RawResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl RawResponse {
    /// 取一个头（名字大小写不敏感）。
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// 正文按文本读（JSON 接口用）。
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// 网易 link/info 接口的返回。
#[derive(Debug, Deserialize)]
struct LinkInfoResponse {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    data: Option<LinkInfoData>,
}

#[derive(Debug, Deserialize)]
struct LinkInfoData {
    #[serde(default, rename = "linkInfo")]
    link_info: Option<LinkInfoBody>,
}

#[derive(Debug, Deserialize)]
struct LinkInfoBody {
    #[serde(default, rename = "fileInfo")]
    file_info: Option<FileInfo>,
}

#[derive(Debug, Deserialize)]
struct FileInfo {
    #[serde(default)]
    filename: String,
}

/// 网易 dl/prepare 接口的返回。
#[derive(Debug, Deserialize)]
struct PrepareResponse {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default, rename = "downloadUrl")]
    download_url: Option<String>,
    #[serde(default)]
    data: Option<PrepareData>,
}

#[derive(Debug, Deserialize)]
struct PrepareData {
    #[serde(default, rename = "downloadUrl")]
    download_url: Option<String>,
}

/// 组装一条错误；文案直接给用户看，所以写人话。
fn bad(message: &str) -> EngineError {
    EngineError::BadRequest(message.to_string())
}

/// 把网易的错误码翻成中文。
fn netease_error(code: i64, desc: Option<&str>) -> EngineError {
    let detail = desc.unwrap_or("").trim();
    if code == 601 || detail.eq_ignore_ascii_case("EXPIRED") {
        return bad("这个超大附件已经过期，取不回来了");
    }
    if detail.eq_ignore_ascii_case("PARAM_ILLEGAL") {
        return bad("这个超大附件链接认不出来，可能已经失效");
    }
    if code == 0 && detail.is_empty() {
        return bad("网易没给出下载地址，这一次没下成");
    }
    match detail {
        "" => bad(&format!("网易拒绝了这次下载（错误码 {code}）")),
        text => bad(&format!("网易拒绝了这次下载（{code} {text}）")),
    }
}

/// 主机名是不是网易那几家。
fn is_allowed_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    ALLOWED_HOST_SUFFIXES
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

/// 从正文链接里解析出 file 参数；不是网易域名直接拒绝。
fn parse_external_link(href: &str) -> Result<ExternalLink, EngineError> {
    let url = Url::parse(href.trim()).map_err(|_| bad("这个超大附件链接没法解析"))?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err(bad("这个超大附件链接的协议不受支持"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| bad("这个超大附件链接缺少主机名"))?
        .to_ascii_lowercase();
    if !is_allowed_host(&host) {
        return Err(bad("这个外部附件不在支持范围内，只有网易的超大附件能直接下"));
    }
    let file_key = url
        .query_pairs()
        .find(|(key, _)| key == "file")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| bad("这个超大附件链接里没有附件标识"))?;
    Ok(ExternalLink { host, file_key })
}

/// 发一次请求；`json` 有值就按 JSON 提交。
async fn request(
    route: Option<&ProxyRoute>,
    method: &str,
    url: &Url,
    json: Option<&str>,
    timeout: Duration,
) -> Result<RawResponse, EngineError> {
    let host = url
        .host_str()
        .ok_or_else(|| bad("外部附件地址缺少主机名"))?
        .to_string();
    let secure = match url.scheme() {
        "https" => true,
        "http" => false,
        _ => return Err(bad("外部附件地址的协议不受支持")),
    };
    let port = url
        .port_or_known_default()
        .ok_or_else(|| bad("外部附件地址缺少端口"))?;
    let mut target = if url.path().is_empty() {
        "/".to_string()
    } else {
        url.path().to_string()
    };
    if let Some(query) = url.query() {
        target.push('?');
        target.push_str(query);
    }

    let mut head =
        format!("{method} {target} HTTP/1.1\r\nHost: {host}\r\nAccept: */*\r\nAccept-Encoding: identity\r\n");
    let payload = json.unwrap_or("");
    if !payload.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
        head.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    }
    head.push_str("Connection: close\r\n\r\n");

    let exchange = async {
        let tcp = mail_net::connect_tcp(&host, port, route, timeout)
            .await
            .map_err(|error| bad(&format!("连不上网易服务器：{error}")))?;
        let mut stream = if secure {
            mail_net::tls_wrap(tcp, &host)
                .await
                .map_err(|error| bad(&format!("和网易的加密连接没建起来：{error}")))?
        } else {
            Stream::plain(tcp)
        };
        stream
            .write_all(head.as_bytes())
            .await
            .map_err(|error| bad(&format!("请求没发出去：{error}")))?;
        if !payload.is_empty() {
            stream
                .write_all(payload.as_bytes())
                .await
                .map_err(|error| bad(&format!("请求没发完：{error}")))?;
        }
        stream
            .flush()
            .await
            .map_err(|error| bad(&format!("请求没发完：{error}")))?;

        let mut raw = Vec::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = stream
                .read(&mut buffer)
                .await
                .map_err(|error| bad(&format!("读网易的回应失败：{error}")))?;
            if read == 0 {
                break;
            }
            raw.extend_from_slice(&buffer[..read]);
            if raw.len() > MAX_RESPONSE_BYTES {
                return Err(bad("这个附件太大，一次下载会吃掉太多内存"));
            }
        }
        Ok::<Vec<u8>, EngineError>(raw)
    };

    let raw = tokio::time::timeout(timeout, exchange)
        .await
        .map_err(|_| bad("连网易超时了，稍后再试"))??;
    parse_response(&raw)
}

/// 拆状态行、头部与正文；正文支持定长和分块两种传输方式。
fn parse_response(raw: &[u8]) -> Result<RawResponse, EngineError> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| bad("网易的回应不完整"))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or("");
    if !status_line.starts_with("HTTP/") {
        return Err(bad("网易的回应不完整"));
    }
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| bad("网易的回应不完整"))?;
    let mut headers = Vec::new();
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if name == "transfer-encoding" && value.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        }
        headers.push((name, value));
    }
    let body = &raw[split + 4..];
    let body = if chunked { dechunk(body)? } else { body.to_vec() };
    Ok(RawResponse {
        status,
        headers,
        body,
    })
}

/// 解分块传输编码。
fn dechunk(mut body: &[u8]) -> Result<Vec<u8>, EngineError> {
    let mut out = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| bad("网易的回应不完整"))?;
        let size_text = String::from_utf8_lossy(&body[..line_end]);
        let size = usize::from_str_radix(size_text.trim().split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| bad("网易的回应不完整"))?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if body.len() < size + 2 {
            return Err(bad("网易的回应不完整"));
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

/// 发 GET 并跟随跳转，返回最后一次响应。
async fn get_following_redirects(
    route: Option<&ProxyRoute>,
    url: &str,
    timeout: Duration,
) -> Result<RawResponse, EngineError> {
    let mut current = Url::parse(url).map_err(|_| bad("网易给的地址没法解析"))?;
    for _ in 0..=MAX_REDIRECTS {
        let response = request(route, "GET", &current, None, timeout).await?;
        if (300..400).contains(&response.status) {
            let location = response
                .header("location")
                .ok_or_else(|| bad("网易的跳转缺了目标地址"))?
                .to_string();
            current = current.join(&location).map_err(|_| bad("网易给的跳转地址不对"))?;
            if current.scheme() != "https" && current.scheme() != "http" {
                return Err(bad("网易的跳转去了不支持的协议"));
            }
            continue;
        }
        return Ok(response);
    }
    Err(bad("网易的跳转次数太多，先不下了"))
}

/// 取附件信息（文件名、大小）。
async fn fetch_link_info(
    route: Option<&ProxyRoute>,
    link: &ExternalLink,
) -> Result<ExternalFileInfo, EngineError> {
    let base = format!("https://{}/filehub/bg/link/info/get", link.host);
    let url = Url::parse_with_params(&base, &[("key", link.file_key.as_str())])
        .map_err(|_| bad("外部附件地址拼不出来"))?;
    let response = get_following_redirects(route, url.as_str(), REQUEST_TIMEOUT).await?;
    if response.status != 200 {
        return Err(bad(&format!(
            "网易返回了 {}，这一次没拿到附件信息",
            response.status
        )));
    }
    let parsed: LinkInfoResponse =
        serde_json::from_str(&response.text()).map_err(|_| bad("网易返回的附件信息看不懂"))?;
    if parsed.code != 200 {
        return Err(netease_error(parsed.code, parsed.desc.as_deref()));
    }
    let filename = parsed
        .data
        .and_then(|data| data.link_info)
        .and_then(|info| info.file_info)
        .map(|info| info.filename)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| bad("网易没给出附件文件名"))?;
    Ok(ExternalFileInfo { filename })
}

/// 换真实下载地址。
async fn prepare_download(route: Option<&ProxyRoute>, link: &ExternalLink) -> Result<String, EngineError> {
    let url = Url::parse(&format!("https://{}/filehub/bg/dl/prepare", link.host))
        .map_err(|_| bad("外部附件地址拼不出来"))?;
    let payload = serde_json::json!({ "linkKey": link.file_key }).to_string();
    let response = request(route, "POST", &url, Some(&payload), REQUEST_TIMEOUT).await?;
    if response.status != 200 {
        return Err(bad(&format!(
            "网易返回了 {}，这一次没拿到下载地址",
            response.status
        )));
    }
    let parsed: PrepareResponse =
        serde_json::from_str(&response.text()).map_err(|_| bad("网易返回的下载信息看不懂"))?;
    let download_url = parsed
        .data
        .and_then(|data| data.download_url)
        .or(parsed.download_url)
        .filter(|value| !value.trim().is_empty());
    match download_url {
        Some(value) => Ok(value),
        None => Err(netease_error(parsed.code, parsed.desc.as_deref())),
    }
}

/// 重名时给文件名加序号，避免互相覆盖。
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("附件");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 1..1000 {
        let next = match extension {
            Some(ext) if !ext.is_empty() => format!("{stem}-{index}.{ext}"),
            _ => format!("{stem}-{index}"),
        };
        let candidate = dir.join(&next);
        if !candidate.exists() {
            return candidate;
        }
    }
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    dir.join(format!("{stem}-{stamp}"))
}

impl MailEngine {
    /// 下载一封邮件里的外部大附件（网易超大附件）。
    ///
    /// `href` 直接来自正文，所以这里再校验一次域名：只认网易那几家，别的直接拒。
    pub async fn download_external_attachment(
        &self,
        href: &str,
    ) -> Result<SavedExternalAttachment, EngineError> {
        let link = parse_external_link(href)?;
        let route = self.resolve_route(AccountProxyMode::InheritGlobal)?;
        let info = fetch_link_info(route.as_ref(), &link).await?;
        let download_url = prepare_download(route.as_ref(), &link).await?;
        let response = get_following_redirects(route.as_ref(), &download_url, DOWNLOAD_TIMEOUT).await?;
        if response.status != 200 {
            return Err(bad(&format!("下载地址返回了 {}，这一次没下成", response.status)));
        }

        let dir = PathBuf::from(self.init_summary().attachment_dir);
        std::fs::create_dir_all(&dir)?;
        let filename = clean_file_name(&info.filename).unwrap_or_else(|| "外部附件".to_string());
        let path = unique_path(&dir, &filename);
        std::fs::write(&path, &response.body)?;
        Ok(SavedExternalAttachment {
            path: path.to_string_lossy().to_string(),
            filename,
            size: response.body.len() as u64,
        })
    }

    /// 把界面给的路径收进下载目录：只接受下载目录里真实存在的文件。
    ///
    /// 界面不能指定任意路径去打开，只能打开我们自己下下来的东西。
    pub fn resolve_download_path(&self, raw: &str) -> Result<PathBuf, EngineError> {
        if raw.trim().is_empty() {
            return Err(bad("没有给文件路径"));
        }
        let dir = PathBuf::from(self.init_summary().attachment_dir);
        let dir = dir
            .canonicalize()
            .map_err(|_| bad("下载目录还不存在，先下载一个附件"))?;
        let candidate = PathBuf::from(raw.trim());
        let candidate = candidate
            .canonicalize()
            .map_err(|_| bad("这个文件已经不在了，可能被移走或删掉了"))?;
        if candidate.parent() != Some(dir.as_path()) {
            return Err(bad("只能打开下载目录里的文件"));
        }
        if !candidate.is_file() {
            return Err(bad("这个路径不是文件"));
        }
        Ok(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 只认网易域名和带_file_参数的链接() {
        let ok =
            parse_external_link("https://mail.163.com/large-attachment-download/index.html?file=abc&title=x")
                .expect("网易链接应通过");
        assert_eq!(ok.host, "mail.163.com");
        assert_eq!(ok.file_key, "abc");

        assert!(parse_external_link("https://mail.126.com/x?file=abc").is_ok());
        assert!(parse_external_link("http://u.163.com/x?file=abc").is_ok());
    }

    #[test]
    fn 别的域名和别的协议一律拒绝() {
        assert!(parse_external_link("https://evil.example.com/x?file=abc").is_err());
        assert!(parse_external_link("https://mail.163.com.evil.com/x?file=abc").is_err());
        assert!(parse_external_link("file:///etc/passwd?file=abc").is_err());
        assert!(parse_external_link("javascript:alert(1)").is_err());
        assert!(parse_external_link("https://mail.163.com/x").is_err());
        assert!(parse_external_link("https://mail.163.com/x?file=").is_err());
    }

    #[test]
    fn 解析定长与分块响应() {
        let fixed = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").expect("可解析");
        assert_eq!(fixed.status, 200);
        assert_eq!(fixed.text(), "{}");

        let chunked = parse_response(
            b"HTTP/1.1 302 Found\r\nLocation: /next\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n",
        )
        .expect("可解析");
        assert_eq!(chunked.status, 302);
        assert_eq!(chunked.header("location"), Some("/next"));
        assert_eq!(chunked.text(), "{}");
    }

    #[test]
    fn 网易错误码翻成中文() {
        let expired = netease_error(601, Some("EXPIRED")).to_string();
        assert!(expired.contains("已经过期"), "{expired}");
        let illegal = netease_error(571, Some("PARAM_ILLEGAL")).to_string();
        assert!(illegal.contains("认不出来"), "{illegal}");
        let other = netease_error(500, Some("BOOM")).to_string();
        assert!(other.contains("500"), "{other}");
    }

    #[tokio::test]
    async fn 跟跳转并把二进制原样拿回来() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定端口");
        let addr = listener.local_addr().expect("取地址");
        tokio::spawn(async move {
            for step in 0..2 {
                let (mut socket, _) = listener.accept().await.expect("接受连接");
                let mut buffer = vec![0u8; 2048];
                let _ = socket.read(&mut buffer).await.expect("读请求");
                let response: &[u8] = if step == 0 {
                    b"HTTP/1.1 302 Found\r\nLocation: /file\r\nContent-Length: 0\r\n\r\n"
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n\x00\x01\xff\r\n0\r\n\r\n"
                };
                socket.write_all(response).await.expect("写响应");
            }
        });

        let start = format!("http://127.0.0.1:{}/start", addr.port());
        let response = get_following_redirects(None, &start, Duration::from_secs(5))
            .await
            .expect("下载成功");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, vec![0x00, 0x01, 0xff]);
    }

    #[tokio::test]
    async fn 只允许打开下载目录里的文件() {
        use std::sync::Arc;

        use crate::secrets::MemorySecretStore;

        let root = tempfile::tempdir().expect("临时目录");
        let engine = MailEngine::initialize_with_secrets(root.path(), Arc::new(MemorySecretStore::new()))
            .expect("初始化引擎");
        let download_dir = PathBuf::from(engine.init_summary().attachment_dir);
        std::fs::create_dir_all(&download_dir).expect("建下载目录");
        let inside = download_dir.join("虚拟.wav");
        std::fs::write(&inside, b"x").expect("写文件");

        let resolved = engine
            .resolve_download_path(&inside.to_string_lossy())
            .expect("下载目录里的文件应通过");
        assert_eq!(
            resolved.file_name().and_then(|value| value.to_str()),
            Some("虚拟.wav")
        );

        // 下载目录外面的文件不给开，界面指定不了任意路径。
        let outside = root.path().join("外面.txt");
        std::fs::write(&outside, b"x").expect("写文件");
        assert!(engine.resolve_download_path(&outside.to_string_lossy()).is_err());
        // 文件不存在、路径为空也给可读错误。
        assert!(engine
            .resolve_download_path(&download_dir.join("没有这个.wav").to_string_lossy())
            .is_err());
        assert!(engine.resolve_download_path("   ").is_err());
    }

    #[test]
    fn 重名文件会加序号() {
        let dir = tempfile::tempdir().expect("临时目录");
        let first = unique_path(dir.path(), "虚拟.wav");
        assert_eq!(first.file_name().and_then(|v| v.to_str()), Some("虚拟.wav"));
        std::fs::write(&first, b"x").expect("写占位文件");
        let second = unique_path(dir.path(), "虚拟.wav");
        assert_eq!(second.file_name().and_then(|v| v.to_str()), Some("虚拟-1.wav"));
    }
}
