//! IMAP 应答解析：把服务器返回的字节行拆成可用结构。
//!
//! 纯计算，不碰网络。读取阶段已把 `{n}` 形式的字面量折进同一行，并重新转义成
//! 引号串，所以这里只需处理 原子 / 引号串 / NIL / 列表 四种取值。

use mail_domain::error::ConnectionError;

/// 一段解析出来的取值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    /// 未加引号的记号（数字、关键字、名字）。
    Atom(Vec<u8>),
    /// 引号串或字面量。
    Str(Vec<u8>),
    /// 括号列表。
    List(Vec<Value>),
    /// NIL：服务器表示「没有」。
    Nil,
}

impl Value {
    /// 取文本；NIL 与列表返回 None。
    pub(crate) fn as_text(&self) -> Option<String> {
        match self {
            Self::Atom(bytes) | Self::Str(bytes) => Some(String::from_utf8_lossy(bytes).to_string()),
            _ => None,
        }
    }

    /// 取无符号整数。
    pub(crate) fn as_number(&self) -> Option<u64> {
        self.as_text()?.trim().parse().ok()
    }

    /// 取列表。
    pub(crate) fn as_list(&self) -> Option<&[Value]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }
}

/// 字节级下降解析器。
struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }

    /// 读一段记号：到空白、括号、引号为止。
    fn take_atom(&mut self) -> Option<&'a [u8]> {
        let start = self.pos;
        while let Some(byte) = self.peek() {
            if matches!(
                byte,
                b' ' | b'\t' | b'\r' | b'\n' | b'(' | b')' | b'[' | b']' | b'"'
            ) {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            None
        } else {
            Some(&self.bytes[start..self.pos])
        }
    }

    fn parse_value(&mut self) -> Option<Value> {
        self.skip_ws();
        match self.peek()? {
            b'(' => self.parse_list(),
            b'"' => self.parse_quoted(),
            b')' | b']' => None,
            _ => {
                let atom = self.take_atom()?;
                if atom.eq_ignore_ascii_case(b"NIL") {
                    Some(Value::Nil)
                } else {
                    Some(Value::Atom(atom.to_vec()))
                }
            }
        }
    }

    fn parse_list(&mut self) -> Option<Value> {
        self.eat(b'(');
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.eat(b')') {
                return Some(Value::List(items));
            }
            items.push(self.parse_value()?);
        }
    }

    fn parse_quoted(&mut self) -> Option<Value> {
        self.eat(b'"');
        let mut out = Vec::new();
        while let Some(byte) = self.peek() {
            self.pos += 1;
            match byte {
                b'"' => return Some(Value::Str(out)),
                b'\\' => {
                    let escaped = self.peek()?;
                    self.pos += 1;
                    out.push(escaped);
                }
                _ => out.push(byte),
            }
        }
        None
    }
}

/// `* LIST (\HasNoChildren) "/" "INBOX"` 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedList {
    pub attributes: Vec<String>,
    pub delimiter: String,
    pub full_path: String,
}

/// 解析一行 LIST 应答；不是 LIST 行返回 None。
pub(crate) fn parse_list_line(line: &[u8]) -> Option<ParsedList> {
    let mut p = Parser::new(line);
    if !p.eat(b'*') {
        return None;
    }
    p.skip_ws();
    if !p.take_atom()?.eq_ignore_ascii_case(b"LIST") {
        return None;
    }
    let attributes = p
        .parse_value()?
        .as_list()?
        .iter()
        .filter_map(Value::as_text)
        .collect();
    let delimiter = p.parse_value()?.as_text().unwrap_or_default();
    let full_path = p.parse_value()?.as_text()?;
    Some(ParsedList {
        attributes,
        delimiter,
        full_path,
    })
}

/// SELECT 过程中出现的状态事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectEvent {
    Exists(u32),
    UidValidity(u32),
    UidNext(u32),
    Unseen(u32),
}

/// 解析一行 SELECT 应答；无关行返回 None。
pub(crate) fn parse_select_line(line: &[u8]) -> Option<SelectEvent> {
    let mut p = Parser::new(line);
    if !p.eat(b'*') {
        return None;
    }
    p.skip_ws();
    let first = p.take_atom()?;
    if let Some(number) = std::str::from_utf8(first)
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
    {
        p.skip_ws();
        let keyword = p.take_atom()?;
        return keyword
            .eq_ignore_ascii_case(b"EXISTS")
            .then_some(SelectEvent::Exists(number));
    }
    if !first.eq_ignore_ascii_case(b"OK") {
        return None;
    }
    p.skip_ws();
    if !p.eat(b'[') {
        return None;
    }
    let key = p.take_atom()?.to_ascii_uppercase();
    p.skip_ws();
    let value: u32 = std::str::from_utf8(p.take_atom()?).ok()?.parse().ok()?;
    if key == b"UIDVALIDITY" {
        Some(SelectEvent::UidValidity(value))
    } else if key == b"UIDNEXT" {
        Some(SelectEvent::UidNext(value))
    } else if key == b"UNSEEN" {
        Some(SelectEvent::Unseen(value))
    } else {
        None
    }
}

