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

/// 时间窗（--recency）：SearXNG 走 `time_range=<as_str>`，Google 走 `tbs=qdr:<qdr_letter>`。
/// 对齐 tavily/exa/brave 等搜索 API 的时间过滤参数（agent 检索高频需求）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recency {
    Day,
    Week,
    Month,
    Year,
}

impl Recency {
    /// SearXNG `time_range` 参数值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }

    /// Google SERP `tbs=qdr:<字母>` 的窗口字母（公开 SERP 约定）。
    pub fn qdr_letter(self) -> &'static str {
        match self {
            Self::Day => "d",
            Self::Week => "w",
            Self::Month => "m",
            Self::Year => "y",
        }
    }
}

pub struct SearchConfig {
    pub query: String,
    pub limit: usize,
    /// Some = 只看该时间窗内的结果；None = 不过滤（请求 URL 与旧版逐字节一致）。
    pub recency: Option<Recency>,
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

/// 搜索结果 + 是否撞码过验证，Agent 拿一次 JSON 就能知道全部状态。
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
    match try_searxng(&cfg).await {
        SearxngAttempt::Results(outcome) => return Ok(outcome),
        // zc6：熔断（SearXNG 故障 + Google:443 预检不通）。shell 没有结构化 status 可标，
        // 报错文案带同款诊断行（o1p：真因透传），不再空耗浏览器 30s 超时。
        SearxngAttempt::CircuitBroken(reason) => return Err(anyhow!("{}", circuit_diag(&reason))),
        // o1p：源健康零结果（兜底全空 + Google 不可达）——shell 无结构化信封，报可读诊断
        SearxngAttempt::HealthyEmpty => return Err(anyhow!("{NO_RESULTS_MSG}")),
        SearxngAttempt::NotConfigured | SearxngAttempt::FallbackGoogle(_) => {}
    }
    let page = browser::open_page(browser).await?;
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
        let url = serp_url(&cfg.query, page_idx * RESULTS_PER_PAGE, cfg.recency);
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
            if let Err(e) = browser::swap_to_headed(browser, h_slot).await {
                let _ = page.close().await;
                return Err(anyhow!("swap_to_headed 失败: {e}"));
            }
            let page2 = match browser::open_page(browser).await {
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
            page = match browser::open_page(browser).await {
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

/// SearXNG 分流。未配置 searxng_url → NotConfigured（直接走 Google）。
/// 翻页：pageno 从 1 递增，凑满 limit / 空页 / 打满 MAX_PAGES 收口。
/// 每页先试 format=json；Err 或零结果时降级抓 HTML 结果页（同页重试一次），
/// 都空才按空页语义收口。provider 语义不变（"searxng" 涵盖 json/html 两种来源）。
///
/// json+html 双空/双失败不再无条件回退——先 TCP 预检 google:443（1.5s）。
/// IP 正常时 FallbackGoogle（老行为不变）；被墙/断网时段 CircuitBroken 熔断，
/// 不再空耗 30s 等浏览器超时。
///
/// 回退链插层为 SearXNG → DDG html → Google——SearXNG 失败先试 DDG 纯 HTTP
///（免浏览器），DDG 命中时 Results 携 provider="duckduckgo"；batch 不走此层（仍 SearXNG-only）。
///
/// 失败分类为 SearxFail{HealthyEmpty, SourceError}——源健康零结果与源故障分立，
/// FallbackGoogle/CircuitBroken 携带 SearXNG 真因（HTTP 错误码透传进诊断行）。
#[derive(Debug)]
pub enum SearxngAttempt {
    /// 未配置 searxng_url——用户没要 SearXNG，直接 Google 直爬（不预检）。
    NotConfigured,
    /// SearXNG 出结果（provider=searxng）。
    Results(SearchOutcome),
    /// SearXNG 空/失败，但 Google 预检通过——按老行为回退（回退 warn 已打）。
    /// Some(真因) = SearXNG 故障（Google 回退也空时据此标 searxng_degraded）；
    /// None = SearXNG 健康但零结果（Google 回退也空时按 filtered_empty/no_results 定态）。
    FallbackGoogle(Option<String>),
    /// 熔断：SearXNG 故障 + Google 预检不通——回退必然空耗，直接返回（诊断行已打 stderr）。
    /// 携带 SearXNG 真因（o1p：HTTP 错误码透传）。
    CircuitBroken(String),
    /// o1p：SearXNG 健康（HTTP 200）但零结果，DDG 亦空且 Google 预检不通——
    /// 不起浏览器直接定态（调用方按 recency 判 filtered_empty / no_results）。
    HealthyEmpty,
}

/// 熔断诊断行（stderr，一行原则；元审计豁免未来任何静默策略）。
pub const SEARXNG_CIRCUIT_MSG: &str =
    "SearXNG 零结果已熔断（基础设施降级，非查询无资料）；建议换短 query/跑 doctor/直接 fetch 已知源";

/// recency 过滤后零结果（SearXNG 源健康）——与基础设施降级显式分立。
pub const FILTERED_EMPTY_MSG: &str =
    "recency 过滤后零结果（SearXNG 源健康，非基础设施故障）；建议去掉 --recency、换时间窗或换词重试";

/// 查询无果（SearXNG 源健康）——非熔断非过滤空，换词重试即可。
pub const NO_RESULTS_MSG: &str =
    "查询无结果（SearXNG 源健康，非基础设施故障）；建议换词或缩短查询重试";

/// SearXNG 失败分类——零结果语义三态的判定依据。
/// HealthyEmpty = json+html 双层 HTTP 200 零结果（源健康，查询真无果）；
/// SourceError = HTTP 状态错/网络错/解析失败（携带真因链，如 HTTP 400）。
#[derive(Debug)]
pub enum SearxFail {
    HealthyEmpty,
    SourceError(String),
}

impl SearxFail {
    /// 真因文本（SourceError）；HealthyEmpty 无故障，给可读描述供回退 warn 消费。
    pub fn describe(&self) -> String {
        match self {
            Self::HealthyEmpty => "查询无结果（源健康）".to_string(),
            Self::SourceError(s) => s.clone(),
        }
    }

    /// o1p 三态映射（单查与 batch 共用）：源健康零结果按 recency 拆
    /// 「过滤后空」（filtered_empty）与「查询无果」（no_results），均非 degraded。
    pub fn empty_status(recency: Option<Recency>) -> (crate::types::RunStatus, &'static str) {
        match recency {
            Some(_) => (crate::types::RunStatus::FilteredEmpty, FILTERED_EMPTY_MSG),
            None => (crate::types::RunStatus::NoResults, NO_RESULTS_MSG),
        }
    }
}

/// 熔断诊断行组装——真因非空时透传追加（run.message 与 stderr 诊断行同源）。
pub fn circuit_diag(reason: &str) -> String {
    if reason.is_empty() {
        SEARXNG_CIRCUIT_MSG.to_string()
    } else {
        format!("{SEARXNG_CIRCUIT_MSG}；真因: {reason}")
    }
}

pub async fn try_searxng(cfg: &SearchConfig) -> SearxngAttempt {
    let Some(base) = crate::config::load().searxng_url.clone() else {
        return SearxngAttempt::NotConfigured;
    };
    match searxng_collect(&base, cfg).await {
        Ok(results) => SearxngAttempt::Results(SearchOutcome::Results {
            results,
            captcha_solved: false,
            provider: "searxng",
        }),
        Err(fail) => {
            let reason = fail.describe();
            // af9：第二源插层——SearXNG 挂/零结果先试 DDG html（纯 HTTP 免浏览器），
            // 命中则以 provider=duckduckgo 直接返回；仍空才走 Google 预检回退/熔断老链。
            match crate::duckduckgo::collect(cfg).await {
                Ok(results) if !results.is_empty() => {
                    eprintln!("SearXNG {base} 查询失败（{reason}），已回退 DuckDuckGo html 直连");
                    return SearxngAttempt::Results(SearchOutcome::Results {
                        results,
                        captcha_solved: false,
                        provider: "duckduckgo",
                    });
                }
                Ok(_) => eprintln!("DDG html 直连零结果，继续 Google 回退链"),
                Err(e) => eprintln!("DDG html 直连失败（{e:#}），继续 Google 回退链"),
            }
            if google_fallback_precheck().await {
                warn_searxng_fallback(&base, &reason);
                // o1p：None = 源健康零结果（HealthyEmpty 无故障真因），供回退空时三态定态
                let fault = matches!(fail, SearxFail::SourceError(_)).then_some(reason);
                SearxngAttempt::FallbackGoogle(fault)
            } else if matches!(fail, SearxFail::HealthyEmpty) {
                // o1p：源健康 + 兜底全空 + Google 不可达——不是熔断，按源健康定态；
                // filtered_empty/no_results 的判定与诊断行由调用方按 recency 打（此处无 recency 上下文）。
                SearxngAttempt::HealthyEmpty
            } else {
                eprintln!("{}", circuit_diag(&reason));
                SearxngAttempt::CircuitBroken(reason)
            }
        }
    }
}

/// Google 回退预检——TCP 连 www.google.com:443，1.5s 封顶（与 doctor 第 5 项同款探测）。
/// ponytail: 直连探测不感知 GSEARCH_PROXY（显式代理场景可能假熔断）；本部署走透明代理直出，无此形态。
async fn google_fallback_precheck() -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(1500),
            tokio::net::TcpStream::connect(("www.google.com", 443)),
        )
        .await,
        Ok(Ok(_))
    )
}

