//! 把邮件正文按块级元素有序切段，供翻译逐段对齐。
//!
//! 为什么要切段：翻译接口一次只能给一段文本，而邮件是多段的。切段规则必须**稳定**——
//! 同一封邮件每次切出来的段落数和顺序都一样，否则「切换三种翻译模式不重新调用模型」
//! 这条要求就守不住了。

/// 一段原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// 段落在原文里的顺序，从 0 开始。
    pub index: usize,
    /// 该段的纯文本（已去掉标签、合并空白）。
    pub text: String,
}

/// 参与切段的块级标签；其余标签只当行内样式丢掉。
const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "li",
    "ul",
    "ol",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "td",
    "th",
    "tr",
    "table",
    "pre",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "dl",
    "dt",
    "dd",
];

/// 这些标签里的内容不是正文，切段时整段丢掉（否则样式表会被当成段落送去翻译）。
const SKIP_TAGS: &[&str] = &["style", "script", "head", "title", "template"];

/// 没有子内容、本身也不会形成段落的空元素。
const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track",
    "wbr",
];

/// 解析出来的轻量节点树：只需要块级结构与纯文本，够切段就行。
enum HtmlNode {
    /// 普通元素：标签名 + 子节点。
    Element { tag: String, children: Vec<HtmlNode> },
    /// 一段纯文本。
    Text(String),
}

/// 单个 HTML 记号。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// 标签：名字（小写）、是否闭合标签、是否自闭合。
    Tag {
        name: String,
        closing: bool,
        self_closing: bool,
    },
    /// 纯文本。
    Text(String),
}

/// 把 HTML 按块级元素切成有序段落。
///
/// 规则必须和前端 `MessageReader.tsx` 的 `collectReaderBlocks` 完全一致，否则翻译回填时
/// 前后端段落对不上，整段会落到「找不到对应段落」的分支里，出现「有些段落没翻译」。
/// 具体口径：
/// - 只认最内层的块级元素，容器继续往下钻；
/// - `<li>` 每个各自成段，译文才能逐条落在对应条目后面，嵌套列表继续下钻；
/// - `<table>` 整块算一段，免得译文被塞进表格行里把排版顶坏。
pub fn split_html(html: &str) -> Vec<Segment> {
    let root = build_tree(html);
    let mut texts: Vec<String> = Vec::new();
    walk_blocks(&root, &mut texts, true);
    texts
        .into_iter()
        .filter_map(|raw| {
            let text = normalize_whitespace(&raw);
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        })
        .enumerate()
        .map(|(index, text)| Segment { index, text })
        .collect()
}

/// 把标签流搭成轻量节点树：容错处理没闭合的标签，尽量贴近浏览器解析的结果。
fn build_tree(html: &str) -> Vec<HtmlNode> {
    // 栈底是虚拟根节点，标签名为空。
    let mut stack: Vec<(String, Vec<HtmlNode>)> = vec![(String::new(), Vec::new())];

    for token in tokenize(html) {
        match token {
            Token::Text(text) => {
                if let Some(frame) = stack.last_mut() {
                    frame.1.push(HtmlNode::Text(text));
                }
            }
            Token::Tag {
                name,
                closing,
                self_closing,
            } => {
                if name == "br" {
                    // 软换行折成一个空格，前后文字就不会粘在一起。
                    if let Some(frame) = stack.last_mut() {
                        frame.1.push(HtmlNode::Text(" ".to_string()));
                    }
                    continue;
                }
                if self_closing || VOID_TAGS.contains(&name.as_str()) {
                    if let Some(frame) = stack.last_mut() {
                        frame.1.push(HtmlNode::Element {
                            tag: name,
                            children: Vec::new(),
                        });
                    }
                    continue;
                }
                if closing {
                    let Some(pos) = stack.iter().rposition(|(tag, _)| tag == &name) else {
                        // 找不到配对的开始标签就当噪音丢掉。
                        continue;
                    };
                    // 先把没闭合的内层标签依次收进父节点，再收这个标签本身。
                    while stack.len() > pos + 1 {
                        let (tag, children) = stack.pop().expect("栈非空");
                        if let Some(frame) = stack.last_mut() {
                            frame.1.push(HtmlNode::Element { tag, children });
                        }
                    }
                    let (tag, children) = stack.pop().expect("栈非空");
                    if let Some(frame) = stack.last_mut() {
                        frame.1.push(HtmlNode::Element { tag, children });
                    }
                } else {
                    stack.push((name, Vec::new()));
                }
            }
        }
    }

    // 收尾：还没闭合的标签按层次合并回父节点。
    while stack.len() > 1 {
        let (tag, children) = stack.pop().expect("栈非空");
        if let Some(frame) = stack.last_mut() {
            frame.1.push(HtmlNode::Element { tag, children });
        }
    }
    stack.pop().map(|(_, children)| children).unwrap_or_default()
}

