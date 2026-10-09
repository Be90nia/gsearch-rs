//! M9 智能 --read：按文章结构自适应分段（无 5000 字硬约束）。
//!
//! - `extract_adaptive`：HTML → AdaptiveRead（h1/h2/h3 + 段落全集 + 自适应摘要）。
//! - 自适应规则：`<10` 段给全文；`10..=50` 段给前 10 段；`>50` 段给前 5 段。
//! - `format_adaptive`：渲染三段（目录 + 摘要 + 段落索引）。
//! - `format_headings_only`：仅目录，最省 token（~50）。
//! - `slice_from`：应用 `--from K`（仅在摘要起点偏移）。

use scraper::{ElementRef, Html, Selector};
use serde::Serialize;

/// 单个标题节点（h1/h2/h3）。
#[derive(Debug, Clone, Serialize)]
pub struct Heading {
    pub level: u8,
    pub text: String,
}

/// 单个段落索引条目（首句 + 字数，agent 决策深入用）。
#[derive(Debug, Clone, Serialize)]
pub struct Paragraph {
    pub index: usize,           // 1-based
    pub first_sentence: String,
    pub char_count: usize,
    /// e1i：--excerpt N 时填该段前 N 字符实际文本（--json 生效）；未启用 None → 不出键，
    /// 默认输出逐键不变（元审计：默认行为零变更）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
}

/// 自适应读取结果：目录 + 选中摘要 + 全量段落索引。
#[derive(Debug, Clone, Serialize)]
pub struct AdaptiveRead {
    pub url: String,
    pub title: String,
    pub headings: Vec<Heading>,
    /// 按文章长度动态选的段落全文：<10 段全给，10-50 给前 10 段，>50 给前 5 段。
    pub summary_paragraphs: Vec<String>,
    /// 全文章段落首句 + 字数（agent 可针对性 `--from K` 拿指定段）。
    pub paragraph_index: Vec<Paragraph>,
    /// 9as：<pre> 代码块全文（文档页签名/Example 段；--read 一次拿齐，免 --full 二跑）。
    /// 文档页的签名与示例代码在 <pre> 里（不在 <p>），单独成字段不挤占摘要段语义；
    /// 空时键缺席（8lp 缺席=正常，默认输出逐键不变）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub code_examples: Vec<String>,
}

const SEL_HEADINGS: &str = "h1, h2, h3";
const SEL_P: &str = "p";
/// 9as：代码块收集（<pre> 覆盖 docs.rs rustdoc 与 GitHub md 围栏块——内层 <code> 文本已被
/// pre 的 text 覆盖；行内 <code> 本就在 <p> 文本里，不重复收）。
const SEL_PRE: &str = "pre";
/// ①打回轮1：GitHub issues/PR 主评论容器（新旧两代 markup 各占其一；URL gate 在调用方）。
const SEL_GITHUB_COMMENT: &str = ".js-comment-body, .comment-body";
/// 段落索引展示上限：超出折叠"..另 X 段省略"，免索引段把 token 吃光。
const INDEX_DISPLAY_LIMIT: usize = 20;

/// 短文 / 中等 / 长文的摘要段数阈值。
const SHORT_MAX: usize = 10;        // < SHORT_MAX 给全文
const MEDIUM_TAKE: usize = 10;     // 10..=50 给前 MEDIUM_TAKE 段
const LONG_TAKE: usize = 5;        // > 50 给前 LONG_TAKE 段
const LONG_THRESHOLD: usize = 50;

/// 9as：代码块预算——只收前 2 块（文档页首两块 = 函数签名 + 首个 Example）、
/// 单块 1500 字符封顶（防 minified 巨块把摘要吃光），超长尾部标注截断。
const CODE_EXAMPLE_MAX_BLOCKS: usize = 2;
const CODE_EXAMPLE_BLOCK_MAX_CHARS: usize = 1500;

