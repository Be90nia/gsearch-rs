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
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chromiumoxide::Page;
use chromiumoxide::browser::Browser;
use serde::Deserialize;

use gsearch::browser;
use gsearch::output::print_text;
use gsearch::search::{SearchConfig, is_captcha, run_search};
use gsearch::skeleton::{extract_adaptive, format_adaptive, format_headings_only, format_json};
use gsearch::types::SearchResult;
use gsearch::util::{b64_decode, filename_from_url};

/// M9 shell `read` / `browse` 选项集。与 postproc::ReadOpts / general::BrowseOpts 字段一致。
#[derive(Debug, Clone, Default)]
struct ReadShellOpts {
    full: bool,
    json: bool,
    headings_only: bool,
    from: usize,
}

const TEXT_MAX_CHARS: usize = 5000;
const PAGE_TIMEOUT_SECS: u64 = 30;
const PROMPT: &str = "gsearch> ";
/// snap/click @eN 共用的可交互元素选择器；click 靠同一列表的 DOM 序号重定位元素。
const SNAP_SELECTOR: &str = "a,button,input,select,textarea,[onclick]";

/// M14 snap 抓到的单个可交互元素。`ref_id` 是可见元素列表的 eN 编号（click @eN 用），
/// `index` 是 querySelectorAll 的 DOM 序号（元素重定位用）；页面变动后两者漂移，重新 snap 即可。
#[derive(Debug, Clone, PartialEq)]
pub struct SnapElem {
    ref_id: String,
    tag: String,
    text: String,
    /// a 标签的绝对 href（el.href 属性，浏览器已解析相对路径），click @eN 直接 goto。
    href: String,
    id: String,
    index: usize,
}

/// JS 侧原始返回（字段名对齐 JS 对象）；snap_elems_from_raw 补 eN 编号后转 SnapElem。
#[derive(Debug, Deserialize)]
struct RawSnapElem {
    i: usize,
    tag: String,
    text: String,
    href: String,
    id: String,
}

/// `click` 参数：`@eN` ref（来自 snap）或数字（last_results 序号，原行为）。
#[derive(Debug, Clone, PartialEq)]
enum ClickTarget {
    Ref(String),
    Idx(usize),
}

/// shell 会话上下文：一次启动的 Chrome + 主 page + 上次搜索结果 + 当前 URL。
/// 整个 shell 生命周期内复用，跨 prompt 保持 cookie / 页面状态。
pub struct ShellCtx {
    pub browser: Browser,
    pub page: Page,
    pub last_results: Vec<SearchResult>,
    pub last_snap: Vec<SnapElem>,
    pub current_url: String,
}

/// 起一次 headless Chrome，进入 `gsearch> ` REPL；EOF / Ctrl+D 走 graceful 关闭。
/// exit/quit 走二次确认（提示用户 EOF），状态机不增。
/// Ctrl+C 在 tokio runtime 默认是 process kill；本进程按 Ctrl+C = 退出 shell（同 Ctrl+D）。
pub async fn run_shell() -> Result<ExitCode> {
    let (browser, handler) = browser::launch(true).await.context("启动 Chrome 失败")?;
    let _h = browser::spawn_handler(handler);

    let page = browser
        .new_page("about:blank")
        .await
        .context("创建初始 page 失败")?;

    let mut ctx = ShellCtx {
        browser,
        page,
        last_results: Vec::new(),
        last_snap: Vec::new(),
        current_url: String::new(),
    };

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut stdout = io::stdout();

    println!("进入 gsearch shell（输入 help 查命令，exit / quit / Ctrl+D 退出）");
    let mut buf = String::new();
    loop {
        buf.clear();
        write!(stdout, "{PROMPT}")?;
        stdout.flush()?;
        let n = reader.read_line(&mut buf)?;
        if n == 0 {
            // EOF：Ctrl+D（Unix）或 Ctrl+Z 回车（Windows）
            println!();
            println!("[EOF] 退出 shell");
            break;
        }
        let line = buf.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(cmd) = parts.next() else {
            continue;
        };
        let args: Vec<&str> = parts.collect();
        // 单条命令出错只打 error，不退出 shell
        if let Err(e) = dispatch(cmd, &args, &mut ctx).await {
            eprintln!("error: {e}");
            for cause in e.chain().skip(1) {
                eprintln!("  原因: {cause}");
            }
        }
    }
    graceful_close(&mut ctx.browser).await;
    Ok(ExitCode::SUCCESS)
}

