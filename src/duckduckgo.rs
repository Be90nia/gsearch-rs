//! af9：DuckDuckGo html 直连第二源（SearXNG → DDG → Google 回退链的第二层）。
//! `POST https://html.duckduckgo.com/html/` 表单（q [+ df]），第一页免 vqd token（9 月预研结论），
//! 纯 HTTP 不起浏览器。与 SearXNG 局域网 no_proxy 相反：公网出口走系统/透明代理
//!（curl 原生感知 HTTPS_PROXY 等环境代理）。

use std::collections::HashSet;

use anyhow::{Context, Result};
use scraper::{Html, Selector};

use crate::search::SearchConfig;
use crate::types::SearchResult;

/// 5s 与 SearXNG 同预算。khn 实测（2026-10-09，同刻同隧道三变量对照）：
/// curl(schannel)+浏览器 UA=200；curl 默认 UA 与 gsearch/* UA=202；reqwest 浏览器 UA+
/// native-tls+http1 仍 202——DDG anomaly 风控同时认 UA 串与 TLS/头指纹（头序 reqwest
/// 不可控，seanmonstar/reqwest#265 not_planned）。传输层切 curl 子进程旁路，
/// UA 伪装浏览器串（亮明 UA 的教训只适用于自托管 SearXNG）。
static DDG_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// 首页收集（第二源不翻页：翻页需 vqd token，超出首页的量由 Google 链兜底）。
/// Ok(零条) = 正常响应但解析不出条目（零结果/风控页/改版），调用方按「DDG 未命中」继续回退链。
pub async fn collect(cfg: &SearchConfig) -> Result<Vec<SearchResult>> {
    let body = form_body(&cfg.query, cfg.recency);
    let text = curl_post("https://html.duckduckgo.com/html/", &body).await?;
    Ok(parse_limited(&text, cfg.limit))
}

/// DDG html POST（curl 子进程）：环境代理感知与 reqwest 语义对齐（HTTPS_PROXY 系环境
/// 变量 curl 原生读取，透明代理场景零配置）。
async fn curl_post(url: &str, body: &str) -> Result<String> {
    let args = curl_args(url, body);
    let out = tokio::process::Command::new("curl")
        .args(&args)
        .output()
        .await
        .context("curl 不可用（DDG 直连依赖系统 curl，Windows 10+ 自带）")?;
    classify_curl_result(out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
}

/// curl 参数构造（抽纯函数供单测锁）；不走 shell，body 含 & 无需引号。
/// 不显式传 Content-Type：--data-raw 自动生成时 curl 把它排在 Content-Length 之后
///（khn 实测：-H 显式提供会把它提前，DDG anomaly 风控按头序识别直接 202）。
fn curl_args(url: &str, body: &str) -> Vec<String> {
    vec![
        "-sS".into(),
        "--max-time".into(),
        "5".into(),
        "-A".into(),
        DDG_UA.into(),
        "--data-raw".into(),
        body.into(),
        url.into(),
    ]
}

/// khn：curl 子进程结果分类（纯函数，单测锁）。Err 一律走调用方回退链——超时
///（curl 28）与传输失败同语义，不区分重试策略；challenge 页不得静默算零结果
///（知乎限流页同款教训：服务端风控空 ≠ 时序空）。
fn classify_curl_result(exit: Option<i32>, body: String) -> Result<String> {
    match exit {
        Some(0) => {
            if is_challenge(&body) {
                anyhow::bail!("DDG 风控 challenge 页（anomaly），出口 IP/TLS 指纹被识别");
            }
            Ok(body)
        }
        Some(28) => anyhow::bail!("curl exit 28：DDG 直连超时（5s 预算）"),
        Some(c) => anyhow::bail!("curl exit {c}：DDG 直连传输失败"),
        None => anyhow::bail!("curl 被信号终止"),
    }
}

/// DDG anomaly challenge 页特征（202 与拦截页共用文案）。
fn is_challenge(body: &str) -> bool {
    body.contains("anomaly") || body.contains(" unfortunately, bots use DuckDuckGo too")
}

/// 表单体手写 urlencoded（q [+ df]）：不为此引 form/percent-encoding 依赖（Cargo.toml 冷构建红线）。
/// df 与 Google qdr 同字母（d/w/m/y）。
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
    // khn 打回：DDG html 返回序即相关性序——按最终序给递减分（首条 = n，末条 = 1），
    // 与 SearXNG score 同消费语义（高分更相关，agent 可按分筛序），不另造打分体系。
    let n = out.len();
    for (i, r) in out.iter_mut().enumerate() {
        r.score = Some((n - i) as f64);
    }
    out
}