/// 解析一行 SEARCH 应答，返回 UID 列表（保持服务器顺序）。
pub(crate) fn parse_search_line(line: &[u8]) -> Option<Vec<u32>> {
    let mut p = Parser::new(line);
    if !p.eat(b'*') {
        return None;
    }
    p.skip_ws();
    if !p.take_atom()?.eq_ignore_ascii_case(b"SEARCH") {
        return None;
    }
    let mut uids = Vec::new();
    loop {
        p.skip_ws();
        match p.take_atom() {
            Some(atom) => {
                if let Some(uid) = std::str::from_utf8(atom).ok().and_then(|s| s.parse::<u32>().ok()) {
                    uids.push(uid);
                }
            }
            None => break,
        }
    }
    Some(uids)
}

/// 一行 FETCH 应答里我们关心的原始字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedFetch {
    pub uid: Option<u32>,
    pub flags: Vec<String>,
    pub internal_date: Option<String>,
    pub size: Option<u32>,
    pub envelope: Option<Vec<Value>>,
}

/// 解析一行 FETCH 应答；不是 FETCH 行返回 None。
pub(crate) fn parse_fetch_line(line: &[u8]) -> Option<ParsedFetch> {
    let mut p = Parser::new(line);
    if !p.eat(b'*') {
        return None;
    }
    p.skip_ws();
    let _sequence = p.take_atom()?;
    p.skip_ws();
    if !p.take_atom()?.eq_ignore_ascii_case(b"FETCH") {
        return None;
    }
    p.skip_ws();
    let outer = p.parse_value()?;
    let items = outer.as_list()?;

    let mut parsed = ParsedFetch {
        uid: None,
        flags: Vec::new(),
        internal_date: None,
        size: None,
        envelope: None,
    };
    for pair in items.chunks(2) {
        if pair.len() < 2 {
            break;
        }
        let Some(key) = pair[0].as_text() else { continue };
        let value = &pair[1];
        if key.eq_ignore_ascii_case("UID") {
            parsed.uid = value.as_number().and_then(|n| u32::try_from(n).ok());
        } else if key.eq_ignore_ascii_case("FLAGS") {
            parsed.flags = value
                .as_list()
                .map(|list| list.iter().filter_map(Value::as_text).collect())
                .unwrap_or_default();
        } else if key.eq_ignore_ascii_case("INTERNALDATE") {
            parsed.internal_date = value.as_text();
        } else if key.eq_ignore_ascii_case("RFC822.SIZE") {
            parsed.size = value.as_number().and_then(|n| u32::try_from(n).ok());
        } else if key.eq_ignore_ascii_case("ENVELOPE") {
            parsed.envelope = value.as_list().map(|list| list.to_vec());
        }
    }
    Some(parsed)
}

/// 若一行以 `{n}` 结尾，返回 n（字面量字节长度）。
pub(crate) fn trailing_literal(line: &[u8]) -> Option<usize> {
    if !line.ends_with(b"}") {
        return None;
    }
    let open = line.iter().rposition(|byte| *byte == b'{')?;
    let inner = &line[open + 1..line.len() - 1];
    let digits = inner.strip_suffix(b"+").unwrap_or(inner);
    if digits.is_empty() {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// 把 UID 列表压成 IMAP 序列集（连续区间合并）。
pub(crate) fn format_uid_set(uids: &[u32]) -> String {
    let mut sorted: Vec<u32> = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut parts = Vec::new();
    let mut index = 0;
    while index < sorted.len() {
        let start = sorted[index];
        let mut end = start;
        while index + 1 < sorted.len() && sorted[index + 1] == end.saturating_add(1) {
            index += 1;
            end = sorted[index];
        }
        if start == end {
            parts.push(start.to_string());
        } else {
            parts.push(format!("{start}:{end}"));
        }
        index += 1;
    }
    parts.join(",")
}

/// 把字符串包成 IMAP 引号形式；含换行则拒绝。
pub(crate) fn quote_imap_string(value: &str) -> Result<String, ConnectionError> {
    if value.contains(['\r', '\n']) {
        return Err(ConnectionError::protocol("内容包含不支持的换行符"));
    }
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!("\"{escaped}\""))
}
