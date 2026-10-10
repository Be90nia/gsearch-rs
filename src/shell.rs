//! M7 交互式 shell 子命令（PLAN §3.7 追加里程碑）。
//!
//! 起一次 headless Chrome + 维持后台会话复用，cookie / 页面状态在 prompt 间延续。
//! 与顶层一次性 search/browse/dl 不同：shell 内部命令都复用 `ShellCtx.browser` + `ShellCtx.page`，
//! 退出（EOF / exit / quit）才走 graceful 关 Chrome。
//!
//! 单 exe "用完即走" 原则不破：shell 是可选模式，顶层命令全部不变。

use std::path::Path;
use std::io::{self, BufRead, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use chromiumoxide::Page;
use chromiumoxide::browser::Browser;

use gsearch::browser;
use gsearch::output::print_text;
use gsearch::search::{SearchConfig, SearchOutcome, is_captcha, run_search};
use gsearch::skeleton::extract_adaptive;
use gsearch::types::SearchResult;
use gsearch::util::filename_from_url;

use crate::general::browser_alive;
use crate::postproc::{cap_chars, content_retry, read_max_chars, render_read, wait_content_stable};

const TEXT_MAX_CHARS: usize = 5000;
const PAGE_TIMEOUT_SECS: u64 = 30;
const PROMPT: &str = "gsearch> ";

use crate::shell_snap::{
    ClickTarget, SnapElem, click_snap_elem, find_snap_elem, format_snap_line, parse_click_target,
    snap_page,
};

/// M9 shell `read` / `browse` 选项集。与 postproc::ReadOpts / general::BrowseOpts 字段一致。
#[derive(Debug, Clone, Default)]
struct ReadShellOpts {
    full: bool,
    json: bool,
    headings_only: bool,
    from: usize,
}


/// shell 会话上下文：一次启动的 Chrome + 主 page + 上次搜索结果 + 当前 URL。
/// 整个 shell 生命周期内复用，跨 prompt 保持 cookie / 页面状态。
pub struct ShellCtx {
    pub browser: Browser,
    pub page: Page,
    pub last_results: Vec<SearchResult>,
    pub last_snap: Vec<SnapElem>,
    pub current_url: String,
    /// M16：swap_to_headed 会 abort 旧 handler task 再起新 task，避免 chromiumoxide 0.9.1
    /// "send failed because receiver is gone"（旧 handler 还跑着，新 Browser sender 没人接）
    pub handler_task: Option<tokio::task::JoinHandle<()>>,
    /// M17：撞 CAPTCHA 时人工按 Enter 立即加速 poll loop 退出（不需等 1s tick 或超时）
    pub human_solved: Arc<AtomicBool>,
}
/// 起一次 headless Chrome，进入 `gsearch> ` REPL；EOF / Ctrl+D 走 graceful 关闭。
/// exit / quit 直接退出（8i3：banner/help/实际行为三者一致），rc=0。
pub async fn run_shell() -> Result<ExitCode> {
    let (browser, handler) = browser::launch(true).await.context("启动 Chrome 失败")?;
    let handler_task = Some(browser::spawn_handler(handler));

    // n9j（对齐 main.rs H2 收尾）：browser 停在外层 slot，内层 async 块任何 ? 早退（page
    // 创建失败）由外层统一 graceful_close 再返回——Browser::drop 在 Windows 上不杀子进程，
    // 裸 ? 早退会留 chrome.exe 残留 + profile 锁。
    let mut slot: Option<Browser> = Some(browser);
    let rc: Result<ExitCode> = async {
        let page = browser::open_page(slot.as_ref().expect("browser 刚放入 slot"))
            .await
            .context("创建初始 page 失败")?;
        let browser = slot.take().expect("browser 刚放入 slot");
        let mut ctx = ShellCtx {
            browser,
            page,
            last_results: Vec::new(),
            last_snap: Vec::new(),
            current_url: String::new(),
            handler_task,
            human_solved: Arc::new(AtomicBool::new(false)),
        };
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        println!("进入 gsearch shell（输入 help 查命令，exit / quit / Ctrl+D 退出）");
        let mut stdout = io::stdout();
        let mut buf = String::new();
        loop {
            buf.clear();
            print!("{PROMPT}");
            // n9j：prompt 刷写失败（stdout 断开）不再 `?` 早退——按 EOF 走 break，
            // 保证 REPL 结束后 graceful_close 必达。
            if stdout.flush().is_err() {
                break;
            }
            let n = reader.read_line(&mut buf).unwrap_or(0);
            if n == 0 {
                // EOF：Ctrl+D（Unix）或 Ctrl+Z 回车（Windows）；read_line Err 同按 EOF 处理
                println!();
                break;
            }
            // 命令行输入也算"用户活跃"——重置 human_solved 让下条 search 命令不会被旧信号立即 break
            ctx.human_solved.store(false, std::sync::atomic::Ordering::Relaxed);
            let line = buf.trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(cmd) = parts.next() else {
                continue;
            };
            // 8i3：quit/exit 真退出（graceful 关 Chrome 后 rc=0）；EOF（read_line=0）同样 break。
            if cmd == "exit" || cmd == "quit" {
                break;
            }
            let args: Vec<&str> = parts.collect();
            if let Err(e) = dispatch(cmd, &args, &mut ctx).await {
                eprintln!("error: {e}");
                for cause in e.chain().skip(1) {
                    eprintln!(" 原因: {cause}");
                }
            }
        }
        gsearch::browser::graceful_close(&mut ctx.browser).await;
        Ok(ExitCode::SUCCESS)
    }
    .await;
    // 内层 ? 早退且 browser 尚未移入 ctx（page 创建失败路径）→ slot 仍有 browser，兜底关。
    if let Some(mut b) = slot.take() {
        gsearch::browser::graceful_close(&mut b).await;
    }
    rc
}

/// M2 迁移：以前 shell 自带一个本地 graceful_close；现在统一走 gsearch::browser::graceful_close
/// （带超时 + kill 兜底，与顶层命令收尾路径完全一致）。
/// 分派单条 shell 命令
async fn dispatch(cmd: &str, args: &[&str], ctx: &mut ShellCtx) -> Result<()> {
    match cmd {
        "help" | "?" => {
            print_help();
            Ok(())
        }
        "search" => cmd_search(args, ctx).await,
        "click" | "open" => cmd_click(args, ctx).await,
        "snap" | "snapshot" => cmd_snap(ctx).await,
        "read" => cmd_read(args, ctx).await,
        "dl" => cmd_dl(args, ctx).await,
        "browse" => cmd_browse(args, ctx).await,
        "login" => cmd_login(args, ctx).await,
        "back" => cmd_back(ctx).await,
        "status" => cmd_status(ctx).await,
        other => Err(anyhow!("未知命令: {other:?}（输入 help 查命令列表）")),
    }
}

fn print_help() {
    println!(
        "shell 命令集：\n\
         search <query> [--limit N]   Google 搜索\n\
         browse <url>                 渲染 + 取正文\n\
         snap                         列出可交互元素 (@eN)\n\
         click N | @eN                跳到第 N 结果 / 第 N snap 元素\n\
         read [N] [--full]            读第 N 结果正文\n\
         dl N [--output DIR]          下载第 N 个结果\n\
         login <url>                  有头窗登录\n\
         back / status                后退 / / 当前 URL + 标题\n\
         help                         帮助\n\
         exit / quit                  退出（rc=0；EOF / Ctrl+D 同效）"
    );
}
async fn cmd_search(args: &[&str], ctx: &mut ShellCtx) -> Result<()> {
    let (query, limit) = parse_search_args(args)?;
    let outcome = run_search(
        &mut ctx.browser,
        SearchConfig {
            query: query.clone(),
            limit,
            recency: None,
        },
        &mut ctx.handler_task,
        ctx.human_solved.clone(),
    )
    .await?;
    let (results, provider) = match outcome {
        SearchOutcome::Results { results, provider, .. } => (results, provider),
        SearchOutcome::CaptchaTimeout => {
            println!("CAPTCHA 亲解超时（{}s）—— profile 已养熟，下次 search 会自动跳过",
                gsearch::search::CAPTCHA_TIMEOUT_SECS);
            // I3：超时后浏览器还在 headed（CAPTCHA 路径 swap_to_headed 起 headed）；
            // 切回 headless 并重建 page，让下条命令（read / dl / browse）能用。
            if let Err(e) = browser::swap_to_headless(&mut ctx.browser, &mut ctx.handler_task).await {
                eprintln!("切回 headless 失败: {e}");
                return Err(e);
            }
            ctx.page = browser::open_page(&ctx.browser)
                .await
                .context("切回 headless 后创建 page 失败")?;
            ctx.current_url.clear();
            return Ok(());
        }
    };
    // 随旧 browser 死掉（后续命令报 "receiver is gone"）。检测失效即重建。
    if ctx.page.evaluate("1").await.is_err() {
        ctx.page = browser::open_page(&ctx.browser)
            .await
            .context("CAPTCHA 换 browser 后重建 page 失败")?;
    }
    if let Ok(Some(u)) = ctx.page.url().await {
        ctx.current_url = u;
    }
    print_text(&results);
    if results.is_empty() {
        println!("（搜索无结果）");
    } else {
        println!("共 {} 条结果（来源: {provider}；输入 click N / read / dl N 继续）", results.len());
    }
    ctx.last_results = results;
    Ok(())
}

/// `search <query> [--limit N]`：query 拼接 `--limit` 之前所有 token；limit 缺省 10。
fn parse_search_args(args: &[&str]) -> Result<(String, usize)> {
    let mut query_parts: Vec<&str> = Vec::new();
    let mut limit: usize = 10;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--limit" {
            i += 1;
            let v = args.get(i).ok_or_else(|| anyhow!("--limit 缺值"))?;
            limit = v.parse().map_err(|_| anyhow!("--limit 非数字: {v:?}"))?;
            if limit == 0 {
                return Err(anyhow!("--limit 必须 ≥ 1"));
            }
        } else {
            query_parts.push(args[i]);
        }
        i += 1;
    }
    if query_parts.is_empty() {
        return Err(anyhow!("search 缺 query"));
    }
    Ok((query_parts.join(" "), limit))
}

