//! SERP HTML → Vec<SearchResult>（对照 plsearch parse_page.py，行为真值）
//!
//! Python 版语义：遍历 `a[href]`，内含 `<h3>` 即一条结果（title=h3 文本、
//! url=href 原样）；snippet 取该 `<a>` 之后文档序里第一个 `div.VwiC3b`
//! 的文本（find_next，不限同级），拿不到空串；nbsp 清理为空格。

use std::collections::HashSet;

use scraper::{ElementRef, Html, Selector};

use crate::types::SearchResult;

/// Google SERP 选择器集中一处（改版第一排查点，PLAN §5）。
/// WALK 按文档序产出 a[href] 与 VwiC3b 两类节点，流式配对等价 find_next。
const SEL_WALK: &str = "a[href], div.VwiC3b";
const SEL_H3: &str = "h3";

/// 解析一页 Google SERP HTML。空结果打 warn（Google 改版或验证码/零结果页）。
pub fn parse_serp(html: &str) -> Vec<SearchResult> {
    let doc = Html::parse_document(html);
    let walk = Selector::parse(SEL_WALK).expect("静态选择器必然合法");
    let h3 = Selector::parse(SEL_H3).expect("静态选择器必然合法");

    let mut results: Vec<SearchResult> = Vec::new();
    // 尚未配到 snippet 的结果下标；遇到下一个 VwiC3b 时统一配给。
    let mut pending: Vec<usize> = Vec::new();

    for el in doc.select(&walk) {
        if el.value().name() == "a" {
            let Some(title_el) = el.select(&h3).next() else {
                continue;
            };
            results.push(SearchResult {
                title: text_of(&title_el),
                url: el.value().attr("href").unwrap_or_default().trim().to_string(),
                snippet: String::new(),
            });
            pending.push(results.len() - 1);
        } else {
            let text = text_of(&el);
            for &i in &pending {
                results[i].snippet = text.clone();
            }
            pending.clear();
        }
    }

    // URL 去重保首次（Google 页内/跨页重复 listing；search.rs 的 seen 是第二道）
    let mut seen = HashSet::new();
    results.retain(|r| !r.url.is_empty() && seen.insert(r.url.clone()));

    if results.is_empty() {
        tracing::warn!("SERP 解析为空，可能 Google 改版或页面为验证码/零结果页");
    }
    results
}

/// 元素全文本：收集 + nbsp→空格 + 去首尾空白（等价 get_text(strip=True) + replace \xa0）
fn text_of(el: &ElementRef) -> String {
    el.text().collect::<String>().replace('\u{a0}', " ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::parse_serp;

    /// 单 listing：h3 标题 + VwiC3b 摘要 + 链接 + 去 nbsp
    #[test]
    fn parse_serp_single_listing() {
        let html = r#"<!doctype html><html><body>
            <a href="https://example.com/foo"><h3>Example Title</h3></a>
            <div class="VwiC3b">An example snippet for testing purposes here.</div>
        </body></html>"#;
        let r = parse_serp(html);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "Example Title");
        assert_eq!(r[0].url, "https://example.com/foo");
        assert!(r[0].snippet.contains("An example snippet"));
    }

    /// 多 listing：snippet 流式配对；VwiC3b 跟着的多个未配 title 都被填
    #[test]
    fn parse_serp_pairing_pending() {
        let html = r#"<!doctype html><html><body>
            <a href="https://a.com"><h3>Title A</h3></a>
            <a href="https://b.com"><h3>Title B</h3></a>
            <div class="VwiC3b">Snippet for both A and B.</div>
        </body></html>"#;
        let r = parse_serp(html);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].snippet, r[1].snippet);
        assert_eq!(r[0].snippet, "Snippet for both A and B.");
    }

    /// 同 URL 跨段重复应去重保首次
    #[test]
    fn parse_serp_dedup_same_url() {
        let html = r#"<!doctype html><html><body>
            <a href="https://dup.com"><h3>First</h3></a>
            <a href="https://dup.com"><h3>Second</h3></a>
        </body></html>"#;
        let r = parse_serp(html);
        assert_eq!(r.len(), 1, "URL 去重保首次");
        assert_eq!(r[0].title, "First");
    }

    /// 零结果：返回空（warning 在 tracing 层，不在这里断言）
    #[test]
    fn parse_serp_empty_returns_empty() {
        let html = "<!doctype html><html><body>no results here</body></html>";
        let r = parse_serp(html);
        assert!(r.is_empty());
    }
}
