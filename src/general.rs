//! M6 通用代理子命令（PLAN §3.5）：browse 正文 / login 人工登录 / dl CDP 下载。
//! 与 search 后处理（postproc.rs）平行，是独立使用入口，共享 browser.rs 的 profile/启动链路。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use chromiumoxide::cdp::browser_protocol::browser::{
    SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
};

use gsearch::browser::{launch, spawn_handler};
use gsearch::search::is_captcha;

const TEXT_MAX_CHARS: usize = 5000;
const PAGE_TIMEOUT_SECS: u64 = 30;
/// login 轮询间隔（任务规格：每 2s）
const LOGIN_POLL_SECS: u64 = 2;
/// dl 下载完成总超时
const DL_TOTAL_TIMEOUT_SECS: u64 = 60;
/// 下载嗅探窗口：窗口内目录无任何新文件（连 .crdownload 都没有）→ 判定渲染型 URL，走页内 fetch 落盘
const DL_SNIFF_SECS: u64 = 4;

/// `browse <url>`：headless 渲染 → innerText 截 5000 字 + URL/标题。
/// CAPTCHA 路径（M3 检测复用、M6 不换有头）：撞码报错退出，提示用 login 手工验证。
pub async fn cmd_browse(url: &str) -> Result<ExitCode> {
    let (mut browser, handler) = launch(true).await?;
    let _h = spawn_handler(handler);

    let page = browser.new_page("about:blank").await?;
    tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
        .await
        .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
        .map_err(|e| anyhow!("goto {url} 失败: {e}"))?;

    if is_captcha(&page.content().await.unwrap_or_default()) {
        // ponytail: browse 是一次性、亮状态命令，撞码换有头（close+relaunch 抢 profile 锁）代价大于收益；
        // login 子命令已覆盖"人工解一次"的场景。CAPTCHA 判定复用 search.rs（M3）。
        return Err(anyhow!(
            "{url} 遇 CAPTCHA：用 `gsearch login {url}` 开有头窗手工验证后重试"
        ));
    }

    let title = page
        .evaluate("document.title")
        .await?
        .into_value::<String>()
        .unwrap_or_default();
    let text = page
        .evaluate("document.body.innerText")
        .await?
        .into_value::<String>()
        .unwrap_or_default();
    let text: String = text.chars().take(TEXT_MAX_CHARS).collect();
    println!("=== {url} | {title} ===\n{text}");

    browser.close().await?;
    let _ = browser.wait().await; // 等 Chrome 进程死透，免 Drop 时报"was not closed manually" WARN
    Ok(ExitCode::SUCCESS)
}

/// `login <url>`：有头窗人工登录，轮询不限时；人关窗（或关页签）= 完成，cookie 随 profile 落盘。
/// 不判 CAPTCHA（登录页就是真人登录页，PLAN §3.5）。
pub async fn cmd_login(url: &str) -> Result<ExitCode> {
    let (browser, handler) = launch(false).await?;
    let _h = spawn_handler(handler);

    let page = browser.new_page("about:blank").await?;
    tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
        .await
        .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
        .map_err(|e| anyhow!("goto {url} 失败: {e}"))?;
    tracing::info!("已打开登录窗口: {url}，完成登录后直接关窗（或本页签）即算完成，不限时等待");

    loop {
        tokio::time::sleep(Duration::from_secs(LOGIN_POLL_SECS)).await;
        if page.evaluate("1").await.is_ok() {
            continue; // 页面还活着，人还在登录
        }
        // evaluate 失败三态：浏览器死（关窗）/ 本页签死 / 导航重建执行上下文的瞬态。
        // 只有第三种继续等：浏览器还活着且我们的 target 仍在 pages 列表里。
        if browser_alive(&browser).await
            && browser
                .pages()
                .await
                .map(|ps| ps.iter().any(|p| p.target_id() == page.target_id()))
                .unwrap_or(false)
        {
            tracing::debug!("evaluate 瞬态失败（页面导航中），继续等待");
            continue;
        }
        tracing::info!("用户关窗 = 登录完成，cookie 已落 profile");
        return Ok(ExitCode::SUCCESS); // Chrome 随正常关窗已刷盘 cookie，不再 close（连接已死）
    }
}

async fn browser_alive(browser: &chromiumoxide::Browser) -> bool {
    browser.version().await.is_ok()
}

