//! 连接与命令收发：建连、可选 STARTTLS、登录、能力协商、抓取与 IDLE。

use std::time::Duration;

use mail_domain::account::Security;
use mail_domain::error::ConnectionError;
use mail_domain::proxy::ProxyRoute;
use mail_net::io::write_crlf_line;
use mail_net::{connect_tcp, tls_wrap, Stream};
use tokio::io::AsyncReadExt;

use super::parse::{
    format_uid_set, parse_fetch_line, parse_list_line, parse_search_line, parse_select_line,
    quote_imap_string, trailing_literal, SelectEvent, Value,
};
use super::{
    Address, ClientConfig, Envelope, FolderInfo, IdleOutcome, ImapClient, MailboxStatus, MessageMeta,
};

/// 单行上限。
const MAX_LINE: usize = 8 * 1024 * 1024;
/// 一条应答里累计内容上限。
const MAX_TOTAL: usize = 32 * 1024 * 1024;
/// 自报身份用的客户端名。
const CLIENT_NAME: &str = "em-master";
const TIMEOUT_TEXT: &str = "连接超时：服务器在规定时间内没有完成应答";

/// 一条服务器命令的应答。
pub(crate) struct CommandReply {
    pub status: String,
    pub detail: String,
    pub lines: Vec<Vec<u8>>,
}

impl ImapClient {
    /// 建连 → 欢迎语 → STARTTLS（可选）→ 登录 → 能力 → 必要时自报身份。
    pub async fn connect(config: &ClientConfig, route: Option<&ProxyRoute>) -> Result<Self, ConnectionError> {
        validate(config)?;
        match tokio::time::timeout(config.timeout, connect_inner(config, route)).await {
            Ok(result) => result,
            Err(_) => Err(ConnectionError::timeout(TIMEOUT_TEXT)),
        }
    }

    /// 服务器能力清单（大写）。
    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    /// 服务器是否广告了某项能力（大小写不敏感）。
    pub fn supports(&self, capability: &str) -> bool {
        self.capabilities
            .iter()
            .any(|item| item.eq_ignore_ascii_case(capability))
    }

    /// 列出全部文件夹。
    pub async fn list_folders(&mut self) -> Result<Vec<FolderInfo>, ConnectionError> {
        let reply = self.command("LIST \"\" \"*\"").await?;
        if reply.status != "OK" {
            return Err(ConnectionError::protocol(format!(
                "读取文件夹列表失败：{}",
                reply.detail
            )));
        }
        let mut folders = Vec::new();
        for line in &reply.lines {
            if let Some(parsed) = parse_list_line(line) {
                folders.push(FolderInfo {
                    full_path: parsed.full_path,
                    delimiter: parsed.delimiter,
                    attributes: parsed.attributes,
                });
            }
        }
        Ok(folders)
    }

    /// 选中一个文件夹并读取 UIDVALIDITY / UIDNEXT / 未读数。
    pub async fn select(&mut self, folder: &str) -> Result<MailboxStatus, ConnectionError> {
        let name = quote_imap_string(folder)?;
        let reply = self.command(&format!("SELECT {name}")).await?;
        if reply.status != "OK" {
            return Err(ConnectionError::rejected(format!(
                "打开文件夹失败：{}",
                reply.detail
            )));
        }
        let mut status = MailboxStatus::default();
        for line in &reply.lines {
            match parse_select_line(line) {
                Some(SelectEvent::Exists(n)) => status.exists = n,
                Some(SelectEvent::UidValidity(n)) => status.uidvalidity = Some(n),
                Some(SelectEvent::UidNext(n)) => status.uidnext = Some(n),
                Some(SelectEvent::Unseen(n)) => status.unseen = n,
                None => {}
            }
        }
        Ok(status)
    }

