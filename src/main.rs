//! gsearch-rs 入口：clap 子命令派发 + Windows 控制台 UTF-8 + tracing 初始化

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand};

mod fetch;
mod general;
mod postproc;
mod shell;
mod shell_snap;
mod stealth;

// Windows 控制台 UTF-8：让 println! / eprintln! 正确输出中文标题与 SERP 摘要。
// 走 extern "system" 直接调 Win32，不引 windows-sys（PLAN §1 依赖表未列）。
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleOutputCP(wCodePageID: u32) -> i32;
}

#[cfg(windows)]
fn enable_utf8_console() {
    // CP_UTF8 = 65001
    unsafe {
        let _ = SetConsoleOutputCP(65001);
    }
}

#[cfg(not(windows))]
fn enable_utf8_console() {}

#[derive(Parser, Debug)]
#[command(
    name = "gsearch",
    version = gsearch::build::version_line(),
    about = "Google 搜索 + 通用浏览器代理 CLI（真 Chrome + 持久 profile）"
)]
struct Cli {
    #[arg(long, global = true, default_value = "info")]
    verbose: String,
    /// 浏览器代理，例：http://127.0.0.1:7890 / socks5://127.0.0.1:1080；走环境 GSEARCH_PROXY 同效。
    #[arg(long, global = true)]
    proxy: Option<String>,
    /// 配置文件路径（gsearch.json；不指定则依次找 ./gsearch.json、~/.gsearch/config.json）
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Command,
}
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default)]
enum BrowserArg {
    #[default]
    Auto,
    Chrome,
    Edge,
}

impl From<BrowserArg> for Option<gsearch::browser::BrowserKind> {
    fn from(a: BrowserArg) -> Self {
        match a {
            BrowserArg::Auto => None,
            BrowserArg::Chrome => Some(gsearch::browser::BrowserKind::Chrome),
            BrowserArg::Edge => Some(gsearch::browser::BrowserKind::Edge),
        }
    }
}
/// --recency 的 CLI 枚举（镜像 lib 侧 gsearch::search::Recency，同 BrowserArg/BrowserKind 惯例）
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum RecencyArg {
    Day,
    Week,
    Month,
    Year,
}

impl From<RecencyArg> for gsearch::search::Recency {
    fn from(r: RecencyArg) -> Self {
        match r {
            RecencyArg::Day => Self::Day,
            RecencyArg::Week => Self::Week,
            RecencyArg::Month => Self::Month,
            RecencyArg::Year => Self::Year,
        }
    }
}

/// nx4：--envelope 的 CLI 枚举。v2 = batch --json 输出顶层 {meta,results}（批统计一次）。
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum EnvelopeArg {
    V2,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Google 搜索（M2 实现；M9 `--read N` 默认 AdaptiveRead）
    Search(SearchArgs),
    /// 任意 URL → 渲染后页面正文（M1 主验收点；M9 默认 AdaptiveRead）
    Browse {
        url: String,
        /// fve：纯 innerText 全文（50000 cap）；与 --headings-only 互斥
        #[arg(long, default_value_t = false, group = "browse_mode")]
        full: bool,
        /// 3gw：输出默认 JSON（AI-first 契约）；此 flag 切回人读文本。
        #[arg(long, default_value_t = false)]
        human: bool,
        /// 兼容占位：JSON 已是默认输出，此 flag 解析但无效果（存量脚本零破坏）。
        #[arg(long, hide = true, default_value_t = false)]
        json: bool,
        #[arg(long)]
        from: Option<usize>,
        #[arg(long, default_value_t = false, group = "browse_mode")]
        headings_only: bool,
        /// 6dp：meta 压缩（仅 --json --full 信封生效；debug 日志强制全量）
        #[arg(long, default_value_t = false)]
        compact_meta: bool,
        #[arg(long, value_enum, default_value_t = BrowserArg::Auto)]
        /// 选择浏览器（M11）
        browser: BrowserArg,
    },
    /// 有头窗人工登录，cookie 落 profile
    Login {
        url: String,
        #[arg(long, value_enum, default_value_t = BrowserArg::Auto)]
        /// 选择浏览器（M11）
        browser: BrowserArg,
    },
    /// 带 profile 登录态下载（M6）。-o 末段带扩展名 = 落该文件；纯目录名 = 目录语义（README 不变）。
    Dl {
        url: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// 显式文件语义：产物固定落该文件（与 -o 的目录/文件二义解耦）。
        #[arg(long)]
        output_file: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = BrowserArg::Auto)]
        /// 选择浏览器（M11）
        browser: BrowserArg,
    },
    /// 交互式 shell：起一次 Chrome 会话复用（M7 追加里程碑）
    Shell,
    /// 检测浏览器 / profile / 网络连通性 / 出口 IP / SearXNG 健康度（M11 doctor + ptb 探测）
    Doctor {
        /// 3gw：输出默认 JSON（AI-first 契约）；此 flag 切回人读检查表。
        #[arg(long, default_value_t = false)]
        human: bool,
        /// 兼容占位：JSON 已是默认输出，此 flag 解析但无效果（存量脚本零破坏）。
        #[arg(long, hide = true, default_value_t = false)]
        json: bool,
    },
    /// HEADless URL 健康检查：HEAD/GET + redirect 链 + SSL + 延迟（M14-1A，无需 Chrome）
    Verify {
        /// 单 URL = 原行为；多 URL 或 --urls-file = 批量对比表（issue gsearch-rs-e58）。
        /// 不设 required=true 以放行 --urls-file；零值由 required_unless_present 拒绝。
        #[arg(required_unless_present = "urls_file", num_args = 1..)]
        url: Vec<String>,
        /// 3gw：输出默认 JSON（AI-first 契约）；此 flag 切回人读表格。
        #[arg(long, default_value_t = false)]
        human: bool,
        /// 兼容占位：JSON 已是默认输出，此 flag 解析但无效果（存量脚本零破坏）。
        #[arg(long, hide = true, default_value_t = false)]
        json: bool,
        /// 单条探测总预算秒数（含 redirect；issue gsearch-rs-7qx：CDN 抖动端点可调高）
        #[arg(long, default_value_t = gsearch::verify::VERIFY_TIMEOUT_SECS)]
        timeout: u64,
        /// 批量：逐行读 URL 的文件（空行忽略），与位置参数互斥
        #[arg(long, conflicts_with = "url")]
        urls_file: Option<std::path::PathBuf>,
    },
    /// 纯 HTTP GET 取网页正文（issue gsearch-rs-fetch，无需 Chrome；JS 壳页会提示用 browse）。
    /// 单 URL = 原行为；多 URL = batch 并发（上限 5、单条失败不阻塞，退出码 0 全成功 / 1 部分失败 / 2 全失败）。
    Fetch {
        #[arg(required = true, num_args = 1..)]
        url: Vec<String>,
        /// 逗号分隔 CSS selector（如 "main,article"）：命中时取首个命中容器的正文并跳过 JS 壳判定；
        /// 未命中回退全文提取，--json 在 meta.include_hit=false 标注。
        #[arg(long)]
        include: Option<String>,
        /// 3gw：输出默认 JSON（AI-first 契约）；此 flag 切回人读文本。
        #[arg(long, default_value_t = false)]
        human: bool,
        /// 兼容占位：JSON 已是默认输出，此 flag 解析但无效果（存量脚本零破坏）。
        #[arg(long, hide = true, default_value_t = false)]
        json: bool,
        /// 放行私网地址（loopback / RFC1918 / link-local / 云 metadata），同时允许内网明文 http。
        /// 默认拒（SSRF 门）；也可通过 `GSEARCH_FETCH_ALLOW_PRIVATE=1` 环境变量放行。
        #[arg(long, default_value_t = false)]
        allow_private: bool,
    },
}
#[derive(Args, Debug)]
struct SearchArgs {
    /// 一到多个查询串：单查询 = 原行为（searxng → Google 回退链）；
    /// 多查询 = batch 模式（并发 searxng、单条失败不阻塞、禁浏览器回退——浏览器单例不可并发）。
    #[arg(required = true, num_args = 1..)]
    query: Vec<String>,
    /// cxa：1..=100——SearXNG 单查最多 10 页×10 条，更大的值只会翻页白耗时（实测 10000→18s）
    #[arg(long, default_value_t = 10, value_parser = clap::builder::RangedI64ValueParser::<usize>::from(1..=100))]
    limit: usize,
    /// 时间过滤：只看 day/week/month/year 内的结果。SearXNG 加 time_range，Google SERP 加 tbs=qdr。
    /// `site:` 等查询语法原样透传，无专属参数。
    #[arg(long, value_enum)]
    recency: Option<RecencyArg>,
    /// `--open / --read / --dl` 互斥：每次只能指定一个；不可同时传。
    /// l6o：1..——`--read 0` 曾被 Some(0) 当真值白起完整浏览器读阶段。
    #[arg(long, group = "post", value_parser = clap::builder::RangedI64ValueParser::<usize>::from(1..))]
    read: Option<usize>,
    #[arg(long, group = "post")]
    dl: Option<usize>,
    #[arg(long, group = "post")]
    open: Option<usize>,
    /// 跳过搜索前的 warmup（Wikipedia/GitHub/HN 随机访问 + 滚动）+ 指纹补丁。
    /// Agent 反复调时建议加；人用保留默认 warmup。
    #[arg(long = "no-humanize", default_value_t = true, action = clap::ArgAction::SetFalse)]
    humanize: bool,
    #[arg(long, default_value_t = false)]
    full: bool,
    /// 3gw：输出默认 JSON（AI-first 契约）；此 flag 切回人读文本。
    #[arg(long, default_value_t = false)]
    human: bool,
    /// 兼容占位：JSON 已是默认输出，此 flag 解析但无效果（存量脚本零破坏）。
    #[arg(long, hide = true, default_value_t = false)]
    json: bool,
    /// cw8：JSON 结果 snippet 截断长度（按字符）；默认 160。
    #[arg(long, default_value_t = 160, value_parser = clap::builder::RangedI64ValueParser::<usize>::from(1..=100000))]
    snippet_len: usize,
    /// `--read N --from K`：摘要段从第 K 段开始（1-based；默认 0 = 从首段）
    #[arg(long)]
    from: Option<usize>,
    /// `--read N --headings-only`：只输出目录（最省 token fast path）
    #[arg(long, default_value_t = false)]
    headings_only: bool,
    /// e1i：`--read N --excerpt N`——paragraph_index 每项附该段前 N 字符实际文本（--json 生效，
    /// 受 read_max_chars 总 cap 约束）。与 --full/--headings-only 互斥；默认不启用（输出逐键不变）。
    #[arg(long, conflicts_with_all = ["full", "headings_only"])]
    excerpt: Option<usize>,
    /// nx4：batch --json 输出信封形态。v2 = 顶层 {meta,results}（批统计一次，元素不带 meta）；
    /// 默认裸数组（存量 agent 零破坏）。单查询模式忽略此 flag。
    #[arg(long, value_enum)]
    envelope: Option<EnvelopeArg>,
    /// 6dp：meta 压缩到少量字段（query/truncated/provider/elapsed_ms/recency）。
    /// 默认关（13 字段全量）；--verbose debug 或 GSEARCH_LOG=debug 时强制全量（排障现场保留）。
    #[arg(long, default_value_t = false)]
    compact_meta: bool,
    /// `--dl N -o DIR`：把下载文件落到 DIR 下（按 URL 末段命名）；DIR 缺省落 CWD。M13 修复两处不一致。
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = BrowserArg::Auto)]
    /// 选择浏览器：auto = Chrome 优先缺则 Edge，强制选 chrome/edge。M11。
    browser: BrowserArg,
}

