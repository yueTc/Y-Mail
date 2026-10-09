//! HTML 白名单清洗、CSS 清洗与远程图片拦截。
//!
//! 安全约定：正文是不可信输入。这里只输出白名单内的标签和属性，
//! 脚本、表单、iframe 与事件属性一律删掉；`style` 属性与 `<style>` 块里的
//! CSS 只保留排版 / 颜色 / 字体 / 间距 / 边框等安全属性，`url(...)`、
//! `@import`、`expression(...)`、`javascript:`、`behavior:`、`-moz-binding`、
//! `position:fixed` 等危险写法全部清掉；远程图片默认改写成
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
    "col",
    "colgroup",
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
///
/// `style` 留在这里是兜底：正常的 `<style>` 块在清洗前就被单独摘出来处理，
/// 万一有漏网的畸形写法，也不至于把 CSS 文本当正文显示出来。
const DROPPED_CONTENT_TAGS: &[&str] = &["script", "style", "iframe", "object", "embed", "form", "noscript"];

/// img 上允许的属性。
const IMG_ATTRIBUTES: &[&str] = &[
    "src",
    "alt",
    "width",
    "height",
    "data-em-original-src",
    "title",
    "class",
    "style",
    "border",
    "align",
    "valign",
];

/// 全局允许的属性（会出现在所有保留标签上）。
const GENERIC_ATTRIBUTES: &[&str] = &["title", "class", "style", "dir"];

/// 表格 / 布局类标签允许的属性。
const LAYOUT_ATTRIBUTES: &[&str] = &[
    "width",
    "height",
    "align",
    "valign",
    "bgcolor",
    "cellpadding",
    "cellspacing",
    "border",
    "colspan",
    "rowspan",
    "class",
    "style",
];

/// 内联样式里允许保留的属性（只留排版、颜色、字体、间距、边框）。
const SAFE_STYLE_PROPERTIES: &[&str] = &[
    "color",
    "background-color",
    "font",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "font-variant",
    "line-height",
    "letter-spacing",
    "word-spacing",
    "text-align",
    "text-decoration",
    "text-decoration-color",
    "text-indent",
    "text-transform",
    "text-shadow",
    "white-space",
    "word-break",
    "word-wrap",
    "overflow-wrap",
    "vertical-align",
    "direction",
    "margin",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "padding",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "border",
    "border-top",
    "border-right",
    "border-bottom",
    "border-left",
    "border-color",
    "border-style",
    "border-width",
    "border-radius",
    "border-collapse",
    "border-spacing",
    "width",
    "min-width",
    "max-width",
    "height",
    "min-height",
    "max-height",
    "display",
    "float",
    "clear",
    "list-style-type",
    "list-style-position",
    "table-layout",
    "caption-side",
    "empty-cells",
    "box-shadow",
    "opacity",
];

/// 值里允许出现的 CSS 函数；其余 `xxx(` 一律拒绝，`url(` 自然也在其中。
const SAFE_CSS_FUNCTIONS: &[&str] = &[
    "rgb",
    "rgba",
    "hsl",
    "hsla",
    "linear-gradient",
    "radial-gradient",
    "repeating-linear-gradient",
    "repeating-radial-gradient",
    "calc",
    "min",
    "max",
    "clamp",
    "translate",
    "translatex",
    "translatey",
    "scale",
    "rotate",
    "skew",
    "matrix",
    "cubic-bezier",
    "steps",
];