/// khn 打回：解析 + limit 截断（DDG 首页一次抓全，截断在解析后、返回前）。
fn parse_limited(html: &str, limit: usize) -> Vec<SearchResult> {
    let mut results = parse(html);
    results.truncate(limit);
    results
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
        // score 占位 None，parse 尾部统一按返回序打分（去重后才知最终名次）
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
    use super::{parse, parse_limited, percent_decode, real_url};

    /// 容器路径：uddg 跳转链解码、snippet 配对、score 递减分、domain_class 装配。
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
        // khn 打回：返回序即相关性序 → 递减分（首条 n、末条 1），同 SearXNG score 消费语义
        assert_eq!(rs[0].score, Some(2.0));
        assert_eq!(rs[1].score, Some(1.0));
        assert_eq!(rs[1].url, "https://docs.rs/tokio", "绝对 URL 原样");
    }

    /// khn 打回：limit 截断在解析后、返回前——DDG 首页一次抓全，`--limit 3` 不得多给。
    #[test]
    fn parse_limited_caps_results() {
        let html = r#"<!doctype html><html><body>
            <div class="result">
              <h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.example%2F1&rut=1">One</a></h2>
              <a class="result__snippet" href="/s1">s1</a>
            </div>
            <div class="result">
              <h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.example%2F2&rut=2">Two</a></h2>
              <a class="result__snippet" href="/s2">s2</a>
            </div>
            <div class="result">
              <h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.example%2F3&rut=3">Three</a></h2>
              <a class="result__snippet" href="/s3">s3</a>
            </div>
        </body></html>"#;
        let capped = parse_limited(html, 2);
        assert_eq!(capped.len(), 2, "limit 截断在解析后返回前");
        // 截断保前缀：截断后的首条仍是全量相关序第一（fixture 3 条 → 首条 3.0）
        assert_eq!(capped[0].score, Some(3.0));
        assert_eq!(capped[1].score, Some(2.0));
        assert_eq!(capped[0].url, "https://a.example/1");
        let uncapped = parse_limited(html, 100);
        assert_eq!(uncapped.len(), 3);
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

    /// khn：curl 退出码分类——成功透传 body、challenge 页显式报错（不静默算零结果）、
    /// 超时（28）与传输失败同走 Err（调用方回退链语义不变）、信号终止兜底。
    #[test]
    fn classify_curl_result_locks_fallback_semantics() {
        use super::classify_curl_result;
        assert!(classify_curl_result(Some(0), "<html>results</html>".into()).is_ok());
        let err = classify_curl_result(Some(0), "anomaly detection page".into()).unwrap_err();
        assert!(err.to_string().contains("challenge"), "{err}");
        let err = classify_curl_result(Some(0), " unfortunately, bots use DuckDuckGo too".into()).unwrap_err();
        assert!(err.to_string().contains("challenge"), "{err}");
        let err = classify_curl_result(Some(28), String::new()).unwrap_err();
        assert!(err.to_string().contains("超时"), "{err}");
        let err = classify_curl_result(Some(7), String::new()).unwrap_err();
        assert!(err.to_string().contains("exit 7"), "{err}");
        assert!(classify_curl_result(None, String::new()).is_err());
    }

    /// khn：curl 参数构造——UA/超时/body/URL 就位（--max-time 5 与旧 reqwest 同预算）。
    /// 不显式传 Content-Type：--data-raw 自动生成的头序（Content-Type 在 Content-Length
    /// 后）才是 curl 原生指纹，显式 -H 会把它提前被 DDG 风控识别（202）。
    #[test]
    fn curl_args_carry_ua_timeout_body_url() {
        let args = super::curl_args("https://html.duckduckgo.com/html/", "q=rust");
        let at = |name: &str| args.iter().position(|a| a == name).expect(name);
        assert_eq!(args[at("-A") + 1], super::DDG_UA);
        assert_eq!(args[at("--max-time") + 1], "5");
        assert_eq!(args[at("--data-raw") + 1], "q=rust");
        assert_eq!(args.last().unwrap(), "https://html.duckduckgo.com/html/");
        assert!(
            !args.iter().any(|a| a.starts_with("Content-Type")),
            "显式 Content-Type 会破坏 curl 原生头序被 DDG 识别"
        );
    }
}