/// shell `read` / `browse` 通用 flag → ReadShellOpts。支持 `--full` / `--json` / `--headings-only` / `--from K`。
/// 与顶层 CLI 的 flag 名一致（agent 心智统一）；非零退出码 = 错误（含未知 flag）。
fn parse_shell_read_opts(args: &[&str], cmd_name: &str) -> Result<ReadShellOpts> {
    let mut opts = ReadShellOpts::default();
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--full" => opts.full = true,
            "--json" => opts.json = true,
            "--headings-only" => opts.headings_only = true,
            "--from" => {
                i += 1;
                let v = args.get(i).ok_or_else(|| anyhow!("--from 缺值"))?;
                opts.from = v.parse().map_err(|_| anyhow!("--from 非数字: {v:?}"))?;
            }
            other => return Err(anyhow!("{cmd_name} 未知 flag: {other:?}")),
        }
        i += 1;
    }
    Ok(opts)
}

async fn cmd_click(args: &[&str], ctx: &mut ShellCtx) -> Result<()> {
    let a = args.first().ok_or_else(|| anyhow!("click 缺参数（N 或 @eN）"))?;
    match parse_click_target(a)? {
        ClickTarget::Idx(n) => {
            if n == 0 || n > ctx.last_results.len() {
                return Err(anyhow!("click {n} 越界（结果数 {}）", ctx.last_results.len()));
            }
            let url = ctx.last_results[n - 1].url.clone();
            // 4bq M3 收尾：结果集 URL = 不可信输入，与顶层 search --read N 同门（SEO 毒化防一线）
            let url = crate::general::ensure_browsable_url(&url, false)?;
            goto(&ctx.page, &url).await?;
            ctx.current_url = url.clone();
            println!("已跳转到: {url}");
        }
        ClickTarget::Ref(r) => {
            let el = find_snap_elem(&ctx.last_snap, &r)?.clone();
            if el.tag == "a" && !el.href.is_empty() {
                goto(&ctx.page, &el.href).await?;
                ctx.current_url = el.href.clone();
                println!("已跳转到: {}", el.href);
            } else {
                click_snap_elem(&ctx.page, &el).await?;
                // 元素 click 可能触发导航（如 onclick location.href），导航销毁旧 JS context，
                // 紧跟的 evaluate 撞 CDP -32000 "Cannot find context"。j44：等语义定稿走
                // postproc::wait_content_stable——marker 连续两次相同才过。旧版只等 readyState，
                // 晚跳转（setTimeout 后 location）在旧页 complete 时首轮漏判（原 settle_after_click
                // 的 ponytail 记账项，本次由 marker 判稳闭合：跳转触发后 evaluate Err 重置 marker，
                // 在新页重新积累到连续两次相同才放行）。无导航时两轮快照（200ms 间隔）即过，
                // 无固定 sleep；窗口 20×200ms ≈ 4s 与旧版量级一致。
                wait_content_stable(&ctx.page, 20).await;
                if let Some(u) = ctx.page.url().await.ok().flatten() {
                    ctx.current_url = u;
                }
                println!("已点击 {r} <{}>", el.tag);
            }
        }
    }
    Ok(())
}