/// 深度优先收「成段」的元素文本，顺序即原文顺序，规则与前端完全对齐。
fn walk_blocks(nodes: &[HtmlNode], out: &mut Vec<String>, loose_text: bool) {
    for node in nodes {
        let HtmlNode::Element { tag, children } = node else {
            // 容器 / 文档里的裸文字也各算一段（有些邮件正文没有 <p>）。
            if let (true, HtmlNode::Text(text)) = (loose_text, node) {
                if !text.trim().is_empty() {
                    out.push(text.clone());
                }
            }
            continue;
        };
        if SKIP_TAGS.contains(&tag.as_str()) {
            continue;
        }
        if !BLOCK_TAGS.contains(&tag.as_str()) {
            // 行内元素不单独成段，继续往里找块级元素。
            walk_blocks(children, out, loose_text);
            continue;
        }
        let is_list_item = tag == "li";
        if is_list_item || tag == "table" {
            let text = element_text(node);
            if !text.trim().is_empty() {
                out.push(text);
            }
            // 列表项里的嵌套列表还要继续往下找，但不再拆列表项自己的文字。
            if is_list_item {
                walk_blocks(children, out, false);
            }
            continue;
        }
        if has_block_child(children) {
            // 里面还有块级元素，它只当容器，继续往里找最内层的段落。
            walk_blocks(children, out, loose_text);
            continue;
        }
        let text = element_text(node);
        if !text.trim().is_empty() {
            out.push(text);
        }
    }
}

/// 直接子元素里有没有块级元素；有的话当前元素只当容器。
fn has_block_child(children: &[HtmlNode]) -> bool {
    children.iter().any(|child| match child {
        HtmlNode::Element { tag, .. } => {
            BLOCK_TAGS.contains(&tag.as_str()) && !SKIP_TAGS.contains(&tag.as_str())
        }
        HtmlNode::Text(_) => false,
    })
}

/// 一个元素里的纯文本（不含样式 / 脚本内容）。
fn element_text(node: &HtmlNode) -> String {
    let mut out = String::new();
    collect_text(node, &mut out);
    out
}

fn collect_text(node: &HtmlNode, out: &mut String) {
    match node {
        HtmlNode::Text(text) => out.push_str(text),
        HtmlNode::Element { tag, children } => {
            if SKIP_TAGS.contains(&tag.as_str()) {
                return;
            }
            for child in children {
                collect_text(child, out);
            }
        }
    }
}

/// 把纯文本切成有序段落：优先按空行切；整篇没有空行时退回按单行切。
pub fn split_text(text: &str) -> Vec<Segment> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut blocks: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in normalized.split('\n') {
        if line.trim().is_empty() {
            if !current.trim().is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(line.trim());
        }
    }
    if !current.trim().is_empty() {
        blocks.push(current);
    }

    if blocks.len() <= 1 {
        // 没有空行分隔的邮件：按单行再切一次。
        let lines: Vec<String> = normalized
            .split('\n')
            .map(normalize_whitespace)
            .filter(|line| !line.is_empty())
            .collect();
        if lines.len() > 1 {
            blocks = lines;
        }
    }

    blocks
        .into_iter()
        .filter_map(|raw| {
            let text = normalize_whitespace(&raw);
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        })
        .enumerate()
        .map(|(index, text)| Segment { index, text })
        .collect()
}

/// 把一行里的连续空白压成一个空格并去掉首尾空白。
pub fn normalize_whitespace(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    out
}