/// SearXNG-only 收集内核（单查询与 batch 共用）：翻页凑 limit。
/// Err(原因) = 未能凑到任何结果；中途双源失败但已有部分结果时有多少用多少。
/// 失败分类 SearxFail——json+html 双层 HTTP 200 零结果 = HealthyEmpty（源健康），
/// HTTP 状态/网络/解析失败 = SourceError（真因链含 json 层 + html 层）。
/// 回退 warn 不在此打——单查询措辞是"已回退 Google 直爬"，batch 无回退，由调用方决定。
async fn searxng_collect(base: &str, cfg: &SearchConfig) -> Result<Vec<SearchResult>, SearxFail> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut collected: Vec<SearchResult> = Vec::new();
    let mut degraded = false; // 降级提示整次查询只打一行
    for page_no in 1..=MAX_PAGES as u32 {
        let page = match crate::searxng::search(base, &cfg.query, page_no, cfg.recency).await {
            Ok(results) if !results.is_empty() => Ok(results),
            // json 层 HTTP 200 但零结果：html 降级定最终分类（html 200 空 = 源健康）
            Ok(_) => degrade_html(base, &cfg.query, page_no, cfg.recency, &mut degraded, "查询无结果").await,
            Err(e) => {
                let jr = format!("{e:#}");
                // o1p：json 层故障真因进链——html 层也故障时拼全链，不吞 400 等状态码
                degrade_html(base, &cfg.query, page_no, cfg.recency, &mut degraded, &jr)
                    .await
                    .map_err(|f| match f {
                        SearxFail::HealthyEmpty => SearxFail::HealthyEmpty,
                        SearxFail::SourceError(h) => SearxFail::SourceError(format!("{jr}；HTML 层: {h}")),
                    })
            }
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
        // 循环正常结束仍为空：各页结果全被去重掉——源给过结果，非故障（o1p 归源健康）
        return Err(SearxFail::HealthyEmpty);
    }
    collected.truncate(cfg.limit);
    Ok(collected)
}