/// 危险写法关键字：去掉注释、统一小写、去掉空白后必须一个都不含。
const DANGEROUS_CSS_TOKENS: &[&str] = &[
    "url(",
    "expression(",
    "javascript:",
    "vbscript:",
    "behavior:",
    "-moz-binding",
    "@import",
    "@charset",
    "@namespace",
    "image-set(",
];
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
    // `<style>` 块在清洗器里会被整块删掉（它不做样式表判断），所以先单独摘出来，
    // 用我们自己的 CSS 清洗器处理后再拼回去。
    let (html_without_styles, styles) = extract_style_blocks(input);
    let (rewritten, blocked) = rewrite_remote_images(&html_without_styles);
    let cleaned = build_builder().clean(&rewritten).to_string();
    let mut out = String::with_capacity(cleaned.len() + styles.iter().map(String::len).sum::<usize>());
    for css in styles {
        if css.is_empty() {
            continue;
        }
        out.push_str("<style>");
        out.push_str(&css);
        out.push_str("</style>");
    }
    out.push_str(&cleaned);
    (out, blocked)
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
    tag_attributes.insert("a", ["href", "title", "class", "style"].into_iter().collect());
    let layout: HashSet<&str> = LAYOUT_ATTRIBUTES.iter().copied().collect();
    for tag in [
        "table", "td", "th", "tr", "tbody", "thead", "tfoot", "caption", "col", "colgroup",
    ] {
        tag_attributes.insert(tag, layout.clone());
    }
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
            match attribute {
                "style" => sanitize_style_attribute(value).map(Into::into),
                "class" => sanitize_class(value).map(Into::into),
                "dir" => sanitize_keyword(value, &["ltr", "rtl", "auto"]).map(Into::into),
                "align" | "valign" => sanitize_keyword(
                    value,
                    &[
                        "left", "right", "center", "justify", "top", "middle", "bottom", "baseline",
                    ],
                )
                .map(Into::into),
                "bgcolor" => sanitize_color(value).map(Into::into),
                "width" | "height" => sanitize_dimension(value).map(Into::into),
                "colspan" | "rowspan" | "cellpadding" | "cellspacing" | "border" => {
                    sanitize_number(value).map(Into::into)
                }
                _ => match (element, attribute) {
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
                },
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
///
/// 只处理会破坏属性边界的引号与尖括号，**不碰 &**：
/// 属性值在 HTML 里本来就用实体表示（&amp;），再转一次会让浏览器拿到
/// 多出来的 &amp;，带查询串的图片地址就会因为签名对不上而被拒绝。
fn escape_attribute(value: &str) -> String {
    value
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 清洗 `style` 属性：只保留安全声明；全被清掉时返回 None（属性直接删掉）。
fn sanitize_style_attribute(value: &str) -> Option<String> {
    let cleaned = sanitize_declarations(value);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// 清洗一段声明列表（如 `color:red; font-size:12px`）。
fn sanitize_declarations(text: &str) -> String {
    let without_comments = strip_css_comments(text);
    let mut out = String::new();
    for declaration in split_css(&without_comments, ';') {
        let Some(kept) = sanitize_declaration(&declaration) else {
            continue;
        };
        if !out.is_empty() {
            out.push(';');
        }
        out.push_str(&kept);
    }
    out
}

/// 清洗单条声明；属性不在白名单、或值里有危险写法时返回 None。
fn sanitize_declaration(declaration: &str) -> Option<String> {
    let (name, raw_value) = declaration.split_once(':')?;
    let name = name.trim().to_ascii_lowercase();
    if !SAFE_STYLE_PROPERTIES.contains(&name.as_str()) {
        return None;
    }
    let mut value = raw_value.trim();
    // `!important` 本身无害，但没保留的必要，直接去掉（不参与安全判断）。
    if let Some(position) = value.rfind('!') {
        if value
            .get(position + 1..)
            .is_some_and(|tail| tail.trim().eq_ignore_ascii_case("important"))
        {
            value = value.get(..position)?.trim_end();
        }
    }
    if value.is_empty()
        || value.contains('\\')
        || value.contains('<')
        || value.contains('>')
        || value.contains('{')
        || value.contains('}')
    {
        return None;
    }
    // 去掉注释、统一小写、去掉空白后再比对，专门对付大小写 / 空格 / 注释绕过。
    let canonical: String = value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if DANGEROUS_CSS_TOKENS.iter().any(|token| canonical.contains(token)) {
        return None;
    }
    if !css_functions_are_safe(&canonical) {
        return None;
    }
    Some(format!("{name}:{value}"))
}

/// 值里出现的函数只能是白名单里的（`rgb(...)`、`calc(...)` 等）。
fn css_functions_are_safe(value: &str) -> bool {
    let chars: Vec<char> = value.chars().collect();
    let mut quote: Option<char> = None;
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        match quote {
            Some(active) => {
                if ch == active {
                    quote = None;
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    quote = Some(ch);
                } else if ch == '(' {
                    let mut start = index;
                    while start > 0 {
                        let previous = chars[start - 1];
                        if previous.is_ascii_alphanumeric() || previous == '-' || previous == '_' {
                            start -= 1;
                        } else {
                            break;
                        }
                    }
                    if start == index {
                        return false;
                    }
                    let name: String = chars[start..index].iter().collect();
                    if !SAFE_CSS_FUNCTIONS.contains(&name.as_str()) {
                        return false;
                    }
                }
            }
        }
        index += 1;
    }
    true
}

/// 去掉 CSS 注释并用空格代替，避免把前后两个记号粘起来绕过检查。
fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    loop {
        let Some(start) = rest.find("/*") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("*/") else {
            // 注释没闭合：后面的内容全部吃掉。
            break;
        };
        out.push(' ');
        rest = &after[end + 2..];
    }
    out
}

/// 按分隔符拆分，但括号内和引号内的分隔符不算。
fn split_css(text: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    for ch in text.chars() {
        match quote {
            Some(active) => {
                current.push(ch);
                if ch == active {
                    quote = None;
                }
            }
            None => match ch {
                '"' | '\'' => {
                    quote = Some(ch);
                    current.push(ch);
                }
                '(' => {
                    depth += 1;
                    current.push(ch);
                }
                ')' => {
                    depth = depth.saturating_sub(1);
                    current.push(ch);
                }
                value if value == separator && depth == 0 => {
                    parts.push(std::mem::take(&mut current));
                }
                value => current.push(value),
            },
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// 清洗一整段样式表（`<style>` 块里的内容）。
///
/// 选择器原样保留（但拒绝含 `<`、`\`、`@`、`;` 的畸形写法），声明走与内联样式
/// 相同的白名单；所有 `@` 开头的规则（`@import`、`@media`、`@font-face` 等）
/// 整块丢弃。输出前再兜底检查一遍，确保没有 `<` 或 `\` 漏网。
fn sanitize_stylesheet(css: &str) -> String {
    let without_comments = strip_css_comments(css);
    let chars: Vec<char> = without_comments.chars().collect();
    let mut out = String::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index].is_whitespace() {
            index += 1;
            continue;
        }
        if chars[index] == '@' {
            index = skip_at_rule(&chars, index);
            continue;
        }
        let selector_start = index;
        let mut quote: Option<char> = None;
        let mut brace = None;
        while index < chars.len() {
            let ch = chars[index];
            match quote {
                Some(active) => {
                    if ch == active {
                        quote = None;
                    }
                }
                None => {
                    if ch == '"' || ch == '\'' {
                        quote = Some(ch);
                    } else if ch == '{' {
                        brace = Some(index);
                        break;
                    }
                }
            }
            index += 1;
        }
        let Some(brace) = brace else {
            break;
        };
        let selector: String = chars[selector_start..brace].iter().collect();
        let selector = selector.trim().to_string();
        let (block, next) = read_css_block(&chars, brace);
        index = next;
        if selector.is_empty() || !is_safe_selector(&selector) {
            continue;
        }
        let declarations = sanitize_declarations(&block);
        if declarations.is_empty() {
            continue;
        }
        out.push_str(&selector);
        out.push('{');
        out.push_str(&declarations);
        out.push('}');
    }
    if out.contains('<') || out.contains('\\') {
        return String::new();
    }
    out
}

/// 读一个 `{ ... }` 块的内容，返回内容与块结束后面的下标；支持嵌套。
fn read_css_block(chars: &[char], open: usize) -> (String, usize) {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut inner = String::new();
    let mut index = open;
    while index < chars.len() {
        let ch = chars[index];
        match quote {
            Some(active) => {
                if ch == active {
                    quote = None;
                }
                if depth >= 1 {
                    inner.push(ch);
                }
            }
            None => match ch {
                '"' | '\'' => {
                    quote = Some(ch);
                    if depth >= 1 {
                        inner.push(ch);
                    }
                }
                '{' => {
                    depth += 1;
                    if depth >= 2 {
                        inner.push(ch);
                    }
                }
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return (inner, index + 1);
                    }
                    inner.push(ch);
                }
                value => {
                    if depth >= 1 {
                        inner.push(value);
                    }
                }
            },
        }
        index += 1;
    }
    (inner, chars.len())
}

