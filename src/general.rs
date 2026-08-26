//! M6 通用代理子命令（PLAN §3.5）：browse 正文 / login 人工登录 / dl CDP 下载。
//! 与 search 后处理（postproc.rs）平行，是独立使用入口，共享 browser.rs 的 profile/启动链路。
//!
//! M9：`browse <url>` 默认 AdaptiveRead，`--full/--json/--headings-only/--from K` 互斥选择（与 `search --read` 同步）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use chromiumoxide::cdp::browser_protocol::browser::{
    SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
};

use gsearch::browser::{BrowserKind, launch_with_kind, spawn_handler};
use gsearch::search::is_captcha;
use gsearch::skeleton::{extract_adaptive, format_adaptive, format_headings_only, format_json};
use gsearch::util::filename_from_url;

const TEXT_MAX_CHARS: usize = 5000;
const PAGE_TIMEOUT_SECS: u64 = 30;
/// login 轮询间隔
const LOGIN_POLL_SECS: u64 = 2;
/// dl 下载完成总超时
const DL_TOTAL_TIMEOUT_SECS: u64 = 60;
/// 下载嗅探窗口：窗口内目录无任何新文件（连 .crdownload 都没有）→ 判定渲染型 URL，走页内 fetch 落盘
const DL_SNIFF_SECS: u64 = 4;

/// M9 `browse <url>` 选项集。与 postproc::ReadOpts 字段一致（agent 心智统一）。
#[derive(Debug, Clone, Default)]
pub struct BrowseOpts {
    pub full: bool,
    pub json: bool,
    pub headings_only: bool,
    pub from: usize,
    /// M11 浏览器选择；None = 自动检测
    /// M11 浏览器选择；None = 自动检测
    pub browser: Option<BrowserKind>,
}

/// `browse <url>`：headless 渲染 → 默认 AdaptiveRead（M9），`--full` 拿纯 innerText 5000 字。
/// CAPTCHA 路径：撞码报错退出，提示用 login 手工验证。
pub async fn cmd_browse(url: &str, opts: &BrowseOpts) -> Result<ExitCode> {
    let (mut browser, handler) = launch_with_kind(true, opts.browser).await?;
    let _h = spawn_handler(handler);
    let page = browser.new_page("about:blank").await?;
    tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
        .await
        .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
        .map_err(|e| anyhow!("goto {url} 失败: {e}"))?;

    if is_captcha(&page.content().await.unwrap_or_default()) {
        return Err(anyhow!(
            "{url} 遇 CAPTCHA：用 `gsearch login {url}` 开有头窗手工验证后重试"
        ));
    }

    // --full：纯 innerText 5000 字
    if opts.full {
        let text = page
            .evaluate("document.body.innerText")
            .await?
            .into_value::<String>()
            .unwrap_or_default();
        let text: String = text.chars().take(TEXT_MAX_CHARS).collect();
        println!("=== {url} ===\n{text}");
        if let Err(e) = browser.close().await {
            tracing::warn!("close browser 失败: {e}");
        }
        let _ = browser.wait().await;
        return Ok(ExitCode::SUCCESS);
    }

    let title = page
        .evaluate("document.title")
        .await?
        .into_value::<String>()
        .unwrap_or_default();
    let html = page.content().await.unwrap_or_default();
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

    if let Err(e) = browser.close().await {
        tracing::warn!("close browser 失败: {e}");
    }
    let _ = browser.wait().await;
    Ok(ExitCode::SUCCESS)
}

/// `login <url>`：有头窗人工登录，轮询不限时；人关窗（或关页签）= 完成，cookie 随 profile 落盘。
/// 不判 CAPTCHA（登录页是真人登录页，PLAN §3.5）。
pub async fn cmd_login(url: &str, browser: Option<BrowserKind>) -> Result<ExitCode> {
    let (browser_inst, handler) = launch_with_kind(false, browser).await?;
    let _h = spawn_handler(handler);

    let page = browser_inst.new_page("about:blank").await?;
    tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
        .await
        .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
        .map_err(|e| anyhow!("goto {url} 失败: {e}"))?;
    tracing::info!("已打开登录窗口: {url}，完成登录后直接关窗（或本页签）即算完成，不限时等待");

    loop {
        tokio::time::sleep(Duration::from_secs(LOGIN_POLL_SECS)).await;
        if page.evaluate("1").await.is_ok() {
            continue;
        }
        if browser_alive(&browser_inst).await
            && browser_inst
                .pages()
                .await
                .map(|ps| ps.iter().any(|p| p.target_id() == page.target_id()))
                .unwrap_or(false)
        {
            tracing::debug!("evaluate 瞬态失败（页面导航中），继续等待");
            continue;
        }
        tracing::info!("用户关窗 = 登录完成，cookie 已落 profile");
        return Ok(ExitCode::SUCCESS);
    }
}

