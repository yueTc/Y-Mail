//! HTML 白名单清洗与远程图片拦截。
//!
//! 安全约定：正文是不可信输入。这里只输出白名单内的标签和属性，
//! 去掉脚本、表单、内联样式与事件属性；远程图片默认改写成
//! `data-em-original-src`，只有用户明确放行时才还原成 `src`。

use std::collections::{HashMap, HashSet};

/// 允许保留的标签白名单。
const ALLOWED_TAGS: &[&str] = &[
    "a",
    "abbr",
    "b",
    "blockquote",
    "br",
    "caption",
    "center",
    "code",
    "dd",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "li",
    "ol",
    "p",
    "pre",
    "q",
    "s",
    "small",
    "span",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "u",
    "ul",
];

/// 原样丢弃、连正文内容都不保留的标签。
const DROPPED_CONTENT_TAGS: &[&str] = &["script", "style", "iframe", "object", "embed", "form", "noscript"];

/// img 上允许的属性。
const IMG_ATTRIBUTES: &[&str] = &["src", "alt", "width", "height", "data-em-original-src", "title"];

/// 全局允许的属性（会出现在所有保留标签上）。
const GENERIC_ATTRIBUTES: &[&str] = &["title"];

/// 远程图片改写为这个属性，清洗后再按需还原。
pub const REMOTE_SRC_ATTRIBUTE: &str = "data-em-original-src";

/// 单张图片是否是远程地址（绝对 http/https 或协议相对 //）。
pub fn is_remote_image_url(value: &str) -> bool {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || value.starts_with("//")
}

/// 清洗 HTML，并默认把远程图片改写成待放行属性。
///
/// 返回清洗后的 HTML 与被拦截的远程图片数量。
pub fn sanitize_html(input: &str) -> (String, usize) {
    let (rewritten, blocked) = rewrite_remote_images(input);
    let cleaned = build_builder().clean(&rewritten).to_string();
    (cleaned, blocked)
}

/// 把已经清洗过的 HTML 里的远程图片还原成 `src`（用户放行本封时调用）。
pub fn restore_remote_images(input: &str) -> String {
    transform_img_tag(input, |tag| {
        let attr = find_attribute(tag, REMOTE_SRC_ATTRIBUTE)?;
        if !is_remote_image_url(attr.value) {
            return None;
        }
        let replacement = format!("src=\"{}\"", escape_attribute(attr.value));
        Some(replace_attribute(tag, &attr, &replacement))
    })
}

/// 构造白名单清洗器。
fn build_builder() -> ammonia::Builder<'static> {
    let tags: HashSet<&str> = ALLOWED_TAGS.iter().copied().collect();
    let mut tag_attributes: HashMap<&str, HashSet<&str>> = HashMap::new();
    tag_attributes.insert("img", IMG_ATTRIBUTES.iter().copied().collect());
    tag_attributes.insert("a", ["href", "title"].into_iter().collect());
    let generic: HashSet<&str> = GENERIC_ATTRIBUTES.iter().copied().collect();
    let url_schemes: HashSet<&str> = ["http", "https", "mailto", "data", "cid"].into_iter().collect();

    let mut builder = ammonia::Builder::default();
    builder
        .tags(tags)
        .tag_attributes(tag_attributes)
        .generic_attributes(generic)
        .url_schemes(url_schemes)
        .url_relative(ammonia::UrlRelative::Deny)
        .link_rel(None)
        .clean_content_tags(DROPPED_CONTENT_TAGS.iter().copied().collect())
        .attribute_filter(|element, attribute, value| {
            if attribute.starts_with("on") {
                return None;
            }
            match (element, attribute) {
                ("img", "src") => {
                    let lower = value.trim().to_ascii_lowercase();
                    if lower.starts_with("data:") || lower.starts_with("cid:") {
                        Some(value.into())
                    } else {
                        None
                    }
                }
                ("img", REMOTE_SRC_ATTRIBUTE) => {
                    if is_remote_image_url(value) {
                        Some(value.into())
                    } else {
                        None
                    }
                }
                ("a", "href") => {
                    let lower = value.trim().to_ascii_lowercase();
                    if lower.starts_with("http://")
                        || lower.starts_with("https://")
                        || lower.starts_with("mailto:")
                    {
                        Some(value.into())
                    } else {
                        None
                    }
                }
                _ => Some(value.into()),
            }
        });
    builder
}