    /// 执行一次 UID 搜索，返回升序去重的 UID 列表。
    pub async fn uid_search(&mut self, criteria: &str) -> Result<Vec<u32>, ConnectionError> {
        let reply = self.command(&format!("UID SEARCH {criteria}")).await?;
        if reply.status != "OK" {
            return Err(ConnectionError::protocol(format!(
                "搜索邮件失败：{}",
                reply.detail
            )));
        }
        let mut uids: Vec<u32> = reply
            .lines
            .iter()
            .flat_map(|line| parse_search_line(line).unwrap_or_default())
            .collect();
        uids.sort_unstable();
        uids.dedup();
        Ok(uids)
    }

    /// 搜索某天（含）之后的邮件。
    pub async fn uid_search_since(&mut self, date: &str) -> Result<Vec<u32>, ConnectionError> {
        self.uid_search(&format!("SINCE {date}")).await
    }

    /// 搜索不小于 `from_uid` 的邮件。
    ///
    /// 注意 RFC 3501 的坑：`n:*` 在 n 超过最大 UID 时仍会返回最后一封，所以要再过滤。
    pub async fn uid_search_after(&mut self, from_uid: u32) -> Result<Vec<u32>, ConnectionError> {
        let found = self.uid_search(&format!("UID {from_uid}:*")).await?;
        Ok(found.into_iter().filter(|uid| *uid >= from_uid).collect())
    }

    /// 搜索小于 `bound` 的全部邮件（后台补齐用）。
    pub async fn uid_search_older_than(&mut self, bound: u32) -> Result<Vec<u32>, ConnectionError> {
        if bound <= 1 {
            return Ok(Vec::new());
        }
        self.uid_search(&format!("UID 1:{}", bound - 1)).await
    }

