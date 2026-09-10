//! M16 SearXNG JSON API 客户端：纯 HTTP GET，不经浏览器、不走代理。
//!
//! 请求形态对照参考实现 D:/Project/searxng/src/searxng.rs（ureq 同步版 → reqwest async）：
//! `GET {base}/search?q=<urlencoded>&format=json&pageno=<N>`，5s 超时。
//! SearXNG 是局域网直连实例，**不走 GSEARCH_PROXY / 系统代理**。

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::types::SearchResult;

/// SearXNG 对缺失字段会输出 `null` 而非省略键；`null` → Default（serde default 不兜 null）。
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

/// GET `{base}/search?q=<encoded>&format=json&pageno=<page>`；5s 超时、明确禁代理。
/// 成功返回映射后的 SearchResult（snippet ← SearXNG content 字段）。
pub async fn search(base_url: &str, query: &str, page: u32) -> Result<Vec<SearchResult>> {
    let url = format!(
        "{}/search?q={}&format=json&pageno={}",
        base_url.trim_end_matches('/'),
        crate::search::urlencode(query),
        page,
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        // 局域网直连：禁掉环境代理（HTTP_PROXY 等），配了 GSEARCH_PROXY 也绝不经代理
        .no_proxy()
        .build()
        .context("构建 HTTP 客户端失败")?;
    let resp = client
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
}