/// `dl <url> [-o PATH]`：CDP `Browser.setDownloadBehavior` 走 Chrome 原生下载（带 profile 登录态）。
/// 渲染型 URL（普通网页，Chrome 不触发下载）回退页内 fetch 落盘（PLAN §3.5 raw-file 路径，同源 cookie）。
pub async fn cmd_dl(url: &str, output: Option<&Path>) -> Result<ExitCode> {
    // Chrome 对相对 downloadPath 按自身 CWD 解析，统一转绝对（同 browser.rs profile_dir 的教训）
    let dir: PathBuf = std::path::absolute(output.unwrap_or(Path::new(".")))?;
    std::fs::create_dir_all(&dir).with_context(|| format!("创建下载目录失败: {}", dir.display()))?;

    let (mut browser, handler) = launch(true).await?;
    let _h = spawn_handler(handler);

    let params = SetDownloadBehaviorParams::builder()
        .behavior(SetDownloadBehaviorBehavior::Allow)
        .download_path(dir.to_string_lossy().into_owned())
        .build()
        .map_err(|e| anyhow!("构造 setDownloadBehavior 参数失败: {e}"))?;
    browser
        .execute(params)
        .await
        .context("设置下载行为失败（Browser.setDownloadBehavior）")?;

    // 快照必须在 goto 之前拍：下载型 URL 的 goto 会挂到超时（~30s），文件在 goto 期间就落盘了，
    // 事后拍快照会把已下完的文件当"旧文件"过滤掉 → 误判无下载 → 错走页内 fetch fallback
    let before = list_dir(&dir)?;
    let page = browser.new_page("about:blank").await?;
    // 下载型 URL 的 goto 因下载导航被 abort（ERR_ABORTED）——不致命，下载在后台继续
    let _ = tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url)).await;

    match wait_new_file(&dir, &before).await? {
        Some(name) => {
            let size = std::fs::metadata(dir.join(&name)).map(|m| m.len()).unwrap_or(0);
            println!("已下载: {} ({size} bytes)", dir.join(&name).display());
        }
        None => {
            // 嗅探窗口内无下载迹象：URL 是渲染型页面，页内 fetch 取真实字节落盘
            let bytes = fetch_in_page(&page, url).await?;
            if bytes.is_empty() {
                return Err(anyhow!("下载内容为空（{url}）"));
            }
            let name = filename_from_url(url);
            let path = dir.join(&name);
            std::fs::write(&path, &bytes).with_context(|| format!("写文件失败: {}", path.display()))?;
            println!("已下载: {} ({})", path.display(), bytes.len());
        }
    }

    browser.close().await?;
    let _ = browser.wait().await; // 同 cmd_browse：等进程死透再 Drop
    Ok(ExitCode::SUCCESS)
}

/// 页内 fetch → JSON 字节数组回传（serde 反序列化 Vec<u8>，不走 base64）。
/// credentials:'include' 带同源 cookie；必须 `async function ()` 而非箭头函数
/// （chromiumoxide 的函数探测不认 async 箭头，见 postproc.rs 同款注释）。
/// URL 用 serde_json::to_string 编码成 JS 字符串字面量，引号/反斜杠转义零手写。
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

/// 轮询 dir 等 before 之外的新文件。Some(name) = 下载完成；None = 嗅探窗口内无任何下载迹象（渲染型 URL）。
/// ponytail: 不订阅 Page.downloadProgress 事件（chromiumoxide 0.9 要 listener 样板），
/// 目录轮询 + 大小连续两次相同判完，误差 ≤ 1 个轮询周期；多文件并发下载只认第一个稳定文件。
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
                continue; // Chrome 下载中的临时文件
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

/// 文件名 = URL 路径最后一段（去 query/hash、剥 scheme://authority）；为空（裸 origin/尾斜杠）则 download.bin。
/// 与 postproc.rs 的同名逻辑一致（那边私有不可复用，3 个用例不值得跨模块 pub 化）。
fn filename_from_url(url: &str) -> String {
    let path = url.split(['#', '?']).next().unwrap_or(url);
    let path = match path.find("://") {
        Some(i) => path[i + 3..].find('/').map_or("", |j| &path[i + 3 + j..]),
        None => path,
    };
    let last = path.rsplit('/').next().unwrap_or("");
    if last.is_empty() { "download.bin".into() } else { last.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_from_url_cases() {
        assert_eq!(filename_from_url("https://x.com/a/b/file.pdf?x=1#f"), "file.pdf");
        assert_eq!(filename_from_url("https://example.com/"), "download.bin");
        assert_eq!(filename_from_url("https://example.com"), "download.bin");
        assert_eq!(filename_from_url("https://example.com/index.html"), "index.html");
    }
}