/// 扫描 `<img>` 标签，把远程 `src` 改名为 `data-em-original-src`。
fn rewrite_remote_images(input: &str) -> (String, usize) {
    let mut blocked = 0;
    let result = transform_img_tag(input, |tag| {
        let src = find_attribute(tag, "src")?;
        if !is_remote_image_url(src.value) {
            return None;
        }
        let replacement = format!("{}=\"{}\"", REMOTE_SRC_ATTRIBUTE, escape_attribute(src.value));
        blocked += 1;
        Some(replace_attribute(tag, &src, &replacement))
    });
    (result, blocked)
}

/// 对每个 `<img ...>` 标签调用一次改写函数。
fn transform_img_tag(input: &str, mut transform: impl FnMut(&str) -> Option<String>) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let Some(relative) = find_img_start(&input[cursor..]) else {
            out.push_str(&input[cursor..]);
            break;
        };
        let start = cursor + relative;
        let Some(end_relative) = find_tag_end(&bytes[start..]) else {
            out.push_str(&input[cursor..]);
            break;
        };
        let end = start + end_relative + 1;
        out.push_str(&input[cursor..start]);
        let tag = &input[start..end];
        match transform(tag) {
            Some(replaced) => out.push_str(&replaced),
            None => out.push_str(tag),
        }
        cursor = end;
    }
    out
}

/// 找到大小写不敏感的 `<img` 起始位置（后面必须是空白、`>` 或 `/`）。
fn find_img_start(input: &str) -> Option<usize> {
    let lower = input.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(found) = lower[from..].find("<img") {
        let index = from + found;
        let next = lower.as_bytes().get(index + 4).copied();
        if matches!(
            next,
            None | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') | Some(b'>') | Some(b'/')
        ) {
            return Some(index);
        }
        from = index + 4;
    }
    None
}

/// 找到标签结束的 `>`；引号内的 `>` 不算结束。
fn find_tag_end(input: &[u8]) -> Option<usize> {
    let mut quote = None;
    for (index, byte) in input.iter().enumerate() {
        match (quote, byte) {
            (None, b'\'') | (None, b'"') => quote = Some(*byte),
            (Some(q), b) if b == &q => quote = None,
            (None, b'>') => return Some(index),
            _ => {}
        }
    }
    None
}

/// 一个属性的位置与原始值。
struct AttributeSpan<'a> {
    name_start: usize,
    value_end: usize,
    quoted: bool,
    value: &'a str,
}

/// 在单个标签里查属性（大小写不敏感），返回位置与值。
fn find_attribute<'a>(tag: &'a str, name: &str) -> Option<AttributeSpan<'a>> {
    let bytes = tag.as_bytes();
    let mut cursor = 0usize;
    if bytes.first() == Some(&b'<') {
        cursor = 1;
    }
    cursor = skip_tag_name(bytes, cursor);
    loop {
        cursor = skip_whitespace(bytes, cursor);
        if cursor >= bytes.len() || bytes[cursor] == b'>' || bytes[cursor] == b'/' {
            return None;
        }
        let name_start = cursor;
        while cursor < bytes.len() && is_attribute_name_byte(bytes[cursor]) {
            cursor += 1;
        }
        let name_end = cursor;
        let attr_name = &tag[name_start..name_end];
        cursor = skip_whitespace(bytes, cursor);
        if bytes.get(cursor) != Some(&b'=') {
            continue;
        }
        cursor += 1;
        cursor = skip_whitespace(bytes, cursor);
        let (value_start, value_end, quoted) = match bytes.get(cursor).copied() {
            Some(q @ (b'"' | b'\'')) => {
                let start = cursor + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != q {
                    end += 1;
                }
                (start, end, true)
            }
            _ => {
                let start = cursor;
                let mut end = cursor;
                while end < bytes.len()
                    && !bytes[end].is_ascii_whitespace()
                    && bytes[end] != b'>'
                    && bytes[end] != b'/'
                {
                    end += 1;
                }
                (start, end, false)
            }
        };
        if attr_name.eq_ignore_ascii_case(name) {
            return Some(AttributeSpan {
                name_start,
                value_end,
                quoted,
                value: &tag[value_start..value_end],
            });
        }
        cursor = if quoted { value_end + 1 } else { value_end };
    }
}

