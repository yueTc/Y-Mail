//! RFC 2047 编码字（encoded-word）解码。
//!
//! 真实邮件里，主题和显示名经常是 `=?UTF-8?B?...?=`、`=?GBK?Q?...?=` 这样的
//! 编码字。这里把它们解成正常文字；一段里混着多个编码字和普通文字也能处理。
//!
//! 安全与鲁棒性：
//! - 只做纯计算，不联网、不写库、不执行任何动作；
//! - 解不出来（坏数据、未知字符集、缺结束符）就把原文原样保留；
//! - 绝不 panic：所有下标访问都走 `get`／`find` 的返回值。

use base64::Engine as _;
use mail_parser::decoders::charsets::map::charset_decoder;

/// 编码字里字符集名允许出现的字符（字母数字与 `- _ . *`）。
fn is_charset_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'*')
}

/// 一个解析出来的编码字。
struct EncodedWord<'a> {
    charset: &'a str,
    encoding: u8,
    data: &'a str,
    /// 占用的字节数（含结尾的 `?=`）。
    end: usize,
}

/// 把一段文本里的 RFC 2047 编码字解成正常文字。
///
/// 普通文字原样保留；相邻两个编码字之间的空白按标准丢弃；解不出来的编码字
/// 原样保留。对已经是正常文字的输入再调用一次，结果不变（幂等）。
pub fn decode_encoded_words(raw: &str) -> String {
    // 快速路径：没有编码字起始标记时原样返回，既幂等又省开销。
    if !raw.contains("=?") {
        return raw.to_string();
    }

    let mut out = String::with_capacity(raw.len());
    let mut cursor = 0usize;
    let mut last_was_encoded = false;

    while let Some(relative) = raw[cursor..].find("=?") {
        let start = cursor + relative;
        match parse_encoded_word(&raw[start..]) {
            Some(word) => {
                let between = &raw[cursor..start];
                // 标准要求：两个相邻编码字之间的空白不保留。
                if !(last_was_encoded && between.chars().all(char::is_whitespace)) {
                    out.push_str(between);
                }
                match decode_word(&word) {
                    Some(text) => {
                        out.push_str(&text);
                        last_was_encoded = true;
                    }
                    None => {
                        out.push_str(&raw[start..start + word.end]);
                        last_was_encoded = false;
                    }
                }
                cursor = start + word.end;
            }
            None => {
                // 不是完整编码字：把 `=?` 当普通文字，继续往后找。
                out.push_str(&raw[cursor..start + 2]);
                cursor = start + 2;
                last_was_encoded = false;
            }
        }
    }
    out.push_str(&raw[cursor..]);
    out
}

/// 从 `=?` 开头解析一个完整编码字；不完整或语法不对时返回 None。
fn parse_encoded_word(input: &str) -> Option<EncodedWord<'_>> {
    let bytes = input.as_bytes();
    if bytes.len() < 6 || bytes[0] != b'=' || bytes[1] != b'?' {
        return None;
    }
    let charset_end = input.get(2..)?.find('?')? + 2;
    let charset = input.get(2..charset_end)?;
    if charset.is_empty() || charset.len() > 45 || !charset.bytes().all(is_charset_byte) {
        return None;
    }
    let encoding = *bytes.get(charset_end + 1)?;
    if !matches!(encoding, b'b' | b'B' | b'q' | b'Q') {
        return None;
    }
    if *bytes.get(charset_end + 2)? != b'?' {
        return None;
    }
    let data_start = charset_end + 3;
    let data_end = input.get(data_start..)?.find("?=")? + data_start;
    if data_end == data_start {
        return None;
    }
    let data = input.get(data_start..data_end)?;
    Some(EncodedWord {
        charset,
        encoding,
        data,
        end: data_end + 2,
    })
}

/// 解一个编码字的内容；字符集不认识或数据坏了都返回 None。
fn decode_word(word: &EncodedWord<'_>) -> Option<String> {
    let bytes = match word.encoding {
        b'b' | b'B' => decode_base64(word.data)?,
        b'q' | b'Q' => decode_quoted_printable(word.data)?,
        _ => return None,
    };
    decode_charset(word.charset, &bytes)
}

