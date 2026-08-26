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
