//! M4 搜索结果后处理：`--open N` / `--read N` / `--dl N`（PLAN §3.4 / §3.5）。
//! 复用 cmd_search 已启动的同一 browser 实例（同 profile → 同 cookie），不另起 Chrome。
//!
//! M9：`--read N` 默认输出 AdaptiveRead（按文章结构自适应三段：目录/摘要/段落索引），
//! `--full` 拿纯 innerText 5000 字（兜底），`--json` 拿结构化 JSON，`--from K` 摘要偏移，
//! `--headings-only` 仅目录。

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow};
use chromiumoxide::browser::Browser;

use crate::general::{LOGIN_POLL_SECS, browser_alive, login_poll_decision};
use gsearch::search::is_captcha;
use gsearch::skeleton::{extract_adaptive, format_adaptive, format_headings_only, format_json};
use gsearch::types::SearchResult;
use gsearch::util::{b64_decode, filename_from_url};

const READ_FULL_MAX_CHARS: usize = 5000;
const PAGE_TIMEOUT_SECS: u64 = 30;
const DL_LARGE_BYTES: usize = 50_000_000;
/// M17 登录墙:等人工登录的总超时(顶层命令无人守窗,不能像 `login` 命令那样不限时)
const LOGIN_WALL_TIMEOUT_SECS: u64 = 180;
/// 登录墙 title 判定要求的「正文极短」阈值(innerText 字符数;登录页只有表单文案)
const LOGIN_WALL_SHORT_BODY: usize = 400;

/// `--{flag} N` 下标校验：1-based；0 或超出结果数报错（含结果数为 0 的情况）
fn pick<'a>(results: &'a [SearchResult], n: usize, flag: &str) -> Result<&'a str> {
    if n == 0 || n > results.len() {
        return Err(anyhow!("--{flag} {n} 越界（结果数 {}）", results.len()));
    }
    Ok(&results[n - 1].url)
}

/// `--open N`：默认浏览器开窗。
pub fn open(results: &[SearchResult], n: usize) -> Result<()> {
    let url = pick(results, n, "open")?;
    // 三平台原生开窗命令；cfg! 让三个分支都在全平台编译（无 cfg 死代码）。
    let spawned = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd").args(["/c", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = spawned {
        tracing::warn!("打开默认浏览器失败（不影响输出）: {e}");
    }
    Ok(())
}

/// `--read N` 选项集。M9：默认 AdaptiveRead，`--full/--json/--headings-only/--from K` 互斥选择。
#[derive(Debug, Clone, Default)]
pub struct ReadOpts {
    pub full: bool,
    pub json: bool,
    pub headings_only: bool,
    pub from: usize,
}

/// `--read N`：M9 默认走 AdaptiveRead（按文章结构自适应）。opts 见 ReadOpts。
pub async fn read(
    browser: &mut Browser,
    h_slot: &mut Option<tokio::task::JoinHandle<()>>,
    results: &[SearchResult],
    n: usize,
    opts: &ReadOpts,
) -> Result<String> {
    let url = pick(results, n, "read")?;
    let page = open_page(browser, h_slot, url).await?;
    let title = eval_string_retry(&page, "document.title").await;
    let html = content_retry(&page).await;
    let mut read = extract_adaptive(&html);
    read.url = url.to_string();
    read.title = title;

    let out = if opts.json {
        format_json(&read)
    } else if opts.headings_only {
        format_headings_only(&read)
    } else {
        format_adaptive(&read, opts.from)
    };
    println!("{out}");
    Ok(out)
}

/// `--read N --full` 兜底：纯 innerText 5000 字。
pub async fn read_full(
    browser: &mut Browser,
    h_slot: &mut Option<tokio::task::JoinHandle<()>>,
    results: &[SearchResult],
    n: usize,
) -> Result<String> {
    let url = pick(results, n, "read")?;
    let page = open_page(browser, h_slot, url).await?;
    read_full_inner(&page, url).await
}