/// batch 单条（issue gsearch-rs-doh）：SearXNG-only，禁浏览器回退——浏览器单例不可并发，
/// 这是刻意边界（Google 回退链仅单查询模式走）。o1p：失败按 SearxFail 分类，
/// SourceError 文本仍以「SearXNG 查询失败（…）」格式进该条目 message（存量契约不破）。
async fn batch_one(cfg: &SearchConfig) -> Result<SearchOutcome, SearxFail> {
    let base = crate::config::load().searxng_url.clone().ok_or_else(|| {
        SearxFail::SourceError(
            "SearXNG 未配置（gsearch.json 缺 searxng_url）；batch 模式禁浏览器回退，请改用单查询".to_string(),
        )
    })?;
    searxng_collect(&base, cfg).await.map(|results| SearchOutcome::Results {
        results,
        captcha_solved: false,
        provider: "searxng",
    })
}

/// batch 入口：多查询并发走 SearXNG，单条失败不阻塞其他条目，返回顺序与输入一致。
/// ponytail: 并发硬封顶 `min(queries.len(), 32)`（SearXNG 默认限速 + botdetection
/// 在 ~30 并发触发 403；硬顶放这里，分批/限速是后续优化方向）。
pub async fn run_batch(
    queries: &[String],
    limit: usize,
    recency: Option<Recency>,
) -> Vec<(String, Result<SearchOutcome, SearxFail>)> {
    use futures::stream::StreamExt;
    let cap = queries.len().min(32);
    // stream::iter(...).map(|q| async {...}).buffer_unordered(cap)
    //   -> 流式拉取，cap 控制并发在飞数量；map 同步产出保证输入序（输出顺序无关性靠 slots 重排）。
    let mut futs = futures::stream::iter(queries.iter().enumerate())
        .map(|(pos, q)| async move {
            let cfg = SearchConfig { query: q.clone(), limit, recency };
            (pos, q.clone(), batch_one(&cfg).await)
        })
        .buffer_unordered(cap);
    let mut slots: Vec<Option<(String, Result<SearchOutcome, SearxFail>)>> =
        (0..queries.len()).map(|_| None).collect();
    while let Some((pos, q, r)) = futs.next().await {
        slots[pos] = Some((q, r));
    }
    slots.into_iter().map(Option::unwrap).collect()
}