async fn browser_alive(browser: &chromiumoxide::Browser) -> bool {
    browser.version().await.is_ok()
}

/// `dl <url> [-o PATH]`：CDP `Browser.setDownloadBehavior` 走 Chrome 原生下载（带 profile 登录态）。
/// 渲染型 URL（普通网页，Chrome 不触发下载）回退页内 fetch 落盘（PLAN §3.5 raw-file 路径，同源 cookie）。
pub async fn cmd_dl(url: &str, output: Option<&Path>, browser: Option<BrowserKind>) -> Result<ExitCode> {
    let dir: PathBuf = std::path::absolute(output.unwrap_or(Path::new(".")))?;
    std::fs::create_dir_all(&dir).with_context(|| format!("创建下载目录失败: {}", dir.display()))?;

    let (mut browser_inst, handler) = launch_with_kind(true, browser).await?;
    let _h = spawn_handler(handler);

    let params = SetDownloadBehaviorParams::builder()
        .behavior(SetDownloadBehaviorBehavior::Allow)
        .download_path(dir.to_string_lossy().into_owned())
        .build()
        .map_err(|e| anyhow!("构造 setDownloadBehavior 参数失败: {e}"))?;
    browser_inst
        .execute(params)
        .await
        .context("设置下载行为失败（Browser.setDownloadBehavior）")?;

    let before = list_dir(&dir)?;
    let page = browser_inst.new_page("about:blank").await?;
    let _ = tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url)).await;

    match wait_new_file(&dir, &before).await? {
        Some(name) => {
            let size = std::fs::metadata(dir.join(&name)).map(|m| m.len()).unwrap_or(0);
            println!("已下载: {} ({size} bytes)", dir.join(&name).display());
        }
        None => {
            let bytes = fetch_in_page(&page, url).await?;
            if bytes.is_empty() {
                return Err(anyhow!("下载内容为空（{url}"));
            }
            let name = filename_from_url(url);
            let path = dir.join(&name);
            std::fs::write(&path, &bytes).with_context(|| format!("写文件失败: {}", path.display()))?;
            println!("已下载: {} ({})", path.display(), bytes.len());
        }
    }

    if let Err(e) = browser_inst.close().await {
        tracing::warn!("close browser 失败: {e}");
    }
    let _ = browser_inst.wait().await;
    Ok(ExitCode::SUCCESS)
}

/// 页内 fetch → JSON 字节数组回传（serde 反序列化 Vec<u8>，不走 base64）。
async fn fetch_in_page(page: &chromiumoxide::Page, url: &str) -> Result<Vec<u8>> {
    let js = format!(
        "async function () {{
            const r = await fetch({}, {{credentials: 'include'}});
            if (!r.ok) throw new Error('HTTP ' + r.status);
            return Array.from(new Uint8Array(await r.arrayBuffer()));
        }}",
        serde_json::to_string(url)?
    );
    page.evaluate(js)
        .await
        .map_err(|e| anyhow!("页内 fetch 失败（{url}）: {e}"))?
        .into_value::<Vec<u8>>()
        .map_err(|e| anyhow!("fetch 返回值非字节数组（{url}）: {e}"))
}

/// 轮询 dir 等 before 之外的新文件。
async fn wait_new_file(dir: &Path, before: &HashSet<String>) -> Result<Option<String>> {
    let start = Instant::now();
    let mut prev: Option<(String, u64)> = None;
    let mut seen_any = false;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let names = list_dir(dir)?;
        let news: Vec<&String> = names.iter().filter(|n| !before.contains(*n)).collect();
        if !news.is_empty() {
            seen_any = true;
        }
        for n in news {
            if n.ends_with(".crdownload") || n.ends_with(".tmp") {
                continue;
            }
            let size = std::fs::metadata(dir.join(n)).map(|m| m.len()).unwrap_or(0);
            if size > 0 && prev.as_ref().is_some_and(|(pn, ps)| pn == n && *ps == size) {
                return Ok(Some(n.clone()));
            }
            prev = Some((n.clone(), size));
        }
        let elapsed = start.elapsed().as_secs();
        if seen_any {
            if elapsed >= DL_TOTAL_TIMEOUT_SECS {
                return Err(anyhow!("下载超时（{DL_TOTAL_TIMEOUT_SECS}s）：临时文件已出现但未完成"));
            }
        } else if elapsed >= DL_SNIFF_SECS {
            return Ok(None);
        }
    }
}

fn list_dir(dir: &Path) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("读目录失败: {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            out.insert(entry.file_name().to_string_lossy().into_owned());
        }
    }
    Ok(out)
}