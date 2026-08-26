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

use crate::browser::{self, spawn_handler};
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
    html.contains("captcha-form")
        || html.contains("recaptcha")
        || unusual_traffic(html)
}

pub fn unusual_traffic(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("unusual traffic")
        || lower.contains("our systems have detected")
        || lower.contains("/sorry/index")
}

/// 翻页搜索直到凑满 limit / 空页 / 打满 MAX_PAGES；中途首页撞 CAPTCHA 切有头轮询等人解。
pub async fn run_search(browser: &mut Browser, cfg: SearchConfig) -> Result<Vec<SearchResult>> {
    let page = browser.new_page("about:blank").await?;
    run_search_on_page(browser, cfg, page).await
}

pub async fn run_search_on_page(
    browser: &mut Browser,
    cfg: SearchConfig,
    page: Page,
) -> Result<Vec<SearchResult>> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut collected: Vec<SearchResult> = Vec::new();
    tracing::info!("搜索 {:?}（limit={}）", cfg.query, cfg.limit);
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
            tracing::info!("检测到 CAPTCHA，起有头窗等人解（≤{}s）", CAPTCHA_TIMEOUT_SECS);
            swap_to_headed(browser).await?;
            let page2 = browser.new_page("about:blank").await?;
            page2.goto(&url).await.map_err(|e| anyhow!("goto {url} 失败: {e}"))?;
            content = match poll_until_solved(&page2, CAPTCHA_TIMEOUT_SECS).await? {
                Some(html) => html,
                None => {
                    return Err(anyhow!(
                        "CAPTCHA 亲解超时（{}s）",
                        CAPTCHA_TIMEOUT_SECS
                    ));
                }
            };
            // ponytail: 解完后不切回 headless。CLI 一次性 search 无后续轮询，
            // 切回意味着再 close + launch 一次同 profile，多一次 profile 锁竞态风险 + 丢 page 状态。
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
    collected.truncate(cfg.limit); // 不要 [..limit] 裸切片：len < limit 时会 panic
    Ok(collected)
}

/// close 当前 browser 并同 profile 起重起有头实例。
/// 等价 plsearch AppContext.reveal_for_captcha（main.py:133-137）。
async fn swap_to_headed(browser: &mut Browser) -> Result<()> {
    browser.close().await.map_err(|e| anyhow!("close 当前 browser 失败: {e}"))?;
    let (new_browser, handler) = browser::launch(false).await?;
    *browser = new_browser;
    spawn_handler(handler);
    Ok(())
}

/// 轮询 page content 直到非 captcha 或超时。等价 plsearch wait_until_captcha_solved（config.py:117-139）。
/// 瞬态错误（页面 mid-navigation / 连接抖动）debug 跳过，deadline 到才返回 None。
/// 返回 Some(html) 表示解完；None 表示超时；Err 表示浏览器已被手关（连接断开）。
async fn poll_until_solved(page: &Page, timeout_secs: u64) -> Result<Option<String>> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
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
#[cfg(test)]
mod tests {
    use super::{is_captcha, unusual_traffic};

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
}

/// 查询串 percent-encode，等价 Python `urllib.parse.quote`（字母数字与 -_.~/ 直通，其余 %XX）。
/// 查询串就几十字节，手写 10 行，不引 percent_encoding crate。
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