/// 把常见 HTML 实体还原成字符，让后端切出的文本和前端 DOM 的 `textContent` 一致。
fn decode_entities(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        // 只在合理的长度内找分号，避免把一整段文字当成实体。
        let mut end = None;
        for (offset, ch) in rest.char_indices().take(12) {
            if ch == ';' {
                end = Some(offset);
                break;
            }
        }
        let Some(end) = end else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let decoded = match &rest[1..end] {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some('\u{00a0}'),
            other => decode_numeric_entity(other),
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 还原 `&#NN;` / `&#xHH;` 形式的数字实体。
fn decode_numeric_entity(entity: &str) -> Option<char> {
    let digits = entity.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    char::from_u32(code)
}

/// 把 HTML 拆成「标签」「文本」两类记号。
fn tokenize(html: &str) -> Vec<Token> {
    let bytes = html.as_bytes();
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] != b'<' {
            let start = index;
            while index < bytes.len() && bytes[index] != b'<' {
                index += 1;
            }
            text.push_str(&decode_entities(&html[start..index]));
            continue;
        }

        // 注释整段丢掉。
        if html[index..].starts_with("<!--") {
            if let Some(end) = html[index..].find("-->") {
                index += end + 3;
            } else {
                index = bytes.len();
            }
            continue;
        }

        if !text.is_empty() {
            tokens.push(Token::Text(std::mem::take(&mut text)));
        }

        let Some(end) = html[index..].find('>') else {
            break;
        };
        let raw = &html[index + 1..index + end];
        index += end + 1;

        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('!') || trimmed.starts_with('?') {
            continue;
        }
        let self_closing = trimmed.ends_with('/');
        let body = trimmed.trim_end_matches('/').trim();
        let closing = body.starts_with('/');
        let name: String = body
            .trim_start_matches('/')
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        tokens.push(Token::Tag {
            name,
            closing,
            self_closing,
        });
    }

    if !text.is_empty() {
        tokens.push(Token::Text(text));
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 按块级元素切段并保序() {
        let html =
            "<html><body><p>第一段</p><p>第二段<br>换行</p><blockquote>引用</blockquote></body></html>";
        let segments = split_html(html);
        let texts: Vec<&str> = segments.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, vec!["第一段", "第二段 换行", "引用"]);
        assert_eq!(segments[0].index, 0);
        assert_eq!(segments[2].index, 2);
    }

    #[test]
    fn 列表项各算一段() {
        let html = "<p>前面</p><ul><li>甲</li><li>乙</li></ul><p>后面</p>";
        let segments = split_html(html);
        let texts: Vec<&str> = segments.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, vec!["前面", "甲", "乙", "后面"]);
    }

    #[test]
    fn 样式与脚本内容不进正文() {
        let html = "<style>body{color:red}</style><p>正文</p><script>alert(1)</script>";
        let segments = split_html(html);
        let texts: Vec<&str> = segments.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, vec!["正文"]);
    }

    #[test]
    fn 列表后面的段落不会并进列表项() {
        let html = "<div><ul><li>甲</li></ul><p>乙</p></div>";
        let segments = split_html(html);
        let texts: Vec<&str> = segments.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, vec!["甲", "乙"]);
    }

    #[test]
    fn 嵌套块级元素不会重复切段() {
        let html = "<blockquote><p>里面的段落</p></blockquote>";
        let segments = split_html(html);
        assert_eq!(segments.len(), 1, "嵌套时应算一段：{segments:?}");
        assert_eq!(segments[0].text, "里面的段落");
    }

    #[test]
    fn 容器里的多段各自成段() {
        let html = "<div><h1>标题</h1><p>正文</p><p>结尾</p></div>";
        let segments = split_html(html);
        let texts: Vec<&str> = segments.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, vec!["标题", "正文", "结尾"]);
    }

    #[test]
    fn 实体还原后和前端文本一致() {
        let html = "<p>AT&amp;T 113 &#9650; &nbsp;ok</p>";
        let segments = split_html(html);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "AT&T 113 ▲ ok");
    }

    #[test]
    fn 顶层裸文字也算一段() {
        let html = "开头一句<p>段落</p>结尾一句";
        let segments = split_html(html);
        let texts: Vec<&str> = segments.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, vec!["开头一句", "段落", "结尾一句"]);
    }

    #[test]
    fn 注释与行内标签被丢掉() {
        let html = "<p>你好<!-- 隐藏 --><strong>世界</strong></p>";
        let segments = split_html(html);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "你好世界");
    }

    #[test]
    fn 同一封邮件两次切段结果一致() {
        let html = "<p>甲</p><p>乙</p><p>丙</p>";
        assert_eq!(split_html(html), split_html(html));
    }

    #[test]
    fn 纯文本按空行切段() {
        let text = "第一段第一行\n第一段第二行\n\n第二段\n\n\n第三段";
        let segments = split_text(text);
        let texts: Vec<&str> = segments.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, vec!["第一段第一行 第一段第二行", "第二段", "第三段"]);
    }

    #[test]
    fn 纯文本没有空行时按单行切段() {
        let text = "第一行\n第二行\n第三行";
        let segments = split_text(text);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[1].text, "第二行");
    }

    #[test]
    fn 空白会被规整() {
        assert_eq!(normalize_whitespace("  你好   世界 \n"), "你好 世界");
        assert_eq!(normalize_whitespace("   "), "");
    }

    #[test]
    fn 空正文切出零段() {
        assert!(split_html("").is_empty());
        assert!(split_html("<div></div>").is_empty());
        assert!(split_text("   \n\n  ").is_empty());
    }
}
