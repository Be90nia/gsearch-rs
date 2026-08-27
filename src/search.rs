//! Google SERP 翻页状态机（对照 plsearch main.py:236-343 `_search`）
//!
//! 单页签复用（每次 goto）、`&start=N` 翻页、URL 去重、空页自然终止。
//! M3：CAPTCHA 双模式——首页撞码切有头轮询（≤120s）等人解；后页撞码返回部分结果。
//! 行为真值逐条对照 Python 版（plsearch main.py:236-343 / config.py:18-19, 112-139）。

use std::collections::HashSet;
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
    // 注意：裸 "recaptcha" 子串在正常结果页脚本里也出现（审计挂账的误报源），
    // 只认验证页形态：captcha-form 容器 / g-recaptcha 部件 / 三条文案。
    html.contains("captcha-form")
        || html.contains("g-recaptcha")
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
    },
    /// 等人解超时，无结果。
    CaptchaTimeout,
}

/// 翻页搜索直到凑满 limit / 空页 / 打满 MAX_PAGES；中途首页撞 CAPTCHA 切有头轮询等人解。
pub async fn run_search(browser: &mut Browser, cfg: SearchConfig, h_slot: &mut Option<tokio::task::JoinHandle<()>>) -> Result<SearchOutcome> {
    let page = browser.new_page("about:blank").await?;
    run_search_on_page(browser, cfg, page, h_slot).await
}

pub async fn run_search_on_page(
    browser: &mut Browser,
    cfg: SearchConfig,
    page: Page,
    h_slot: &mut Option<tokio::task::JoinHandle<()>>,
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
        let mut content = load(&page, &url).await?;

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
            swap_to_headed(browser, h_slot).await?;
            let page2 = browser.new_page("about:blank").await?;
            page2.goto(&url).await.map_err(|e| anyhow!("goto {url} 失败: {e}"))?;
            content = match poll_until_solved(&page2, CAPTCHA_TIMEOUT_SECS).await? {
                Some(html) => {
                    captcha_solved = true;
                    html
                }
                None => {
                    // M15：超时不是错误而是 SearchOutcome，让调用方按 mode 决定
                    // JSON 输出 captcha_timeout 而非 anyhow 退出。
                    return Ok(SearchOutcome::CaptchaTimeout);
                }
            };
            // 若以后接多轮 search，复用 plsearch main.py 的 hide_after_captcha() 模式即可。
            // 后续页沿用同一 page（不切回也不重 open）→ continue 翻页
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
    })
}

/// close 当前 browser 并同 profile 起重起有头实例。
/// 等价 plsearch AppContext.reveal_for_captcha（main.py:133-137）。
async fn swap_to_headed(browser: &mut Browser, h_slot: &mut Option<tokio::task::JoinHandle<()>>) -> Result<()> {
    browser::swap_to_headed(browser, h_slot).await
}

/// 轮询 page content 直到非 captcha 或超时。等价 plsearch wait_until_captcha_solved（config.py:117-139）。
/// 瞬态错误（页面 mid-navigation / 连接抖动）debug 跳过，deadline 到才返回 None。
/// 返回 Some(html) 表示解完；None 表示超时；Err 表示浏览器已被手关（连接断开）。
async fn poll_until_solved(page: &Page, timeout_secs: u64) -> Result<Option<String>> {
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
            tracing::info!("CAPTCHA 轮询中（还剩约 {remaining}s/{timeout_secs}s，解完请关窗）");
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
fn urlencode(s: &str) -> String {
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