/// 关 Chrome 并等进程死透；与 general.rs 同模式，避免 Drop 报 "was not closed manually" WARN。
async fn graceful_close(browser: &mut Browser) {
    if let Err(e) = browser.close().await {
        tracing::warn!("close browser 失败: {e}");
    }
    let _ = browser.wait().await;
}


/// 分派单条 shell 命令
async fn dispatch(cmd: &str, args: &[&str], ctx: &mut ShellCtx) -> Result<()> {
    match cmd {
        "help" | "?" => {
            print_help();
            Ok(())
        }
        "exit" | "quit" => {
            // 真正退出走 EOF（read_line 返回 0 → 主循环 break）。这里只提示，
            // 不引入"特殊返回值中断主循环"的状态扩张。
            println!("退出请输入 EOF（Ctrl+D / Ctrl+Z+Enter）");
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
         search <query> [--limit N]   Google 搜索，结果存入 last_results\n\
         click <N> / open <N>         跳到 last_results[N-1].url\n\
         click @eN                    点击 snap 元素（a 跳 href，其余 JS click）\n\
         snap / snapshot              列出当前页可交互元素（ref e1..eN，供 click @eN）\n\
         read                         打印当前页（默认 AdaptiveRead 三段）\n\
         dl [N] [-o DIR]              下载 last_results[N-1].url 到 DIR 或 CWD（N 缺省走 current_url）\n\
         browse <url>                 goto <url> 并更新 current_url\n\
         login <url>                  切有头窗人工登录；关窗后提示是否切回 headless\n\
         back                         页面后退\n\
         status                       打印 current_url / title / 结果数 / profile 路径\n\
         help                         本帮助\n\
         exit / quit                  提示退出；EOF / Ctrl+D 真退出"
    );
}

async fn cmd_search(args: &[&str], ctx: &mut ShellCtx) -> Result<()> {
    let (query, limit) = parse_search_args(args)?;
    let results = run_search(
        &mut ctx.browser,
        SearchConfig {
            query: query.clone(),
            limit,
        },
    )
    .await?;
    print_text(&results);
    if results.is_empty() {
        println!("（搜索无结果）");
    } else {
        println!("共 {} 条结果（输入 click N / read / dl N 继续）", results.len());
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
                settle_after_click(&ctx.page).await;
                if let Some(u) = ctx.page.url().await.ok().flatten() {
                    ctx.current_url = u;
                }
                println!("已点击 {r} <{}>", el.tag);
            }
        }
    }
    Ok(())
}

/// 元素 click 可能触发导航（如 onclick location.href），导航会销毁旧 JS context，
/// 紧跟的 evaluate 会撞 CDP -32000 "Cannot find context"。轮询 readyState 到 complete
/// （无导航时首轮即过零开销），让后续命令落在稳定 context 上。
/// ponytail: 只等 readyState，不监听网络空闲；晚于 4s 窗口的异步跳转（setTimeout 后 location）仍可能漏。
async fn settle_after_click(page: &Page) {
    for _ in 0..20 {
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

/// 页内 JS 遍历 SNAP_SELECTOR 元素，getBoundingClientRect 过滤宽/高为 0 的不可见项。
/// `async function ()` 声明形态（chromiumoxide 函数探测不认箭头函数）。
/// ponytail: 不滚动加载，懒加载页先等渲染完再 snap；需要时加滚动采集。
async fn snap_page(page: &Page) -> Result<Vec<SnapElem>> {
    let js = format!(
        "async function () {{
            const out = [];
            document.querySelectorAll({sel}).forEach(function (el, i) {{
                const r = el.getBoundingClientRect();
                if (r.width === 0 || r.height === 0) return;
                let text = (el.innerText || '').trim();
                if (!text && el.tagName === 'INPUT') {{
                    const p = el.getAttribute('placeholder') || '';
                    text = el.value || (p ? 'placeholder=\"' + p + '\"' : '');
                }}
                if (!text) text = el.getAttribute('aria-label') || '';
                out.push({{
                    i: i,
                    tag: el.tagName.toLowerCase(),
                    text: text.replace(/\\s+/g, ' ').slice(0, 30),
                    href: el.href || '',
                    id: el.id || ''
                }});
            }});
            return out;
        }}",
        sel = serde_json::to_string(SNAP_SELECTOR)?
    );
    let raw = page
        .evaluate(js)
        .await
        .map_err(|e| anyhow!("snap 失败: {e}"))?
        .into_value::<Vec<RawSnapElem>>()
        .map_err(|e| anyhow!("snap 返回结构异常: {e}"))?;
    Ok(snap_elems_from_raw(raw))
}