fn init_tracing(level: &str) {
    use tracing_subscriber::EnvFilter;
    // chromiumoxide 0.9 与新版 Chrome 之间常打出无害的 "WS Invalid message" 噪音
    // （Chrome 加新 ws message 变体，依赖没跟上，serde 不能匹配的 fallback）。
    // 默认把它压到 error 级：需要 chromiumoxide 细节时再加 RUST_LOG=chromiumoxide=warn。
    let composed = format!("{},chromiumoxide=error", level);
    let filter = EnvFilter::try_new(composed).unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    enable_utf8_console();
    let cli = Cli::parse();
    init_tracing(&cli.verbose);
    if let Some(p) = &cli.config
        && let Err(e) = gsearch::config::set_explicit_and_load(p.clone())
    {
        eprintln!("error: {e:#}");
        return ExitCode::from(2);
    }
    // GSEARCH_PROXY env 作为默认值（CLI --proxy 覆盖）
    let proxy = cli.proxy.clone().or_else(|| std::env::var("GSEARCH_PROXY").ok().filter(|s| !s.is_empty()));
    // 6dp：--compact-meta 生效门——opt-in 且 debug 日志（--verbose debug / GSEARCH_LOG=debug）强制全量。
    // 进程级一次性设置（信封构造方零改动），派发前算好。
    let compact_requested = match &cli.cmd {
        Command::Search(a) => a.compact_meta,
        Command::Browse { compact_meta, .. } => *compact_meta,
        _ => false,
    };
    let debug_logging = cli.verbose.eq_ignore_ascii_case("debug")
        || std::env::var("GSEARCH_LOG")
            .map(|v| v.to_ascii_lowercase().contains("debug"))
            .unwrap_or(false);
    gsearch::types::set_compact_meta(compact_requested && !debug_logging);
    let result: Result<ExitCode> = match cli.cmd {
        Command::Search(args) => cmd_search(args, proxy.clone()).await,
        Command::Browse { url, full, human, from, headings_only, compact_meta: _, browser, .. } => {
            let opts = general::BrowseOpts {
                full,
                // 3gw：--json 已是默认，--human 才切人读
                json: !human,
                from: from.unwrap_or(0),
                headings_only,
                browser: browser.into(),
                proxy: proxy.clone(),
            };
            general::cmd_browse(&url, &opts).await
        }
        Command::Login { url, browser } => general::cmd_login(&url, browser.into(), proxy.clone()).await,
        Command::Dl { url, output, output_file, browser } => {
            general::cmd_dl(&url, output.as_deref(), output_file.as_deref(), browser.into(), proxy.clone()).await
        }
        Command::Shell => shell::run_shell().await,
        Command::Doctor { human, .. } => cmd_doctor(!human).await,
        Command::Verify { url, human, timeout, urls_file, .. } => {
            gsearch::verify::cmd_verify(&url, !human, proxy.as_deref(), timeout, urls_file.as_deref())
        }
        Command::Fetch { url, human, allow_private, include, .. } => {
            fetch::cmd_fetch(&url, &fetch::FetchOpts { json: !human, proxy: proxy.clone(), allow_private, include }).await
        }
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            for cause in e.chain().skip(1) {
                eprintln!("  原因: {cause}");
            }
            eprintln!("（用 --verbose debug 查详细）");
            ExitCode::from(1)
        }
    }
}