fn skip_tag_name(bytes: &[u8], mut cursor: usize) -> usize {
    while cursor < bytes.len()
        && !bytes[cursor].is_ascii_whitespace()
        && bytes[cursor] != b'>'
        && bytes[cursor] != b'/'
    {
        cursor += 1;
    }
    cursor
}

fn skip_whitespace(bytes: &[u8], mut cursor: usize) -> usize {
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

fn is_attribute_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
}

/// 把整个属性替换成新的文本。
fn replace_attribute(tag: &str, attr: &AttributeSpan<'_>, replacement: &str) -> String {
    let after = attr.value_end + if attr.quoted { 1 } else { 0 };
    let mut out = String::with_capacity(tag.len() + replacement.len());
    out.push_str(&tag[..attr.name_start]);
    out.push_str(replacement);
    out.push_str(&tag[after..]);
    out
}

/// 属性值写回 HTML 前做最小转义。
fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::{restore_remote_images, sanitize_html};

    #[test]
    fn 脚本和事件属性会被清掉() {
        let (html, _) = sanitize_html(
            r#"<p onclick="alert(1)">你好<script>alert(1)</script><img src="data:image/png;base64,AA" onerror="alert(1)"></p>"#,
        );
        assert!(!html.contains("<script"));
        assert!(!html.contains("onclick"));
        assert!(!html.contains("onerror"));
        assert!(html.contains("<p>你好"));
    }

    #[test]
    fn 远程追踪像素默认被拦截并能按需还原() {
        let (html, blocked) = sanitize_html(r#"<img src="https://tracker.example/1x1.gif" alt="像素">"#);
        assert_eq!(blocked, 1);
        assert!(!html.contains("<img src=\"https://tracker.example"));
        assert!(html.contains("data-em-original-src"));
        let restored = restore_remote_images(&html);
        assert!(restored.contains(r#"src="https://tracker.example/1x1.gif""#));
        assert!(!restored.contains("data-em-original-src"));
    }

    #[test]
    fn 协议相对远程图片也会被拦截() {
        let (html, blocked) = sanitize_html(r#"<img src="//tracker.example/p.gif">"#);
        assert_eq!(blocked, 1);
        assert!(html.contains("data-em-original-src"));
        assert!(!html.contains("<img src="));
    }

    #[test]
    fn iframe_object_embed_form_和javascript链接都不留() {
        let (html, _) = sanitize_html(
            r#"<iframe src="https://evil"></iframe><object data="x"></object><embed src="x"><form action="x"></form><a href="javascript:alert(1)">点我</a>"#,
        );
        assert!(!html.contains("<iframe"));
        assert!(!html.contains("<object"));
        assert!(!html.contains("<embed"));
        assert!(!html.contains("<form"));
        assert!(!html.contains("javascript:"));
        assert!(html.contains("点我"));
    }

    #[test]
    fn 允许data图片但不允许远程src漏网() {
        let (html, blocked) = sanitize_html(
            r#"<img src="data:image/png;base64,AA"><img src="http://a/b.png"><img src="cid:abc">"#,
        );
        assert_eq!(blocked, 1);
        assert!(html.contains("data:image/png"));
        assert!(html.contains("cid:abc"));
        assert!(!html.contains("<img src=\"http://a/b.png\""));
    }
}