/// json 不可用（Err / 零结果）时的单页降级：抓 HTML 结果页。
/// Ok(results) = html 命中（整次查询首次命中打一行降级提示）；
/// Err 携分类——html 亦 HTTP 200 零结果 = HealthyEmpty（源健康，查询真无果）；
/// html 层故障 = SourceError（真因由调用方拼进全链）。
async fn degrade_html(
    base: &str,
    query: &str,
    page_no: u32,
    recency: Option<Recency>,
    degraded: &mut bool,
    reason: &str,
) -> Result<Vec<SearchResult>, SearxFail> {
    match crate::searxng::search_html(base, query, page_no, recency).await {
        Ok(results) if !results.is_empty() => {
            if !*degraded {
                *degraded = true;
                eprintln!("SearXNG JSON 不可用（{reason}），已降级 HTML 结果页");
            }
            Ok(results)
        }
        // html 层 HTTP 200 零结果：实例健康、查询真无果（o1p 分类以最终层为准）
        Ok(_) => Err(SearxFail::HealthyEmpty),
        Err(e) => Err(SearxFail::SourceError(format!("{e:#}"))),
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

/// similar 内核——从 URL 提取 title 关键词派生查询，SearXNG 单查（超采样 limit×3，
/// 8..=15 条），title 词重合 ×2 + 同域 ×1 加权 stable 重排（同分保留 SearXNG 相关序）。
/// **启发式派生查询，非 exa 神经 findSimilar**（README 预期管理）。返回（重排后条目，派生查询串）。
pub async fn similar(
    src_url: &str,
    limit: usize,
) -> Result<(Vec<crate::types::SimilarHit>, String), String> {
    let base = crate::config::load()
        .searxng_url
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "SearXNG 未配置（gsearch.json 缺 searxng_url 或未设 GSEARCH_SEARXNG_URL）；similar 走 SearXNG 派生查询"
                .to_string()
        })?;
    let (src_host, keywords) = split_site_keys(src_url);
    if src_host.is_empty() {
        return Err(format!("无法从 {src_url} 解析 host（需 http(s):// 或 // 前缀的绝对 URL）"));
    }
    // 纯域名 URL 无路径关键词 → 退化 site: 查询（同域相关页语义）
    let query = if keywords.is_empty() { format!("site:{src_host}") } else { keywords.join(" ") };
    let cfg = SearchConfig { query: query.clone(), limit: (limit * 3).clamp(8, 15), recency: None };
    // o1p：SearxFail 分类转 String（similar 无三态信封，错误文本语义不变）
    let hits = searxng_collect(&base, &cfg)
        .await
        .map_err(|f| match f {
            SearxFail::HealthyEmpty => "查询无结果（源健康）".to_string(),
            SearxFail::SourceError(s) => s,
        })?;
    let mut scored: Vec<(i32, crate::types::SimilarHit)> = hits
        .into_iter()
        .map(|hit| {
            let same = host_of(&hit.url) == src_host;
            let overlap = title_overlap(&hit.title, &keywords);
            let mut tags: Vec<String> = Vec::new();
            if !overlap.is_empty() {
                tags.push(format!("title={}", overlap.join(",")));
            }
            if same {
                tags.push(format!("site={src_host}"));
            }
            let score = overlap.len() as i32 * 2 + i32::from(same);
            let similarity = if tags.is_empty() {
                "none（仅派生查询命中，无 title/同域启发）".to_string()
            } else {
                tags.join("; ")
            };
            (score, crate::types::SimilarHit { hit, similarity })
        })
        .collect();
    scored.sort_by_key(|(s, _)| -(*s)); // stable：同分保留 SearXNG 原始相关序
    scored.truncate(limit);
    Ok((scored.into_iter().map(|(_, h)| h).collect(), query))
}

