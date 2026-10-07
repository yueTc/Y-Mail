//! IMAP 邮箱名使用的 Modified UTF-7（RFC 3501 5.1.3）。
//!
//! 这个编码不是普通 UTF-7：`&` 单独出现写成 `&-`，`&...-` 中间是
//! “逗号替代斜杠、去掉填充”的 Base64，解出来内容是 UTF-16BE。
//! 解析失败的策略是保留原始字符串，不让一个坏名字拖垮整个文件夹列表。

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

/// 把一个 Modified UTF-7 字符串解码成正常文本；失败时原样返回。
pub(crate) fn decode(input: &str) -> String {
    let mut output = String::new();
    let mut index = 0usize;

    while index < input.len() {
        let rest = &input[index..];
        let Some(ch) = rest.chars().next() else {
            break;
        };
        if ch != '&' {
            output.push(ch);
            index += ch.len_utf8();
            continue;
        }

        if input[index..].starts_with("&-") {
            output.push('&');
            index += 2;
            continue;
        }

        let after_ampersand = &input[index + 1..];
        let Some(relative_end) = after_ampersand.find('-') else {
            return input.to_string();
        };
        let end = index + 1 + relative_end;
        let encoded = &input[index + 1..end];
        let Ok(decoded) = decode_run(encoded) else {
            return input.to_string();
        };
        output.push_str(&decoded);
        index = end + 1;
    }

    output
}

/// 把正常文本编码成 Modified UTF-7，主要供测试和对称性验证。
#[cfg(test)]
pub(crate) fn encode(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut output = String::new();
    let mut index = 0usize;

    while index < chars.len() {
        let ch = chars[index];
        if ch == '&' {
            output.push_str("&-");
            index += 1;
            continue;
        }
        if ch.is_ascii() {
            output.push(ch);
            index += 1;
            continue;
        }

        let mut units = Vec::new();
        while index < chars.len() && !chars[index].is_ascii() {
            let mut buffer = [0u16; 2];
            units.extend_from_slice(chars[index].encode_utf16(&mut buffer));
            index += 1;
        }
        let bytes: Vec<u8> = units.iter().flat_map(|unit| unit.to_be_bytes()).collect();
        let encoded = STANDARD.encode(bytes).trim_end_matches('=').replace('/', ",");
        output.push('&');
        output.push_str(&encoded);
        output.push('-');
    }

    output
}

fn decode_run(run: &str) -> Result<String, ()> {
    let mut normalized = run.replace(',', "/");
    while !normalized.len().is_multiple_of(4) {
        normalized.push('=');
    }
    let bytes = STANDARD.decode(normalized).map_err(|_| ())?;
    if !bytes.len().is_multiple_of(2) {
        return Err(());
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn 已知乱码能解成中文() {
        assert_eq!(decode("&Xn9USpCuTvY-"), "广告邮件");
        assert_eq!(decode("&dcVr0mWHTvZZOQ-"), "病毒文件夹");
        assert_eq!(decode("&i6KWBZCuTvY-"), "订阅邮件");
    }

    #[test]
    fn 编码与解码能往返() {
        for value in ["收件箱/项目", "广告邮件", "A&B", "emoji 😀 也能过"] {
            assert_eq!(decode(&encode(value)), value);
        }
    }

    #[test]
    fn 坏的编码保留原样不报错() {
        for value in ["&bad-", "&A-", "&not-finished", "&Xn9USpCuTvY"] {
            assert_eq!(decode(value), value);
        }
    }
}
