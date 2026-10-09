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

use gsearch::browser::{BrowserKind, launch_with_kind_proxy, open_page, spawn_handler};
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
/// v97 direct 直链总超时：流式 GET 大文件慢网场景，独立于 fetch 的 10s。
const DL_DIRECT_TIMEOUT_SECS: u64 = 300;

/// M9 `browse <url>` 选项集。与 postproc::ReadOpts 字段一致（agent 心智统一）。
#[derive(Debug, Clone, Default)]
pub struct BrowseOpts {
    pub full: bool,
    pub json: bool,
    pub headings_only: bool,
    pub from: usize,
    /// xih：正文以 markdown 输出（渲染后 HTML 转换，隐含全文模式）。--json 时 content_text
    /// 字段换源为 markdown，meta.format="markdown" 标注；无 flag 输出逐字节不变。
    pub markdown: bool,
    /// M11 浏览器选择；None = 自动检测
    pub browser: Option<BrowserKind>,
    /// M12 浏览器代理；None = 走直连
    pub proxy: Option<String>,
    /// pkp：放行私网地址（仅 browse 子命令挂此 flag；语义与 fetch --allow-private 对齐）。
    pub allow_private: bool,
    /// n76：正文字符预算（HTML/innerText/markdown 上限；超限截断 meta.truncated 如实）。CLI 默认 50000。
    pub max_chars: usize,
}

/// pkp：浏览器入口 scheme 白名单（http/https）——非白名单（file:///javascript:/data: 等）
/// 快失败；无 scheme 的裸 host 按浏览器默认语义补 https://。返回规范化 URL 供 goto 消费。
/// rationale（I9 威胁模型）：入口 URL 可能来自 LLM 输出（搜索结果/页面内容间接注入），
/// file:// 可把本地文件内容带进 agent 上下文外泄，javascript:/data: 是注入向量。
pub(crate) fn browsable_scheme_ok(url: &str) -> Result<String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    // 无 :// 但以已知 scheme 词头开头的（javascript:alert(1) / data:text/html / file:C:/x）
    //——不得落入补全分支变成 https://javascript:…。
    const BLOCKED_SCHEMES: [&str; 6] = ["javascript:", "data:", "file:", "blob:", "vbscript:", "view-source:"];
    if trimmed.is_empty() {
        anyhow::bail!("URL 不能为空")
    } else if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(trimmed.to_string())
    } else if trimmed.contains("://") || BLOCKED_SCHEMES.iter().any(|s| lower.starts_with(s)) {
        anyhow::bail!("拒绝非 http/https URL（file:///javascript:/data: 等 scheme 不安全或无意义；URL 可能来自不可信来源）: {url}")
    } else {
        // 裸 host（含 host:port，如 localhost:3000）→ 浏览器默认语义补 https://
        Ok(format!("https://{trimmed}"))
    }
}

/// pkp：browse 入口完整门 = scheme 白名单 + 私网门（SSRF 对齐 fetch，私网判定同源 classify_url）。
/// login/dl 仅走 scheme 白名单（browsable_scheme_ok）——内网登录页/内网下载是既有合法场景。
pub(crate) fn ensure_browsable_url(url: &str, allow_private: bool) -> Result<String> {
    let normalized = browsable_scheme_ok(url)?;
    let (host, ip, private) = crate::fetch::classify_url(&normalized)?;
    if private && !allow_private {
        anyhow::bail!("browse 拒绝私网地址 {ip}（host={host}）。如确需内网页面，请传 --allow-private");
    }
    Ok(normalized)
}

/// 7z0：dl -o/--output-file 相对路径禁 .. 穿越段（防静默落盘出 CWD）；绝对路径显式放行。
fn has_parent_traversal(p: &Path) -> bool {
    !p.is_absolute() && p.components().any(|c| matches!(c, std::path::Component::ParentDir))
}

/// h90：browse goto 超时给代理出口建议（B 受试者：直连超时只报 Request timed out，
/// 不知道有 --proxy 可救）。纯函数供单测锁文案。
fn goto_timeout_error(url: &str) -> anyhow::Error {
    anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}；若目标站点需代理可达，试 `--proxy http://127.0.0.1:7890`（GSEARCH_PROXY 同效）")
}