/// `snap` / `snapshot`：抓当前页可交互元素，打印 eN ref 列表存入 last_snap。
/// 空列表也存（旧 ref 对新页面失效应清掉）。
async fn cmd_snap(ctx: &mut ShellCtx) -> Result<()> {
    let elems = snap_page(&ctx.page).await?;
    if elems.is_empty() {
        println!("（无可交互元素）");
    } else {
        for e in &elems {
            println!("{}", format_snap_line(e));
        }
        println!("共 {} 个元素（click @eN 点击）", elems.len());
    }
    ctx.last_snap = elems;
    Ok(())
}

async fn cmd_read(args: &[&str], ctx: &mut ShellCtx) -> Result<()> {
    let opts = parse_shell_read_opts(args, "read")?;
    // 7l0：content_retry 替代裸 content().unwrap_or_default()——-32000 context 重建窗口期
    // 不再静默吞成空串。
    let html_full = content_retry(&ctx.page).await;
    if is_captcha(&html_full) {
        return Err(anyhow!(
            "当前页遇 CAPTCHA：用 `login <url>` 切有头窗人工验证后再回 shell"
        ));
    }
    // 7l0：空正文 stderr hint，与顶层 read 的 [hint] 同款（shell 无 --markdown，指到 --full）。
    if html_full.trim().is_empty() && !opts.full {
        eprintln!("[hint] 正文提取为空，试 --full");
    }
    // --full：纯 innerText 5000 字
    if opts.full {
        let text = ctx
            .page
            .evaluate("document.body.innerText")
            .await?
            .into_value::<String>()
            .unwrap_or_default();
        let text: String = text.chars().take(TEXT_MAX_CHARS).collect();
        let cur = if ctx.current_url.is_empty() { "(未设)" } else { &ctx.current_url };
        println!("=== {cur} ===\n{text}");
        return Ok(());
    }
    // jp4：与顶层 read 同一截断契约（read_max_chars 硬顶 + --json meta 标注）
    let (html, truncated, omitted, truncated_at_offset) = cap_chars(&html_full, read_max_chars());
    let title = ctx
        .page
        .evaluate("document.title")
        .await?
        .into_value::<String>()
        .unwrap_or_default();
    let mut read = extract_adaptive(&html, None);
    read.url = if ctx.current_url.is_empty() { "(未设)".into() } else { ctx.current_url.clone() };
    read.title = title;
    let out = render_read(
        &read,
        opts.json,
        opts.headings_only,
        opts.from,
        truncated,
        omitted,
        truncated_at_offset,
        false,
    );
    println!("{out}");
    Ok(())
}

