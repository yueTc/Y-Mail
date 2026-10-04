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
pub fn split_html(html: &str) -> Vec<Segment> {
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth: usize = 0;

    for token in tokenize(html) {
        match token {
            Token::Text(text) => {
                if depth == 0 {
                    // 顶层裸文字也算一段（有些邮件正文没有 <p>）。
                    if !text.trim().is_empty() {
                        segments.push(text);
                    }
                } else {
                    current.push_str(&text);
                }
            }
            Token::Tag {
                name,
                closing,
                self_closing,
            } => {
                if name == "br" {
                    // <br> 表示软换行：留一个空格，避免前后文字粘在一起。
                    if !current.is_empty() {
                        current.push(' ');
                    }
                    continue;
                }
                if !BLOCK_TAGS.contains(&name.as_str()) {
                    // 行内标签只影响样式，文字已经进 current 了。
                    continue;
                }
                if closing {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && !current.trim().is_empty() {
                        segments.push(std::mem::take(&mut current));
                    }
                } else if self_closing {
                    // <br> 表示软换行，要留一个空格避免前后文字粘在一起；其它自闭合标签忽略。
                    if name == "br" && !current.is_empty() {
                        current.push(' ');
                    }
                } else {
                    if depth == 0 {
                        current.clear();
                    }
                    depth += 1;
                }
            }
        }
    }
    if !current.trim().is_empty() {
        segments.push(current);
    }

    segments
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
            text.push_str(&html[start..index]);
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
    fn 嵌套块级元素不会重复切段() {
        let html = "<blockquote><p>里面的段落</p></blockquote>";
        let segments = split_html(html);
        assert_eq!(segments.len(), 1, "嵌套时应算一段：{segments:?}");
        assert_eq!(segments[0].text, "里面的段落");
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