/// h90（复测残口）：chromiumoxide 内部超时（Request timed out）常先于外层 30s 包装触发，
/// 同样裸报无出口——传输层失败（超时/DNS/reset）统一附代理建议。
fn goto_nav_error(url: &str, e: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("goto {url} 失败: {e}；若目标站点需代理可达，试 `--proxy http://127.0.0.1:7890`（GSEARCH_PROXY 同效）")
}

/// 打回轮2：302 竞态探针——goto 只覆盖首个响应，/issues/N → /pull/N 落定前 content()
/// 常拿空/极短（wait_content_stable 对空串 marker 恒稳，兜不住）。阈值 200 字符。
fn content_needs_settle_retry(html: &str) -> bool {
    html.trim().chars().count() < 200
}

/// 打回轮2：browse 空正文保底指引判定（PM 公式：summary 空 && (omitted>0 || 原始 content 为空)；
/// omitted>0 = 截断吃光（B 上轮 58 万字符形态），content 空 = 302 竞态/-32000 变体拿空——
/// 两条失败形态都要给出口。headings-only 摘要本就为空，不适用）。
fn needs_empty_body_hint(summary_len: usize, headings_only: bool, omitted: usize, raw_content_empty: bool) -> bool {
    summary_len == 0 && !headings_only && (omitted > 0 || raw_content_empty)
}