/// 跳过一条 `@` 规则（到 `;` 或配对的 `}`），整条丢弃。
fn skip_at_rule(chars: &[char], start: usize) -> usize {
    let mut index = start;
    let mut quote: Option<char> = None;
    while index < chars.len() {
        let ch = chars[index];
        match quote {
            Some(active) => {
                if ch == active {
                    quote = None;
                }
            }
            None => match ch {
                '"' | '\'' => quote = Some(ch),
                ';' => return index + 1,
                '{' => {
                    let (_, next) = read_css_block(chars, index);
                    return next;
                }
                _ => {}
            },
        }
        index += 1;
    }
    chars.len()
}

/// 选择器是否安全：不许出现会破坏 `<style>` 结构的字符或 CSS 转义。
fn is_safe_selector(selector: &str) -> bool {
    if selector.is_empty() || selector.chars().count() > 2000 {
        return false;
    }
    !selector
        .chars()
        .any(|ch| matches!(ch, '<' | '@' | '\\' | '{' | '}' | ';') || ch.is_control())
}

/// 找 `<style`（大小写不敏感，后面必须是空白 / `/` / `>`）。
fn find_style_start(input: &str) -> Option<usize> {
    let lower = input.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(found) = lower[from..].find("<style") {
        let index = from + found;
        let next = lower.as_bytes().get(index + 6).copied();
        if matches!(
            next,
            None | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') | Some(b'/') | Some(b'>')
        ) {
            return Some(index);
        }
        from = index + 6;
    }
    None
}