async fn cmd_search(args: SearchArgs, proxy: Option<String>) -> Result<ExitCode> {
    // batch 多查询：并发 searxng、单条失败不阻塞、禁浏览器回退（issue gsearch-rs-doh）
    if args.query.len() > 1 {
        return cmd_search_batch(args).await;
    }
    let started = std::time::Instant::now();
    // M14-1B：早解析浏览器路径 → meta 头部字段（与 launch 实际选用的 kind 一致）。
    let browser_kind = browser_arg_to_kind(args.browser);
    let (browser_path, resolved_kind) = resolve_browser_meta(browser_kind);
    // clap 保证位置参数 ≥1、上面分支保证 =1：单查询路径沿用原行为
    let query = args.query.first().cloned().unwrap_or_default();
    let recency = args.recency.map(gsearch::search::Recency::from);
    // M17 惰性启动（Part 1）：先跑 SearXNG 纯 HTTP 源——命中则全程零浏览器；
    // 未配置/失败（try_searxng = None，回退 warn 已打）才 launch 走 Google 直爬。
    let mut browser_opt: Option<chromiumoxide::browser::Browser> = None;
    let mut h_slot: Option<tokio::task::JoinHandle<()>> = None;
    let cfg = gsearch::search::SearchConfig { query: query.clone(), limit: args.limit, recency };
    // 3gw：JSON 默认，--human 切人读
    let json_mode = !args.human;
    // zc6：SearXNG 单次尝试四态——Results 直接用；CircuitBroken 熔断早退；
    // NotConfigured/FallbackGoogle 走原 Google 链（IP 正常时行为与旧版一致）。
    let searxng = gsearch::search::try_searxng(&cfg).await;
    if matches!(searxng, gsearch::search::SearxngAttempt::CircuitBroken) {
        if json_mode {
            // stderr 诊断行已由 try_searxng 打（豁免未来任何静默策略）
            emit_searxng_degraded_json(&query, &args, &browser_path, &resolved_kind, proxy.clone(), recency, started.elapsed().as_millis());
        }
        // 人读模式 stdout 不打假结果；退出码 2 = 无结果语义族
        return Ok(ExitCode::from(2));
    }
    // yq6：记下主源失败——若 Google 回退也空，run.status 统一打 searxng_degraded
    //（此前该分支 exit 2 且信封无状态标记，agent 无从区分「没资料」与「源降级」）。
    let searxng_fell_back = matches!(searxng, gsearch::search::SearxngAttempt::FallbackGoogle);
    let (mut results, captcha_solved, provider) = match searxng {
        gsearch::search::SearxngAttempt::Results(
            gsearch::search::SearchOutcome::Results { results, captcha_solved, provider },
        ) => (results, captcha_solved, provider),
        // NotConfigured（未配 SearXNG）/ FallbackGoogle（预检通过，回退 warn 已打）→ Google 直爬
        _ => {
        // H2 包成 async 块统一收尾：launch 成功后所有 ? 早返回路径（new_page / install_init_script /
        // browser 用 Cell 模式（Option<Browser>）保留到外层，Err 路径也走 graceful_close 再上抛
        // （防 Browser::drop 在 Windows 上不杀子进程的漏）。
        let slot: std::cell::RefCell<Option<chromiumoxide::browser::Browser>> = std::cell::RefCell::new(None);
        let outcome: anyhow::Result<gsearch::search::SearchOutcome> = async {
            let (b, handler) = gsearch::browser::launch_with_kind_proxy(true, browser_kind, proxy.clone())
                .await
                .context("启动 Chrome/Edge 失败：检查 GSEARCH_CHROME 是否指向 chrome.exe/msedge.exe，或 profile 被另一实例占用")?;
            // h_slot: swap_to_headed 时 abort 旧 handler task，再起新 task 接新 Browser 的 sender
            h_slot = Some(gsearch::browser::spawn_handler(handler));
            let mut browser = b;
            let page = browser.new_page("about:blank").await?;
            if args.humanize {
                stealth::install_init_script(&page).await?;
                stealth::warmup(&page).await?;
            }
            // ponytail: 顶层 search 没人在场 stdin 给 noop Arc（human_solved 永远是 false，不影响行为）
            let human_solved = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let cfg = gsearch::search::SearchConfig { query: query.clone(), limit: args.limit, recency };
            let r = gsearch::search::run_search_on_page(&mut browser, cfg, page, &mut h_slot, human_solved).await?;
            *slot.borrow_mut() = Some(browser);
            Ok::<_, anyhow::Error>(r)
        }
        .await;
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                // Err 路径：浏览器可能起好但未移交，graceful_close 兜底关。
                // 取走 Option 后释放 RefCell 借用，再 await 关浏览器。
                let mut b = slot.borrow_mut().take();
                if let Some(b_ref) = b.as_mut() {
                    gsearch::browser::graceful_close(b_ref).await;
                }
                drop(b);
                return Err(e);
            }
        };
        let mut browser = match slot.borrow_mut().take() {
            Some(b) => b,
            None => return Err(anyhow!("google_path Ok 后 slot 应含 browser，逻辑漏洞")),
        };
        match outcome {
            gsearch::search::SearchOutcome::Results { results, captcha_solved, provider } => {
                // browser 存进 slot 供 postproc（--read/--dl）与收尾 close 复用
                browser_opt = Some(browser);
                (results, captcha_solved, provider)
            }
            gsearch::search::SearchOutcome::CaptchaTimeout => {
                // 输出 captcha_timeout JSON（Agent 看到 status 字段就知道等人解超时）
                if json_mode {
                    emit_captcha_timeout_json(&query, &args, &browser_path, &resolved_kind, proxy.clone(), recency, started.elapsed().as_millis());
                } else {
                    eprintln!("error: CAPTCHA 亲解超时（{}s）；profile 已养熟，再次执行会跳过 CAPTCHA",
                        gsearch::search::CAPTCHA_TIMEOUT_SECS);
                }
                gsearch::browser::graceful_close(&mut browser).await;
                return Ok(ExitCode::from(3));
            }
        }
        }
    };
    // cw8/3gw：SERP snippet 默认 160 字符封顶（人读渲染本就 160，JSON 不再例外），--snippet-len 可调
    for r in &mut results {
        r.snippet = gsearch::output::truncate_snippet(&r.snippet, args.snippet_len);
    }
    // yq6：主源降级且回退也空——如实证状态，防 agent 把基础设施问题当「话题无资料」
    let degraded = results.is_empty() && searxng_fell_back;
    // 0mf：--json + --read 时 read 产物并入单一 JSON 文档——envelope 延后装配，stdout 只出
    // 一份可解析 JSON（旧行为 envelope 先打 + read raw 追加 = json.loads 崩）。
    // b95：--headings-only 连 SERP 集都不进输出（text 模式同样跳过 print_text）。
    let read_solo_json = json_mode && args.read.is_some();
    let headings_solo = args.read.is_some() && args.headings_only;
    let meta = gsearch::types::MetaOutput {
        tool: "gsearch",
        version: env!("CARGO_PKG_VERSION"),
        query: query.clone(),
        profile: gsearch::browser::profile_name_only(),
        browser_kind: format!("{resolved_kind:?}"),
        browser_path: browser_path.to_string_lossy().into_owned(),
        proxy: proxy.clone(),
        humanize: args.humanize,
        limit: args.limit,
        elapsed_ms: started.elapsed().as_millis(),
        truncated: results.len() >= args.limit,
        provider: provider.into(),
        recency: recency.map(|r| r.as_str().into()),
    };
    let run = gsearch::types::RunStatusInfo {
        // yq6：searxng 主源失败且回退也空 → 与熔断同款 degraded 标记（零结果 ≠ Ok）
        status: if degraded {
            gsearch::types::RunStatus::SearxngDegraded
        } else {
            gsearch::types::RunStatus::Ok
        },
        captcha_solved,
        message: if captcha_solved {
            "本次搜索经过了人工 CAPTCHA 验证".into()
        } else if degraded {
            gsearch::search::SEARXNG_CIRCUIT_MSG.into()
        } else {
            String::new()
        },
    };
    let envelope = gsearch::types::OutputEnvelope { meta, run, results: &results };
    if !read_solo_json {
        if json_mode {
            gsearch::output::print_envelope_json(&envelope)?;
        } else if !headings_solo {
            gsearch::output::print_text(&results);
        }
    }
    // M4 后处理（PLAN §3.4）：clap ArgGroup \"post\" 保证 --open/--read/--dl 互斥；仅剩运行时分发。
    let post = async {
        if let Some(n) = args.open {
            postproc::open(&results, n)?;
        }
        if let Some(n) = args.read {
            // M17 惰性：SearXNG 命中时浏览器尚未启动，--read/--dl 首次用到才 launch(headless)
            let browser = ensure_search_browser(&mut browser_opt, &mut h_slot, browser_kind, proxy.clone()).await?;
            let opts = postproc::ReadOpts {
                full: args.full,
                json: json_mode,
                headings_only: args.headings_only,
                from: args.from.unwrap_or(0),
                excerpt: args.excerpt,
            };
            let content = if opts.full {
                postproc::read_full(browser, &mut h_slot, &results, n, &opts).await?
            } else {
                postproc::read(browser, &mut h_slot, &results, n, &opts).await?
            };
            if read_solo_json {
                // 0mf：单一 JSON 文档 = envelope（meta/run/results）+ read 产物字段
                let mut doc = serde_json::to_value(&envelope)?;
                if let Some(obj) = doc.as_object_mut() {
                    if opts.full {
                        obj.insert("content_text".into(), serde_json::Value::String(content));
                    } else {
                        if opts.headings_only {
                            // b95：headings-only 信封只放 read 产物，SERP 全集不进输出
                            obj.insert("results".into(), serde_json::json!([]));
                        }
                        obj.insert("read".into(), serde_json::from_str(&content)?);
                    }
                }
                println!("{doc}");
            }
        }
        if let Some(n) = args.dl {
            let browser = ensure_search_browser(&mut browser_opt, &mut h_slot, browser_kind, proxy.clone()).await?;
            postproc::dl(browser, &results, n, args.output.as_deref()).await?;
        }
        anyhow::Ok(())
    }
    .await;
    // 仅 Google 路径（或 --read/--dl 惰性启动过）才有 browser 需要收尾
    if let Some(browser) = browser_opt.as_mut() {
        gsearch::browser::graceful_close(browser).await;
    }
    if let Err(e) = post {
        eprintln!("postproc 失败: {e}");
        if read_solo_json {
            // w9y：read 失败必须结构化可见——补打信封并顶层挂 read_error，
            // agent 不再把「SERP 正常 + 读失败」误读为全成功。
            let mut doc = serde_json::to_value(&envelope)?;
            if let Some(obj) = doc.as_object_mut() {
                obj.insert("read_error".into(), serde_json::Value::String(format!("{e:#}")));
            }
            println!("{doc}");
        }
        if args.read.is_some() {
            // w9y：显式请求的读失败不再静默 exit 0
            return Ok(ExitCode::from(1));
        }
    }
    if results.is_empty() {
        eprintln!("未找到结果");
        Ok(ExitCode::from(2))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// 早解析浏览器路径 → meta 头部字段（与 launch 实际选用的 kind 一致）；只探测不启动。
/// batch 模式同样用它填 meta（batch 全程零浏览器）。
fn resolve_browser_meta(
    browser_kind: Option<gsearch::browser::BrowserKind>,
) -> (std::path::PathBuf, gsearch::browser::BrowserKind) {
    match browser_kind {
        Some(k) => gsearch::browser::find_specific(k)
            .or_else(|| gsearch::browser::find_browser().ok())
            .unwrap_or_else(|| {
                // 兜底：连 find_browser 都失败 → 留空让 launch 自己报错。
                (std::path::PathBuf::new(), k)
            }),
        None => gsearch::browser::find_browser().unwrap_or_else(|_| {
            (std::path::PathBuf::new(), gsearch::browser::BrowserKind::Chrome)
        }),
    }
}

/// batch 多查询（issue gsearch-rs-doh）：并发走 SearXNG，单条失败不阻塞其他条目。
/// 刻意边界：禁浏览器回退（浏览器单例不可并发）；--read/--dl/--open 不参与 batch。
/// 输出：--json 为裸 BatchEntry 数组；人读模式逐条分隔标题。退出码：全成功 0 / 部分失败 1 / 全部失败 2。
async fn cmd_search_batch(args: SearchArgs) -> Result<ExitCode> {
    if args.read.is_some() || args.dl.is_some() || args.open.is_some() {
        return Err(anyhow!(
            "batch 多查询暂不支持 --read/--dl/--open（batch 无浏览器参与）；请对单查询使用"
        ));
    }
    // 3gw：JSON 默认，--human 切人读
    let json_mode = !args.human;
    let started = std::time::Instant::now();
    let (browser_path, resolved_kind) = resolve_browser_meta(browser_arg_to_kind(args.browser));
    let recency = args.recency.map(gsearch::search::Recency::from);
    let outcomes = gsearch::search::run_batch(&args.query, args.limit, recency).await;
    let elapsed_ms = started.elapsed().as_millis();
    let profile = gsearch::browser::profile_name_only();
    let entries: Vec<gsearch::types::BatchEntry> = outcomes
        .into_iter()
        .map(|(query, outcome)| {
            let (status, message, mut results) = match outcome {
                Ok(gsearch::search::SearchOutcome::Results { results, .. }) => {
                    (gsearch::types::RunStatus::Ok, String::new(), results)
                }
                // batch 只走 searxng 不撞码，CaptchaTimeout 不可达；防御性兜为 error
                Ok(_) => (
                    gsearch::types::RunStatus::Error,
                    "batch 无浏览器路径，CaptchaTimeout 不应出现".into(),
                    vec![],
                ),
                Err(reason) => (gsearch::types::RunStatus::Error, reason, vec![]),
            };
            // cw8/3gw：snippet 封顶与单查询同规则
            for r in &mut results {
                r.snippet = gsearch::output::truncate_snippet(&r.snippet, args.snippet_len);
            }
            let meta = gsearch::types::MetaOutput {
                tool: "gsearch",
                version: env!("CARGO_PKG_VERSION"),
                query: query.clone(),
                profile: profile.clone(),
                browser_kind: format!("{resolved_kind:?}"),
                browser_path: browser_path.to_string_lossy().into_owned(),
                // batch 全程纯 HTTP 直连局域网 searxng（searxng.rs 固定 no_proxy），代理字段恒空
                proxy: None,
                humanize: args.humanize,
                limit: args.limit,
                elapsed_ms,
                truncated: results.len() >= args.limit,
                provider: "searxng".into(),
                recency: recency.map(|r| r.as_str().into()),
            };
            gsearch::types::BatchEntry {
                query,
                status,
                message,
                meta,
                results,
            }
        })
        .collect();
    if json_mode {
        if args.envelope == Some(EnvelopeArg::V2) {
            // nx4：v2 信封——批统计一次，元素丢 14 字段 meta（opt-in，默认裸数组不变）
            let n_ok = entries
                .iter()
                .filter(|e| e.status == gsearch::types::RunStatus::Ok)
                .count();
            let env = gsearch::types::BatchEnvelopeV2 {
                meta: gsearch::types::BatchMetaV2 {
                    n_total: entries.len(),
                    n_ok,
                    n_fail: entries.len() - n_ok,
                    elapsed_ms,
                },
                results: entries
                    .iter()
                    .map(|e| gsearch::types::BatchEntryV2 {
                        query: e.query.clone(),
                        status: e.status.clone(),
                        message: e.message.clone(),
                        results: e.results.clone(),
                    })
                    .collect(),
            };
            gsearch::output::print_batch_envelope_v2(&env)?;
        } else {
            gsearch::output::print_batch_json(&entries)?;
        }
    } else {
        // 人读：逐条分隔标题，条目内部沿用单查询格式
        for (i, e) in entries.iter().enumerate() {
            println!("=== [{}/{}] {} ===", i + 1, entries.len(), e.query);
            if e.status == gsearch::types::RunStatus::Error {
                println!("  出错: {}", e.message);
            } else {
                gsearch::output::print_text(&e.results);
            }
        }
    }
    let ok_count = entries
        .iter()
        .filter(|e| e.status == gsearch::types::RunStatus::Ok)
        .count();
    let total = entries.len();
    eprintln!("batch 完成：{ok_count}/{total} 条成功");
    if ok_count == total {
        Ok(ExitCode::SUCCESS)
    } else if ok_count == 0 {
        eprintln!("未找到结果");
        Ok(ExitCode::from(2))
    } else {
        Ok(ExitCode::from(1))
    }
}

/// M17 惰性启动：--read/--dl 首次用到浏览器时才 launch(headless)。
/// SearXNG 命中路径全程零浏览器；Google 路径已 launch 则直接复用（slot 已 Some）。
/// ponytail: Option::insert 返回 &mut——省掉「先判空再取出」的双 borrow 样板。
async fn ensure_search_browser<'a>(
    slot: &'a mut Option<chromiumoxide::browser::Browser>,
    h_slot: &mut Option<tokio::task::JoinHandle<()>>,
    kind: Option<gsearch::browser::BrowserKind>,
    proxy: Option<String>,
) -> Result<&'a mut chromiumoxide::browser::Browser> {
    match slot {
        Some(b) => Ok(b),
        None => {
            let (browser, handler) = gsearch::browser::launch_with_kind_proxy(true, kind, proxy)
                .await
                .context("启动 Chrome/Edge 失败：检查 GSEARCH_CHROME 是否指向 chrome.exe/msedge.exe，或 profile 被另一实例占用")?;
            *h_slot = Some(gsearch::browser::spawn_handler(handler));
            Ok(slot.insert(browser))
        }
    }
}