    /// 按 UID 抓取信封元数据。
    pub async fn fetch_metadata(&mut self, uids: &[u32]) -> Result<Vec<MessageMeta>, ConnectionError> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let set = format_uid_set(uids);
        let command = format!("UID FETCH {set} (UID FLAGS INTERNALDATE RFC822.SIZE ENVELOPE)");
        let reply = self.command(&command).await?;
        if reply.status != "OK" {
            return Err(ConnectionError::protocol(format!(
                "抓取邮件失败：{}",
                reply.detail
            )));
        }
        let mut messages = Vec::new();
        for line in &reply.lines {
            if let Some(parsed) = parse_fetch_line(line) {
                if let Some(uid) = parsed.uid {
                    messages.push(MessageMeta {
                        uid,
                        flags: parsed.flags,
                        internal_date: parsed.internal_date.unwrap_or_default(),
                        size: parsed.size.unwrap_or_default(),
                        envelope: parsed.envelope.as_deref().map(build_envelope).unwrap_or_default(),
                    });
                }
            }
        }
        Ok(messages)
    }

    /// 在收件箱挂一次 IDLE，最多等 `wait`。
    pub async fn idle_wait(&mut self, wait: Duration) -> Result<IdleOutcome, ConnectionError> {
        let tag = self.next_tag();
        write_crlf_line(&mut self.stream, &format!("{tag} IDLE")).await?;
        let deadline = tokio::time::Instant::now() + wait;
        let tagged = format!("{tag} ");

        // 先等 `+` 继续提示；服务器直接回 tagged 应答说明不支持 IDLE。
        loop {
            let line =
                match tokio::time::timeout_at(deadline, read_response(&mut self.stream, MAX_LINE, MAX_TOTAL))
                    .await
                {
                    Ok(result) => result?,
                    Err(_) => return Err(ConnectionError::timeout("等待 IDLE 继续提示超时")),
                };
            if line.starts_with(b"+") {
                break;
            }
            if line.starts_with(tagged.as_bytes()) {
                let detail = self.sanitize(&String::from_utf8_lossy(&line));
                return Err(ConnectionError::rejected(format!("服务器不支持 IDLE：{detail}")));
            }
            if line.starts_with(b"* BYE") {
                return Err(ConnectionError::rejected("服务器中断了连接"));
            }
        }

        let outcome = loop {
            match tokio::time::timeout_at(deadline, read_response(&mut self.stream, MAX_LINE, MAX_TOTAL))
                .await
            {
                Ok(result) => {
                    let line = result?;
                    if is_change_line(&line) {
                        break IdleOutcome::Changed;
                    }
                    if line.starts_with(b"* BYE") {
                        return Err(ConnectionError::rejected("服务器中断了连接"));
                    }
                }
                Err(_) => break IdleOutcome::Timeout,
            }
        };

        // 无论超时还是有变化，都要用 DONE 收尾，再读掉 tagged 应答。
        write_crlf_line(&mut self.stream, "DONE").await?;
        let command_timeout = self.timeout;
        match tokio::time::timeout(command_timeout, self.read_until_tag(&tag)).await {
            Ok(result) => {
                result?;
            }
            Err(_) => return Err(ConnectionError::timeout("等待 IDLE 结束应答超时")),
        }
        Ok(outcome)
    }

    /// 体面退出；失败不报错（连接随对象一起释放）。
    pub async fn logout(&mut self) {
        let tag = self.next_tag();
        let line = format!("{tag} LOGOUT");
        let command_timeout = self.timeout;
        let _ = tokio::time::timeout(command_timeout, self.exchange(&tag, &line)).await;
    }

    async fn login(&mut self, username: &str, password: &str) -> Result<(), ConnectionError> {
        let user = quote_imap_string(username)?;
        let pass = quote_imap_string(password)?;
        let reply = self.command(&format!("LOGIN {user} {pass}")).await?;
        if reply.status != "OK" {
            tracing::debug!(reply = %reply.detail, "IMAP 登录未通过");
            return Err(ConnectionError::auth(
                "登录被服务器拒绝：请检查登录名与授权码（多数邮箱需要单独申请授权码）",
            ));
        }
        Ok(())
    }

    async fn load_capabilities(&mut self) -> Result<(), ConnectionError> {
        let reply = self.command("CAPABILITY").await?;
        if reply.status != "OK" {
            return Err(ConnectionError::protocol("服务器没有返回能力清单"));
        }
        let mut capabilities: Vec<String> = Vec::new();
        for line in &reply.lines {
            let text = String::from_utf8_lossy(line).to_ascii_uppercase();
            if let Some(rest) = text.strip_prefix("* CAPABILITY") {
                capabilities.extend(rest.split_whitespace().map(str::to_string));
            }
        }
        self.capabilities = capabilities;
        Ok(())
    }

    async fn send_id_if_supported(&mut self) -> Result<(), ConnectionError> {
        if !self.supports("ID") {
            return Ok(());
        }
        let id = format!(
            "ID (\"name\" \"{CLIENT_NAME}\" \"version\" \"{}\" \"vendor\" \"{CLIENT_NAME}\")",
            env!("CARGO_PKG_VERSION")
        );
        match self.command(&id).await {
            Ok(reply) if reply.status == "OK" => Ok(()),
            Ok(reply) => {
                tracing::debug!(status = %reply.status, "IMAP ID 命令未被接受");
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    async fn command(&mut self, command: &str) -> Result<CommandReply, ConnectionError> {
        let tag = self.next_tag();
        let line = format!("{tag} {command}");
        let command_timeout = self.timeout;
        match tokio::time::timeout(command_timeout, self.exchange(&tag, &line)).await {
            Ok(result) => result,
            Err(_) => Err(ConnectionError::timeout(TIMEOUT_TEXT)),
        }
    }

    async fn exchange(&mut self, tag: &str, line: &str) -> Result<CommandReply, ConnectionError> {
        write_crlf_line(&mut self.stream, line).await?;
        self.read_until_tag(tag).await
    }

    async fn read_until_tag(&mut self, tag: &str) -> Result<CommandReply, ConnectionError> {
        let prefix = format!("{tag} ");
        let mut lines = Vec::new();
        loop {
            let line = read_response(&mut self.stream, MAX_LINE, MAX_TOTAL).await?;
            if line.starts_with(prefix.as_bytes()) {
                let rest = String::from_utf8_lossy(&line[prefix.len()..]).to_string();
                let status = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_ascii_uppercase();
                return Ok(CommandReply {
                    status,
                    detail: self.sanitize(&rest),
                    lines,
                });
            }
            if line.starts_with(b"* BYE") {
                return Err(ConnectionError::rejected("服务器中断了连接"));
            }
            lines.push(line);
        }
    }

    fn next_tag(&mut self) -> String {
        let value = self.tag;
        self.tag = if self.tag >= 999 { 1 } else { self.tag + 1 };
        format!("a{value:03}")
    }

    fn sanitize(&self, text: &str) -> String {
        let secrets: Vec<&str> = self.redactor.iter().map(String::as_str).collect();
        mail_net::error::redact(text, &secrets).trim().to_string()
    }
}

fn validate(config: &ClientConfig) -> Result<(), ConnectionError> {
    if config.host.trim().is_empty() {
        return Err(ConnectionError::protocol("收件服务器地址为空"));
    }
    if config.username.trim().is_empty() {
        return Err(ConnectionError::protocol("登录名为空"));
    }
    if config.password.is_empty() {
        return Err(ConnectionError::auth("请先填写授权码"));
    }
    Ok(())
}

async fn connect_inner(
    config: &ClientConfig,
    route: Option<&ProxyRoute>,
) -> Result<ImapClient, ConnectionError> {
    let tcp = connect_tcp(&config.host, config.port, route, config.timeout).await?;
    let mut stream = match config.security {
        Security::Tls => tls_wrap(tcp, &config.host).await?,
        Security::StartTls | Security::Plain => Stream::plain(tcp),
    };

    let greeting = mail_net::io::read_crlf_line(&mut stream, 65536).await?;
    let upper = greeting.to_ascii_uppercase();
    let mut authenticated = false;
    if upper.starts_with("* PREAUTH") {
        authenticated = true;
    } else if upper.starts_with("* OK") {
        // 正常欢迎语，继续登录流程。
    } else if upper.starts_with("* BYE") {
        return Err(ConnectionError::rejected("服务器拒绝连接，并主动断开了会话"));
    } else {
        return Err(ConnectionError::protocol(
            "服务器欢迎语无法识别：请确认收件地址与端口是否正确",
        ));
    }

    let mut tag = 1u32;
    if config.security == Security::StartTls {
        let reply = send_raw_command(&mut stream, &format!("a{tag:03}"), "STARTTLS").await?;
        if reply.status != "OK" {
            return Err(ConnectionError::tls(
                "服务器没有接受 STARTTLS 升级请求，请改用 SSL/TLS 或检查端口",
            ));
        }
        tag += 1;
        stream = stream.wrap_tls(&config.host).await?;
    }

    let mut client = ImapClient {
        stream,
        tag,
        capabilities: Vec::new(),
        redactor: vec![config.password.expose().to_string()],
        timeout: config.timeout,
    };

    if !authenticated {
        client.login(&config.username, config.password.expose()).await?;
    }
    client.load_capabilities().await?;
    client.send_id_if_supported().await?;
    Ok(client)
}

async fn send_raw_command(
    stream: &mut Stream,
    tag: &str,
    command: &str,
) -> Result<CommandReply, ConnectionError> {
    write_crlf_line(stream, &format!("{tag} {command}")).await?;
    let prefix = format!("{tag} ");
    let mut lines = Vec::new();
    loop {
        let line = read_response(stream, MAX_LINE, MAX_TOTAL).await?;
        if line.starts_with(prefix.as_bytes()) {
            let rest = String::from_utf8_lossy(&line[prefix.len()..]).to_string();
            let status = rest
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            return Ok(CommandReply {
                status,
                detail: rest.trim().to_string(),
                lines,
            });
        }
        lines.push(line);
    }
}

async fn read_line_bytes(stream: &mut Stream, max: usize) -> Result<Vec<u8>, ConnectionError> {
    let mut buffer = Vec::with_capacity(128);
    loop {
        if buffer.len() > max {
            return Err(ConnectionError::protocol("服务器返回的单行内容过长"));
        }
        let mut byte = [0u8; 1];
        let read = stream.read(&mut byte).await.map_err(|err| {
            ConnectionError::network(format!(
                "读取服务器响应失败：{}",
                mail_net::error::describe_io(&err)
            ))
        })?;
        if read == 0 {
            return Err(ConnectionError::protocol("服务器没有应答就断开了连接"));
        }
        buffer.push(byte[0]);
        if buffer.ends_with(b"\r\n") {
            buffer.truncate(buffer.len() - 2);
            return Ok(buffer);
        }
    }
}

/// 读一条完整应答：把 `{n}` 字面量折进同一逻辑行，并重新转义成引号串。
async fn read_response(
    stream: &mut Stream,
    max_line: usize,
    max_total: usize,
) -> Result<Vec<u8>, ConnectionError> {
    let mut physical = read_line_bytes(stream, max_line).await?;
    let mut logical = physical.clone();
    while let Some(length) = trailing_literal(&physical) {
        if length > max_line {
            return Err(ConnectionError::protocol("服务器返回的字面量过大"));
        }
        let mut literal = vec![0u8; length];
        stream.read_exact(&mut literal).await.map_err(|err| {
            ConnectionError::network(format!(
                "读取服务器响应失败：{}",
                mail_net::error::describe_io(&err)
            ))
        })?;
        if let Some(open) = physical.iter().rposition(|byte| *byte == b'{') {
            logical.truncate(logical.len().saturating_sub(physical.len() - open));
        }
        logical.extend_from_slice(&quote_literal(&literal));
        physical = read_line_bytes(stream, max_line).await?;
        logical.extend_from_slice(&physical);
        if logical.len() > max_total {
            return Err(ConnectionError::protocol("服务器返回的内容过大"));
        }
    }
    Ok(logical)
}

fn quote_literal(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 2);
    out.push(b'"');
    for byte in bytes {
        if *byte == b'"' || *byte == b'\\' {
            out.push(b'\\');
        }
        out.push(*byte);
    }
    out.push(b'"');
    out
}

fn is_change_line(line: &[u8]) -> bool {
    let Some(rest) = line.strip_prefix(b"* ") else {
        return false;
    };
    let upper = String::from_utf8_lossy(rest).to_ascii_uppercase();
    upper.contains(" EXISTS")
        || upper.contains(" EXPUNGE")
        || upper.contains(" FETCH")
        || upper.contains(" RECENT")
}

fn build_envelope(items: &[Value]) -> Envelope {
    let text_at = |index: usize| items.get(index).and_then(Value::as_text).unwrap_or_default();
    Envelope {
        date: text_at(0),
        subject: text_at(1),
        from: addresses_at(items, 2),
        to: addresses_at(items, 5),
        cc: addresses_at(items, 6),
        in_reply_to: text_at(8),
        message_id: text_at(9),
    }
}

fn addresses_at(items: &[Value], index: usize) -> Vec<Address> {
    items
        .get(index)
        .and_then(Value::as_list)
        .map(parse_addresses)
        .unwrap_or_default()
}

fn parse_addresses(list: &[Value]) -> Vec<Address> {
    let mut out = Vec::new();
    for entry in list {
        let Some(fields) = entry.as_list() else {
            continue;
        };
        let name = fields.first().and_then(Value::as_text).unwrap_or_default();
        let mailbox = fields.get(2).and_then(Value::as_text).unwrap_or_default();
        let host = fields.get(3).and_then(Value::as_text).unwrap_or_default();
        if mailbox.is_empty() {
            // 组起始/结束标记（mailbox 为空），忽略。
            continue;
        }
        let address = if host.is_empty() {
            mailbox
        } else {
            format!("{mailbox}@{host}")
        };
        out.push(Address { name, address });
    }
    out
}