/// 共享 innerText 5000 字截断 + 打印。
async fn read_full_inner(page: &chromiumoxide::Page, url: &str) -> Result<String> {
    let txt = eval_string_retry(page, "document.body.innerText").await;
    let txt: String = txt.chars().take(READ_FULL_MAX_CHARS).collect();
    println!("=== {url} ===\n{txt}");
    Ok(txt)
}

/// M4 健壮性修复：打开结果页并等 DOM 稳定——read / read_full 共用的 goto 前置。
/// 知乎等站 goto resolve 后仍会内部跳转（登录墙/风控重定向），旧 JS context 被销毁，
/// 紧跟的 content()/evaluate 会撞 CDP -32000 "Cannot find context"（此前被 unwrap_or_default
/// 吞成空串 → 正文为空）。修法对照 shell.rs settle_after_click 先例：goto 后轮询
/// document.readyState 到 complete（约 10s 上限），evaluate 类错误（含 -32000）吞掉继续轮询。
///
/// M17 登录墙（Part 2）：检测到登录墙 → eprintln 提示 → swap_to_headed 弹有头窗
/// 等人工登录（复用 general::login_poll_decision 决策，180s 超时）→ 登录成功重抓正文。
/// cookie 随 profile 落盘，下次无感。人关窗 = browser 已死 → 重起 headless 再试一次；
/// 重抓仍撞墙/CAPTCHA 报错退出（限一次登录机会，防循环弹窗）。
async fn open_page(
    browser: &mut Browser,
    h_slot: &mut Option<tokio::task::JoinHandle<()>>,
    url: &str,
) -> Result<chromiumoxide::Page> {
    let page = goto_page(browser, url).await?;
    if is_captcha(&content_retry(&page).await) {
        return Err(anyhow!("{url} 遇 CAPTCHA，M4 后处理不支持人解，请重试或手动浏览器打开"));
    }
    if !login_wall_hit_page(&page).await {
        return Ok(page);
    }

    eprintln!(
        "检测到登录墙（{url}），弹出有头窗口请登录，登录后自动继续（最长 {LOGIN_WALL_TIMEOUT_SECS}s；cookie 落 profile，下次无感）"
    );
    gsearch::browser::swap_to_headed(browser, h_slot).await?;
    wait_login(browser, url).await?;
    // 登录完成（URL 变化）或用户关窗。关窗路径 browser 已死：重起 headless 重抓。
    if !browser_alive(browser).await {
        let (b, handler) = gsearch::browser::launch(true).await?;
        *h_slot = Some(gsearch::browser::spawn_handler(handler));
        *browser = b;
    }
    let page = goto_page(browser, url).await?;
    if is_captcha(&content_retry(&page).await) {
        return Err(anyhow!("{url} 遇 CAPTCHA，请重试或手动浏览器打开"));
    }
    if login_wall_hit_page(&page).await {
        return Err(anyhow!("登录后重抓 {url} 仍遇登录墙（登录未生效？）；可先 `gsearch login <url>` 手动完成登录"));
    }
    Ok(page)
}

/// new_page + goto + 等 DOM 稳定（原 open_page 前半，登录墙重抓路径复用）。
async fn goto_page(browser: &Browser, url: &str) -> Result<chromiumoxide::Page> {
    let page = browser.new_page("about:blank").await?;
    tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
        .await
        .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
        .map_err(|e| anyhow!("goto {url} 失败（浏览器被手关？）: {e}"))?;
    wait_dom_complete(&page, 50).await; // 50×200ms ≈ 10s
    Ok(page)
}

/// 登录墙页内判定：URL 跳转特征 + title/正文特征（见 login_wall_hit）。
async fn login_wall_hit_page(page: &chromiumoxide::Page) -> bool {
    let url = page.url().await.ok().flatten().unwrap_or_default();
    let title = eval_string_retry(page, "document.title").await;
    let body = eval_string_retry(page, "document.body ? document.body.innerText : ''").await;
    login_wall_hit(&url, &title, &body, body.chars().count())
}

