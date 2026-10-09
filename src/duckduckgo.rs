//! af9：DuckDuckGo html 直连第二源（SearXNG → DDG → Google 回退链的第二层）。
//! `POST https://html.duckduckgo.com/html/` 表单（q [+ df]），第一页免 vqd token（9 月预研结论），
//! 纯 HTTP 不起浏览器。与 SearXNG 局域网 no_proxy 相反：公网出口走系统/透明代理
//!（reqwest 默认感知 HTTP_PROXY 等环境代理）。

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result};
use scraper::{Html, Selector};

use crate::search::SearchConfig;
use crate::types::SearchResult;

/// 5s 与 SearXNG 同预算；亮明 UA（部分端点对 reqwest 默认 UA 有超时/风控假象，9-18 实证）。
/// use_native_tls：rustls 的 TLS 指纹（JA3）被 DDG anomaly 风控识别直接回 202 challenge
///（2026-10-09 实测：schannel/curl 同 IP 同头 200，rustls 202）——DDG 是唯一指纹敏感源。
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .use_native_tls()
        .http1_only()
        .timeout(Duration::from_secs(5))
        .user_agent(concat!("gsearch/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("构建 DDG 共享 HTTP 客户端失败")
});

/// 首页收集（第二源不翻页：翻页需 vqd token，超出首页的量由 Google 链兜底）。
/// Ok(零条) = 正常响应但解析不出条目（零结果/风控页/改版），调用方按「DDG 未命中」继续回退链。
pub async fn collect(cfg: &SearchConfig) -> Result<Vec<SearchResult>> {
    let text = CLIENT
        .post("https://html.duckduckgo.com/html/")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form_body(&cfg.query, cfg.recency))
        .send()
        .await
        .context("请求 DDG html 失败")?
        .error_for_status()
        .context("DDG html 返回错误状态")?
        .text()
        .await
        .context("读取 DDG html 响应失败")?;
    // 202 = DDG anomaly 风控 challenge 页（2xx 不被 error_for_status 拦）。
    // 显式报错防静默：解析出 0 条会误导为「查询无资料」，实为出口 IP/指纹被风控。
    // （知乎限流页同款教训：服务端风控空 ≠ 时序空，必须可辨识）
    if text.contains("anomaly") || text.contains(" unfortunately, bots use DuckDuckGo too") {
        anyhow::bail!("DDG 风控 challenge 页（anomaly），出口 IP/TLS 指纹被识别");
    }
    Ok(parse(&text))
}

/// 表单体手写 urlencoded（q [+ df]）：reqwest .form() 在 default-features=false 下不可用，
/// 不为此开 feature（Cargo.toml 冷构建红线）。df 与 Google qdr 同字母（d/w/m/y）。
fn form_body(query: &str, recency: Option<crate::search::Recency>) -> String {
    let mut body = format!("q={}", crate::search::urlencode(query));
    if let Some(r) = recency {
        body.push_str("&df=");
        body.push_str(r.qdr_letter());
    }
    body
}

/// 解析 DDG html 结果页（纯函数，单测直接喂片段）。首选 `div.result` 容器
/// （标题 a.result__a / 摘要 a.result__snippet）；容器缺失（改版）时退化为两选择器
/// 文档序流式配对（同 searxng extract_walk 手法）。
fn parse(html: &str) -> Vec<SearchResult> {
    let doc = Html::parse_document(html);
    let container = Selector::parse("div.result").expect("静态选择器必然合法");
    let link = Selector::parse("a.result__a").expect("静态选择器必然合法");
    let snippet = Selector::parse("a.result__snippet").expect("静态选择器必然合法");
    let mut out: Vec<SearchResult> = Vec::new();
    if doc.select(&container).next().is_some() {
        for res in doc.select(&container) {
            let Some(a) = res.select(&link).next() else {
                continue;
            };
            let snippet = res.select(&snippet).next().map(|el| crate::parse::text_of(&el)).unwrap_or_default();
            push_result(&mut out, a.value().attr("href").unwrap_or_default(), crate::parse::text_of(&a), snippet);
        }
    } else {
        let walk = Selector::parse("a.result__a, a.result__snippet").expect("静态选择器必然合法");
        let mut pending: Option<(String, String)> = None; // (href, title) 等摘要配对
        for el in doc.select(&walk) {
            if el.value().has_class("result__a", scraper::CaseSensitivity::CaseSensitive) {
                if let Some((href, title)) = pending.take() {
                    push_result(&mut out, &href, title, String::new());
                }
                pending = Some((
                    el.value().attr("href").unwrap_or_default().to_string(),
                    crate::parse::text_of(&el),
                ));
            } else if let Some((href, title)) = pending.take() {
                push_result(&mut out, &href, title, crate::parse::text_of(&el));
            }
        }
        if let Some((href, title)) = pending {
            push_result(&mut out, &href, title, String::new());
        }
    }
    // URL 去重保首次（同 parse.rs / searxng.rs；search.rs 的 seen 是第二道）
    let mut seen = HashSet::new();
    out.retain(|r| !r.url.is_empty() && seen.insert(r.url.clone()));
    out
}