/// `dl [N] [-o DIR]`：下载 last_results[N-1].url（N 缺省走 current_url）。
/// `-o DIR` 把文件落到 DIR 下；缺省写 CWD（filename_from_url 末段）。M13 修三处一致。
/// ponytail: 不引 clap，shell 内嵌 5 行 flag parser 同 `parse_shell_read_opts`，
/// 同名 `-o DIR / --output DIR` 优先顺序最简单：扫一遍 args，遇 flag 收 value，遇到位置 token 收 N。
/// 拒绝多余位置参数（不暗中吞掉）。三处一致基于 `dir = std::path::absolute(output.unwrap_or_else(|| Path::new(".")))?` + `dir.join(filename_from_url(url))` 共用式（仅出现在 dl_in_page，cmd_dl 只搬运 output）。
async fn cmd_dl(args: &[&str], ctx: &mut ShellCtx) -> Result<()> {
    // 解析 flag：-o DIR / --output DIR，剩余第一个 token 是 N（可缺省）。
    let mut n_token: Option<&str> = None;
    let mut output: Option<&Path> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-o" | "--output" => {
                i += 1;
                let v = args.get(i).ok_or_else(|| anyhow!("dl -o 缺值"))?;
                output = Some(Path::new(v));
            }
            other => {
                if n_token.is_some() {
                    return Err(anyhow!("dl 多余位置参数: {other:?}（只接受 N）"));
                }
                n_token = Some(other);
            }
        }
        i += 1;
    }
    let url = match n_token {
        Some(n_str) => {
            // 数字 → 取第 N 条搜索结果；其他 → 视为 URL 直下
            if let Ok(n) = n_str.parse::<usize>() {
                if n == 0 || n > ctx.last_results.len() {
                    return Err(anyhow!("dl {n} 越界（结果数 {}）", ctx.last_results.len()));
                }
                ctx.last_results[n - 1].url.clone()
            } else {
                n_str.to_string()
            }
        }
        None => {
            if ctx.current_url.is_empty() {
                return Err(anyhow!("dl 缺 N/URL 且 current_url 未设置（先 browse 或 click）"));
            }
            ctx.current_url.clone()
        }
    };
    // 3t9：与顶层 dl 同门——仅 scheme 白名单（dl 私网不设门，内网下载走既有页内 fetch 路径）
    let url = crate::general::browsable_scheme_ok(&url)?;
    dl_in_page(&ctx.page, &url, output).await
}