/// M17 登录墙判定（契约特征，纯函数可单测）：
/// 1. 最终 URL 路径段精确命中 signin/signup/login 等登录跳转（知乎 /signin、GitHub /login）；
/// 2. 或 title/DOM 命中「登录/sign in/log in」且正文极短——SPA 型墙 URL 不变的兜底，
///    也覆盖知乎 IP 风控页（正文是含「登录后…反馈」的短 JSON 错误体；带登录态访问即解，
///    所以弹窗登录是正确动作而非误伤）。
/// ponytail: 路径段精确匹配而非全文 contains——防「文章标题/路径含 login」误伤正文页；
/// 正文长度门槛 <400 字挡住「正文提到登录」的正常文章。不做站点特征库（Non-goal）。
fn login_wall_hit(url: &str, title: &str, body: &str, body_chars: usize) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let path_hit = path
        .split('/')
        .any(|seg| matches!(seg, "login" | "signin" | "signup" | "log-in" | "sign-in" | "accounts"));
    if path_hit {
        return true;
    }
    let t = title.to_lowercase();
    let b = body.to_lowercase();
    let text_hit = ["登录", "sign in", "log in"].iter().any(|p| t.contains(p) || b.contains(p));
    text_hit && body_chars < LOGIN_WALL_SHORT_BODY
}

/// 决策函数与 general::cmd_login 同源（URL 变化 = 登录完成跳转；evaluate 死 + page 消失 =
/// 用户关窗 = 完成），语义对齐其 login_poll_decision_* 单测。差异仅两点：
/// 180s deadline（顶层命令无人守窗）+ 完成后不退出进程而是返回重抓。
async fn wait_login(browser: &Browser, url: &str) -> Result<()> {
    let page = goto_page(browser, url).await?;
    let initial_url = page
        .url()
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| url.to_string());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(LOGIN_WALL_TIMEOUT_SECS);
    loop {
        tokio::time::sleep(Duration::from_secs(LOGIN_POLL_SECS)).await;
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "登录等待超时（{LOGIN_WALL_TIMEOUT_SECS}s）: {url}；可先 `gsearch login <url>` 完成登录后重试"
            ));
        }
        let evaluate_ok = page.evaluate("1").await.is_ok();
        let current_url = page.url().await.ok().flatten().unwrap_or_default();
        let attached = browser_alive(browser).await
            && browser
                .pages()
                .await
                .map(|ps| ps.iter().any(|p| p.target_id() == page.target_id()))
                .unwrap_or(false);
        if login_poll_decision(evaluate_ok, &initial_url, &current_url, attached) {
            return Ok(());
        }
    }
}

