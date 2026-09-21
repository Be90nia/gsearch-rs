//! M16 SearXNG 客户端：JSON API 优先 + HTML 结果页降级（format=json 不可用时），
//! 纯 HTTP GET，不经浏览器、不走代理。
//!
//! 请求形态对照参考实现 D:/Project/searxng/src/searxng.rs（ureq 同步版 → reqwest async）：
//! `GET {base}/search?q=<urlencoded>&format=json[&time_range=<recency>]&pageno=<N>`，5s 超时。
//! SearXNG 是局域网直连实例，**不走 GSEARCH_PROXY / 系统代理**。

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result};
use scraper::{Html, Selector};
use serde::Deserialize;

use crate::parse::{absolutize, text_of};
use crate::types::SearchResult;

/// Low①：复用 reqwest::Client（连接池/tcp keepalive/handshake 重用）。原来每次请求
/// 都 builder().build() 一遍——每次新建连接池、多页搜索（10 页）= 10 次握手。
/// LazyLock 进程内单例；clear() 不会因为共享 client 被毒化（panic 也不影响其他调用方）。
static SHARED_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        // 局域网直连：禁掉环境代理（HTTP_PROXY 等），配了 GSEARCH_PROXY 也绝不经代理
        .no_proxy()
        .build()
        .expect("构建 SearXNG 共享 HTTP 客户端失败")
});
fn null_as_default<'de, D, T>(de: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(de)?.unwrap_or_default())
}

#[derive(Deserialize)]
struct SearxngResult {
    #[serde(default, deserialize_with = "null_as_default")]
    title: String,
    #[serde(default, deserialize_with = "null_as_default")]
    url: String,
    #[serde(default, deserialize_with = "null_as_default")]
    content: String,
}

#[derive(Deserialize)]
struct SearxngResponse {
    #[serde(default)]
    results: Vec<SearxngResult>,
}

/// 构造 `/search` 请求 URL：`q → [format=json] → [time_range] → pageno`。
/// recency=None 时与旧版 URL 逐字节一致（验收要求）。
fn build_url(
    base_url: &str,
    query: &str,
    page: u32,
    json: bool,
    recency: Option<crate::search::Recency>,
) -> String {
    let mut url = format!(
        "{}/search?q={}",
        base_url.trim_end_matches('/'),
        crate::search::urlencode(query),
    );
    if json {
        url.push_str("&format=json");
    }
    if let Some(r) = recency {
        url.push_str("&time_range=");
        url.push_str(r.as_str());
    }
    url.push_str("&pageno=");
    url.push_str(&page.to_string());
    url
}

/// GET `{base}/search?q=<encoded>&format=json[&time_range=]<&pageno>`；5s 超时、明确禁代理。
/// recency=Some 追加 `&time_range=<day|week|month|year>`（None 时 URL 与旧版逐字节一致）。
/// 成功返回映射后的 SearchResult（snippet ← SearXNG content 字段）。
pub async fn search(
    base_url: &str,
    query: &str,
    page: u32,
    recency: Option<crate::search::Recency>,
) -> Result<Vec<SearchResult>> {
    let url = build_url(base_url, query, page, true, recency);
    let resp = SHARED_CLIENT
        .get(&url)
        .send()
        .await
        .with_context(|| format!("请求 SearXNG 失败: {url}"))?
        .error_for_status()
        .with_context(|| format!("SearXNG 返回错误状态: {url}"))?;
    let text = resp.text().await.context("读取 SearXNG 响应失败")?;
    parse(&text)
}