/// qbw：similar 入参形态闸——拒明显非 URL（含空白 / 无 scheme 又无点分 host 与 path 结构）。
/// 宽松放行 `docs.rs/serde` 这类无 scheme 的 host/path 形态（与 split_site_keys 既有解析对齐）。
pub fn looks_like_url(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.chars().any(char::is_whitespace) {
        return false;
    }
    match s.split_once("://") {
        Some((scheme, rest)) => !scheme.is_empty() && !rest.is_empty(),
        None => s.contains('.') || s.contains('/'),
    }
}

/// host 提取：去 scheme（含 protocol-relative //）、剥 www.、截 path 前；小写。解析不出返回空串。
fn host_of(url: &str) -> String {
    let rest = url.split_once("//").map(|(_, r)| r).unwrap_or(url);
    let bare = rest.split(['/', '?', '#']).next().unwrap_or("");
    bare.strip_prefix("www.").unwrap_or(bare).trim_end_matches('.').to_ascii_lowercase()
}

/// URL → (host, title 关键词)：path 末段去扩展名后分词。纯域名 → 空关键词表。
fn split_site_keys(url: &str) -> (String, Vec<String>) {
    let rest = url.split_once("//").map(|(_, r)| r).unwrap_or(url);
    let (host_part, path) = rest.split_once('/').unwrap_or((rest, ""));
    let lowered = host_part.to_ascii_lowercase();
    let host = lowered.strip_prefix("www.").unwrap_or(&lowered).to_string();
    let last = path.trim_end_matches('/').rsplit('/').find(|s| !s.is_empty());
    let keywords = last
        .map(|seg| tokenize(seg.split('.').next().unwrap_or(seg)))
        .unwrap_or_default();
    (host, keywords)
}

/// 非字母数字切段，保留 ≥2 字符且含字母的段（serde → [serde]；Rust_(lang) → [rust, lang]；2024 → 丢）。
fn tokenize(seg: &str) -> Vec<String> {
    seg.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2 && w.chars().any(char::is_alphabetic))
        .map(str::to_lowercase)
        .collect()
}