/// 轮询 document.readyState 到 complete；evaluate 报错（context 重建中的 -32000 等）视为未就绪。
/// 对照 shell.rs settle_after_click（click 后 4s 版）；此处窗口放宽到 rounds×200ms。
async fn wait_dom_complete(page: &chromiumoxide::Page, rounds: usize) {
    for _ in 0..rounds {
        let ok = page
            .evaluate("document.readyState")
            .await
            .ok()
            .and_then(|v| v.into_value::<String>().ok())
            .is_some_and(|s| s == "complete");
        if ok {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// page.content() 带 -32000 容错：context 重建中等 DOM 稳定后重取，至多 3 次。
async fn content_retry(page: &chromiumoxide::Page) -> String {
    for _ in 0..3 {
        match page.content().await {
            Ok(c) => return c,
            Err(_) => wait_dom_complete(page, 25).await, // ≈5s 内等 context 重建
        }
    }
    String::new()
}

/// page.evaluate(js) → String，带 -32000 容错：等 DOM 稳定后重试，至多 3 次。
async fn eval_string_retry(page: &chromiumoxide::Page, js: &str) -> String {
    for _ in 0..3 {
        match page.evaluate(js).await {
            Ok(v) => return v.into_value::<String>().unwrap_or_default(),
            Err(_) => wait_dom_complete(page, 25).await,
        }
    }
    String::new()
}
/// `--dl N [-o DIR]`：走浏览器 cookie 的简化下载（M13 加 `-o`）。
/// `-o DIR` 把文件落到 DIR 下（按 URL 末段命名）；缺省落 CWD（M4 历史行为）。
/// ponytail: `general::cmd_dl` 已对（同名 `output: Option<&Path>` + `dir.join(filename_from_url(url))`），
/// 此处只把 `postproc::dl` 改一致，不动 general。
pub async fn dl(
    browser: &Browser,
    results: &[SearchResult],
    n: usize,
    output: Option<&Path>,
) -> Result<()> {
    let url = pick(results, n, "dl")?;
    let page = browser.new_page("about:blank").await?;
    let _ = tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url)).await;
    let js = format!(
        "async function () {{
            const r = await fetch({}, {{credentials: 'include'}});
            const b = await r.arrayBuffer();
            const u8 = new Uint8Array(b);
            let s = '';
            for (const x of u8) s += String.fromCharCode(x);
            return btoa(s);
        }}",
        serde_json::to_string(url)?
    );
    let b64 = page
        .evaluate(js)
        .await
        .map_err(|e| anyhow!(
            "下载失败（{url}）：M4 同源 fetch 受 CORS 限制，未放行的站会在此报错，待 M6 升级为 Page download flow。原因: {e}"
        ))?
        .into_value::<String>()
        .map_err(|e| anyhow!("fetch 返回值非字符串（{url}）: {e}"))?;
    let bytes = b64_decode(&b64)?;
    if bytes.is_empty() {
        return Err(anyhow!("下载内容为空（{url}）"));
    }

    let dir = std::path::absolute(output.unwrap_or_else(|| Path::new(".")))?;
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("创建下载目录 {} 失败: {e}", dir.display()))?;
    let path = dir.join(filename_from_url(url));
    std::fs::write(&path, &bytes).map_err(|e| anyhow!("写文件 {} 失败: {e}", path.display()))?;
    if bytes.len() > DL_LARGE_BYTES {
        tracing::warn!(
            "下载文件 {} MB，fetch→base64 路径吃内存，考虑 M6 Page download flow",
            bytes.len() / 1_000_000
        );
    }
    println!("已下载: {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_bounds() {
        let r = [SearchResult {
            title: "t".into(),
            url: "u".into(),
            snippet: "s".into(),
        }];
        assert!(pick(&r, 0, "read").is_err());
        assert!(pick(&r, 2, "read").is_err());
        assert_eq!(pick(&r, 1, "read").unwrap(), "u");
        assert!(pick(&[], 1, "read").is_err());
    }

    /// PM 契约：postproc.rs tests 加 skeleton_extract_cases（h1/h2/h3 各提取 + 段数 + first_chars）。
    /// M9 升级为：直接验证 AdaptiveRead 三个核心行为（短文 / 中等 / 长文自适应）。
    /// 测试 HTML 通过 in-memory 字符串驱动，不启 Chrome → CI 安全。
    #[test]
    fn skeleton_extract_cases() {
        use gsearch::skeleton::extract_adaptive;
        let html = r#"<!doctype html><html><head><title>T</title></head><body>
<h1>One</h1>
<p>p1 alpha.</p>
<h2>Two</h2>
<p>p2 bravo charlie delta echo.</p>
<h3>Three</h3>
<p>p3.</p>
</body></html>"#;
        let r = extract_adaptive(html);
        assert_eq!(r.headings.len(), 3);
        assert_eq!(r.headings[0].level, 1);
        assert_eq!(r.headings[0].text, "One");
        assert_eq!(r.headings[1].level, 2);
        assert_eq!(r.headings[2].level, 3);
        assert_eq!(r.paragraph_index.len(), 3);
        assert!(r.paragraph_index[0].char_count > 0);
    }

    /// M17 登录墙判定:URL 路径段特征 / title 或 DOM+短正文特征 / 正文页不误伤。
    #[test]
    fn login_wall_hit_cases() {
        // URL 路径段:知乎 /signin、GitHub /login、注册墙
        assert!(login_wall_hit("https://www.zhihu.com/signin?next=%2Fp%2F1", "登录 - 知乎", "", 300));
        assert!(login_wall_hit("https://github.com/login", "Sign in to GitHub", "", 500));
        assert!(login_wall_hit("https://example.com/signup", "注册", "", 100));
        // title 特征 + 正文极短(SPA 型墙,URL 不变)
        assert!(login_wall_hit("https://example.com/article", "请登录后继续", "", 50));
        assert!(login_wall_hit("https://example.com/x", "Sign in required", "", 399));
        // DOM 特征 + 正文极短:知乎 IP 风控页(title 空,body 是含「登录」的短 JSON 错误体)
        assert!(login_wall_hit(
            "https://zhuanlan.zhihu.com/p/405555378",
            "",
            r#"{"error":{"message":"您当前请求存在异常，暂时限制本次访问。……登录后私信知乎小管家反馈。","code":40362}}"#,
            100
        ));
        // 正文页不误伤:title/DOM 撞词但正文长;URL 含 login 但非登录段
        assert!(!login_wall_hit("https://example.com/article", "如何登录的完全指南", "登录教程正文……", 3000));
        assert!(!login_wall_hit("https://blog.example.com/how-to-login", "How to login (guide)", "long body", 5000));
        assert!(!login_wall_hit("https://example.com/", "Example Domain", "This domain is for use in illustrative examples", 100));
    }
}

