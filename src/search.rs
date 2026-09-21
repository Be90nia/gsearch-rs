//! Google SERP 翻页状态机（对照 plsearch main.py:236-343 `_search`）
//!
//! 单页签复用（每次 goto）、`&start=N` 翻页、URL 去重、空页自然终止。
//! M3：CAPTCHA 双模式——首页撞码切有头轮询（≤120s）等人解；后页撞码返回部分结果。
//! 行为真值逐条对照 Python 版（plsearch main.py:236-343 / config.py:18-19, 112-139）。

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use anyhow::{Result, anyhow};
use chromiumoxide::Page;
use chromiumoxide::browser::Browser;

use crate::browser;
use crate::parse::parse_serp;
use crate::types::SearchResult;

/// 常量对照 plsearch config.py / main.py：RESULTS_PER_PAGE=10 / MAX_PAGES=10 / CAPTCHA_WAIT_TIMEOUT_SECONDS=120
const RESULTS_PER_PAGE: usize = 10;
const MAX_PAGES: usize = 10;
const PAGE_TIMEOUT_SECS: u64 = 30;
/// CAPTCHA 判定串（plsearch config.py:18-19 CAPTCHA_FORM_ID / RECAPTCHA_ID）
pub const CAPTCHA_TIMEOUT_SECS: u64 = 120;
const CAPTCHA_POLL_SECS: u64 = 1;

pub struct SearchConfig {
    pub query: String,
    pub limit: usize,
}

pub fn is_captcha(html: &str) -> bool {
    // 只认验证页形态：captcha-form 容器 + 三条文案。
    // "g-recaptcha" 子串在普通 SERP 脚本预加载里也出现，不能独立作判定
    // （否则每次搜索都被误判成撞码，弹有头窗；审计挂账的 is_captcha false positives）。
    html.contains("captcha-form")
        || unusual_traffic(html)
}

pub fn unusual_traffic(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("unusual traffic")
        || lower.contains("our systems have detected")
        || lower.contains("/sorry/index")
}

/// M15：搜索结果 + 是否撞码过验证，Agent 拿一次 JSON 就能知道全部状态。
#[derive(Debug)]
pub enum SearchOutcome {
    /// 正常出结果，可能包含“本轮经过人工 CAPTCHA 验证”的标记。
    Results {
        results: Vec<SearchResult>,
        captcha_solved: bool,
        /// M16：结果来源（"searxng" / "google"），供 MetaOutput.provider 按实际来源填值。
        provider: &'static str,
    },
    /// 等人解超时，无结果。
    CaptchaTimeout,
}

/// 翻页搜索直到凑满 limit / 空页 / 打满 MAX_PAGES；中途首页撞 CAPTCHA 切有头轮询等人解。
pub async fn run_search(
    browser: &mut Browser,
    cfg: SearchConfig,
    h_slot: &mut Option<tokio::task::JoinHandle<()>>,
    human_solved: Arc<AtomicBool>,
) -> Result<SearchOutcome> {
    // M16/M17：SearXNG 纯 HTTP 源在此分流（shell 会话入口）。顶层 search 命令已在
    // cmd_search 惰性启动时先跑过 try_searxng 并直接调 run_search_on_page——此处不再
    // 重复查询（否则 SearXNG 双失败会打两遍回退 warn、白跑两轮 HTTP）。
    if let Some(outcome) = try_searxng(&cfg).await {
        return Ok(outcome);
    }
    let page = browser.new_page("about:blank").await?;
    run_search_on_page(browser, cfg, page, h_slot, human_solved).await
}