/// click @eN 元素路径：按 snap 时的 DOM 序号在同一 selector 列表里重定位，
/// tag 校验一致才 JS click（DOM 变动导致序号漂移时报"snap 已过期"，不静默点错元素）。
/// ponytail: JS .click() 不模拟鼠标移动，hover 展开类菜单不适用；需要时换 CDP Input 域。
async fn click_snap_elem(page: &Page, el: &SnapElem) -> Result<()> {
    let js = format!(
        "async function () {{
            const els = document.querySelectorAll({sel});
            const t = els[{i}];
            if (!t) throw new Error('snap 已过期: 序号 {i} 越界（当前 ' + els.length + ' 个）');
            const tag = t.tagName.toLowerCase();
            if (tag !== {want}) throw new Error('snap 已过期: 序号 {i} 是 <' + tag + '>，期望 <' + {want} + '>');
            t.click();
        }}",
        sel = serde_json::to_string(SNAP_SELECTOR)?,
        i = el.index,
        want = serde_json::to_string(&el.tag)?
    );
    page.evaluate(js).await.map_err(|e| anyhow!("click @{} 失败: {e}", el.ref_id))?;
    Ok(())
}

/// `click` 目标解析：`@eN` ref 或数字序号。
fn parse_click_target(s: &str) -> Result<ClickTarget> {
    if let Some(rest) = s.strip_prefix('@') {
        let ok = rest.starts_with('e') && rest.len() > 1 && rest[1..].bytes().all(|b| b.is_ascii_digit());
        if ok {
            Ok(ClickTarget::Ref(rest.to_string()))
        } else {
            Err(anyhow!("click 非法 ref: {s:?}（应为 @eN，如 @e3）"))
        }
    } else {
        s.parse::<usize>()
            .map(ClickTarget::Idx)
            .map_err(|_| anyhow!("click 参数非法: {s:?}（应为 N 或 @eN）"))
    }
}

/// last_snap 里按 ref 查元素；未命中给可行动提示。
fn find_snap_elem<'a>(snap: &'a [SnapElem], ref_id: &str) -> Result<&'a SnapElem> {
    snap.iter()
        .find(|e| e.ref_id == ref_id)
        .ok_or_else(|| anyhow!("snap 无 {ref_id}（共 {} 个元素；页面变动后先重新 snap）", snap.len()))
}

/// JS 原始返回 → SnapElem：eN 编号按可见元素顺序 1 起，index 保留 DOM 序号（两者不混淆）。
fn snap_elems_from_raw(raw: Vec<RawSnapElem>) -> Vec<SnapElem> {
    raw.into_iter()
        .enumerate()
        .map(|(n, r)| SnapElem {
            ref_id: format!("e{}", n + 1),
            tag: r.tag,
            text: r.text,
            href: r.href,
            id: r.id,
            index: r.i,
        })
        .collect()
}