#[cfg(test)]
mod live_tests {
    //! 免 Google 集成测试。
    //! 起真 Chrome 且独占 profile：并行会因 profile 锁竞态挂（ExitStatus(21)），
    //! 必须串行跑 `cargo test -- --test-threads=1`（CI 已 skip live，不受影响）。

    use super::*;

    fn fixture() -> Vec<SearchResult> {
        vec![SearchResult {
            title: "t".into(),
            url: "https://example.com/".into(),
            snippet: "s".into(),
        }]
    }

    /// 单一 async 测试独占 profile 锁，避免并行启两个 Chrome 抢锁。
    /// #[ignore]：真启 Chrome + 访问外网，属手工验证测试（cargo test --ignored 跑）。
    /// 之前混进默认 cargo test：真 Chrome 不 close 造成进程残留。
    #[ignore]
    #[tokio::test]
    async fn postproc_live() {
        let (mut browser, handler) = gsearch::browser::launch(true).await.unwrap();
        let mut h_slot = Some(gsearch::browser::spawn_handler(handler));
        let results = fixture();

        let err = read(&mut browser, &mut h_slot, &[], 1, &ReadOpts::default()).await.unwrap_err();
        assert!(err.to_string().contains("越界"), "got: {err}");

        let txt = read(&mut browser, &mut h_slot, &results, 1, &ReadOpts::default()).await.unwrap();
        assert!(txt.contains("[目录]"), "default read missing [目录]: {txt:?}");
        assert!(txt.contains("[摘要"), "default read missing [摘要]: {txt:?}");
        assert!(txt.contains("Example Domain"), "default read got: {txt:?}");

        let txt = read_full(&mut browser, &mut h_slot, &results, 1).await.unwrap();
        assert!(txt.contains("Example Domain"), "read_full got: {txt:?}");

        dl(&browser, &results, 1, None).await.unwrap();
        let bytes = std::fs::read("download.bin").unwrap();
        assert!(!bytes.is_empty());
        assert!(
            bytes.windows(14).any(|w| w == b"Example Domain"),
            "dl content head: {}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
        );
        std::fs::remove_file("download.bin").unwrap();
    }

    /// 直接断言 `cmd /c start` 机制——Windows 专属；Linux/macOS 由 CI matrix 的编译覆盖。
    /// #[ignore]：真开默认浏览器窗口（每次 cargo test 弹 example.com 就是它），
    /// 属手工验证测试（cargo test --ignored 跑）。
    #[ignore]
    #[cfg(windows)]
    #[test]
    fn open_mechanism() {
        let st = std::process::Command::new("cmd")
            .args(["/c", "start", "", "https://example.com"])
            .status()
            .unwrap();
        assert!(st.success());
        assert!(open(&fixture(), 1).is_ok());
    }
}