pub async fn run_search_on_page(
    browser: &mut Browser,
    cfg: SearchConfig,
    mut page: Page,
    h_slot: &mut Option<tokio::task::JoinHandle<()>>,
    human_solved: Arc<AtomicBool>,
) -> Result<SearchOutcome> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut collected: Vec<SearchResult> = Vec::new();
    let mut captcha_solved = false;
    'pages: for page_idx in 0..MAX_PAGES {
        if collected.len() >= cfg.limit {
            break;
        }
        let url = format!(
            "https://www.google.com/search?q={}&start={}",
            urlencode(&cfg.query),
            page_idx * RESULTS_PER_PAGE
        );
        // H1：每个 ? 早返回前先 close page（句柄已死的 swap 后旧 page close 吞错），
        // 防 load / new_page / poll_until_solved 任一失败时 page 在 headless/headed 浏览器上残留。
        let mut content = match load(&page, &url).await {
            Ok(c) => c,
            Err(e) => {
                let _ = page.close().await;
                return Err(e);
            }
        };

        if is_captcha(&content) {
            if !collected.is_empty() {
                // 后页撞码：给部分结果，不打扰人（plsearch main.py:272-280）
                tracing::warn!(
                    "第 {} 页遇 CAPTCHA，返回 {} 条已有结果",
                    page_idx + 1,
                    collected.len()
                );
                break;
            }
            // 首页撞码：切有头轮询等人解（plsearch main.py:339-343 reveal_for_captcha + _search wait_for_captcha=True）
            eprintln!("Google 对无头浏览器有独立风控（豁免 cookie 约 3 小时且对无头无效），弹出窗口验证后本会话将切回无头继续；高频场景建议用 gsearch shell");
            if let Err(e) = swap_to_headed(browser, h_slot).await {
                let _ = page.close().await;
                return Err(anyhow!("swap_to_headed 失败: {e}"));
            }
            let page2 = match browser.new_page("about:blank").await {
                Ok(p) => p,
                Err(e) => {
                    let _ = page.close().await;
                    return Err(anyhow!("new_page 失败: {e}"));
                }
            };

            if let Err(e) = page2.goto(&url).await {
                let _ = page.close().await;
                let _ = page2.close().await;
                return Err(anyhow!("goto {url} 失败: {e}"));
            }
            content = match poll_until_solved(&page2, CAPTCHA_TIMEOUT_SECS, human_solved.clone()).await {
                Ok(Some(html)) => {
                    captcha_solved = true;
                    html
                }
                Ok(None) => {
                    // M15：超时不是错误而是 SearchOutcome，让调用方按 mode 决定
                    // JSON 输出 captcha_timeout 而非 anyhow 退出。
                    // I3：超时后保留 headed 浏览器给调用方回收（main / shell 都按 CaptchaTimeout 走
                    // graceful_close 整段关）；这里关掉原始 page 句柄（swap_to_headed 后已死，close 吞错）。
                    let _ = page.close().await;
                    let _ = page2.close().await;
                    return Ok(SearchOutcome::CaptchaTimeout);
                }
                Err(e) => {
                    let _ = page.close().await;
                    let _ = page2.close().await;
                    return Err(e);
                }
            };
            // 解码完成即切回无头：GAEX 豁免 cookie 对 headed 有效（headed 直接过验证），
            // 切回后本次命令/会话内后续页不再弹窗。swap 换了 Browser 实例，page2 与旧
            // 句柄一起失效——重新 new_page 顶回循环变量再继续翻页。
            if let Err(e) = browser::swap_to_headless(browser, h_slot).await {
                let _ = page.close().await;
                let _ = page2.close().await;
                return Err(e);
            }
            page = match browser.new_page("about:blank").await {
                Ok(p) => p,
                Err(e) => {
                    let _ = page2.close().await;
                    return Err(anyhow!("解码后 new_page 失败: {e}"));
                }
            };
            let _ = page2.close().await;
            let results = parse_serp(&content);
            if results.is_empty() {
                tracing::info!("第 {} 页（解码后）无结果，终止翻页", page_idx + 1);
                break;
            }
            for r in results {
                if seen.insert(r.url.clone()) {
                    collected.push(r);
                }
            }
            continue 'pages;
        }

        let results = parse_serp(&content);
        if results.is_empty() {
            tracing::info!("第 {} 页无结果，终止翻页", page_idx + 1);
            break;
        }
        // Google 在不同 start 偏移间会重排，页内也会重复；按 URL 去重
        for r in results {
            if seen.insert(r.url.clone()) {
                collected.push(r);
            }
        }
    }
    collected.truncate(cfg.limit);
    Ok(SearchOutcome::Results {
        results: collected,
        captcha_solved,
        provider: "google",
    })
}

/// M16：SearXNG 分流。未配置 searxng_url → None（直接走 Google）。
/// 翻页：pageno 从 1 递增，凑满 limit / 空页 / 打满 MAX_PAGES 收口。
/// 每页先试 format=json；Err 或零结果时降级抓 HTML 结果页（同页重试一次），
/// 都空才按空页语义收口。provider 语义不变（"searxng" 涵盖 json/html 两种来源）。
///   * 成功凑到结果 → Some(Results, provider="searxng")
///   * json+html 双失败/双空：已有部分结果自然终止，否则 warn 一行后 None
///     （回退 Google 直爬）。
pub async fn try_searxng(cfg: &SearchConfig) -> Option<SearchOutcome> {
    let base = crate::config::load().searxng_url.clone()?;
    match searxng_collect(&base, cfg).await {
        Ok(results) => Some(SearchOutcome::Results {
            results,
            captcha_solved: false,
            provider: "searxng",
        }),
        Err(reason) => {
            warn_searxng_fallback(&base, &reason);
            None
        }
    }
}