/// snap 单行：`e1  <a> "Sign in" → https://x/login`；href 优先，无 href 用 #id，都无则省略尾部。
/// 文本自身带引号（如 input 的 `placeholder="..."`）不再包外层引号。
fn format_snap_line(e: &SnapElem) -> String {
    let text = if e.text.contains('"') {
        e.text.clone()
    } else {
        format!("\"{}\"", e.text)
    };
    let target = if !e.href.is_empty() {
        e.href.clone()
    } else if !e.id.is_empty() {
        format!("#{}", e.id)
    } else {
        return format!("{}  <{}> {}", e.ref_id, e.tag, text);
    };
    format!("{}  <{}> {} → {}", e.ref_id, e.tag, text, target)
}

async fn cmd_read(args: &[&str], ctx: &mut ShellCtx) -> Result<()> {
    let opts = parse_shell_read_opts(args, "read")?;
    let html = ctx.page.content().await.unwrap_or_default();
    if is_captcha(&html) {
        return Err(anyhow!(
            "当前页遇 CAPTCHA：用 `login <url>` 切有头窗人工验证后再回 shell"
        ));
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
    let title = ctx
        .page
        .evaluate("document.title")
        .await?
        .into_value::<String>()
        .unwrap_or_default();
    let mut read = extract_adaptive(&html);
    read.url = if ctx.current_url.is_empty() { "(未设)".into() } else { ctx.current_url.clone() };
    read.title = title;
    let out = if opts.json {
        format_json(&read)
    } else if opts.headings_only {
        format_headings_only(&read)
    } else {
        format_adaptive(&read, opts.from)
    };
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
            let n: usize = n_str.parse().map_err(|_| anyhow!("dl N 非数字: {n_str:?}"))?;
            if n == 0 || n > ctx.last_results.len() {
                return Err(anyhow!("dl {n} 越界（结果数 {}）", ctx.last_results.len()));
            }
            ctx.last_results[n - 1].url.clone()
        }
        None => {
            if ctx.current_url.is_empty() {
                return Err(anyhow!("dl 缺 N 且 current_url 未设置（先 browse 或 click）"));
            }
            ctx.current_url.clone()
        }
    };
    dl_in_page(&ctx.page, &url, output).await
}