/// 从 HTML 提取 AdaptiveRead。url/title 由调用方拿到 HTML 后补（HTML <title> 不一定可信）。
/// excerpt_chars：Some(N) = paragraph_index 每项附该段前 N 字符（html 已被调用方 cap 过
/// read_max_chars，excerpt 总量天然受总 cap 约束）。
pub fn extract_adaptive(html: &str, excerpt_chars: Option<usize>) -> AdaptiveRead {
    let doc = Html::parse_document(html);
    let h_sel = Selector::parse(SEL_HEADINGS).expect("静态选择器必然合法");
    let p_sel = Selector::parse(SEL_P).expect("静态选择器必然合法");

    let headings: Vec<Heading> = doc
        .select(&h_sel)
        .map(|el| {
            let level = match el.value().name() {
                "h1" => 1,
                "h2" => 2,
                "h3" => 3,
                _ => 0,
            };
            Heading {
                level,
                text: el.text().collect::<String>().replace('\u{a0}', " ").trim().to_string(),
            }
        })
        .filter(|h| !h.text.is_empty())
        .collect();

    // 收集所有 <p> 文本（按文档序）。空段（无文字）也保留在 paragraph_index 里，
    // 但不进 summary（空段给 agent 看无意义）。
    let all_paragraphs: Vec<String> = doc
        .select(&p_sel)
        .map(|el| el.text().collect::<String>().replace('\u{a0}', " ").trim().to_string())
        .collect();

    let total = all_paragraphs.len();
    let take_n = if total < SHORT_MAX {
        total
    } else if total <= LONG_THRESHOLD {
        MEDIUM_TAKE
    } else {
        LONG_TAKE
    };
    let summary_paragraphs: Vec<String> = all_paragraphs
        .iter()
        .take(take_n)
        .filter(|p| !p.is_empty())
        .cloned()
        .collect();

    let paragraph_index: Vec<Paragraph> = all_paragraphs
        .iter()
        .enumerate()
        .map(|(i, p)| Paragraph {
            index: i + 1,
            first_sentence: first_sentence(p),
            char_count: p.chars().count(),
            excerpt: excerpt_chars.map(|n| p.chars().take(n).collect()),
        })
        .collect();

    // 9as：<pre> 代码块（文档页签名 + Example 段）。取前 2 块、单块 1500 字符封顶，
    // 超长尾部标注截断——预算固定，摘要 token 上界不因巨块失控。
    let pre_sel = Selector::parse(SEL_PRE).expect("静态选择器必然合法");
    let code_examples: Vec<String> = doc
        .select(&pre_sel)
        .map(|el| code_block_text(&el).trim().to_string())
        .filter(|t| !t.is_empty())
        .take(CODE_EXAMPLE_MAX_BLOCKS)
        .map(|t| {
            if t.chars().count() > CODE_EXAMPLE_BLOCK_MAX_CHARS {
                let head: String = t.chars().take(CODE_EXAMPLE_BLOCK_MAX_CHARS).collect();
                format!("{head}\n..（代码块超长已截断）")
            } else {
                t
            }
        })
        .collect();

    AdaptiveRead {
        url: String::new(),
        title: String::new(),
        headings,
        summary_paragraphs,
        paragraph_index,
        code_examples,
    }
}

/// ②打回轮1：pre 文本提取——元素边界无空白文本节点时按词边界补一个空格（rustdoc 签名的
/// token 间距靠 CSS margin，`text()` 直拼会把 `Result<T>where` 粘成非法 Rust 字面）。
/// 内部空白原样保留（禁空白折叠）：只在两侧都是词字符、或左侧以 `>`/`)` 收尾且右侧为词字符
/// 时插入；`from_str(`、`&'a` 等天然相邻形态不被拆开。
fn code_block_text(root: &ElementRef) -> String {
    use scraper::Node;
    let mut out = String::new();
    // 显式栈 DFS，children 逆序入栈保持文档序（ego_tree 未被 scraper re-export，类型全程推断）
    let mut stack: Vec<_> = root.children().rev().collect();
    while let Some(node) = stack.pop() {
        match node.value() {
            Node::Text(t) => {
                // 原样保留（禁空白折叠）；仅当节点以非空白字符开头且上一输出以词字符收尾时补一个
                // 空格——节点自带前导空白（如 where 后的 "\n    T:"）时插空格会产生尾随空格
                let s: &str = &t.text;
                if let (Some(last), Some(first)) = (out.chars().last(), s.chars().next())
                    && !first.is_whitespace()
                    && joins_word(last, first)
                {
                    out.push(' ');
                }
                out.push_str(s);
            }
            Node::Element(_) => {
                stack.extend(node.children().rev());
            }
            _ => {}
        }
    }
    out
}