/// title 分词后与派生关键词的交集（保关键词序，小写比较）。
fn title_overlap(title: &str, keywords: &[String]) -> Vec<String> {
    let words = tokenize(title);
    keywords.iter().filter(|k| words.contains(k)).cloned().collect()
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

/// Google SERP URL：`q → [tbs=qdr:<w>] → start`。recency=None 时与旧版逐字节一致。
fn serp_url(query: &str, start: usize, recency: Option<Recency>) -> String {
    let tbs = recency
        .map(|r| format!("&tbs=qdr:{}", r.qdr_letter()))
        .unwrap_or_default();
    format!(
        "https://www.google.com/search?q={}{tbs}&start={}",
        urlencode(query),
        start
    )
}

/// ponytail: 查询串就几十字节，手写 10 行不引 percent_encoding crate。
/// searxng.rs 复用同一 URL 编码（q 参数语义相同），故 pub(crate)。
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
    use super::{is_captcha, unusual_traffic, SearxFail, SEARXNG_CIRCUIT_MSG, FILTERED_EMPTY_MSG, NO_RESULTS_MSG};

    /// o1p：源健康零结果三态映射——recency 过滤后空与查询无果分立，均非 degraded。
    #[test]
    fn searx_fail_empty_status_maps_recency() {
        use super::{circuit_diag, Recency};
        let (filtered, fmsg) = SearxFail::empty_status(Some(Recency::Week));
        assert_eq!(filtered, crate::types::RunStatus::FilteredEmpty);
        assert_eq!(fmsg, FILTERED_EMPTY_MSG);
        assert!(fmsg.contains("recency") && fmsg.contains("源健康"));

        let (none, nmsg) = SearxFail::empty_status(None);
        assert_eq!(none, crate::types::RunStatus::NoResults);
        assert_eq!(nmsg, NO_RESULTS_MSG);
        assert!(!nmsg.contains("recency"));

        // 三态与 degraded 互斥：源健康空 ≠ 基础设施降级
        assert_ne!(filtered, crate::types::RunStatus::SearxngDegraded);
        assert_ne!(none, crate::types::RunStatus::SearxngDegraded);

        // 熔断诊断行：真因透传（HTTP 错误码进诊断行），无真因回退原文
        assert_eq!(circuit_diag(""), SEARXNG_CIRCUIT_MSG);
        let with_reason = circuit_diag("SearXNG 返回错误状态: HTTP 400 Bad Request");
        assert!(with_reason.starts_with(SEARXNG_CIRCUIT_MSG));
        assert!(with_reason.contains("400"));
    }

    /// o1p：SearxFail 分类描述——HealthyEmpty 可读化，SourceError 原样透传真因链。
    #[test]
    fn searx_fail_describe_keeps_reason_chain() {
        assert!(SearxFail::HealthyEmpty.describe().contains("源健康"));
        let chain = "SearXNG 返回错误状态: HTTP 400 Bad Request；HTML 层: 请求 SearXNG HTML 失败";
        assert_eq!(SearxFail::SourceError(chain.to_string()).describe(), chain);
    }

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

    use super::{serp_url, Recency};

    /// --recency 未传：Google URL 与改动前逐字节一致（验收：无 tbs 段）。
    #[test]
    fn serp_url_without_recency_is_unchanged() {
        assert_eq!(
            serp_url("rust async", 0, None),
            "https://www.google.com/search?q=rust%20async&start=0"
        );
    }

    /// tbs=qdr 映射：day/week/month/year → d/w/m/y；段序 q → tbs → start。
    #[test]
    fn serp_url_appends_tbs_qdr_letter() {
        assert_eq!(
            serp_url("rust", 10, Some(Recency::Week)),
            "https://www.google.com/search?q=rust&tbs=qdr:w&start=10"
        );
        assert_eq!(Recency::Day.qdr_letter(), "d");
        assert_eq!(Recency::Month.qdr_letter(), "m");
        assert_eq!(Recency::Year.qdr_letter(), "y");
    }

    /// SearXNG time_range 参数值映射（searxng.rs 拼 URL 时消费）。
    #[test]
    fn recency_as_str_matches_searxng_time_range_values() {
        assert_eq!(Recency::Day.as_str(), "day");
        assert_eq!(Recency::Week.as_str(), "week");
        assert_eq!(Recency::Month.as_str(), "month");
        assert_eq!(Recency::Year.as_str(), "year");
    }

    /// e7c：URL → (host, 关键词) 分词。路径末段去扩展名、非字母数字切分、纯数字/单字符段丢弃。
    #[test]
    fn split_site_keys_extracts_host_and_keywords() {
        use super::split_site_keys;
        assert_eq!(
            split_site_keys("https://docs.rs/serde"),
            ("docs.rs".into(), vec!["serde".to_string()])
        );
        assert_eq!(
            split_site_keys("https://en.wikipedia.org/wiki/Rust_(programming_language)"),
            ("en.wikipedia.org".into(), vec!["rust".to_string(), "programming".to_string(), "language".to_string()])
        );
        assert_eq!(
            split_site_keys("https://example.com/posts/2024/07.html"),
            ("example.com".into(), Vec::<String>::new()),
            "末段 07 纯数字被丢弃 → 关键词空（不回溯猜前段，similar 退化 site: 查询）"
        );
    }

    /// 纯域名（无 path 关键词）与 www 前缀、大写归一。
    #[test]
    fn split_site_keys_pure_domain_and_www() {
        use super::split_site_keys;
        let (host, kws) = split_site_keys("https://WWW.Example.COM/");
        assert_eq!(host, "example.com");
        assert!(kws.is_empty(), "纯域名无关键词 → similar 退化 site: 查询");
    }

    #[test]
    fn title_overlap_and_host_of() {
        use super::{host_of, title_overlap};
        let kws = vec!["serde".to_string(), "rust".to_string()];
        assert_eq!(title_overlap("Serde — Rust serialization", &kws), vec!["serde", "rust"]);
        assert_eq!(title_overlap("unrelated page", &kws), Vec::<String>::new());
        assert_eq!(host_of("https://docs.rs/serde?q=1"), "docs.rs");
        assert_eq!(host_of("https://www.Example.com/x"), "example.com", "www 剥离与同域比较归一一致");
    }

    /// qbw：验收点名用例——not-a-url 拒、带 scheme 过、无 scheme 的 host/path 形态
    /// 向后兼容放行（既有 split_site_keys 接受），含空白句子拒。
    #[test]
    fn looks_like_url_gates_similar_input() {
        use super::looks_like_url;
        assert!(!looks_like_url("not-a-url"));
        assert!(looks_like_url("https://tokio.rs"));
        assert!(looks_like_url("docs.rs/serde"));
        assert!(!looks_like_url("not a url"));
        assert!(!looks_like_url(""));
        assert!(!looks_like_url("https://"));
        assert!(looks_like_url("https://docs.rs/serde"));
    }
}