/// 解析 SearXNG JSON 响应 → SearchResult 列表（纯函数，单测直接喂样例 payload）。
fn parse(text: &str) -> Result<Vec<SearchResult>> {
    let body: SearxngResponse = serde_json::from_str(text)
        .with_context(|| format!("SearXNG 响应 JSON 解析失败（检查实例 format=json 是否启用）: {text:.120}"))?;
    Ok(body
        .results
        .into_iter()
        .map(|r| SearchResult {
            title: r.title,
            url: r.url,
            snippet: r.content,
        })
        .collect())
}
/// 降级路径：抓 SearXNG HTML 结果页（format=json 不可用如 403 时）。
/// `GET {base}/search?q=<encoded>&pageno=<page>`（不带 format=json），
/// 5s 超时、明确禁代理，与 json 同款请求参数（含 time_range）。
pub async fn search_html(
    base_url: &str,
    query: &str,
    page: u32,
    recency: Option<crate::search::Recency>,
) -> Result<Vec<SearchResult>> {
    let url = build_url(base_url, query, page, false, recency);
    let resp = SHARED_CLIENT
        .get(&url)
        .send()
        .await
        .with_context(|| format!("请求 SearXNG HTML 失败: {url}"))?
        .error_for_status()
        .with_context(|| format!("SearXNG HTML 返回错误状态: {url}"))?;
    let text = resp.text().await.context("读取 SearXNG HTML 响应失败")?;
    Ok(parse_html(&text, base_url))
}
/// 解析 SearXNG HTML 结果页 → SearchResult 列表（纯函数，单测直接喂 HTML 片段）。
/// 首选 `article.result` 容器；容器缺失（主题差异/改版）时放宽为 h3>a 与
/// p.content 的文档序流式配对（同 parse.rs SEL_WALK 手法）。
fn parse_html(html: &str, base_url: &str) -> Vec<SearchResult> {
    let doc = Html::parse_document(html);
    let article = Selector::parse("article.result").expect("静态选择器必然合法");
    let mut results = if doc.select(&article).next().is_some() {
        extract_articles(&doc, &article, base_url)
    } else {
        extract_walk(&doc, base_url)
    };
    // URL 去重保首次（同 parse.rs；search.rs 的 seen 是第二道）
    let mut seen = HashSet::new();
    results.retain(|r| !r.url.is_empty() && seen.insert(r.url.clone()));
    results
}

/// article.result 容器路径：title=h3>a 文本（无 h3 退任一 a[href]），snippet=p.content。
fn extract_articles(doc: &Html, article: &Selector, base_url: &str) -> Vec<SearchResult> {
    let h3a = Selector::parse("h3 > a[href]").expect("静态选择器必然合法");
    let anya = Selector::parse("a[href]").expect("静态选择器必然合法");
    let content = Selector::parse("p.content").expect("静态选择器必然合法");
    let mut out = Vec::new();
    for art in doc.select(article) {
        let Some(a) = art.select(&h3a).next().or_else(|| art.select(&anya).next()) else {
            continue;
        };
        let snippet = art.select(&content).next().map(|c| text_of(&c)).unwrap_or_default();
        out.push(SearchResult {
            title: text_of(&a),
            url: absolutize(a.value().attr("href").unwrap_or_default().trim(), base_url),
            snippet,
        });
    }
    out
}