/// 词边界补空格判定：两侧都是词字符，或左侧 `>`/`)` 收尾且右侧词字符（`Result<T>where` →
/// `Result<T> where`）；`from_str(`、`&'a`、`foo,` 等不插。
fn joins_word(left: char, right: char) -> bool {
    let left_word = left.is_alphanumeric() || left == '_' || left == '>' || left == ')';
    let right_word = right.is_alphanumeric() || right == '_';
    left_word && right_word
}

/// ①打回轮1：GitHub issues/PR 主评论容器抽取——页面正文在 DOM 尾部（head/nav/SVG sprite
/// 占掉前几十万字符），调用方先 cap 后抽会把正文全裁掉（实测 omitted=589331 / summary 空）。
/// 命中 .js-comment-body/.comment-body 返回容器 inner_html 拼接（None = 非 GitHub 形态，
/// 调用方按 URL gate 后才调）。产物交 extract_adaptive 正常走 <p>/<pre> 提取。
pub fn github_comment_html(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse(SEL_GITHUB_COMMENT).expect("静态选择器必然合法");
    let mut buf = String::new();
    for el in doc.select(&sel) {
        buf.push_str(&el.inner_html());
        buf.push('\n');
    }
    (!buf.is_empty()).then_some(buf)
}

fn first_sentence(p: &str) -> String {
    if p.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = p.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let is_ascii_punct = matches!(c, '.' | '!' | '?');
        let is_cn_punct = matches!(c, '。' | '！' | '？');
        if !is_ascii_punct && !is_cn_punct {
            continue;
        }
        // ASCII 标点要求后随空白 / 串尾；全角直接切
        let ok = if is_ascii_punct {
            chars.get(i + 1).is_none_or(|&nx| nx.is_whitespace())
        } else {
            true
        };
        if ok {
            return chars[..=i].iter().collect();
        }
    }
    p.trim().to_string()
}

/// 渲染三段（目录 + 摘要 + 段落索引）。`from_offset` 是 `--from K`，对摘要起点偏移。
pub fn format_adaptive(read: &AdaptiveRead, from_offset: usize) -> String {
    let mut out = String::new();
    out.push_str(&format!("=== {} | {} ===\n", read.url, read.title));

    // [目录]
    if read.headings.is_empty() {
        out.push_str("[目录]\n(无 h1/h2/h3 标题)\n\n");
    } else {
        out.push_str(&format!("[目录]\n本文 {} 个标题:\n", read.headings.len()));
        for h in &read.headings {
            // Markdown 风格缩进：h1=# h2=## h3=### ，靠前导空格区分视觉层级
            let prefix = match h.level {
                1 => "#",
                2 => "##",
                3 => "###",
                _ => "-",
            };
            out.push_str(&format!("  {prefix} {}\n", h.text));
        }
        out.push('\n');
    }

    // [摘要]（按 from 偏移）
    let summary: Vec<&String> = read
        .summary_paragraphs
        .iter()
        .skip(from_offset)
        .collect();
    if summary.is_empty() {
        if read.summary_paragraphs.is_empty() {
            out.push_str("[摘要]\n(无段落)\n\n");
        } else {
            out.push_str(&format!(
                "[摘要]\n(--from {from_offset} 越界，共 {} 段)\n\n",
                read.summary_paragraphs.len()
            ));
        }
    } else {
        out.push_str(&format!(
            "[摘要 - {} 段]\n",
            summary.len()
        ));
        for p in summary {
            out.push_str(p);
            out.push('\n');
        }
        out.push('\n');
    }

    // 9as：[示例代码]（<pre> 块；文档页 = 签名 + Example，--read 一次拿齐）
    if !read.code_examples.is_empty() {
        out.push_str(&format!("[示例代码 - {} 块]\n", read.code_examples.len()));
        for (i, code) in read.code_examples.iter().enumerate() {
            out.push_str(&format!("[code {}]\n{code}\n", i + 1));
        }
        out.push('\n');
    }

    // [段落索引]
    if read.paragraph_index.is_empty() {
        out.push_str("[段落索引]\n(无段落)\n");
    } else {
        let _shown = read.paragraph_index.len().min(INDEX_DISPLAY_LIMIT);
        out.push_str(&format!(
            "[段落索引 - 全文 {} 段]\n",
            read.paragraph_index.len()
        ));
        for p in read.paragraph_index.iter().take(INDEX_DISPLAY_LIMIT) {
            out.push_str(&format!(
                "  段{} ({} 字): {}\n",
                p.index,
                p.char_count,
                p.first_sentence
            ));
        }
        if read.paragraph_index.len() > INDEX_DISPLAY_LIMIT {
            let omitted = read.paragraph_index.len() - INDEX_DISPLAY_LIMIT;
            out.push_str(&format!("  ..另 {omitted} 段省略\n"));
        }
    }

    out
}