/// M15：CAPTCHA 超时时输出 status=captcha_timeout 的 JSON 信封。
fn emit_captcha_timeout_json(
    query: &str,
    args: &SearchArgs,
    browser_path: &std::path::Path,
    resolved_kind: &gsearch::browser::BrowserKind,
    proxy: Option<String>,
    recency: Option<gsearch::search::Recency>,
    elapsed_ms: u128,
) {
    let meta = gsearch::types::MetaOutput {
        tool: "gsearch",
        version: env!("CARGO_PKG_VERSION"),
        query: query.to_string(),
        profile: gsearch::browser::profile_name_only(),
        browser_kind: format!("{resolved_kind:?}"),
        browser_path: browser_path.to_string_lossy().into_owned(),
        proxy,
        humanize: args.humanize,
        limit: args.limit,
        elapsed_ms,
        truncated: false,
        // CAPTCHA 超时只发生在 Google 直爬路径（searxng 不撞码）
        provider: "google".into(),
        recency: recency.map(|r| r.as_str().into()),
    };
    let run = gsearch::types::RunStatusInfo {
        status: gsearch::types::RunStatus::CaptchaTimeout,
        captcha_solved: false,
        message: format!("CAPTCHA 亲解超时（{}s）；profile 已养熟，下次执行会自动跳过 CAPTCHA",
            gsearch::search::CAPTCHA_TIMEOUT_SECS),
    };
    let envelope: gsearch::types::OutputEnvelope<Vec<()>> =
        gsearch::types::OutputEnvelope { meta, run, results: vec![] };
    let _ = gsearch::output::print_envelope_json(&envelope);
}

