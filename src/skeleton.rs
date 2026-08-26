//! M9 智能 --read：按文章结构自适应分段（无 5000 字硬约束）。
//!
//! - `extract_adaptive`：HTML → AdaptiveRead（h1/h2/h3 + 段落全集 + 自适应摘要）。
//! - 自适应规则：`<10` 段给全文；`10..=50` 段给前 10 段；`>50` 段给前 5 段。
//! - `format_adaptive`：渲染三段（目录 + 摘要 + 段落索引）。
//! - `format_headings_only`：仅目录，最省 token（~50）。
//! - `format_json`：返回 AdaptiveRead 结构 JSON。
//! - `slice_from`：应用 `--from K`（仅在摘要起点偏移）。

use scraper::{Html, Selector};
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
}

const SEL_HEADINGS: &str = "h1, h2, h3";
const SEL_P: &str = "p";
/// 段落索引展示上限：超出折叠"..另 X 段省略"，免索引段把 token 吃光。
const INDEX_DISPLAY_LIMIT: usize = 20;

/// 短文 / 中等 / 长文的摘要段数阈值。
const SHORT_MAX: usize = 10;        // < SHORT_MAX 给全文
const MEDIUM_TAKE: usize = 10;     // 10..=50 给前 MEDIUM_TAKE 段
const LONG_TAKE: usize = 5;        // > 50 给前 LONG_TAKE 段
const LONG_THRESHOLD: usize = 50;

/// 从 HTML 提取 AdaptiveRead。url/title 由调用方拿到 HTML 后补（HTML <title> 不一定可信）。
pub fn extract_adaptive(html: &str) -> AdaptiveRead {
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
        })
        .collect();

    AdaptiveRead {
        url: String::new(),
        title: String::new(),
        headings,
        summary_paragraphs,
        paragraph_index,
    }
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

/// JSON 序列化（agent 解析友好）。
pub fn format_json(read: &AdaptiveRead) -> String {
    serde_json::to_string(read).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
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
        let read = extract_adaptive(&html);
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
        let read = extract_adaptive(&html);
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
        let read = extract_adaptive(&html);
        assert_eq!(read.summary_paragraphs.len(), 5);
        assert_eq!(read.paragraph_index.len(), 100);
        assert!(read.summary_paragraphs[4].contains("Paragraph number 5."));
    }

    #[test]
    fn boundary_at_10_takes_10() {
        let paras = empty_paragraphs(10);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let read = extract_adaptive(&html);
        // <10 段走 SHORT_MAX → 给全文 = 10 段
        assert_eq!(read.summary_paragraphs.len(), 10);
    }

    #[test]
    fn boundary_at_50_takes_10() {
        let paras = empty_paragraphs(50);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let read = extract_adaptive(&html);
        // 10..=50 走 MEDIUM_TAKE = 10
        assert_eq!(read.summary_paragraphs.len(), 10);
    }

    #[test]
    fn boundary_at_51_takes_5() {
        let paras = empty_paragraphs(51);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let read = extract_adaptive(&html);
        assert_eq!(read.summary_paragraphs.len(), 5);
    }

    #[test]
    fn paragraph_index_always_complete() {
        let paras = empty_paragraphs(100);
        let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
        let html = build_html(&refs, &["h1Title"]);
        let read = extract_adaptive(&html);
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
        let read = extract_adaptive(&html);
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
        let mut read = extract_adaptive(&html);
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
        let mut read = extract_adaptive(&html);
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
        let mut read = extract_adaptive(&html);
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

    #[test]
    fn format_json_roundtrip() {
        let html = build_html(&["p1. more.", "p2"], &["h1T"]);
        let mut read = extract_adaptive(&html);
        read.url = "u".into();
        read.title = "t".into();
        let json = format_json(&read);
        // 应为合法 JSON
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["url"], "u");
        assert_eq!(v["title"], "t");
        assert_eq!(v["headings"][0]["text"], "T");
        assert_eq!(v["summary_paragraphs"][0], "p1. more.");
        assert_eq!(v["paragraph_index"][0]["index"], 1);
    }
}