/// SearXNG-only 收集内核（单查询与 batch 共用）：翻页凑 limit。
/// Err(原因) = 未能凑到任何结果；中途双源失败但已有部分结果时有多少用多少。
/// 回退 warn 不在此打——单查询措辞是"已回退 Google 直爬"，batch 无回退，由调用方决定。
async fn searxng_collect(base: &str, cfg: &SearchConfig) -> Result<Vec<SearchResult>, String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut collected: Vec<SearchResult> = Vec::new();
    let mut degraded = false; // 降级提示整次查询只打一行
    for page_no in 1..=MAX_PAGES as u32 {
        let page = match crate::searxng::search(base, &cfg.query, page_no).await {
            Ok(results) if !results.is_empty() => Ok(results),
            Ok(_) => degrade_html(base, &cfg.query, page_no, &mut degraded, "查询无结果").await,
            Err(e) => degrade_html(base, &cfg.query, page_no, &mut degraded, &format!("{e:#}")).await,
        };
        match page {
            Ok(results) => {
                for r in results {
                    // 与 Google 路径同一去重语义（不同页可能重复）
                    if seen.insert(r.url.clone()) {
                        collected.push(r);
                    }
                }
                if collected.len() >= cfg.limit {
                    break;
                }
            }
            // json+html 均无结果：已有部分结果视为自然终止（有多少用多少）
            Err(_) if !collected.is_empty() => break,
            Err(reason) => return Err(reason),
        }
    }
    if collected.is_empty() {
        // 循环正常结束仍为空：各页结果全被去重掉（首查即空走的是上面的 Err 分支）
        return Err("各页结果去重后为空".into());
    }
    collected.truncate(cfg.limit);
    Ok(collected)
}

/// batch 单条（issue gsearch-rs-doh）：SearXNG-only，禁浏览器回退——浏览器单例不可并发，
/// 这是刻意边界（Google 回退链仅单查询模式走）。Err 文本直接作为该条目的 error message。
async fn batch_one(cfg: &SearchConfig) -> Result<SearchOutcome, String> {
    let base = crate::config::load().searxng_url.clone().ok_or_else(|| {
        "SearXNG 未配置（gsearch.json 缺 searxng_url）；batch 模式禁浏览器回退，请改用单查询".to_string()
    })?;
    searxng_collect(&base, cfg)
        .await
        .map(|results| SearchOutcome::Results {
            results,
            captcha_solved: false,
            provider: "searxng",
        })
        .map_err(|reason| format!("SearXNG 查询失败（{reason}）；batch 模式禁浏览器回退"))
}

/// batch 入口：多查询并发走 SearXNG，单条失败不阻塞其他条目，返回顺序与输入一致。
/// ponytail: 并发数未设上限——共享 reqwest 客户端 + 局域网实例，查询量到几十条再加分批。
pub async fn run_batch(
    queries: &[String],
    limit: usize,
) -> Vec<(String, Result<SearchOutcome, String>)> {
    let futs = queries.iter().map(|q| async {
        let cfg = SearchConfig {
            query: q.clone(),
            limit,
        };
        (q.clone(), batch_one(&cfg).await)
    });
    futures::future::join_all(futs).await
}

/// json 不可用（Err / 零结果）时的单页降级：抓 HTML 结果页。
/// Ok(results) = html 命中（整次查询首次命中打一行降级提示）；
/// Err(原因) = html 亦空/失败（原因含 json 层 + html 层；回退措辞由调用方按单/批语义打）。
async fn degrade_html(
    base: &str,
    query: &str,
    page_no: u32,
    degraded: &mut bool,
    reason: &str,
) -> Result<Vec<SearchResult>, String> {
    match crate::searxng::search_html(base, query, page_no).await {
        Ok(results) if !results.is_empty() => {
            if !*degraded {
                *degraded = true;
                eprintln!("SearXNG JSON 不可用（{reason}），已降级 HTML 结果页");
            }
            Ok(results)
        }
        Ok(_) => Err(format!("{reason}，HTML 结果页亦无结果")),
        Err(e) => Err(format!("{e:#}")),
    }
}

/// SearXNG 失败回退的一行 warn（stderr）。错误链含 403 时追加 format=json 提示。
fn warn_searxng_fallback(base: &str, err: &str) {
    let mut msg = format!("SearXNG {base} 查询失败（{err}），已回退 Google 直爬");
    if err.contains("403") {
        msg.push_str("；提示：检查 SearXNG 实例已启用 JSON format（settings.yml 的 search.formats 加 json）");
    }
    eprintln!("{msg}");
}

/// close 当前 browser 并同 profile 起重起有头实例。
/// 等价 plsearch AppContext.reveal_for_captcha（main.py:133-137）。
async fn swap_to_headed(browser: &mut Browser, h_slot: &mut Option<tokio::task::JoinHandle<()>>) -> Result<()> {
    browser::swap_to_headed(browser, h_slot).await
}