/// `browse <url>`：headless 渲染 → 默认 AdaptiveRead（M9），`--full` 纯 innerText（50000 cap，
/// fve：--json 走 0mf 同款信封 + content_text；与 --headings-only clap 互斥）。
/// CAPTCHA 路径：撞码报错退出，提示用 login 手工验证。
/// H2+M2：launch 后所有 ? 早返回路径（new_page / goto / evaluate / content / parse）由外层
/// graceful_close 收尾；不再裸 close+wait。
/// uhp/j44：goto 后等语义定稿走 postproc::wait_content_stable 原子快照（title 一并带回）；
/// jp4：html 过 read_max_chars 硬截断，--json 在 meta 字段标注 truncated/omitted/content_untrusted。
pub async fn cmd_browse(url: &str, opts: &BrowseOpts) -> Result<ExitCode> {
    use crate::postproc;
    // pkp：scheme 白名单 + 私网门（--allow-private 放行）——快失败不启动 Chrome
    let url = match ensure_browsable_url(url, opts.allow_private) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("error: {e:#}");
            return Ok(ExitCode::from(2));
        }
    };
    let url = url.as_str(); // 规范化后仍按 &str 流转，后续代码零改动
    let started = std::time::Instant::now();
    let mut browser_opt: Option<chromiumoxide::browser::Browser> = None;
    let result: Result<()> = async {
        let (browser, handler) =
            launch_with_kind_proxy(true, opts.browser, opts.proxy.clone()).await?;
        let _h = spawn_handler(handler);
        let page = open_page(&browser).await?;
        tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
            .await
            .map_err(|_| goto_timeout_error(url))?
            .map_err(|e| goto_nav_error(url, e))?;
        // uhp/j44：等语义定稿（marker 连续两次相同），避免 -32000 与风控页假 complete。
        let snap = postproc::wait_content_stable(&page, 50).await; // 50×200ms ≈ 10s

        let mut html_probe = postproc::content_retry(&page).await;
        // 打回轮2：/issues/N → /pull/N 的 302 竞态——重定向落定前 content() 拿空/极短。
        // 重新 goto（直达落定页）+ content 重试一次。
        if content_needs_settle_retry(&html_probe) {
            tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
                .await
                .map_err(|_| goto_timeout_error(url))?
                .map_err(|e| goto_nav_error(url, e))?;
            let _ = postproc::wait_content_stable(&page, 50).await;
            html_probe = postproc::content_retry(&page).await;
        }
        if is_captcha(&html_probe) {
            return Err(anyhow!(
                "{url} 遇 CAPTCHA：用 `gsearch login {url}` 开有头窗手工验证后重试"
            ));
        }

        // fve：--full 纯 innerText（READ_BODY_MAX_CHARS 50000 cap，截断照标）；
        // xih：--markdown 隐含全文模式，源换渲染后 HTML→markdown（转换在截断前的完整 HTML 上做，
        // md 产物再过同一字符上限——先截 HTML 会把表格腰斩）。
        // --json 对齐 0mf search 契约：信封（meta.truncated 标内容截断）+ content_text 单文档
        if opts.full || opts.markdown {
            let (txt, truncated, omitted) = if opts.markdown {
                let (md, t, o) = postproc::cap_chars(&crate::convert::html_to_markdown(&html_probe)?, opts.max_chars);
                (md, t, o)
            } else {
                postproc::read_full_text(&page, opts.max_chars).await?
            };
            if opts.json {
                let meta = gsearch::types::MetaOutput {
                    tool: "gsearch",
                    version: env!("CARGO_PKG_VERSION"),
                    // browse 侧 query 恒空（schema 约定同 browse/dl）
                    query: String::new(),
                    profile: gsearch::browser::profile_name_only(),
                    proxy: opts.proxy.clone(),
                    humanize: false,
                    limit: 0,
                    elapsed_ms: started.elapsed().as_millis(),
                    truncated,
                    // h90：browse 无搜索来源——provider 置空串，键整体缺席（types.rs 缺席语义），
                    // 不再伪装成 "google" 误导 agent 分流；搜索路径恒非空不受影响
                    provider: String::new(),
                    recency: None,
                };
                let run = gsearch::types::RunStatusInfo {
                    status: gsearch::types::RunStatus::Ok,
                    captcha_solved: false,
                    message: String::new(),
                };
                let mut doc = serde_json::to_value(gsearch::types::OutputEnvelope {
                    meta,
                    run,
                    results: Vec::<()>::new(),
                })?;
                if let Some(obj) = doc.as_object_mut() {
                    obj.insert("content_text".into(), serde_json::Value::String(txt));
                    if opts.markdown {
                        obj["meta"]["format"] = serde_json::json!("markdown");
                    }
                    // dsg：omitted 与 read 路径同语义（被截字符数；>0 才占键，缺席=未截断）
                    if omitted > 0 {
                        obj["meta"]["omitted"] = serde_json::json!(omitted);
                    }
                }
                println!("{doc}");
            } else {
                println!("=== {url} ===\n{txt}");
                if truncated {
                    eprintln!("注意：正文超上限已截断（省略 {omitted} 字符；--json 输出在 meta 字段标注）");
                }
            }
            browser_opt = Some(browser);
            return Ok(());
        }

        let title = match &snap {
            Some(s) => s.title.clone(),
            None => postproc::eval_string_retry(&page, "document.title").await,
        };
        let html_full = html_probe;
        let (html, truncated, omitted) = postproc::cap_extract_source(&html_full, opts.max_chars, url);
        let mut read = extract_adaptive(&html, None);
        read.url = url.to_string();
        read.title = title;
        // 打回轮2：browse 空正文保底（PM 公式两形态：截断吃光 / content 拿空）——
        // agent 一轮自救（--markdown 或 --full），不再静默空输出
        if needs_empty_body_hint(read.summary_paragraphs.len(), opts.headings_only, omitted, html_full.trim().is_empty()) {
            eprintln!("[hint] 正文提取为空，试 --markdown 或 --full");
        }

        let out = postproc::render_read(&read, opts.json, opts.headings_only, opts.from, truncated, omitted, false);
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
    // pkp：scheme 白名单（login 无 --allow-private，私网不设门——内网登录页是合法场景）
    let url = match browsable_scheme_ok(url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("error: {e:#}");
            return Ok(ExitCode::from(2));
        }
    };
    let url = url.as_str(); // 规范化后仍按 &str 流转
    let mut browser_opt: Option<chromiumoxide::browser::Browser> = None;
    let result: Result<bool> = async {
        let (browser_inst, handler) = launch_with_kind_proxy(false, browser, proxy).await?;
        let _h = spawn_handler(handler);
        browser_opt = Some(browser_inst);

        let page = open_page(browser_opt.as_ref().unwrap()).await?;
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

/// `dl <url> [-o DIR|FILE] [--output-file FILE]`：
/// v97 先 reqwest HEAD 预检——无 Set-Cookie 且非 text/html 的公开直链直接流式 GET 落盘（mode: direct），
/// 免 Chrome 导航 30s；有登录墙/挑战嫌疑才起 Chrome（mode: browser，profile 登录态）。
/// i9a 消歧义：-o 末段带扩展名 = 文件路径；纯目录名 = 目录语义（README 不变）；--output-file 显式文件。
/// 渲染型 URL（普通网页，Chrome 不触发下载）回退页内 fetch 落盘（PLAN §3.5 raw-file 路径，同源 cookie）。
pub async fn cmd_dl(url: &str, output: Option<&Path>, output_file: Option<&Path>, browser: Option<BrowserKind>, proxy: Option<String>) -> Result<ExitCode> {
    use crate::postproc;
    // pkp：scheme 白名单（与 browse/login 同门快失败；dl 私网不设门——内网下载走既有 browser 回退路径）
    let url = match browsable_scheme_ok(url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("error: {e:#}");
            return Ok(ExitCode::from(2));
        }
    };
    let url = url.as_str(); // 规范化后仍按 &str 流转
    // 7z0：相对路径禁 .. 穿越段（防静默落盘出 CWD，URL/文件名可能来自不可信上下文）；
    // 绝对路径 = 用户显式指定，放行。
    if let Some(p) = [output, output_file].into_iter().flatten().find(|p| has_parent_traversal(p)) {
        eprintln!(
            "error: dl -o/--output-file 相对路径不允许包含 ..（防穿越 CWD 落盘）；确需外部路径请用绝对路径: {}",
            p.display()
        );
        return Ok(ExitCode::from(2));
    }
    let (dir, file_target) = resolve_dl_target(output, output_file)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("创建下载目录失败: {}", dir.display()))?;
    // v97 直链快路径；direct 一旦进入下载阶段失败 → Err 冒泡（不伪装回退，半截文件留给用户判断重试）。
    let direct_path = file_target.clone().unwrap_or_else(|| dir.join(filename_from_url(url)));
    if let Some(size) = dl_direct(url, proxy.as_deref(), &direct_path).await? {
        println!("mode: direct");
        println!("已下载: {} ({size} bytes)", direct_path.display());
        pdf_hint(&direct_path);
        return Ok(ExitCode::SUCCESS);
    }

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
        let page = open_page(browser_inst).await?;
        // I4：goto 失败/超时 warn 留痕后继续——下载靠原生下载嗅探或页内 fetch 兜底。
        let _ = postproc::goto_for_download(page.goto(url), url).await;

        match wait_new_file(&dir, &before).await? {
            Some(name) => {
                let mut path = dir.join(&name);
                // i9a：Chrome 自命名 ≠ 目标文件名 → rename 到位（同 dir 下，无跨盘风险）。
                if let Some(target) = &file_target
                    && path != *target
                {
                    std::fs::rename(&path, target)
                        .with_context(|| format!("重命名下载产物失败: {} → {}", path.display(), target.display()))?;
                    path = target.clone();
                }
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                println!("mode: browser");
                println!("已下载: {} ({size} bytes)", path.display());
                pdf_hint(&path);
            }
            None => {
                let bytes = postproc::fetch_in_page(&page, url).await?;
                if bytes.is_empty() {
                    return Err(anyhow!("下载内容为空（{url}"));
                }
                let path = file_target.clone().unwrap_or_else(|| dir.join(filename_from_url(url)));
                std::fs::write(&path, &bytes).with_context(|| format!("写文件失败: {}", path.display()))?;
                println!("mode: browser");
                println!("已下载: {} ({})", path.display(), bytes.len());
                pdf_hint(&path);
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

/// q34：二进制 PDF 落盘后的 stderr 提示（三条 dl 落盘路径共用；本地不解析文本，agent 用外部工具提取）。
pub(crate) fn pdf_hint(path: &Path) {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        eprintln!("提示: 二进制 PDF 已保存（本地未解析文本）；agent 可用外部工具提取");
    }
}

/// i9a：dl 输出目标消歧义。--output-file 显式文件；-o 末段带 '.' 视为文件路径；否则目录语义（README 不变）。
/// 返回 (目录, 指定文件名)。目录恒为绝对路径（Chrome download_path 与落盘都需要）。
/// 已知边界：目录名本身带 '.'（如 v0.2.9/）会被视为文件——help 已注明用 --output-file 消歧义。
fn resolve_dl_target(output: Option<&Path>, output_file: Option<&Path>) -> Result<(PathBuf, Option<PathBuf>)> {
    let dash_o_file = match output {
        Some(p) if p.file_name().is_some_and(|f| f.to_string_lossy().contains('.')) => {
            Some(std::path::absolute(p)?)
        }
        _ => None,
    };
    match output_file.map(std::path::absolute).transpose()?.or(dash_o_file) {
        Some(f) => {
            let dir = f.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
            Ok((dir, Some(f)))
        }
        None => {
            let dir: PathBuf = std::path::absolute(output.unwrap_or(Path::new(".")))?;
            Ok((dir, None))
        }
    }
}

/// v97 直链快路径：HEAD 预检（无 Set-Cookie 且 Content-Type 非 text/html）→ 流式 GET 落盘。
/// 返回 Ok(Some(size)) = 已落盘；Ok(None) = 登录墙/挑战嫌疑或私网 → 回退 Chrome 老路径。
/// SSRF 门与 fetch 同源（gate_check + 重定向每跳 Policy），allow=false：dl 无 --allow-private 语义，
/// 私网/解析失败一律回退 browser（与旧版全 Chrome 路径行为一致，门不削弱）。
async fn dl_direct(url: &str, proxy: Option<&str>, path: &Path) -> Result<Option<u64>> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    if let Err(e) = crate::fetch::gate_check(url, false) {
        // yvz：门层 DNS 失败（域名不存在）→ fail-fast rc=1，免起 Chrome（全链 6.1s vs 此处亚秒）。
        // 误伤防护：本机解析器坏/纯代理代解析环境在这里同样报 DNS 失败——先用（可能带代理的）
        // client 复核一次 HEAD，复核仍是 DNS 类失败才判死；复核成功/超时/SSL 一律回退 browser。
        if is_gate_dns_error(&e) {
            let client = crate::fetch::build_client(proxy, false, Duration::from_secs(DL_DIRECT_TIMEOUT_SECS))?;
            if let Err(head_err) = client.head(url).send().await
                && is_dns_error(&head_err)
            {
                anyhow::bail!("域名不存在（DNS 解析失败），跳过浏览器下载: {url}");
            }
        }
        return Ok(None);
    }
    let client = crate::fetch::build_client(proxy, false, Duration::from_secs(DL_DIRECT_TIMEOUT_SECS))?;
    let head = match client.head(url).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return Ok(None), // HEAD 被拒/不支持/网络错 → 无法判定 → browser
    };
    let html_ct = head
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.to_ascii_lowercase().contains("html"))
        .unwrap_or(false);
    if html_ct || head.headers().get(reqwest::header::SET_COOKIE).is_some() {
        return Ok(None); // cookie 挑战/HTML 登录墙嫌疑 → Chrome（profile 登录态正有用武之地）
    }
    let resp = match client.get(url).send().await {
        Ok(r) => r,
        // 预检通过但 GET 被掐（网络抖动/RST 常见）→ 与 HEAD 失败同语义回退 browser；此刻文件未创建零副作用
        Err(e) => {
            eprintln!("direct GET 失败（{e}），回退浏览器下载");
            return Ok(None);
        }
    };
    if !resp.status().is_success() {
        return Ok(None);
    }
    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("创建文件失败: {}", path.display()))?;
    let mut size: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("direct 下载中断")?;
        file.write_all(&chunk).await.with_context(|| format!("写文件失败: {}", path.display()))?;
        size += chunk.len() as u64;
    }
    file.flush().await.context("flush 下载文件失败")?;
    Ok(Some(size))
}