/// zc6：SearXNG 熔断输出——status=searxng_degraded（元审计硬约束值）、provider 照实标
/// searxng、results 空；message 带诊断行让 Agent 无需解析 stderr。
fn emit_searxng_degraded_json(
    query: &str,
    args: &SearchArgs,
    browser_path: &std::path::Path,
    resolved_kind: &gsearch::browser::BrowserKind,
    proxy: Option<String>,
    recency: Option<gsearch::search::Recency>,
    elapsed_ms: u128,
) {
    let meta = gsearch::types::MetaOutput {
        tool: "gsearch",
        version: env!("CARGO_PKG_VERSION"),
        query: query.to_string(),
        profile: gsearch::browser::profile_name_only(),
        browser_kind: format!("{resolved_kind:?}"),
        browser_path: browser_path.to_string_lossy().into_owned(),
        proxy,
        humanize: args.humanize,
        limit: args.limit,
        elapsed_ms,
        truncated: false,
        // 熔断时结果确实来自 SearXNG 链路（provider 照实标注，非 google）
        provider: "searxng".into(),
        recency: recency.map(|r| r.as_str().into()),
    };
    let run = gsearch::types::RunStatusInfo {
        status: gsearch::types::RunStatus::SearxngDegraded,
        captcha_solved: false,
        message: gsearch::search::SEARXNG_CIRCUIT_MSG.into(),
    };
    let envelope: gsearch::types::OutputEnvelope<Vec<()>> =
        gsearch::types::OutputEnvelope { meta, run, results: vec![] };
    let _ = gsearch::output::print_envelope_json(&envelope);
}