/// 页内 fetch → 字节落盘（I4/I5：goto 预算与 fetch 实现共用 postproc，本函数只管 URL 解析与落盘）。
/// `output` 缺省落 CWD；提供时 create_dir_all(DIR) + DIR.join(filename)，与 postproc::dl / general::cmd_dl 三处行为一致。
async fn dl_in_page(page: &Page, url: &str, output: Option<&Path>) -> Result<()> {
    use crate::postproc;
    let _ = postproc::goto_for_download(page.goto(url), url).await;
    // goto 已跟完重定向；fetch 当前页真实 URL（相对当前页同源，绕开 /goto?url= 类
    // 重定向链的 CORS 限制——真机踩坑：dl 1 对搜索结果的 google.com/goto 链直接 Failed to fetch）。
    let final_url = page
        .url()
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| url.to_string());
    let bytes = postproc::fetch_in_page(page, &final_url).await?;
    if bytes.is_empty() {
        return Err(anyhow!("下载内容为空（{url}）"));
    }
    let dir = std::path::absolute(output.unwrap_or_else(|| Path::new(".")))?;
    let path = dir.join(filename_from_url(&final_url));
    std::fs::write(&path, &bytes).with_context(|| format!("写文件失败: {}", path.display()))?;
    println!("已下载: {} ({} bytes)", path.display(), bytes.len());
    crate::general::pdf_hint(&path);
    Ok(())
}

async fn cmd_browse(args: &[&str], ctx: &mut ShellCtx) -> Result<()> {
    let url = args.first().ok_or_else(|| anyhow!("browse 缺 url"))?;
    // 3t9：与顶层 browse 同门（scheme 白名单 + 私网拒）。shell 无 --allow-private flag，
    // 需内网页面请退出 shell 用顶层命令。
    let url = crate::general::ensure_browsable_url(url, false).map_err(|e| {
        anyhow!(
            "{e:#}\n（shell 内无 --allow-private flag；确需内网页面请退出 shell 后用顶层 `gsearch browse <url> --allow-private`）"
        )
    })?;
    goto(&ctx.page, &url).await?;
    // 7l0：与顶层 browse 对齐——goto 后等语义定稿（50×200ms ≈ 10s），避免重定向竞态
    // 拿到旧 context / 风控页假 complete。
    wait_content_stable(&ctx.page, 50).await;
    ctx.current_url = url.to_string();
    let title = ctx
        .page
        .evaluate("document.title")
        .await?
        .into_value::<String>()
        .unwrap_or_default();
    println!("已跳转: {url} | {title}");
    Ok(())
}