/// yvz：gate_check 错误是否 DNS 解析类（"host 解析失败/为空"）；私网拒绝、URL 畸形不算。
fn is_gate_dns_error(e: &anyhow::Error) -> bool {
    let s = e.to_string();
    s.contains("host 解析失败") || s.contains("host 解析为空")
}

/// yvz：reqwest 错误链是否 DNS 解析类。getaddrinfo 文案随系统语言变化，
/// 特征串之外兜底 Windows WSA DNS 错误码：11001 WSAHOST_NOT_FOUND / 11002 TRY_AGAIN / 11004 NO_DATA。
fn is_dns_error(e: &reqwest::Error) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && matches!(io.raw_os_error(), Some(11001) | Some(11002) | Some(11004))
        {
            return true;
        }
        let s = err.to_string().to_ascii_lowercase();
        if s.contains("dns error")
            || s.contains("failed to lookup address")
            || s.contains("no such host")
            || s.contains("name or service not known")
            || s.contains("temporary failure in name resolution")
            || s.contains("nodename nor servname")
        {
            return true;
        }
        cur = err.source();
    }
    false
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
    use super::*;

    /// yvz：gate 错误分类——DNS 解析类 true；私网拒绝/URL 畸形 false（不 fail-fast，回退老链路）。
    #[test]
    fn gate_dns_error_classification() {
        assert!(is_gate_dns_error(&anyhow::anyhow!("host 解析失败: no-such-zzz.invalid")));
        assert!(is_gate_dns_error(&anyhow::anyhow!("host 解析为空: x")));
        assert!(!is_gate_dns_error(&anyhow::anyhow!("URL 无 scheme: example.com")));
        assert!(!is_gate_dns_error(&anyhow::anyhow!(
            "fetch 拒绝私网地址 127.0.0.1（host=localhost）"
        )));
    }

    /// h90：browse goto 超时文案带 --proxy 出口建议（B 受试者扣分点：超时裸报无出口）。
    #[test]
    fn goto_timeout_error_contains_proxy_hint() {
        let msg = goto_timeout_error("https://slow.example.com").to_string();
        assert!(msg.contains("--proxy"), "{msg}");
        assert!(msg.contains("https://slow.example.com"), "{msg}");
        // 复测残口：chromiumoxide 内部超时先于外层包装触发，传输层失败同给出口
        let msg = goto_nav_error("https://192.0.2.1/", "Request timed out.").to_string();
        assert!(msg.contains("--proxy"), "{msg}");
        assert!(msg.contains("Request timed out."), "{msg}");
    }

    /// 打回轮2：302 竞态探针——空/极短 content 判未落定（重 goto+content 一次），正常正文不触发。
    #[test]
    fn content_needs_settle_retry_threshold() {
        assert!(content_needs_settle_retry(""));
        assert!(content_needs_settle_retry("   \n  "));
        assert!(content_needs_settle_retry("<html><body>partial"), "极短判未落定");
        assert!(!content_needs_settle_retry(&"x".repeat(200)), "达到阈值不重试");
    }

    /// 打回轮2：browse 空正文保底判定——PM 公式两形态（截断吃光 / content 拿空）都给出口；
    /// headings-only 不适用；有正文不适用。
    #[test]
    fn empty_body_hint_gate() {
        // 形态一：截断吃光（B 上轮 omitted=589331）
        assert!(needs_empty_body_hint(0, false, 589_331, false));
        // 形态二：content 拿空（302 竞态，omitted=0）
        assert!(needs_empty_body_hint(0, false, 0, true));
        // 不适用：headings-only / 有正文 / 无截断有 content（真·无段落小页）
        assert!(!needs_empty_body_hint(0, true, 589_331, false));
        assert!(!needs_empty_body_hint(10, false, 0, false));
        assert!(!needs_empty_body_hint(3, false, 0, false));
    }

    /// pkp：scheme 白名单表——http/https 放行；file:///javascript:/data:/ftp:/chrome: 拒；
    /// 无 scheme 裸 host 按浏览器默认语义补 https://（含空白 trim）。
    #[test]
    fn browsable_scheme_whitelist_table() {
        // 白名单原样放行
        assert_eq!(browsable_scheme_ok("https://example.com").unwrap(), "https://example.com");
        assert_eq!(browsable_scheme_ok("http://example.com/a?b=1").unwrap(), "http://example.com/a?b=1");
        assert_eq!(browsable_scheme_ok("  HTTPS://Example.com  ").unwrap(), "HTTPS://Example.com");
        // 无 scheme → 补 https://（goto 前规范化，Chrome 语义一致）
        assert_eq!(browsable_scheme_ok("example.com/page").unwrap(), "https://example.com/page");
        // 非白名单快失败
        for bad in [
            "file:///D:/gsbt5/secret.html",
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "data:text/html,<script>x</script>",
            "ftp://example.com/f.bin",
            "chrome://settings",
            "file:C:/x/y.html",
            "JAVASCRIPT:alert(1)",
        ] {
            assert!(browsable_scheme_ok(bad).is_err(), "{bad} 应被拒");
        }
        // host:port 裸形态放行补全（与 javascript: 区分）
        assert_eq!(browsable_scheme_ok("localhost:3000/app").unwrap(), "https://localhost:3000/app");
        assert!(browsable_scheme_ok("   ").is_err(), "空 URL 应被拒");
    }

    /// pkp：私网门判定表（字面 IP 不走 DNS）——loopback/RFC1918/link-local/::1 默认拒，
    /// --allow-private 放行；公网字面 IP 过；scheme 白名单先于私网门（file:// 即使放行也拒）。
    #[test]
    fn browsable_private_gate_table() {
        for url in [
            "http://127.0.0.1:8888/",
            "http://10.0.0.5/x",
            "http://172.16.1.1/a",
            "http://192.168.89.249:8888/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]:8888/",
        ] {
            assert!(ensure_browsable_url(url, false).is_err(), "{url} 默认应拒");
            assert!(ensure_browsable_url(url, true).is_ok(), "{url} --allow-private 应放行");
        }
        assert!(ensure_browsable_url("http://93.184.216.34/", false).is_ok(), "公网字面 IP 应过私网门");
        // scheme 白名单先行：file:// 无 flag 可绕
        assert!(ensure_browsable_url("file:///C:/Windows/win.ini", true).is_err());
        // 错误文案带 --allow-private 指引（browse 语义，非 fetch 文案）
        let err = ensure_browsable_url("http://127.0.0.1:1/", false).unwrap_err().to_string();
        assert!(err.contains("--allow-private") && err.contains("browse"), "{err}");
    }

    /// 7z0：dl -o/--output-file 相对路径 .. 穿越段拒绝；绝对路径（含 ..）显式放行。
    #[test]
    fn dl_output_parent_traversal_table() {
        assert!(has_parent_traversal(std::path::Path::new("../upone.bin")));
        assert!(has_parent_traversal(std::path::Path::new("a/../../b.bin")));
        assert!(has_parent_traversal(std::path::Path::new("..")));
        assert!(!has_parent_traversal(std::path::Path::new("sub/out.bin")));
        assert!(!has_parent_traversal(std::path::Path::new("D:/out/x.bin")));
        assert!(!has_parent_traversal(std::path::Path::new("D:/out/../x.bin")), "绝对路径显式放行");
    }

    /// i9a：-o 末段带扩展名 = 文件；纯目录名 = 目录；--output-file 优先；都缺省 = CWD 目录。
    #[test]
    fn resolve_dl_target_disambiguation() {
        // -o 带扩展名 → 文件语义（dir = 其父目录）
        let (dir, file) = resolve_dl_target(Some(Path::new("out/foo.exe")), None).unwrap();
        assert_eq!(file.unwrap().file_name().unwrap(), "foo.exe");
        assert!(dir.ends_with("out"), "dir: {}", dir.display());
        // -o 纯目录名 → 目录语义（README 行为不变）
        let (dir, file) = resolve_dl_target(Some(Path::new("out/dir")), None).unwrap();
        assert!(file.is_none());
        assert!(dir.ends_with("dir"), "dir: {}", dir.display());
        // --output-file 显式文件，压过 -o
        let (dir, file) = resolve_dl_target(Some(Path::new("outdir")), Some(Path::new("out/bin/file.bin"))).unwrap();
        assert_eq!(file.unwrap().file_name().unwrap(), "file.bin");
        assert!(dir.ends_with("bin"), "dir: {}", dir.display());
        // 都不给 → CWD 目录
        let (dir, file) = resolve_dl_target(None, None).unwrap();
        assert!(file.is_none());
        assert!(dir.is_absolute());
    }

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