/// 页内 fetch → 字节落盘（同 M4 dl 思路，去掉 postproc.rs 私有耦合）。
/// `output` 缺省落 CWD；提供时 create_dir_all(DIR) + DIR.join(filename)，与 postproc::dl / general::cmd_dl 三处行为一致。
/// credentials:'include' 带同源 cookie；`async function ()` 而非箭头（chromiumoxide 函数探测）。
async fn dl_in_page(page: &Page, url: &str, output: Option<&Path>) -> Result<()> {
    let _ = tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url)).await;
    let js = format!(
        "async function () {{
            const r = await fetch({}, {{credentials: 'include'}});
            if (!r.ok) throw new Error('HTTP ' + r.status);
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
        .map_err(|e| anyhow!("下载失败（{url}）：同源 fetch 受 CORS 限制: {e}"))?
        .into_value::<String>()
        .map_err(|e| anyhow!("fetch 返回值非字符串（{url}）: {e}"))?;
    let bytes = b64_decode(&b64)?;
    if bytes.is_empty() {
        return Err(anyhow!("下载内容为空（{url}）"));
    }
    let dir = std::path::absolute(output.unwrap_or_else(|| Path::new(".")))?;
    std::fs::create_dir_all(&dir).with_context(|| format!("创建下载目录失败: {}", dir.display()))?;
    let path = dir.join(filename_from_url(url));
    std::fs::write(&path, &bytes).with_context(|| format!("写文件失败: {}", path.display()))?;
    println!("已下载: {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

async fn cmd_browse(args: &[&str], ctx: &mut ShellCtx) -> Result<()> {
    let url = args.first().ok_or_else(|| anyhow!("browse 缺 url"))?;
    goto(&ctx.page, url).await?;
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
    // 先切有头（close + 同 profile 重起），保持 cookie 不丢
    swap_to_headed(&mut ctx.browser).await?;
    // 有头模式下旧 page 已随旧 browser 关闭，新开一个
    ctx.page = ctx
        .browser
        .new_page("about:blank")
        .await
        .context("有头模式创建 page 失败")?;
    goto(&ctx.page, url).await?;
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
    io::stdin().lock().read_line(&mut ans).ok();
    let ans = ans.trim().to_lowercase();
    if ans.is_empty() || ans == "y" || ans == "yes" {
        // 同 profile 重起 headless，page 也得重建
        swap_to_headed(&mut ctx.browser).await?;
        ctx.page = ctx
            .browser
            .new_page("about:blank")
            .await
            .context("切回 headless 后创建 page 失败")?;
        ctx.current_url.clear();
        println!("已切回 headless，page 已重建");
    } else {
        println!("保持有头模式，shell 继续（按需退出）");
    }
    Ok(())
}
/// close 当前 browser 并同 profile 起重起有头实例。
/// 等价 plsearch AppContext.reveal_for_captcha（main.py:133-137）。
async fn swap_to_headed(browser: &mut Browser) -> Result<()> {
    browser::swap_to_headed(browser).await
}


async fn browser_alive(browser: &Browser) -> bool {
    browser.version().await.is_ok()
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
    #[test]
    fn parse_click_target_ref_and_idx() {
        assert_eq!(parse_click_target("@e3").unwrap(), ClickTarget::Ref("e3".into()));
        assert_eq!(parse_click_target("3").unwrap(), ClickTarget::Idx(3));
    }

    #[test]
    fn parse_click_target_rejects_invalid() {
        for bad in ["", "e3", "@x3", "@e", "@3", "@E3", "abc", "@"] {
            assert!(parse_click_target(bad).is_err(), "应报错: {bad:?}");
        }
    }

    #[test]
    fn find_snap_elem_hit_and_miss() {
        let snap = vec![
            SnapElem { ref_id: "e1".into(), tag: "a".into(), text: "Sign in".into(), href: "https://x/login".into(), id: String::new(), index: 0 },
            SnapElem { ref_id: "e2".into(), tag: "button".into(), text: "Submit".into(), href: String::new(), id: "submit-btn".into(), index: 4 },
            SnapElem { ref_id: "e3".into(), tag: "input".into(), text: "placeholder=\"Search\"".into(), href: String::new(), id: "search-q".into(), index: 7 },
        ];
        let hit = find_snap_elem(&snap, "e3").unwrap();
        assert_eq!((hit.tag.as_str(), hit.index), ("input", 7));
        assert!(find_snap_elem(&snap, "e99").is_err());
    }

    #[test]
    fn format_snap_line_cases() {
        let a = SnapElem { ref_id: "e1".into(), tag: "a".into(), text: "Sign in".into(), href: "https://x/login".into(), id: String::new(), index: 0 };
        let btn = SnapElem { ref_id: "e2".into(), tag: "button".into(), text: "Submit".into(), href: String::new(), id: "submit-btn".into(), index: 4 };
        let ipt = SnapElem { ref_id: "e3".into(), tag: "input".into(), text: "placeholder=\"Search\"".into(), href: String::new(), id: "search-q".into(), index: 7 };
        let bare = SnapElem { ref_id: "e4".into(), tag: "div".into(), text: "菜单".into(), href: String::new(), id: String::new(), index: 9 };
        assert_eq!(format_snap_line(&a), "e1  <a> \"Sign in\" → https://x/login");
        assert_eq!(format_snap_line(&btn), "e2  <button> \"Submit\" → #submit-btn");
        assert_eq!(format_snap_line(&ipt), "e3  <input> placeholder=\"Search\" → #search-q");
        assert_eq!(format_snap_line(&bare), "e4  <div> \"菜单\"");
    }

    #[test]
    fn snap_elems_from_raw_assigns_ref_ids() {
        let raw = vec![
            RawSnapElem { i: 2, tag: "a".into(), text: "t1".into(), href: "/x".into(), id: String::new() },
            RawSnapElem { i: 5, tag: "button".into(), text: "t2".into(), href: String::new(), id: "b".into() },
        ];
        let snap = snap_elems_from_raw(raw);
        assert_eq!(snap[0].ref_id, "e1");
        assert_eq!(snap[0].index, 2); // DOM 序号原样保留，不与 eN 编号混淆
        assert_eq!(snap[1].ref_id, "e2");
        assert_eq!(snap[1].index, 5);
    }
}

