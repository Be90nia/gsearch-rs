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

use gsearch::browser::{BrowserKind, launch_with_kind_proxy, spawn_handler};
use gsearch::search::is_captcha;
use gsearch::skeleton::extract_adaptive;
use gsearch::util::filename_from_url;

const PAGE_TIMEOUT_SECS: u64 = 30;
/// login 轮询间隔(postproc 登录墙等待复用同一节奏)
pub(crate) const LOGIN_POLL_SECS: u64 = 2;
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
    pub browser: Option<BrowserKind>,
    /// M12 浏览器代理；None = 走直连
    pub proxy: Option<String>,
}

/// `browse <url>`：headless 渲染 → 默认 AdaptiveRead（M9），`--full` 拿纯 innerText 5000 字。
/// CAPTCHA 路径：撞码报错退出，提示用 login 手工验证。
/// H2+M2：launch 后所有 ? 早返回路径（new_page / goto / evaluate / content / parse）由外层
/// graceful_close 收尾；不再裸 close+wait。
/// uhp/j44：goto 后等语义定稿走 postproc::wait_content_stable 原子快照（title 一并带回）；
/// jp4：html 过 read_max_chars 硬截断，--json 在 meta 字段标注 truncated/omitted/content_untrusted。
pub async fn cmd_browse(url: &str, opts: &BrowseOpts) -> Result<ExitCode> {
    use crate::postproc;
    let mut browser_opt: Option<chromiumoxide::browser::Browser> = None;
    let result: Result<()> = async {
        let (browser, handler) =
            launch_with_kind_proxy(true, opts.browser, opts.proxy.clone()).await?;
        let _h = spawn_handler(handler);
        let page = browser.new_page("about:blank").await?;
        tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
            .await
            .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
            .map_err(|e| anyhow!("goto {url} 失败: {e}"))?;
        // uhp/j44：等语义定稿（marker 连续两次相同），避免 -32000 与风控页假 complete。
        let snap = postproc::wait_content_stable(&page, 50).await; // 50×200ms ≈ 10s

        if is_captcha(&postproc::content_retry(&page).await) {
            return Err(anyhow!(
                "{url} 遇 CAPTCHA：用 `gsearch login {url}` 开有头窗手工验证后重试"
            ));
        }

        // --full：纯 innerText 5000 字（与 postproc::read_full 同一实现）
        if opts.full {
            postproc::read_full_inner(&page, url).await?;
            browser_opt = Some(browser);
            return Ok(());
        }

        let title = match &snap {
            Some(s) => s.title.clone(),
            None => postproc::eval_string_retry(&page, "document.title").await,
        };
        let html_full = postproc::content_retry(&page).await;
        let (html, truncated, omitted) = postproc::cap_chars(&html_full, postproc::read_max_chars());
        let mut read = extract_adaptive(&html);
        read.url = url.to_string();
        read.title = title;

        let out = postproc::render_read(&read, opts.json, opts.headings_only, opts.from, truncated, omitted);
        println!("{out}");
        browser_opt = Some(browser);
        Ok(())
    }
    .await;
    if let Some(b) = browser_opt.as_mut() {
        gsearch::browser::graceful_close(b).await;
    }
    result?;
    Ok(ExitCode::SUCCESS)
}

/// `login <url>`：有头窗人工登录，轮询不限时；人关窗（或关页签）= 完成，cookie 随 profile 落盘。
/// 不判 CAPTCHA（登录页是真人登录页，PLAN §3.5）。
/// H2+M2：launch 后所有 ? 早返回由外层 graceful_close 收尾。
pub async fn cmd_login(url: &str, browser: Option<BrowserKind>, proxy: Option<String>) -> Result<ExitCode> {
    let mut browser_opt: Option<chromiumoxide::browser::Browser> = None;
    let result: Result<bool> = async {
        let (browser_inst, handler) = launch_with_kind_proxy(false, browser, proxy).await?;
        let _h = spawn_handler(handler);
        browser_opt = Some(browser_inst);

        let page = browser_opt.as_ref().unwrap().new_page("about:blank").await?;
        tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
            .await
            .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
            .map_err(|e| anyhow!("goto {url} 失败: {e}"))?;
        // 记录登录页 URL；用户登录成功跳到 dashboard = URL 变化 = 登录完成（bug fix）。
        // ponytail: 旧版只用 page.evaluate("1").await.is_ok() 判定「页面是否仍在」——
        // 登录后跳到 dashboard，evaluate 继续成功 → 死循环，只能 Ctrl+C。
        let initial_url = page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| url.to_string());
        tracing::info!("已打开登录窗口: {url}，完成登录后直接关窗（或本页签）即算完成，不限时等待");

        loop {
            tokio::time::sleep(Duration::from_secs(LOGIN_POLL_SECS)).await;
            let browser_inst = browser_opt.as_ref().unwrap();
            let evaluate_ok = page.evaluate("1").await.is_ok();
            let current_url = page
                .url()
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            let page_still_attached = browser_alive(browser_inst).await
                && browser_inst
                    .pages()
                    .await
                    .map(|ps| ps.iter().any(|p| p.target_id() == page.target_id()))
                    .unwrap_or(false);
            if login_poll_decision(evaluate_ok, &initial_url, &current_url, page_still_attached) {
                tracing::info!("检测到登录完成（URL 变化或窗口关闭），cookie 已落 profile");
                return Ok(true);
            }
        }
    }
    .await;
    if let Some(b) = browser_opt.as_mut() {
        gsearch::browser::graceful_close(b).await;
    }
    result?;
    Ok(ExitCode::SUCCESS)
}