/// Base64 解码：容忍多余空白，缺填充时自动补齐。
fn decode_base64(data: &str) -> Option<Vec<u8>> {
    let compact: String = data.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact.is_empty() {
        return None;
    }
    let mut padded = compact.clone();
    while !padded.len().is_multiple_of(4) {
        padded.push('=');
    }
    base64::engine::general_purpose::STANDARD
        .decode(padded.as_bytes())
        .ok()
        .or_else(|| {
            base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(compact.as_bytes())
                .ok()
        })
}

/// Q 解码：`_` 当空格，`=XX` 当字节；出现坏的转义就整体失败。
fn decode_quoted_printable(data: &str) -> Option<Vec<u8>> {
    let bytes = data.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'_' => {
                out.push(b' ');
                index += 1;
            }
            b'=' => {
                let high = hex_value(*bytes.get(index + 1)?)?;
                let low = hex_value(*bytes.get(index + 2)?)?;
                out.push((high << 4) | low);
                index += 3;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    Some(out)
}

/// 十六进制半字节。
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 按字符集把字节转成文字。
///
/// 认得的字符集交给 mail-parser 的表；不认得的只在字节本身就是合法 UTF-8
/// 时才采用，避免瞎猜出乱码。
fn decode_charset(charset: &str, bytes: &[u8]) -> Option<String> {
    // 字符集名可能带语言后缀（如 `utf-8*en`），取 `*` 前面一段即可。
    let name = charset.split('*').next().unwrap_or(charset);
    if name.is_empty() {
        return None;
    }
    if let Some(decoder) = charset_decoder(name.as_bytes()) {
        return Some(decoder(bytes));
    }
    std::str::from_utf8(bytes).ok().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::decode_encoded_words;

    #[test]
    fn base64编码字能解成正常文字() {
        assert_eq!(decode_encoded_words("=?UTF-8?B?R29vZ2xl?="), "Google");
        assert_eq!(decode_encoded_words("=?UTF-8?B?5L2g5aW9?="), "你好");
    }

    #[test]
    fn quoted_printable编码字下划线当空格() {
        assert_eq!(decode_encoded_words("=?ISO-8859-1?Q?Caf=E9?="), "Café");
        assert_eq!(decode_encoded_words("=?UTF-8?Q?hello_world?="), "hello world");
    }

    #[test]
    fn 常见中文字符集能解出来() {
        // 中文（GBK：D6 D0 CE C4）的 base64。
        assert_eq!(decode_encoded_words("=?GBK?B?1tDOxA==?="), "中文");
        assert_eq!(decode_encoded_words("=?gb2312?B?1tDOxA==?="), "中文");
    }

    #[test]
    fn 一段里混着多个编码字和普通文字() {
        assert_eq!(
            decode_encoded_words("前缀 =?UTF-8?B?5L2g5aW9?= 和后缀"),
            "前缀 你好 和后缀"
        );
        assert_eq!(
            decode_encoded_words("Re: =?UTF-8?Q?Report_2026?= (final)"),
            "Re: Report 2026 (final)"
        );
    }

    #[test]
    fn 相邻编码字之间的空白会丢掉() {
        assert_eq!(decode_encoded_words("=?UTF-8?B?5L2g?=  =?UTF-8?B?5aW9?="), "你好");
    }

    #[test]
    fn 坏数据原样返回且不panic() {
        for raw in [
            "=?UTF-8?B?!!!?=",
            "=?UTF-8?X?abc?=",
            "=?UTF-8?B?abc",
            "=?UTF-8?Q?=ZZ?=",
            "=?nonsense",
            "=?=?=?",
            "=?UTF-8??",
            "",
            "=?UTF-8?B??=",
            "=?...?-...?=",
        ] {
            assert_eq!(decode_encoded_words(raw), raw, "应原样返回：{raw}");
        }
    }

    #[test]
    fn 未知字符集但字节是合法utf8时能用() {
        assert_eq!(decode_encoded_words("=?x-unknown?B?5L2g5aW9?="), "你好");
    }

    #[test]
    fn 普通文字幂等不变() {
        for raw in ["普通主题", "Hello 世界", "Re: 报价", "a=?b"] {
            assert_eq!(decode_encoded_words(raw), raw);
            let once = decode_encoded_words(raw);
            assert_eq!(decode_encoded_words(&once), once);
        }
        let decoded = decode_encoded_words("=?UTF-8?B?5L2g5aW9?=");
        assert_eq!(decode_encoded_words(&decoded), decoded);
    }
}