/// 无容器兜底：h3>a 开新结果，其后文档序首个 p.content 配为 snippet（流式配对）。
fn extract_walk(doc: &Html, base_url: &str) -> Vec<SearchResult> {
    let walk = Selector::parse("h3 > a[href], p.content").expect("静态选择器必然合法");
    let mut out: Vec<SearchResult> = Vec::new();
    let mut pending = false; // 最近一条结果还没配到 snippet
    for el in doc.select(&walk) {
        if el.value().name() == "a" {
            out.push(SearchResult {
                title: text_of(&el),
                url: absolutize(el.value().attr("href").unwrap_or_default().trim(), base_url),
                snippet: String::new(),
            });
            pending = true;
        } else if pending {
            let last = out.last_mut().expect("pending 蕴含已有结果");
            last.snippet = text_of(&el);
            pending = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照真实 SearXNG format=json 响应形态（多余键忽略、content → snippet）。
    #[test]
    fn parses_real_shape_payload() {
        let payload = r#"{
            "query": "rust async",
            "results": [
                {
                    "title": "Tokio tutorial",
                    "url": "https://tokio.rs/tokio/tutorial",
                    "content": "An introduction to async Rust with Tokio.",
                    "engine": "bing",
                    "engines": ["bing", "ddg"],
                    "score": 1.5,
                    "parsed_url": ["https", "tokio.rs", "/tokio/tutorial"]
                },
                {
                    "title": "async book",
                    "url": "https://rust-lang.github.io/async-book/",
                    "content": "Async programming in Rust."
                }
            ],
            "answers": [],
            "infoboxes": [],
            "suggestions": ["rust async await"],
            "unresponsive_engines": []
        }"#;
        let results = parse(payload).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Tokio tutorial");
        assert_eq!(results[0].url, "https://tokio.rs/tokio/tutorial");
        assert_eq!(results[0].snippet, "An introduction to async Rust with Tokio.");
        assert_eq!(results[1].snippet, "Async programming in Rust.");
    }

    /// SearXNG 对缺失/空字段输出 null 而非省略键 → 兜底为空串，不炸解析。
    #[test]
    fn null_fields_fall_back_to_default() {
        let payload = r#"{"results": [{"title": null, "url": "https://a.example/", "content": null}]}"#;
        let results = parse(payload).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "");
        assert_eq!(results[0].url, "https://a.example/");
        assert_eq!(results[0].snippet, "");
    }

    #[test]
    fn empty_results_yields_empty_vec() {
        assert!(parse(r#"{"results": []}"#).unwrap().is_empty());
        // results 键缺失也一样
        assert!(parse(r#"{"query": "x"}"#).unwrap().is_empty());
    }

    /// 非 JSON（如实例未启用 format=json 时的 403 HTML）必须报错而非静默空列表。
    #[test]
    fn non_json_payload_is_err() {
        assert!(parse("<html>403 Forbidden</html>").is_err());
    }

    /// HTML 降级：对照真实 SearXNG 结果页形态（article.result + h3>a + p.content）。
    /// 2 条完整 + 1 条缺 content；相对 href 绝对化到实例地址。
    #[test]
    fn parses_article_results() {
        let html = r#"
        <div id="results">
          <article class="result result-default">
            <h3><a href="https://www.rust-lang.org/" rel="noreferrer">Rust Programming Language</a></h3>
            <p class="content">A language empowering everyone&#160;to build reliable software.</p>
          </article>
          <article class="result result-default">
            <h3><a href="/go?url=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2F">The Rust Book</a></h3>
            <p class="content">Learn Rust with an exercise-heavy approach.</p>
          </article>
          <article class="result result-default">
            <h3><a href="https://crates.io">crates.io: Rust Package Registry</a></h3>
          </article>
        </div>"#;
        let results = parse_html(html, "http://192.168.89.249:8888");
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].snippet, "A language empowering everyone to build reliable software.");
        // 相对 href → 绝对化到实例地址
        assert_eq!(
            results[1].url,
            "http://192.168.89.249:8888/go?url=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2F"
        );
        // 缺 p.content → 空 snippet
        assert_eq!(results[2].snippet, "");
    }

    /// HTML 降级兜底：无 article.result 容器时 h3>a 与 p.content 流式配对。
    #[test]
    fn falls_back_to_stream_pairing_without_articles() {
        let html = r#"
        <div id="results">
          <h3><a href="https://a.example/">Alpha</a></h3>
          <p class="content">first snippet</p>
          <h3><a href="https://b.example/">Beta</a></h3>
          <p class="content">second snippet</p>
        </div>"#;
        let results = parse_html(html, "http://s.example");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Alpha");
        assert_eq!(results[0].snippet, "first snippet");
        assert_eq!(results[1].title, "Beta");
        assert_eq!(results[1].snippet, "second snippet");
    }

    use super::build_url;
    use crate::search::Recency;

    /// recency=None：json 与 html 两种 URL 均与改动前逐字节一致（验收要求）。
    #[test]
    fn build_url_without_recency_matches_legacy_shape() {
        assert_eq!(
            build_url("http://192.168.1.10:8888/", "rust", 1, true, None),
            "http://192.168.1.10:8888/search?q=rust&format=json&pageno=1"
        );
        assert_eq!(
            build_url("http://192.168.1.10:8888", "rust", 2, false, None),
            "http://192.168.1.10:8888/search?q=rust&pageno=2"
        );
    }

    /// recency=Some：在 format=json 之后、pageno 之前追加 time_range=<SearXNG 值>。
    #[test]
    fn build_url_appends_time_range() {
        assert_eq!(
            build_url("http://x:8888", "rust async", 1, true, Some(Recency::Week)),
            "http://x:8888/search?q=rust%20async&format=json&time_range=week&pageno=1"
        );
        assert_eq!(
            build_url("http://x:8888", "rust", 3, false, Some(Recency::Day)),
            "http://x:8888/search?q=rust&time_range=day&pageno=3"
        );
    }
}