/// 轮询 page content 直到非 captcha 或超时。等价 plsearch wait_until_captcha_solved（config.py:117-139）。
/// 瞬态错误（页面 mid-navigation / 连接抖动）debug 跳过，deadline 到才返回 None。
/// 返回 Some(html) 表示解完；None 表示超时；Err 表示浏览器已被手关（连接断开）。
async fn poll_until_solved(
    page: &Page,
    timeout_secs: u64,
    human_solved: Arc<AtomicBool>,
) -> Result<Option<String>> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut since_last_log = 0u64;
    loop {
        // URL 先判：解完验证后页面导航到 /search?q=...，比 content 判定可靠
        // （结果页 HTML 可能残留 "recaptcha" 子串导致 is_captcha 永真 → 假超时）。
        if let Ok(Some(u)) = page.url().await
            && u.contains("/search?")
            && !u.contains("/sorry/")
        {
            tracing::info!("CAPTCHA 已解（页面已导航到结果页）");
            if let Ok(html) = page.content().await {
                return Ok(Some(html));
            }
        }
        // human_solved=true = 人工按 Enter 确认已解完（poll 循环下一 tick 即可 break）
        if human_solved.load(Ordering::Relaxed) {
            tracing::info!("人工确认 CAPTCHA 已解（stdin Enter）");
            if let Ok(html) = page.content().await {
                return Ok(Some(html));
            }
        }
        match page.content().await {
            Ok(html) if !is_captcha(&html) => return Ok(Some(html)),
            Ok(_) => {} // 仍是 captcha，继续等
            Err(e) => {
                tracing::debug!("轮询 CAPTCHA 状态时 page.content() 失败: {e}");
            }
        }
        if Instant::now() >= deadline {
            tracing::warn!("CAPTCHA 亲解超时");
            return Ok(None);
        }
        // 心跳：每 15s 报一次剩余时间（120s 默认下用户会看到 8 条进度）。
        if since_last_log == 0 || since_last_log.is_multiple_of(15) {
            let remaining = (deadline - Instant::now()).as_secs();
            tracing::info!("CAPTCHA 轮询中（还剩约 {remaining}s/{timeout_secs}s，解完请按 Enter 加速通过）");
        }
        since_last_log += CAPTCHA_POLL_SECS;
        tokio::time::sleep(Duration::from_secs(CAPTCHA_POLL_SECS)).await;
    }
}

/// goto + 取 HTML；30s 超时兜底，网络挂起不至于永远卡住。
async fn load(page: &Page, url: &str) -> Result<String> {
    tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), async {
        page.goto(url).await?;
        page.content().await
    })
    .await
    .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
    .map_err(|e| anyhow!("加载 {url} 失败: {e}"))
}

/// ponytail: 查询串就几十字节，手写 10 行不引 percent_encoding crate。
/// M16：searxng.rs 复用同一 URL 编码（q 参数语义相同），故 pub(crate)。
pub(crate) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{is_captcha, unusual_traffic};

    /// 结果页脚本残留 "recaptcha" 字样不得误判为验证页（真机假超时的根因）。
    #[test]
    fn serp_with_recaptcha_script_is_not_captcha() {
        let serp = r#"<html><body><script src="https://www.gstatic.com/recaptcha/releases/x.js"></script><a href="https://example.com"><h3>Title</h3></a></body></html>"#;
        assert!(!is_captcha(serp));
    }

    /// M18 回归测试
    #[test]
    fn serp_with_grecaptcha_script_is_not_captcha() {
        let serp = r#"<html><body><script src="https://www.google.com/recaptcha/api.js"></script><div class="g-recaptcha"></div><h3>Real result</h3></html>"#;
        assert!(!is_captcha(serp));
    }

    #[test]
    fn captcha_prompts_are_early_detected() {
        assert!(is_captcha("<p>Unusual traffic from your computer network</p>"));
        assert!(is_captcha("Our systems have detected unusual traffic"));
        assert!(is_captcha("<title>Google</title><a href=\"/sorry/index?x=1\">"));
        assert!(!is_captcha("ordinary search results"));
    }

    #[test]
    fn unusual_traffic_is_case_insensitive() {
        assert!(unusual_traffic("Our Systems Have Detected traffic"));
        assert!(unusual_traffic("UNUSUAL TRAFFIC"));
    }
    #[test]
    fn urlencode_handles_fuzz_inputs() {
        use super::urlencode;
        assert_eq!(urlencode("a&b=c?d"), "a%26b%3Dc%3Fd");
        assert_eq!(urlencode("中文 query"), "%E4%B8%AD%E6%96%87%20query");
        assert_eq!(urlencode("🦀 rust"), "%F0%9F%A6%80%20rust");
        assert_eq!(urlencode("abc-_.~"), "abc-_.~");
        let big = "a".repeat(2000);
        assert_eq!(urlencode(&big).len(), 2000);
    }
}