fn push_result(out: &mut Vec<SearchResult>, href: &str, title: String, snippet: String) {
    let url = real_url(href);
    if url.is_empty() {
        return; // 广告跳转链 / 空 uddg：不算结果
    }
    out.push(SearchResult {
        title,
        url: url.clone(),
        snippet,
        // DDG html 无相关性分 → score 键缺席（契约同 Google/HTML 降级源）
        score: None,
        domain_class: crate::util::domain_class(&url),
    });
}

/// DDG 跳转链解真实 URL：`//duckduckgo.com/l/?uddg=<percent-encoded>&rut=..` → uddg 解码；
/// protocol-relative（`//host/..`）补 https:；其余原样。广告链（/y.js）返回空串（调用方跳过）。
fn real_url(href: &str) -> String {
    if href.contains("/y.js") {
        return String::new();
    }
    if let Some(pos) = href.find("uddg=") {
        let rest = &href[pos + 5..];
        let end = rest.find('&').unwrap_or(rest.len());
        return percent_decode(&rest[..end]);
    }
    if let Some(rest) = href.strip_prefix("//") {
        return format!("https://{rest}");
    }
    href.to_string()
}

/// 最小 percent-decode：%XX 十六进制还原（uddg 值为标准 percent-encoding，'+' 不转空格）。
/// ponytail: 与 search::urlencode 配对的手写小函数，不引 percent_encoding crate。
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{parse, percent_decode, real_url};

    /// 容器路径：uddg 跳转链解码、snippet 配对、score 缺席、domain_class 装配。
    #[test]
    fn parse_container_decodes_uddg_and_snippet() {
        let html = r#"<!doctype html><html><body>
            <div class="result results_links web-result">
              <h2 class="result__title"><a rel="nofollow" class="result__a"
                href="//duckduckgo.com/l/?uddg=https%3A%2F%2Ftokio.rs%2F&rut=abc">Tokio</a></h2>
              <a class="result__snippet" href="//duckduckgo.com/l/?uddg=x&rut=y">An async runtime for Rust</a>
            </div>
            <div class="result">
              <h2><a class="result__a" href="https://docs.rs/tokio">docs.rs</a></h2>
              <a class="result__snippet" href="/x">API documentation</a>
            </div>
        </body></html>"#;
        let rs = parse(html);
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0].url, "https://tokio.rs/");
        assert_eq!(rs[0].title, "Tokio");
        assert_eq!(rs[0].snippet, "An async runtime for Rust");
        assert_eq!(rs[0].score, None, "DDG 无相关性分，score 键缺席");
        assert_eq!(rs[1].url, "https://docs.rs/tokio", "绝对 URL 原样");
    }

    /// 无 div.result 容器（改版）：a.result__a / a.result__snippet 文档序流式配对兜底。
    #[test]
    fn parse_walk_fallback_without_container() {
        let html = r#"<!doctype html><html><body>
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&rut=1">A</a>
            <a class="result__snippet" href="/s1">snippet A</a>
            <a class="result__a" href="https://example.com/b">B</a>
        </body></html>"#;
        let rs = parse(html);
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0].url, "https://example.com/a");
        assert_eq!(rs[0].snippet, "snippet A");
        assert_eq!(rs[1].snippet, "", "末条无摘要 → 空串");
    }

    /// 广告跳转链（/y.js）不算结果。
    #[test]
    fn parse_skips_ad_redirects() {
        let html = r#"<div class="result"><a class="result__a"
            href="//duckduckgo.com/y.js?ad_domain=spam.example&uddg=https%3A%2F%2Fspam.example">SPAM</a></div>"#;
        assert!(parse(html).is_empty());
    }

    #[test]
    fn real_url_variants() {
        assert_eq!(
            real_url("//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.io%2Fx%3Fy%3D1&rut=z"),
            "https://a.io/x?y=1"
        );
        assert_eq!(real_url("//cdn.example.com/img.png"), "https://cdn.example.com/img.png");
        assert_eq!(real_url("https://plain.example/"), "https://plain.example/");
        assert_eq!(real_url("//duckduckgo.com/l/?uddg=&rut=1"), "", "空 uddg → 空 → 跳过");
    }

    /// df 时间过滤参数拼接（与 Google qdr 同字母）。
    #[test]
    fn form_body_appends_df_for_recency() {
        use super::form_body;
        use crate::search::Recency;
        assert_eq!(form_body("rust async", None), "q=rust%20async");
        assert_eq!(form_body("rust", Some(Recency::Day)), "q=rust&df=d");
        assert_eq!(form_body("a&b", Some(Recency::Week)), "q=a%26b&df=w");
    }

    #[test]
    fn percent_decode_basics() {
        assert_eq!(percent_decode("%3A%2F%2F"), "://");
        assert_eq!(percent_decode("a%26b%3Dc"), "a&b=c");
        assert_eq!(percent_decode("%E4%B8%AD%E6%96%87"), "中文");
        assert_eq!(percent_decode("a+b"), "a+b", "'+' 不转空格（uddg 是标准 percent-encoding）");
        assert_eq!(percent_decode("%zz"), "%zz", "非法十六进制原样保留");
        assert_eq!(percent_decode("%4"), "%4", "截断序列原样保留");
    }
}