/// 找 `</style`（大小写不敏感，后面必须是空白 / `>`），返回相对下标。
fn find_style_close(input: &str) -> Option<usize> {
    let lower = input.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(found) = lower[from..].find("</style") {
        let index = from + found;
        let next = lower.as_bytes().get(index + 7).copied();
        if matches!(
            next,
            None | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') | Some(b'>')
        ) {
            return Some(index);
        }
        from = index + 7;
    }
    None
}

/// 把 `<style>` 块摘出来单独清洗，返回「去掉样式块的 HTML」与「清洗后的样式」。
fn extract_style_blocks(input: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(input.len());
    let mut styles = Vec::new();
    let mut rest = input;
    loop {
        let Some(start) = find_style_start(rest) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let Some(tag_end) = find_tag_end(&rest.as_bytes()[start..]) else {
            break;
        };
        let content_start = start + tag_end + 1;
        let Some(close) = find_style_close(&rest[content_start..]) else {
            styles.push(sanitize_stylesheet(&rest[content_start..]));
            break;
        };
        let content_end = content_start + close;
        styles.push(sanitize_stylesheet(&rest[content_start..content_end]));
        let after_close = &rest[content_end..];
        match after_close.find('>') {
            Some(gt) => rest = &after_close[gt + 1..],
            None => break,
        }
    }
    (out, styles)
}