async fn cmd_login(args: &[&str], ctx: &mut ShellCtx) -> Result<()> {
    let url = args.first().ok_or_else(|| anyhow!("login 缺 url"))?;
    // 3t9：与顶层 login 同门——仅 scheme 白名单（login/dl 无私网门，内网登录页是合法场景）
    let url = crate::general::browsable_scheme_ok(url)?;
    // 先切有头（close + 同 profile 重起），保持 cookie 不丢
    browser::swap_to_headed(&mut ctx.browser, &mut ctx.handler_task).await?;
    // 有头模式下旧 page 已随旧 browser 关闭，新开一个
    ctx.page = browser::open_page(&ctx.browser)
        .await
        .context("有头模式创建 page 失败")?;
    goto(&ctx.page, &url).await?;
    ctx.current_url = url.to_string();
    // 记录登录页 URL；用户登录成功跳到 dashboard = URL 变化 = 登录完成（bug fix）。
    // ponytail: 旧版只判 evaluate 失败 + page 死了；登录后跳到 dashboard，evaluate 仍成功 →
    // 死循环，只能 Ctrl+C。复用 general.rs 的纯函数判定。
    let initial_url = ctx
        .page
        .url()
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| url.to_string());
    tracing::info!("已打开登录窗口: {url}，完成登录后直接关窗即可");

    // 轮询直到用户关窗（page 死了 / browser 死了）或登录后 URL 变化。
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let evaluate_ok = ctx.page.evaluate("1").await.is_ok();
        let current_url = ctx.page.url().await.ok().flatten().unwrap_or_default();
        let page_still_attached = browser_alive(&ctx.browser).await
            && ctx
                .browser
                .pages()
                .await
                .map(|ps| ps.iter().any(|p| p.target_id() == ctx.page.target_id()))
                .unwrap_or(false);
        if crate::general::login_poll_decision(
            evaluate_ok,
            &initial_url,
            &current_url,
            page_still_attached,
        ) {
            break;
        }
    }
    println!("检测到登录窗口关闭，cookie 已落 profile");

    // 提示是否切回 headless
    print!("切回 headless 模式？[Y/n]: ");
    io::stdout().flush().ok();
    let mut ans = String::new();
    io::stdin().read_line(&mut ans)?;
    let cut = ans.trim();
    if cut.is_empty() || cut.eq_ignore_ascii_case("y") || cut.eq_ignore_ascii_case("yes") {
        browser::swap_to_headless(&mut ctx.browser, &mut ctx.handler_task).await?;
        ctx.page = browser::open_page(&ctx.browser)
            .await
            .context("切回 headless 后创建 page 失败")?;
        ctx.current_url.clear();
        println!("已切回 headless，page 已重建");
    } else {
        println!("保持有头模式，shell 继续（按需退出）");
    }
    Ok(())
}

async fn cmd_back(ctx: &mut ShellCtx) -> Result<()> {
    // chromiumoxide 0.9 未暴露 go_back；走 DOM history.back()，JS API 不依赖 CDP method
    ctx.page
        .evaluate("history.back()")
        .await
        .map_err(|e| anyhow!("后退失败: {e}"))?;
    let url = ctx
        .page
        .url()
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    ctx.current_url = url;
    println!("已后退");
    Ok(())
}

async fn cmd_status(ctx: &mut ShellCtx) -> Result<()> {
    let title = ctx
        .page
        .evaluate("document.title")
        .await?
        .into_value::<String>()
        .unwrap_or_default();
    let profile = browser::profile_dir().ok();
    let cur = if ctx.current_url.is_empty() { "(未设)" } else { &ctx.current_url };
    println!(
        "current_url : {cur}\n\
         title       : {title}\n\
         results     : {}\n\
         profile     : {}",
        ctx.last_results.len(),
        profile
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(获取失败)".into()),
    );
    Ok(())
}