fn browser_arg_to_kind(arg: BrowserArg) -> Option<gsearch::browser::BrowserKind> {
    match arg {
        BrowserArg::Auto => None,
        BrowserArg::Chrome => Some(gsearch::browser::BrowserKind::Chrome),
        BrowserArg::Edge => Some(gsearch::browser::BrowserKind::Edge),
    }
}

/// doctor 总耗时 <3s；不启动 Chrome。每项输出 `[OK] 描述 + 路径 / [WARN] ... / [FAIL] ...`。
/// 全部 OK 退出 0；任意 FAIL 退出 1；仅 WARN 退出 0。
/// 2i1：--json 输出 {checks:[{name,status,message,value?}], elapsed_ms, fail_count, warn_count}
/// （status 语义 ok/warn/fail/skip；exit 规则不变；8lp③：ok 值类检查 message 缺席、数据进 value）。
/// 人读模式文本与旧版逐字节一致（除新增项）。
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum DoctorStatus {
    Ok,
    Warn,
    Fail,
    Skip,
}

#[derive(serde::Serialize)]
struct DoctorCheck {
    name: &'static str,
    status: DoctorStatus,
    /// 8lp③：ok 的值类检查留空串（序列化缺席）；warn/fail/skip 的行动指引散文保留
    #[serde(skip_serializing_if = "String::is_empty")]
    message: String,
    /// 8lp③：值类检查的数据载荷（路径/IP/目标/profile）；纯散文检查为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

#[derive(serde::Serialize)]
struct DoctorOutput {
    checks: Vec<DoctorCheck>,
    elapsed_ms: u128,
    fail_count: usize,
    warn_count: usize,
}

/// 记一项检查；fail/warn 计数与 status 同源，防双记账漂移。
fn record_check(
    checks: &mut Vec<DoctorCheck>,
    fail: &mut usize,
    warn: &mut usize,
    name: &'static str,
    status: DoctorStatus,
    message: String,
    value: Option<String>,
) {
    match status {
        DoctorStatus::Fail => *fail += 1,
        DoctorStatus::Warn => *warn += 1,
        _ => {}
    }
    checks.push(DoctorCheck { name, status, message, value });
}

/// 8lp③：人读分支从 value 拼回旧散文（格式与旧版逐字节一致）；散文检查原样透出。
fn human_line(c: &DoctorCheck) -> String {
    if !c.message.is_empty() {
        return c.message.clone();
    }
    let v = c.value.as_deref().unwrap_or_default();
    match c.name {
        "chrome" => format!("Chrome: {v}"),
        "edge" => format!("Edge:   {v}"),
        "exit_ip" => format!("出口 IP: {v}"),
        "network" => format!("网络连通 ({v})"),
        "profile_source" => format!("profile 来自配置文件: {v}"),
        _ => v.to_owned(),
    }
}

async fn cmd_doctor(json: bool) -> Result<ExitCode> {
    let started = std::time::Instant::now();
    let mut checks: Vec<DoctorCheck> = Vec::new();
    let mut fail = 0;
    let mut warn = 0;

    // 1) Chrome 可用（走 find_specific，与 launch 一致：含 %LOCALAPPDATA% 用户级安装路径）
    let (st, msg, val) =
        match gsearch::browser::find_specific(gsearch::browser::BrowserKind::Chrome) {
            // 8lp③：值类检查——路径进 value，人读由 human_line 拼回
            Some((p, _)) => (DoctorStatus::Ok, String::new(), Some(p.display().to_string())),
            None => (
                DoctorStatus::Fail,
                "Chrome 不可用（chrome.exe 未找到；含默认路径与用户级 %LOCALAPPDATA%）".to_string(),
                None,
            ),
        };
    record_check(&mut checks, &mut fail, &mut warn, "chrome", st, msg, val);

    // 2) Edge 可用
    let (st, msg, val) =
        match gsearch::browser::find_specific(gsearch::browser::BrowserKind::Edge) {
            Some((p, _)) => (DoctorStatus::Ok, String::new(), Some(p.display().to_string())),
            None => (
                DoctorStatus::Warn,
                "Edge 不可用（msedge.exe 未找到；仅 Chrome 可跑）".to_string(),
                None,
            ),
        };
    record_check(&mut checks, &mut fail, &mut warn, "edge", st, msg, val);

    // 3) profile 可写
    let (st, msg, val) = match gsearch::browser::profile_dir() {
        Ok(dir) => match test_profile_writable(&dir) {
            Ok(()) => (DoctorStatus::Ok, format!("profile 可写: {}", dir.display()), None),
            Err(e) => (
                DoctorStatus::Fail,
                format!("profile 不可写: {} ({e})", dir.display()),
                None,
            ),
        },
        Err(e) => (DoctorStatus::Fail, format!("profile 解析失败: {e}"), None),
    };
    record_check(&mut checks, &mut fail, &mut warn, "profile_writable", st, msg, val);

    // 4) 出口 IP（明文 HTTP GET 80 端口，3s 超时；失败降 WARN）
    let (st, msg, val) = match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fetch_public_ip(),
    )
    .await
    {
        // 8lp③：值类检查——IP 进 value
        Ok(Ok(ip)) => (DoctorStatus::Ok, String::new(), Some(ip.to_string())),
        Ok(Err(e)) => (
            DoctorStatus::Warn,
            format!("出口 IP 不可达（撞码调试辅助；改用代理/VPN 后重试）: {e}"),
            None,
        ),
        Err(_) => (DoctorStatus::Warn, "出口 IP 检测超时（2s）".to_string(), None),
    };
    record_check(&mut checks, &mut fail, &mut warn, "exit_ip", st, msg, val);