/// `cmd_login` 单轮决策。返回 true = 本轮退出（登录完成）。
pub(crate) fn login_poll_decision(
    evaluate_ok: bool,
    initial_url: &str,
    current_url: &str,
    page_still_attached: bool,
) -> bool {
    // 主退出：登录后跳转（dashboard / post-login redirect）→ URL 变化。
    if current_url != initial_url {
        return true;
    }
    // 同 URL + evaluate 成功 → 用户还在登录页，继续等。
    if evaluate_ok {
        return false;
    }
    // evaluate 瞬态失败 + page 还在 → 导航中抖动，继续等。
    if page_still_attached {
        return false;
    }
    // evaluate 失败 + page 不在 → 用户关窗。
    true
}

pub(crate) async fn browser_alive(browser: &chromiumoxide::Browser) -> bool {
    browser.version().await.is_ok()
}

/// `dl <url> [-o PATH]`：CDP `Browser.setDownloadBehavior` 走 Chrome 原生下载（带 profile 登录态）。
/// 渲染型 URL（普通网页，Chrome 不触发下载）回退页内 fetch 落盘（PLAN §3.5 raw-file 路径，同源 cookie）。
pub async fn cmd_dl(url: &str, output: Option<&Path>, browser: Option<BrowserKind>, proxy: Option<String>) -> Result<ExitCode> {
    use crate::postproc;
    let dir: PathBuf = std::path::absolute(output.unwrap_or(Path::new(".")))?;
    std::fs::create_dir_all(&dir).with_context(|| format!("创建下载目录失败: {}", dir.display()))?;
    let mut browser_opt: Option<chromiumoxide::browser::Browser> = None;
    let result: Result<()> = async {
        let (browser_inst, handler) = launch_with_kind_proxy(true, browser, proxy).await?;
        let _h = spawn_handler(handler);
        browser_opt = Some(browser_inst);

        let browser_inst = browser_opt.as_mut().unwrap();
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
        // I4：goto 失败/超时 warn 留痕后继续——下载靠原生下载嗅探或页内 fetch 兜底。
        let _ = postproc::goto_for_download(page.goto(url), url).await;

        match wait_new_file(&dir, &before).await? {
            Some(name) => {
                let size = std::fs::metadata(dir.join(&name)).map(|m| m.len()).unwrap_or(0);
                println!("已下载: {} ({size} bytes)", dir.join(&name).display());
            }
            None => {
                let bytes = postproc::fetch_in_page(&page, url).await?;
                if bytes.is_empty() {
                    return Err(anyhow!("下载内容为空（{url}"));
                }
                let name = filename_from_url(url);
                let path = dir.join(&name);
                std::fs::write(&path, &bytes).with_context(|| format!("写文件失败: {}", path.display()))?;
                println!("已下载: {} ({})", path.display(), bytes.len());
            }
        }
        Ok(())
    }
    .await;
    if let Some(b) = browser_opt.as_mut() {
        gsearch::browser::graceful_close(b).await;
    }
    result?;
    Ok(ExitCode::SUCCESS)
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

#[cfg(test)]
mod tests {
    use super::login_poll_decision;

    /// M13 C1 bug fix: 登录后 URL 变化（跳到 dashboard）= 登录完成，退出循环。
    /// 模拟 evaluate 仍成功（dashboard 页 JS 正常）+ page 仍在列表（target_id 未变），
    /// 这是旧版死锁的核心场景；新版靠 URL 差异退出。
    #[test]
    fn login_poll_decision_url_changed_exits() {
        let initial = "https://example.com/login";
        let current = "https://example.com/dashboard";
        assert!(
            login_poll_decision(true, initial, current, true),
            "URL 变化必须触发退出，不能再依赖 evaluate 失败"
        );
    }

    /// 用户还在登录页（同 URL + evaluate 成功）= 继续等。
    #[test]
    fn login_poll_decision_same_url_alive_waits() {
        let url = "https://example.com/login";
        assert!(!login_poll_decision(true, url, url, true));
    }

    /// evaluate 失败但 page 还在列表（导航中抖动）= 继续等，不误判用户关窗。
    #[test]
    fn login_poll_decision_transient_evaluate_keeps_waiting() {
        let url = "https://example.com/login";
        assert!(!login_poll_decision(false, url, url, true));
    }

    /// 用户关窗（evaluate 失败 + page 死）= 退出（旧行为，保留兜底）。
    #[test]
    fn login_poll_decision_window_closed_exits() {
        let url = "https://example.com/login";
        assert!(login_poll_decision(false, url, url, false));
    }

    /// 边界：URL 变化优先级最高（即便 page 已死也以 URL 变化退出）。
    #[test]
    fn login_poll_decision_url_changed_overrides_page_dead() {
        assert!(login_poll_decision(
            false,
            "https://x.com/login",
            "https://x.com/dashboard",
            false,
        ));
    }
}
