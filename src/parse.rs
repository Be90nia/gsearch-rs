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
            let url = unwrap_google_redirect(&absolutize(
                el.value().attr("href").unwrap_or_default().trim(),
                "https://www.google.com",
            ));
            let domain_class = crate::util::domain_class(&url);
            results.push(SearchResult {
                title: text_of(&title_el),
                url,
                snippet: String::new(),
                score: None,
                domain_class,
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
pub(crate) fn text_of(el: &ElementRef) -> String {
    el.text().collect::<String>().replace('\u{a0}', " ").trim().to_string()
}

/// 结果 href 为相对路径时补 origin 前缀：Google SERP 传 "https://www.google.com"
/// （真机踩坑：click 1 报 "goto /goto?url=... 失败"），SearXNG HTML 传实例地址。
/// 绝对 URL 原样返回。
pub(crate) fn absolutize(href: &str, origin: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if let Some(rest) = href.strip_prefix('/') {
        format!("{}/{rest}", origin.trim_end_matches('/'))
    } else {
        href.to_string()
    }
}

/// 54c：Google 直爬壳 URL 解包——`google.com/url?q=<目标>` 与 `google.com/goto?url=<目标>`
/// 两种包装解出真实目标 URL（percent-decode 后须为 http/https 才采用）。
/// 解不出（非 google 壳 / 无参数 / 目标非 http）保留原样返回，不报错不丢结果。
pub(crate) fn unwrap_google_redirect(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    let is_google = lower.starts_with("https://www.google.com/") || lower.starts_with("https://google.com/");
    if !is_google {
        return url.to_string();
    }
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    if !matches!(
        base.to_ascii_lowercase().as_str(),
        "https://www.google.com/url"
            | "https://www.google.com/goto"
            | "https://google.com/url"
            | "https://google.com/goto"
    ) {
        return url.to_string();
    }
    // q= 优先、url= 兜底（Google 两种包装形态各占其一）
    let mut q: Option<String> = None;
    let mut u: Option<String> = None;
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("q=") {
            q.get_or_insert_with(|| crate::duckduckgo::percent_decode(v));
        } else if let Some(v) = pair.strip_prefix("url=") {
            u.get_or_insert_with(|| crate::duckduckgo::percent_decode(v));
        }
    }
    for cand in [q, u].into_iter().flatten() {
        let lc = cand.to_ascii_lowercase();
        if lc.starts_with("https://") || lc.starts_with("http://") {
            return cand;
        }
    }
    url.to_string()
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

    /// 54c：壳 URL 解包表——直链不动 / /url?q= 与 /goto?url= 解开 / 解不出保留原样。
    #[test]
    fn unwrap_google_redirect_table() {
        use super::unwrap_google_redirect;
        // 直链（含 SearXNG 实例链接）原样
        assert_eq!(unwrap_google_redirect("https://example.com/a?b=1"), "https://example.com/a?b=1");
        assert_eq!(
            unwrap_google_redirect("https://192.168.1.1:8888/search?q=x"),
            "https://192.168.1.1:8888/search?q=x"
        );
        // /url?q= 包装解开（percent-decode）
        assert_eq!(
            unwrap_google_redirect("https://www.google.com/url?q=https%3A%2F%2Fgithub.com%2Ffoo&sa=U"),
            "https://github.com/foo"
        );
        // /goto?url= 包装解开
        assert_eq!(
            unwrap_google_redirect("https://www.google.com/goto?url=https%3A%2F%2Fdocs.rs%2Fserde"),
            "https://docs.rs/serde"
        );
        // 裸域 google.com 壳同样解开
        assert_eq!(
            unwrap_google_redirect("https://google.com/url?q=https%3A%2F%2Fexample.org%2Fx"),
            "https://example.org/x"
        );
        // /url 无 q 参数 → 保留原样
        assert_eq!(
            unwrap_google_redirect("https://www.google.com/url?sa=t&rct=j"),
            "https://www.google.com/url?sa=t&rct=j"
        );
        // q 目标非 http(s)（畸形注入面）→ 保留原样
        assert_eq!(
            unwrap_google_redirect("https://www.google.com/url?q=javascript%3Aalert(1)"),
            "https://www.google.com/url?q=javascript%3Aalert(1)"
        );
    }

    /// 54c 端到端：SERP 里 /url?q= 壳链接解析后 url 是真实目标域。
    #[test]
    fn parse_serp_unwraps_goto_shell_urls() {
        let html = r#"<!doctype html><html><body>
            <a href="/url?q=https%3A%2F%2Fcrates.io%2Fcrates%2Ftokio&sa=U"><h3>tokio</h3></a>
        </body></html>"#;
        let r = parse_serp(html);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].url, "https://crates.io/crates/tokio");
        assert_eq!(r[0].domain_class, "other", "domain_class 按解包后真实 URL 判定");
    }
}