    // 5) 网络连通（TCP connect google.com:443，2s 超时）
    let (st, msg, val) = match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(("www.google.com", 443)),
    )
    .await
    {
        // 8lp③：值类检查——目标进 value
        Ok(Ok(_)) => (
            DoctorStatus::Ok,
            String::new(),
            Some("www.google.com:443".to_string()),
        ),
        Ok(Err(e)) => (
            DoctorStatus::Fail,
            format!("网络不可达 (www.google.com:443): {e}"),
            None,
        ),
        Err(_) => (
            DoctorStatus::Fail,
            "网络连接超时 (www.google.com:443)".to_string(),
            None,
        ),
    };
    record_check(&mut checks, &mut fail, &mut warn, "network", st, msg, val);

    // 6) 生效 profile 来源检查（env > 配置文件 > default）
    let (st, msg, val) = if let Ok(v) = std::env::var("GSEARCH_PROFILE")
        && !v.trim().is_empty()
    {
        let p = std::path::PathBuf::from(v.trim());
        if p.exists() {
            // env 来源的散文带「已验证存在」语义且人读 label 与 config 来源不同，保留散文
            (
                DoctorStatus::Ok,
                format!("GSEARCH_PROFILE 已设置且存在: {}", p.display()),
                None,
            )
        } else {
            (
                DoctorStatus::Warn,
                format!("GSEARCH_PROFILE 已设置但路径不存在: {}（gsearch 会自动创建）", p.display()),
                None,
            )
        }
    } else if let Some(p) = gsearch::config::load().profile.clone() {
        // 8lp③：T-10 实锤「profile 来自配置文件: X」复述语义——name+value 足矣
        (DoctorStatus::Ok, String::new(), Some(p))
    } else {
        (
            DoctorStatus::Ok,
            "GSEARCH_PROFILE 未设置（默认 ~/.gsearch/profiles/default/）".to_string(),
            None,
        )
    };
    record_check(&mut checks, &mut fail, &mut warn, "profile_source", st, msg, val);

    // 7) SearXNG 健康度（ptb）：查询级探测抓「端点活但零结果」盲区——doctor 只查 TCP 查不出
    let (st, msg, val) = if let Some(f) = gsearch::config::parse_failure() {
        // vw2：配置解析失败时 searxng_url 必然丢失，SKIP「未配置」是谎报——显式 FAIL（rc=1 走既有 fail 汇总）
        (
            DoctorStatus::Fail,
            format!(
                "配置文件存在但解析失败（已忽略，回退默认）: {} ({})",
                f.path.display(),
                f.error
            ),
            None,
        )
    } else {
        match gsearch::config::load().searxng_url.clone() {
            None => (
                DoctorStatus::Skip,
                "SearXNG: 未配置（GSEARCH_SEARXNG_URL / gsearch.json searxng_url），跳过".to_string(),
                None,
            ),
            Some(base) => {
                let (st, msg, val) = match gsearch::searxng::probe(&base).await {
                    Ok(p) if p.results > 0 => (
                        DoctorStatus::Ok,
                        format!(
                            "SearXNG: HTTP {}, results={}, unresponsive_engines={} ({base})",
                            p.http_status, p.results, p.unresponsive_engines
                        ),
                        None,
                    ),
                    Ok(p) => (
                        DoctorStatus::Warn,
                        format!(
                            "SearXNG 可达但零结果（引擎降级/IP 信誉嫌疑）：HTTP {}, results=0, unresponsive_engines={} ({base})",
                            p.http_status, p.unresponsive_engines
                        ),
                        None,
                    ),
                    Err(e) => (
                        DoctorStatus::Warn,
                        format!("SearXNG 探测失败: {e} ({base})"),
                        None,
                    ),
                };
                (st, msg, val)
            }
        }
    };
    record_check(&mut checks, &mut fail, &mut warn, "searxng", st, msg, val);

    let elapsed_ms = started.elapsed().as_millis();
    if json {
        let out = DoctorOutput {
            checks,
            elapsed_ms,
            fail_count: fail,
            warn_count: warn,
        };
        // 8lp：JSON 面向 agent 消费，compact 单行省 token
        println!("{}", serde_json::to_string(&out)?);
    } else {
        println!("gsearch doctor");
        for c in &checks {
            let tag = match c.status {
                DoctorStatus::Ok => "[ OK ]",
                DoctorStatus::Warn => "[WARN]",
                DoctorStatus::Fail => "[FAIL]",
                DoctorStatus::Skip => "[SKIP]",
            };
            println!("{tag} {}", human_line(c));
        }
        if fail > 0 {
            println!("\n[{fail} 项 FAIL] 检查上面建议。（耗时 {elapsed_ms}ms）");
        } else if warn > 0 {
            println!("\n[{warn} 项 WARN] 整体可用。（耗时 {elapsed_ms}ms）");
        } else {
            println!("\n所有检查通过 ✓（耗时 {elapsed_ms}ms）");
        }
    }
    if fail > 0 {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn test_profile_writable(dir: &std::path::Path) -> anyhow::Result<()> {
    let probe = dir.join(".doctor_probe");
    std::fs::write(&probe, b"ok").with_context(|| format!("写测试文件失败: {}", probe.display()))?;
    let read_back = std::fs::read(&probe).with_context(|| format!("读测试文件失败: {}", probe.display()))?;
    if read_back != b"ok" {
        anyhow::bail!("内容不一致");
    }
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// 明文 HTTP GET `http://ipv4.icanhazip.com/` → 返回 IP 字符串。
/// ponytail: 仅用 std TCP，不引 HTTP 客户端依赖；服务偶尔挂时降 WARN 不 fail。
async fn fetch_public_ip() -> anyhow::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    let mut stream = TcpStream::connect(("ipv4.icanhazip.com", 80)).await?;
    let req = "GET / HTTP/1.1\r\nHost: ipv4.icanhazip.com\r\nConnection: close\r\nUser-Agent: gsearch-doctor\r\n\r\n";
    stream.write_all(req.as_bytes()).await?;
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 256];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 { break; }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            // 已读到 header/body 边界，body 短到一次性读完
        }
        if buf.len() > 8192 { break; }
    }
    let text = String::from_utf8_lossy(&buf);
    // 提取 HTTP body（\r\n\r\n 之后）
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    let ip = body.trim().lines().next().unwrap_or("").trim().to_owned();
    if ip.is_empty() {
        anyhow::bail!("空响应: {text}");
    }
    Ok(ip)
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, RecencyArg};
    use clap::Parser;


    /// M18：humanize 默认 true（人用保留 warmup），加 --no-humanize 跳过。
    #[test]
    fn humanize_defaults_to_true_unless_opted_out() {
        let cli = Cli::try_parse_from(["gsearch", "search", "test"]).unwrap();
        let Command::Search(args) = cli.cmd else { panic!("expected search") };
        assert!(args.humanize);
        let cli2 = Cli::try_parse_from(["gsearch", "search", "test", "--no-humanize"]).unwrap();
        let Command::Search(args2) = cli2.cmd else { panic!("expected search") };
        assert!(!args2.humanize);
    }

    /// batch（issue gsearch-rs-doh）：多位置参数收集为 Vec；单查询向后兼容；零查询拒绝。
    #[test]
    fn search_accepts_one_or_more_queries() {
        let cli = Cli::try_parse_from(["gsearch", "search", "rust async runtime", "tokio tutorial", "--json"]).unwrap();
        let Command::Search(args) = cli.cmd else { panic!("expected search") };
        assert_eq!(args.query, vec!["rust async runtime", "tokio tutorial"]);
        assert!(args.json);
        // 单查询向后兼容
        let cli = Cli::try_parse_from(["gsearch", "search", "test"]).unwrap();
        let Command::Search(args) = cli.cmd else { panic!("expected search") };
        assert_eq!(args.query, vec!["test"]);
        // 零查询 → clap 解析错误
        assert!(Cli::try_parse_from(["gsearch", "search"]).is_err());
    }

    #[test]
    fn captcha_prompts_are_detected_case_insensitively() {
        assert!(gsearch::search::unusual_traffic("UnUsUaL TrAfFiC"));
        assert!(gsearch::search::is_captcha("Our systems have detected traffic"));
        assert!(gsearch::search::is_captcha("/sorry/index?x=1"));
        assert!(!gsearch::search::is_captcha("normal results"));
    }

    /// M12 互斥：--open/--read/--dl 三个 flag 在 clap 解析阶段就拒绝。
    #[test]
    fn post_flags_mutually_exclusive() {
        let r = Cli::try_parse_from(["gsearch", "search", "x", "--open", "1", "--read", "1"]);
        assert!(r.is_err(), "--open + --read 应在 clap 阶段被拒绝");
        let r = Cli::try_parse_from(["gsearch", "search", "x", "--read", "1", "--dl", "1"]);
        assert!(r.is_err(), "--read + --dl 应在 clap 阶段被拒绝");
        // 单用 OK
        let r = Cli::try_parse_from(["gsearch", "search", "x", "--read", "1"]);
        assert!(r.is_ok(), "--read 单用应通过");
    }

    /// fve：browse --full 与 --headings-only clap 阶段互斥；单用通过。
    #[test]
    fn browse_full_headings_only_mutually_exclusive() {
        let r = Cli::try_parse_from(["gsearch", "browse", "https://example.com", "--full", "--headings-only"]);
        assert!(r.is_err(), "--full + --headings-only 应在 clap 阶段被拒绝");
        let r = Cli::try_parse_from(["gsearch", "browse", "https://example.com", "--full"]);
        assert!(r.is_ok(), "--full 单用应通过");
    }

    /// 6dp：--compact-meta 在 search / browse 上可解析、默认关。
    #[test]
    fn compact_meta_flag_parses() {
        let cli = Cli::try_parse_from(["gsearch", "search", "x", "--compact-meta"]).unwrap();
        let Command::Search(args) = cli.cmd else { panic!("expected search") };
        assert!(args.compact_meta);
        let cli = Cli::try_parse_from(["gsearch", "search", "x"]).unwrap();
        let Command::Search(args) = cli.cmd else { panic!("expected search") };
        assert!(!args.compact_meta, "默认应关（14 字段全量）");
        let cli = Cli::try_parse_from(["gsearch", "browse", "https://example.com", "--compact-meta"]).unwrap();
        let Command::Browse { compact_meta, .. } = cli.cmd else { panic!("expected browse") };
        assert!(compact_meta);
    }

    /// 2i1→3gw：doctor 默认 JSON 输出；--human 切人读；--json 存量兼容（noop）。
    #[test]
    fn doctor_json_flag_parses() {
        let cli = Cli::try_parse_from(["gsearch", "doctor"]).unwrap();
        let Command::Doctor { json, human } = cli.cmd else { panic!("expected doctor") };
        assert!(!human, "默认 JSON（human=false）");
        assert!(!json);
        let cli = Cli::try_parse_from(["gsearch", "doctor", "--human"]).unwrap();
        let Command::Doctor { human, .. } = cli.cmd else { panic!("expected doctor") };
        assert!(human);
        // 存量脚本 --json 仍可解析，且不改变默认 JSON 行为
        let cli = Cli::try_parse_from(["gsearch", "doctor", "--json"]).unwrap();
        let Command::Doctor { json, human } = cli.cmd else { panic!("expected doctor") };
        assert!(json);
        assert!(!human);
    }

    /// M14-1A→3gw：verify 子命令解析；默认 JSON、--human 切人读、--json 存量 noop。
    #[test]
    fn verify_subcommand_parses_with_json_flag() {
        let cli = Cli::try_parse_from(["gsearch", "verify", "https://example.com", "--json"]).unwrap();
        let Command::Verify { url, json, human, .. } = cli.cmd else { panic!("expected verify") };
        assert_eq!(url, vec!["https://example.com"]);
        assert!(json);
        assert!(!human, "--json 存量兼容：仍是 JSON 输出");
        // 默认（不带 flag）也是 JSON
        let cli = Cli::try_parse_from(["gsearch", "verify", "https://example.com"]).unwrap();
        let Command::Verify { human, .. } = cli.cmd else { panic!("expected verify") };
        assert!(!human);
        // --human 才切人读
        let cli = Cli::try_parse_from(["gsearch", "verify", "https://example.com", "--human"]).unwrap();
        let Command::Verify { human, .. } = cli.cmd else { panic!("expected verify") };
        assert!(human);
    }

    /// 3gw 主案：search 默认 JSON（human=false）、--human 翻转、--json 存量 noop、snippet 默认 160；
    /// cxa/l6o：--limit 1..=100、--read 1.. 越界值 clap 阶段拒绝。
    #[test]
    fn ai_first_flip_defaults_and_value_ranges() {
        let cli = Cli::try_parse_from(["gsearch", "search", "x"]).unwrap();
        let Command::Search(args) = cli.cmd else { panic!("expected search") };
        assert!(!args.human, "默认 JSON 输出");
        assert!(!args.json);
        let cli = Cli::try_parse_from(["gsearch", "search", "x", "--human"]).unwrap();
        let Command::Search(args) = cli.cmd else { panic!("expected search") };
        assert!(args.human);
        // 存量 --json 解析接受、行为 noop（默认已是 JSON）
        let cli = Cli::try_parse_from(["gsearch", "search", "x", "--json"]).unwrap();
        let Command::Search(args) = cli.cmd else { panic!("expected search") };
        assert!(args.json && !args.human);
        assert_eq!(args.snippet_len, 160, "snippet cap 默认 160");
        // cxa：--limit 越界拒绝（0 与 >100）
        assert!(Cli::try_parse_from(["gsearch", "search", "x", "--limit", "0"]).is_err());
        assert!(Cli::try_parse_from(["gsearch", "search", "x", "--limit", "101"]).is_err());
        assert!(Cli::try_parse_from(["gsearch", "search", "x", "--limit", "100"]).is_ok());
        // l6o：--read 0 拒绝（不再白起浏览器）
        assert!(Cli::try_parse_from(["gsearch", "search", "x", "--read", "0"]).is_err());
        assert!(Cli::try_parse_from(["gsearch", "search", "x", "--read", "1"]).is_ok());
    }

    /// --recency：day/week/month/year 枚举解析、缺省 None（URL 不得带过滤）、非法值拒绝。
    #[test]
    fn recency_flag_parses_enum_values() {
        for (value, want) in [
            ("day", RecencyArg::Day),
            ("week", RecencyArg::Week),
            ("month", RecencyArg::Month),
            ("year", RecencyArg::Year),
        ] {
            let cli = Cli::try_parse_from(["gsearch", "search", "x", "--recency", value]).unwrap();
            let Command::Search(args) = cli.cmd else { panic!("expected search") };
            assert_eq!(args.recency, Some(want));
        }
        // 缺省 = None：请求 URL 与改动前一致（不带时间过滤参数）
        let cli = Cli::try_parse_from(["gsearch", "search", "x"]).unwrap();
        let Command::Search(args) = cli.cmd else { panic!("expected search") };
        assert_eq!(args.recency, None);
        // 非法值 clap 直接拒绝
        assert!(Cli::try_parse_from(["gsearch", "search", "x", "--recency", "hour"]).is_err());
    }
}