/// 清洗 class 属性：只留标识符允许的字符，长度设上限。
fn sanitize_class(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 200 {
        return None;
    }
    if !trimmed
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b' '))
    {
        return None;
    }
    Some(trimmed.to_string())
}

/// 关键字类属性（align / valign / dir）：只接受白名单里的词。
fn sanitize_keyword(value: &str, allowed: &[&str]) -> Option<String> {
    let lower = value.trim().to_ascii_lowercase();
    if allowed.contains(&lower.as_str()) {
        Some(lower)
    } else {
        None
    }
}

/// 尺寸类属性（width / height）：纯数字 + 常见单位，或百分比。
fn sanitize_dimension(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 12 {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let split = lower.find(|ch: char| !ch.is_ascii_digit()).unwrap_or(lower.len());
    let (digits, unit) = lower.split_at(split);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    match unit {
        "" | "%" | "px" | "em" | "pt" | "ex" | "rem" | "vw" | "vh" => Some(trimmed.to_string()),
        _ => None,
    }
}

/// 纯数字属性（colspan / rowspan / border / cellpadding / cellspacing）。
fn sanitize_number(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 6 || !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(trimmed.to_string())
}

/// 颜色类属性（bgcolor）：`#rrggbb` 十六进制、颜色名、`rgb(...)` 这类写法。
fn sanitize_color(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 32 {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix('#') {
        if !hex.is_empty() && hex.len() <= 8 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Some(trimmed.to_string());
        }
        return None;
    }
    if lower.starts_with("rgb(")
        || lower.starts_with("rgba(")
        || lower.starts_with("hsl(")
        || lower.starts_with("hsla(")
    {
        let canonical: String = lower.chars().filter(|ch| !ch.is_whitespace()).collect();
        if canonical.bytes().all(|byte| {
            byte.is_ascii_digit()
                || matches!(
                    byte,
                    b'(' | b')' | b',' | b'.' | b'%' | b'r' | b'g' | b'b' | b'h' | b's' | b'l' | b'a'
                )
        }) {
            return Some(trimmed.to_string());
        }
        return None;
    }
    if lower.len() <= 24
        && lower
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || byte == b'-')
    {
        return Some(trimmed.to_string());
    }
    None
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
    fn 放行还原后带查询串的地址不会被重复转义() {
        let (html, blocked) = sanitize_html(
            r#"<img src="https://cdn.example.com/logo.png?key=1&amp;Expires=1808215120347&amp;Signature=Kl%2By%3D" alt="logo">"#,
        );
        assert_eq!(blocked, 1);
        let restored = restore_remote_images(&html);
        assert!(
            restored.contains(r#"src="https://cdn.example.com/logo.png?key=1&amp;Expires=1808215120347&amp;Signature=Kl%2By%3D""#),
            "{restored}"
        );
        assert!(!restored.contains("&amp;amp;"), "{restored}");
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

#[cfg(test)]
mod css_tests {
    use super::{sanitize_html, sanitize_style_attribute, sanitize_stylesheet};

    #[test]
    fn 内联样式保留安全排版去掉危险写法() {
        let cleaned = sanitize_style_attribute(
            "font-size:14px; color:#333; background:url(https://evil.example/a.png); position:fixed; behavior:url(#x)",
        )
        .expect("应保留安全声明");
        assert!(cleaned.contains("font-size:14px"), "{cleaned}");
        assert!(cleaned.contains("color:#333"), "{cleaned}");
        assert!(!cleaned.to_ascii_lowercase().contains("url("), "{cleaned}");
        assert!(!cleaned.to_ascii_lowercase().contains("position"), "{cleaned}");
        assert!(!cleaned.to_ascii_lowercase().contains("behavior"), "{cleaned}");
    }

    #[test]
    fn 大小写空格与注释绕过都会被清掉() {
        let cleaned = sanitize_style_attribute(
            "COLOR: #FFF; BACKGROUND-COLOR: u/**/rl(https://evil.example/x); WIDTH: expr/**/ession(alert(1)); color: javas cript:alert(1)",
        )
        .expect("至少 color 应保留");
        let lower = cleaned.to_ascii_lowercase();
        assert!(lower.contains("color:#fff"), "{cleaned}");
        assert!(!lower.contains("url("), "{cleaned}");
        assert!(!lower.contains("expression("), "{cleaned}");
        assert!(!lower.contains("javascript:"), "{cleaned}");
    }

    #[test]
    fn 样式表规则保留安全声明丢掉at规则与url() {
        let css = sanitize_stylesheet(
            "@import url(https://evil.example/a.css); p { color: #333; font-size: 14px; background: url(https://evil.example/1.png); -moz-binding: url(#x) }",
        );
        assert!(css.contains("p{color:#333;font-size:14px}"), "{css}");
        assert!(!css.contains("@import"), "{css}");
        assert!(!css.to_ascii_lowercase().contains("url("), "{css}");
        assert!(!css.contains("-moz-binding"), "{css}");
    }

    #[test]
    fn 危险属性在样式表里也不会漏网() {
        for css in [
            "p{color:red;position:fixed;top:0}",
            "p{color:red;behavior:url(#default#x)}",
            "p{color:red;background:u/**/rl(http://evil/x)}",
            "p{color:red;width:expression(alert(1))}",
            "p{color:red;color:javascript:alert(1)}",
        ] {
            let cleaned = sanitize_stylesheet(css);
            let lower = cleaned.to_ascii_lowercase();
            assert!(!lower.contains("position"), "{cleaned}");
            assert!(!lower.contains("behavior"), "{cleaned}");
            assert!(!lower.contains("url("), "{cleaned}");
            assert!(!lower.contains("expression"), "{cleaned}");
            assert!(!lower.contains("javascript"), "{cleaned}");
            assert!(lower.contains("color:red"), "{cleaned}");
        }
    }

    #[test]
    fn style块与表格属性被保留而脚本仍被清掉() {
        let (html, _) = sanitize_html(concat!(
            "<style>p{color:red}</style>",
            "<p style=\"font-size:12px;position:fixed\">hi</p>",
            "<table width=\"600\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" bgcolor=\"#ffffff\">",
            "<tr><td colspan=\"2\" valign=\"top\">x</td></tr></table>",
            "<script>alert(1)</script><iframe src=\"https://evil\"></iframe>",
            "<p onclick=\"alert(1)\">ok</p>",
        ));
        assert!(html.contains("<style>p{color:red}</style>"), "{html}");
        assert!(html.contains("font-size:12px"), "{html}");
        assert!(!html.contains("position:fixed"), "{html}");
        assert!(html.contains("width=\"600\""), "{html}");
        assert!(html.contains("cellpadding=\"0\""), "{html}");
        assert!(html.contains("bgcolor=\"#ffffff\""), "{html}");
        assert!(html.contains("colspan=\"2\""), "{html}");
        assert!(html.contains("valign=\"top\""), "{html}");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<iframe"), "{html}");
        assert!(!html.contains("onclick"), "{html}");
        assert!(html.contains("ok"), "{html}");
    }

    #[test]
    fn 全是危险声明的style块整块不留() {
        let (html, _) =
            sanitize_html("<style>a{background:url(https://evil/x.png);behavior:url(#x)}</style><p>正文</p>");
        assert!(!html.contains("<style>"), "{html}");
        assert!(!html.to_ascii_lowercase().contains("url("), "{html}");
        assert!(html.contains("正文"), "{html}");
    }

    #[test]
    fn 表格属性里的表达式会被丢掉() {
        let (html, _) =
            sanitize_html("<table><tr><td width=\"expression(alert(1))\" align=\"evil\">x</td></tr></table>");
        assert!(!html.contains("expression"), "{html}");
        assert!(!html.contains("align"), "{html}");
        assert!(html.contains("x"), "{html}");
    }
}