/// 仅目录渲染（最省 token，~50 token）。
pub fn format_headings_only(read: &AdaptiveRead) -> String {
    let mut out = String::new();
    out.push_str(&format!("=== {} | {} ===\n", read.url, read.title));
    if read.headings.is_empty() {
        out.push_str("(无 h1/h2/h3 标题)\n");
        return out;
    }
    out.push_str(&format!("本文 {} 个标题:\n", read.headings.len()));
    for h in &read.headings {
        let prefix = match h.level {
            1 => "#",
            2 => "##",
            3 => "###",
            _ => "-",
        };
        out.push_str(&format!("  {prefix} {}\n", h.text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造指定数量段落 + 一组标题的 HTML。
    fn build_html(paragraphs: &[&str], headings: &[&str]) -> String {
        let mut h = String::from("<!doctype html><html><head><title>T</title></head><body>");
        for hd in headings {
            // 简化：h1/h2/h3 混合，按首字母 h 后跟的 1/2/3 解析
            let level = hd.chars().nth(1).and_then(|c| c.to_digit(10)).unwrap_or(1);
            let text = &hd[2..];
            h.push_str(&format!("<h{level}>{text}</h{level}>\n"));
        }
        for p in paragraphs {
            h.push_str(&format!("<p>{p}</p>\n"));
        }
        h.push_str("</body></html>");
        h
    }

    fn empty_paragraphs(n: usize) -> Vec<String> {
        (1..=n)
            .map(|i| format!("Paragraph number {i}. This is a sample sentence for testing."))
            .collect()
    }

    #[test]
    fn short_article_full_in_summary() {
        let paras = empty_paragraphs(5);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let read = extract_adaptive(&html, None);
        assert_eq!(read.summary_paragraphs.len(), 5);
        assert_eq!(read.paragraph_index.len(), 5);
        // summary 第一段包含全部段落原文
        assert!(read.summary_paragraphs[0].contains("Paragraph number 1."));
    }

    #[test]
    fn medium_article_takes_first_10() {
        let paras = empty_paragraphs(30);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let read = extract_adaptive(&html, None);
        assert_eq!(read.summary_paragraphs.len(), 10);
        assert_eq!(read.paragraph_index.len(), 30);
        // 首段是全文第一个 paragraph
        assert!(read.summary_paragraphs[0].contains("Paragraph number 1."));
        // 第 10 段是第 10 个 paragraph（不是第 11）
        assert!(read.summary_paragraphs[9].contains("Paragraph number 10."));
    }

    #[test]
    fn long_article_takes_first_5() {
        let paras = empty_paragraphs(100);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let read = extract_adaptive(&html, None);
        assert_eq!(read.summary_paragraphs.len(), 5);
        assert_eq!(read.paragraph_index.len(), 100);
        assert!(read.summary_paragraphs[4].contains("Paragraph number 5."));
    }

    #[test]
    fn boundary_at_10_takes_10() {
        let paras = empty_paragraphs(10);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let read = extract_adaptive(&html, None);
        // <10 段走 SHORT_MAX → 给全文 = 10 段
        assert_eq!(read.summary_paragraphs.len(), 10);
    }

    #[test]
    fn boundary_at_50_takes_10() {
        let paras = empty_paragraphs(50);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let read = extract_adaptive(&html, None);
        // 10..=50 走 MEDIUM_TAKE = 10
        assert_eq!(read.summary_paragraphs.len(), 10);
    }

    #[test]
    fn boundary_at_51_takes_5() {
        let paras = empty_paragraphs(51);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let read = extract_adaptive(&html, None);
        assert_eq!(read.summary_paragraphs.len(), 5);
    }

    #[test]
    fn paragraph_index_always_complete() {
        let paras = empty_paragraphs(100);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let read = extract_adaptive(&html, None);
        assert_eq!(read.paragraph_index.len(), 100);
        // 索引条目包含首句
        assert!(!read.paragraph_index[0].first_sentence.is_empty());
        // char_count 大于 0
        assert!(read.paragraph_index[0].char_count > 0);
    }

    #[test]
    fn headings_order_h1_h2_h3_preserved() {
        let html = build_html(
            &["p1"],
            &["h1First", "h2Second", "h3Third", "h1Fourth"],
        );
        let read = extract_adaptive(&html, None);
        assert_eq!(read.headings.len(), 4);
        assert_eq!(read.headings[0].level, 1);
        assert_eq!(read.headings[0].text, "First");
        assert_eq!(read.headings[1].level, 2);
        assert_eq!(read.headings[2].level, 3);
        assert_eq!(read.headings[3].level, 1);
    }

    #[test]
    fn first_sentence_split_ascii() {
        assert_eq!(first_sentence("Hello world. This is more."), "Hello world.");
        assert_eq!(first_sentence("What? Yes."), "What?");
        assert_eq!(first_sentence("Wow! Great."), "Wow!");
    }

    #[test]
    fn first_sentence_split_cn_fullwidth() {
        assert_eq!(first_sentence("你好世界。这是更多。"), "你好世界。");
        assert_eq!(first_sentence("什么？真的吗。"), "什么？");
    }

    #[test]
    fn first_sentence_no_punctuation_returns_full() {
        assert_eq!(first_sentence("no punctuation here"), "no punctuation here");
    }

    #[test]
    fn format_headings_only_contains_all_headings() {
        let html = build_html(&["p"], &["h1A", "h2B", "h3C"]);
        let mut read = extract_adaptive(&html, None);
        read.url = "https://e.test".into();
        read.title = "T".into();
        let out = format_headings_only(&read);
        assert!(out.contains("=== https://e.test | T ==="));
        assert!(out.contains("# A"));
        assert!(out.contains("## B"));
        assert!(out.contains("### C"));
        // 不应包含段落索引段
        assert!(!out.contains("[段落索引]"));
        assert!(!out.contains("[摘要"));
    }

    #[test]
    fn format_adaptive_index_limit_folds_remainder() {
        let paras = empty_paragraphs(100);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let mut read = extract_adaptive(&html, None);
        read.url = "u".into();
        read.title = "t".into();
        let out = format_adaptive(&read, 0);
        // 段落索引只展示前 20 段 + "..另 80 段省略"
        assert!(out.contains("段1 "));
        assert!(out.contains("段20 "));
        assert!(out.contains("..另 80 段省略"));
    }

    #[test]
    fn format_adaptive_from_offset_skips_summary_lead() {
        let paras = empty_paragraphs(100);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let mut read = extract_adaptive(&html, None);
        read.url = "u".into();
        read.title = "t".into();
        // --from 3：摘要从第 3 段开始（共 LONG_TAKE - 3 = 2 段）
        let out = format_adaptive(&read, 3);
        // 段落索引仍然全量（段1 "Paragraph number 1" 必在索引段里）
        assert!(out.contains("段1 "));
        // 摘要段跳过 3 段，应只剩 "Paragraph number 4" 和 "5"
        let summary_section_start = out.find("[摘要").unwrap();
        let index_section_start = out.find("[段落索引").unwrap();
        let summary_section = &out[summary_section_start..index_section_start];
        assert!(!summary_section.contains("Paragraph number 1."));
        assert!(!summary_section.contains("Paragraph number 2."));
        assert!(!summary_section.contains("Paragraph number 3."));
        assert!(summary_section.contains("Paragraph number 4."));
        assert!(summary_section.contains("Paragraph number 5."));
    }

    /// e1i：--excerpt opt-in——Some(N) 填该段前 N 字符实际文本；None 时 excerpt 键
    /// 不出 JSON（默认输出逐键不变，回归闸）。
    #[test]
    fn excerpt_opt_in_fills_and_skips() {
        let html = "<html><body><p>hello world。</p><p>second</p></body></html>";
        let with = extract_adaptive(html, Some(5));
        assert_eq!(with.paragraph_index[0].excerpt.as_deref(), Some("hello"));
        assert_eq!(with.paragraph_index[1].excerpt.as_deref(), Some("secon"));
        // 截断长度上限生效：请求 100 字符只给段落实长
        let full = extract_adaptive(html, Some(100));
        assert_eq!(full.paragraph_index[1].excerpt.as_deref(), Some("second"));

        let none = extract_adaptive(html, None);
        assert!(none.paragraph_index[0].excerpt.is_none());
        let s = serde_json::to_string(&none.paragraph_index[0]).unwrap();
        assert!(!s.contains("excerpt"), "默认档不应出 excerpt 键: {s}");
        let s = serde_json::to_string(&with.paragraph_index[0]).unwrap();
        assert!(s.contains(r#""excerpt":"hello""#), "{s}");
    }

    /// 9as：文档页 Example fixture——签名 + 示例 <pre> 块进 code_examples，
    /// --read 一次拿到签名 + 示例，无需 --full 二跑。无 <pre> 页面键缺席（默认输出逐键不变）。
    #[test]
    fn code_examples_collected_from_pre_blocks() {
        // docs.rs 形态：函数签名 <pre> + 描述 <p> 若干 + Examples 节 <pre>
        let html = r##"<!doctype html><html><body>
<h1>Function from_str</h1>
<pre><code>pub fn from_str&lt;'a, T&gt;(s: &amp;'a str) -&gt; Result&lt;T&gt;
where
    T: Deserialize&lt;'a&gt;,
</code></pre>
<p>Deserializes an instance of type T directly from a string.</p>
<p>Errors section body text goes here.</p>
<h2>Example</h2>
<p>An example of deserializing:</p>
<pre><code>let v: Value = serde_json::from_str(r#"{"a":1}"#).unwrap();</code></pre>
</body></html>"##;
        let read = extract_adaptive(html, None);
        assert_eq!(read.code_examples.len(), 2, "签名 + Example 各一块");
        assert!(read.code_examples[0].contains("pub fn from_str"), "首块 = 函数签名");
        assert!(read.code_examples[1].contains("serde_json::from_str"), "次块 = Example");
        // 无 <pre> 页面：键缺席（serde skip_serializing_if）
        let plain = extract_adaptive("<html><body><p>no code here</p></body></html>", None);
        assert!(plain.code_examples.is_empty());
        let s = serde_json::to_string(&plain).unwrap();
        assert!(!s.contains("code_examples"), "空时不出键: {s}");
    }

    /// 9as：代码块预算——超过 2 块只收前 2；单块超 1500 字符截断并标注。
    #[test]
    fn code_examples_bounded_by_budget() {
        let mut html = String::from("<html><body>");
        for i in 1..=4 {
            html.push_str(&format!("<pre>block {i}</pre>"));
        }
        html.push_str("</body></html>");
        let read = extract_adaptive(&html, None);
        assert_eq!(read.code_examples.len(), 2, "只收前 2 块");
        assert_eq!(read.code_examples[0], "block 1");

        let huge = "x".repeat(2000);
        let big = extract_adaptive(&format!("<html><body><pre>{huge}</pre></body></html>"), None);
        assert_eq!(big.code_examples.len(), 1);
        assert!(
            big.code_examples[0].starts_with("x") && big.code_examples[0].contains("..（代码块超长已截断）"),
            "超长块截断并标注"
        );
        assert!(big.code_examples[0].chars().count() < 2000);
    }

    /// 9as：文本渲染带 [示例代码] 节，夹在摘要与段落索引之间。
    #[test]
    fn format_adaptive_renders_code_section() {
        let html = "<html><body><pre>let x = 1;</pre><p>prose</p></body></html>";
        let mut read = extract_adaptive(html, None);
        read.url = "u".into();
        read.title = "t".into();
        let out = format_adaptive(&read, 0);
        let code_start = out.find("[示例代码").expect("示例代码节存在");
        let summary_start = out.find("[摘要").unwrap();
        let index_start = out.find("[段落索引").unwrap();
        assert!(summary_start < code_start && code_start < index_start, "节序：摘要→示例代码→段落索引");
        assert!(out.contains("let x = 1;"));
    }

    /// ②打回轮1：签名空白保真——rustdoc token 间距靠 CSS（span 相邻无空白文本节点），
    /// 词边界补空格让 `Result<T>where` 还原为合法 Rust；天然相邻形态（`from_str(`、`&'a`）
    /// 不被拆；内部换行缩进原样保留（禁空白折叠）。
    #[test]
    fn code_block_text_preserves_word_boundaries() {
        let html = r##"<html><body><pre><code><span>pub</span><span>fn</span><span>from_str</span>(s: &amp;'a str) -&gt; <span>Result&lt;T&gt;</span><span>where</span>
    T: Deserialize&lt;'a&gt;,
</code></pre></body></html>"##;
        let read = extract_adaptive(html, None);
        assert_eq!(read.code_examples.len(), 1);
        let code = &read.code_examples[0];
        assert!(code.contains("pub fn from_str"), "词字符相邻补空格: {code:?}");
        assert!(code.contains("Result<T> where"), ">词边界补空格: {code:?}");
        assert!(code.contains("\n    T: Deserialize<'a>,"), "内部换行缩进原样: {code:?}");
        assert!(!code.contains("( s:"), "非词边界不插空格: {code:?}");
        assert!(!code.contains(" \n"), "自带前导空白的节点不产生尾随空格: {code:?}");
    }

    /// ①打回轮1：GitHub 主评论容器抽取——容器在 DOM 尾部（head 巨大），抽取命中容器内容；
    /// 无容器的普通页返回 None。
    #[test]
    fn github_comment_html_extracts_thread_containers() {
        let junk = "x".repeat(20_000);
        let html = format!(
            "<html><head><meta>{junk}</meta></head><body><nav>menu</nav>\
             <div class='js-comment-body'><p>issue body text here</p><pre>let a = 1;</pre></div>\
             <div class='comment-body'><p>first comment reply</p></div></body></html>"
        );
        let container = github_comment_html(&html).expect("容器命中");
        assert!(container.contains("issue body text here"));
        assert!(container.contains("first comment reply"));
        assert!(!container.contains(&junk), "head 噪声不进容器产物");
        // 抽取产物走 extract_adaptive 正常通道：<p> 进摘要、<pre> 进 code_examples
        let read = extract_adaptive(&container, None);
        assert!(read.summary_paragraphs.iter().any(|p| p.contains("issue body text here")));
        assert!(read.code_examples.iter().any(|c| c.contains("let a = 1;")));
        // 无容器页面 → None
        assert!(github_comment_html("<html><body><p>plain page</p></body></html>").is_none());
    }
}