async fn goto(page: &Page, url: &str) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
        .await
        .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
        .map_err(|e| anyhow!("goto {url} 失败: {e}"))?;
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use gsearch::util::b64_decode;

    #[test]
    fn b64_decode_known_vectors() {
        assert_eq!(b64_decode("").unwrap(), b"");
        assert_eq!(b64_decode("QQ==").unwrap(), b"A");
        assert_eq!(b64_decode("QUJD").unwrap(), b"ABC");
        assert_eq!(b64_decode("SGVsbG8sIFdvcmxkIQ==").unwrap(), b"Hello, World!");
    }

    #[test]
    fn filename_from_url_cases() {
        assert_eq!(filename_from_url("https://x.com/a/b/file.pdf?x=1#f"), "file.pdf");
        assert_eq!(filename_from_url("https://example.com/"), "download.bin");
        assert_eq!(filename_from_url("https://example.com"), "download.bin");
        assert_eq!(filename_from_url("https://example.com/index.html"), "index.html");
    }

    #[test]
    fn parse_search_args_cases() {
        // 缺 query
        assert!(parse_search_args(&[]).is_err());
        // 仅 query
        let (q, l) = parse_search_args(&["python"]).unwrap();
        assert_eq!(q, "python");
        assert_eq!(l, 10);
        // query + --limit
        let (q, l) = parse_search_args(&["python", "asyncio", "--limit", "3"]).unwrap();
        assert_eq!(q, "python asyncio");
        assert_eq!(l, 3);
        // --limit 非数字
        assert!(parse_search_args(&["q", "--limit", "abc"]).is_err());
        // --limit 缺值
    assert!(parse_search_args(&["q", "--limit"]).is_err());
    }

    // ---------- P1-5：parse_shell_read_opts（read/browse 共用 flag 解析，纯函数） ----------

    /// 合法 flag 全组合 + --from 0 边界（从头读是合法起点，非缺省态）+ 空参数全默认。
    #[test]
    fn parse_shell_read_opts_accepts_flags_and_from_zero() {
        let o = parse_shell_read_opts(&["--full", "--json", "--headings-only", "--from", "100"], "read").unwrap();
        assert!(o.full && o.json && o.headings_only);
        assert_eq!(o.from, 100);
        let o = parse_shell_read_opts(&["--from", "0"], "read").unwrap();
        assert_eq!(o.from, 0);
        let o = parse_shell_read_opts(&[], "browse").unwrap();
        assert!(!o.full && !o.json && !o.headings_only && o.from == 0);
    }

    /// 未知 flag 必须报错（文案含 cmd 名 + flag 名）；--from 缺值/非数字必须报错。
    /// 防 match 回归成 `_ => {}` 静默吞掉打错的 flag——`read --ful` 将静默按默认执行。
    #[test]
    fn parse_shell_read_opts_rejects_unknown_flag_and_bad_from() {
        let e = parse_shell_read_opts(&["--ful"], "read").unwrap_err();
        let msg = format!("{e:#}");
        assert!(msg.contains("read") && msg.contains("--ful"), "错误须含 cmd 名+flag: {msg}");
        let e = parse_shell_read_opts(&["--jsonx"], "browse").unwrap_err();
        let msg = format!("{e:#}");
        assert!(msg.contains("browse") && msg.contains("--jsonx"), "browse 同模板: {msg}");
        assert!(format!("{:#}", parse_shell_read_opts(&["--from"], "read").unwrap_err()).contains("缺值"));
        assert!(format!("{:#}", parse_shell_read_opts(&["--from", "abc"], "read").unwrap_err()).contains("非数字"));
    }

    // ---------- P1-6 / P1-7：cmd_dl 解析契约与 REPL 状态整替 ----------
    // ShellCtx 持有 chromiumoxide Browser/Page，离线无法构造 → 契约以 #[ignore] live
    // 锚点形式落盘（CI 不跑，手工验证有锚；postproc_live 同款先例）。
    // 断言全部落在触网之前：解析错误 / N 越界 / 空态守卫 / scheme 门，零外网。

    fn live_shell_ctx(
        browser: Browser,
        page: Page,
        handler_task: Option<tokio::task::JoinHandle<()>>,
        last_results: Vec<SearchResult>,
    ) -> ShellCtx {
        ShellCtx {
            browser,
            page,
            last_results,
            last_snap: Vec::new(),
            current_url: String::new(),
            handler_task,
            human_solved: Arc::new(AtomicBool::new(false)),
        }
    }

    fn fixture_result(url: &str) -> SearchResult {
        SearchResult {
            title: "t".into(),
            url: url.into(),
            snippet: String::new(),
            score: None,
            domain_class: "other",
        }
    }

    /// P1-6 + P1-7 离线契约（真 Chrome、零外网）：
    /// dl 多余位置参数 / N=0 / N 越界 / -o 缺值 / 无 N 且 current_url 空 / scheme 门；
    /// 空结果集 click 1 必越界（「旧结果复活」防线）；snap 空列表整替清旧 @eN refs。
    /// 跑法：cargo test --bin gsearch shell_dl_parse_and_repl_state_live -- --ignored
    #[ignore]
    #[tokio::test]
    async fn shell_dl_parse_and_repl_state_live() {
        // chromiumoxide 0.9 铁序：launch → spawn_handler → open_page（不 spawn 则 new_page 永不 resolve）
        let (browser, handler) = gsearch::browser::launch(true).await.expect("launch Chrome");
        let handler_task = Some(gsearch::browser::spawn_handler(handler));
        let page = gsearch::browser::open_page(&browser).await.expect("open page");
        let mut ctx = live_shell_ctx(
            browser,
            page,
            handler_task,
            vec![fixture_result("ftp://x/a")], // scheme 白名单外 → dl 1 在触网前被拒
        );

        // P1-6：多余位置参数拒绝（不暗中吞掉 → `dl 1 2` 不得静默下第 2 条）
        let e = cmd_dl(&["1", "2"], &mut ctx).await.unwrap_err();
        assert!(format!("{e:#}").contains("多余位置参数"), "{e:#}");
        // P1-6：N=0 与越界必须报错（不 panic、不错位下载）
        assert!(format!("{:#}", cmd_dl(&["0"], &mut ctx).await.unwrap_err()).contains("越界"));
        assert!(format!("{:#}", cmd_dl(&["99"], &mut ctx).await.unwrap_err()).contains("越界（结果数 1）"));
        // P1-6：-o 缺值
        assert!(format!("{:#}", cmd_dl(&["-o"], &mut ctx).await.unwrap_err()).contains("缺值"));
        // P1-6：无 N 且 current_url 空
        assert!(format!("{:#}", cmd_dl(&[], &mut ctx).await.unwrap_err()).contains("缺 N/URL"));
        // P1-6：dl 1 与顶层 dl 同款 scheme 门（ftp 白名单外，触网前拒绝）
        let e = cmd_dl(&["1"], &mut ctx).await.unwrap_err();
        assert!(format!("{e:#}").contains("拒绝非 http/https"), "{e:#}");

        // P1-7：空结果集守卫——search 空结果后 last_results 整替为 []，click 1 必越界报错。
        // 若整替回归成「空时保留旧结果」，agent 会点到上一条查询的死链接。
        ctx.last_results.clear();
        let e = cmd_click(&["1"], &mut ctx).await.unwrap_err();
        assert!(format!("{e:#}").contains("越界"), "空结果集 click 1 必须越界报错: {e:#}");

        // P1-7③：snap 空列表也整替（旧 @eN ref 对新页面失效必须清掉；about:blank 离线可达）
        ctx.last_snap = vec![SnapElem {
            ref_id: "e1".into(),
            tag: "a".into(),
            text: "old".into(),
            href: "https://stale.example/x".into(),
            id: String::new(),
            index: 0,
        }];
        cmd_snap(&mut ctx).await.expect("snap about:blank 应成功");
        assert!(ctx.last_snap.is_empty(), "空 snap 必须整替清空旧 @eN refs");

        gsearch::browser::graceful_close(&mut ctx.browser).await;
    }

    /// P1-7① 真链路锚点：真 search 空词路径后 last_results 必须整替为空 → click 1 越界。
    /// 需真 Chrome + 外网（Google 直爬或已配置 SearXNG）；CI 不跑，手工验证有锚。
    /// 跑法：cargo test --bin gsearch shell_search_empty_replaces_results_live -- --ignored
    #[ignore]
    #[tokio::test]
    async fn shell_search_empty_replaces_results_live() {
        // chromiumoxide 0.9 铁序：launch → spawn_handler → open_page
        let (browser, handler) = gsearch::browser::launch(true).await.expect("launch Chrome");
        let handler_task = Some(gsearch::browser::spawn_handler(handler));
        let page = gsearch::browser::open_page(&browser).await.expect("open page");
        let mut ctx = live_shell_ctx(browser, page, handler_task, vec![fixture_result("https://old.example/a")]);

        cmd_search(&["gsearchb39-empty-probe-zzzzqqqq", "--limit", "3"], &mut ctx)
            .await
            .expect("空词探测查询应成功返回（需外网；撞码/熔断时换环境手跑）");
        assert!(
            ctx.last_results.is_empty(),
            "空 search 必须整替清空旧结果（不得保留上一条查询的结果）"
        );
        let e = cmd_click(&["1"], &mut ctx).await.unwrap_err();
        assert!(format!("{e:#}").contains("越界"), "空结果后 click 1 必须越界报错: {e:#}");

        gsearch::browser::graceful_close(&mut ctx.browser).await;
    }
}


