//! gsearch-rs 入口：clap 子命令派发 + Windows 控制台 UTF-8 + tracing 初始化

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand};

mod convert;
mod fetch;
mod general;
mod postproc;
mod shell;
mod shell_snap;
mod stealth;
mod update;

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

/// --envelope 的 CLI 枚举。v2 = batch --json 输出顶层 {meta,results}（批统计一次）。
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum EnvelopeArg {
    V2,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Google/SearXNG 搜索（`--browse N` 默认 AdaptiveRead；`--read N` 仅截前 N 条 snippet 不启浏览器）
    Search(SearchArgs),
    /// 任意 URL → 渲染后页面正文（默认 AdaptiveRead）
    Browse {
        url: String,
        /// 纯 innerText 全文（50000 cap）；与 --headings-only 互斥
        #[arg(long, default_value_t = false, group = "browse_mode")]
        full: bool,
        /// 输出默认 JSON（AI-first 契约）；此 flag 切回人读文本。
        #[arg(long, default_value_t = false)]
        human: bool,
        /// 兼容占位：JSON 已是默认输出，此 flag 解析但无效果（存量脚本零破坏）。
        #[arg(long, hide = true, default_value_t = false)]
        json: bool,
        #[arg(long)]
        from: Option<usize>,
        #[arg(long, default_value_t = false, group = "browse_mode")]
        headings_only: bool,
        /// meta 压缩（仅 --json --full 信封生效；debug 日志强制全量）
        #[arg(long, default_value_t = false)]
        compact_meta: bool,
        /// 正文以 markdown 输出（渲染后 HTML 转换，隐含全文模式，与 --headings-only 互斥）。
        /// --json 时 content_text 字段换源为 markdown，meta.format="markdown" 标注。
        #[arg(long, default_value_t = false, conflicts_with = "headings_only")]
        markdown: bool,
        /// 正文字符预算（HTML/innerText/markdown 上限；超限截断并在 meta.truncated 如实标注）。
        #[arg(long, default_value_t = 50_000, value_parser = clap::builder::RangedI64ValueParser::<usize>::from(1..=10_000_000))]
        max_chars: usize,
        /// 放行私网地址（loopback / RFC1918 / link-local / 云 metadata），语义与 fetch 对齐。
        /// 默认拒（SSRF 门）：browse 的 URL 可能来自 LLM 输出（搜索结果/页面内容间接注入），
        /// 私网地址默认不渲染；非 http/https scheme（file:///javascript:/data: 等）一律拒绝，无 flag 可绕。
        #[arg(long, default_value_t = false)]
        allow_private: bool,
        #[arg(long, value_enum, default_value_t = BrowserArg::Auto)]
        /// 选择浏览器
        browser: BrowserArg,
    },
    /// 有头窗人工登录，cookie 落 profile
    Login {
        url: String,
        #[arg(long, value_enum, default_value_t = BrowserArg::Auto)]
        /// 选择浏览器
        browser: BrowserArg,
    },
    /// 带 profile 登录态下载。-o 末段带扩展名 = 落该文件；纯目录名 = 目录语义（README 不变）。
    Dl {
        url: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// 显式文件语义：产物固定落该文件（与 -o 的目录/文件二义解耦）。
        #[arg(long)]
        output_file: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = BrowserArg::Auto)]
        /// 选择浏览器
        browser: BrowserArg,
    },
    /// 交互式 shell：起一次 Chrome 会话复用
    Shell,
    /// 检测浏览器 / profile / 网络连通性 / 出口 IP / SearXNG 健康度
    Doctor {
        /// 输出默认 JSON（AI-first 契约）；此 flag 切回人读检查表。
        #[arg(long, default_value_t = false)]
        human: bool,
        /// 兼容占位：JSON 已是默认输出，此 flag 解析但无效果（存量脚本零破坏）。
        #[arg(long, hide = true, default_value_t = false)]
        json: bool,
    },
    /// HEADless URL 健康检查：HEAD/GET + redirect 链 + SSL + 延迟（无需 Chrome）
    Verify {
        /// 单 URL = 原行为；多 URL 或 --urls-file = 批量对比表。
        /// 不设 required=true 以放行 --urls-file；零值由 required_unless_present 拒绝。
        #[arg(required_unless_present = "urls_file", num_args = 1..)]
        url: Vec<String>,
        /// 输出默认 JSON（AI-first 契约）；此 flag 切回人读表格。
        #[arg(long, default_value_t = false)]
        human: bool,
        /// 兼容占位：JSON 已是默认输出，此 flag 解析但无效果（存量脚本零破坏）。
        #[arg(long, hide = true, default_value_t = false)]
        json: bool,
        /// 单条探测总预算秒数（含 redirect；CDN 抖动端点可调高）
        #[arg(long, default_value_t = gsearch::verify::VERIFY_TIMEOUT_SECS)]
        timeout: u64,
        /// 批量：逐行读 URL 的文件（空行忽略），与位置参数互斥
        #[arg(long, conflicts_with = "url")]
        urls_file: Option<std::path::PathBuf>,
    },
    /// `gsearch fetch <url>...`：GET → 轻量正文提取 → 人读 / --json 输出。
    /// 单 URL = 原行为；多 URL = batch 并发（上限 5、单条失败不阻塞，退出码 0 全成功 / 1 部分失败 / 2 全失败）。
    Fetch {
        #[arg(required = true, num_args = 1..)]
        url: Vec<String>,
        /// 逗号分隔 CSS selector（如 "main,article"）：FixG10 J-3 多选器累加——所有命中容器
        /// inner_html 用 `\n\n---\n\n` 拼接；未命中回退全文提取，--json 在 meta.include_hit=false 标注。
        #[arg(long)]
        include: Option<String>,
        /// 输出默认 JSON（AI-first 契约）；此 flag 切回人读文本。
        #[arg(long, default_value_t = false)]
        human: bool,
        /// 兼容占位：JSON 已是默认输出，此 flag 解析但无效果（存量脚本零破坏）。
        #[arg(long, hide = true, default_value_t = false)]
        json: bool,
        /// 放行私网地址（loopback / RFC1918 / link-local / 云 metadata），同时允许内网明文 http。
        /// 默认拒（SSRF 门）；也可通过 `GSEARCH_FETCH_ALLOW_PRIVATE=1` 环境变量放行。
        #[arg(long, default_value_t = false)]
        allow_private: bool,
        /// 正文以 markdown 输出（保表格/标题/链接结构）。--json 时 text 字段换源，
        /// meta.format="markdown" 标注；无 flag 输出逐字节不变。
        #[arg(long, default_value_t = false)]
        markdown: bool,
        /// 正文字符预算（text 字段上限；超限截断并在 meta.truncated/omitted 如实标注）。
        #[arg(long, default_value_t = 50_000, value_parser = clap::builder::RangedI64ValueParser::<usize>::from(1..=10_000_000))]
        max_chars: usize,
        /// FixG10 J-1：单请求超时（秒）；默认 10、范围 1..=300。
        /// 用于 GitHub 抖动下给单 URL 留更长的握手/读体窗口；与 --retry 配合使用。
        #[arg(long, default_value_t = 10, value_parser = clap::builder::RangedI64ValueParser::<u64>::from(1..=300))]
        timeout: u64,
        /// FixG10 J-1：失败重试次数（不含首次）；默认 0 = 不重试（行为不变）；范围 0..=3。
        /// backoff 1s/2s/4s（第 N 次重试前等 2^(N-1) 秒）；stderr 一行「第 N/总 N 次重试」提示。
        /// 私网门拒 / scheme 错 / PDF 等确定性错误不重试；HTTP 4xx（除 408/429）不重试。
        #[arg(long, default_value_t = 0, value_parser = clap::builder::RangedI64ValueParser::<u32>::from(0..=3))]
        retry: u32,
        /// FixG10 J-2：JSONPath 投影（逗号分隔多路径）——text 为 JSON 时只保留指定字段。
        /// 例：`--json-keys "crate.max_version,crate.max_stable_version"` 命中后 text 换源为
        /// `{"max_version":"...","max_stable_version":"..."}`，meta.truncated_by_json_keys=true。
        /// text 非 JSON 时静默跳过（不动 text）。
        #[arg(long, value_delimiter = ',')]
        json_keys: Vec<String>,
    },
    /// 启发式相似页搜索：URL → title 关键词派生查询，SearXNG 单查 + 词重合/同域重排。
    /// 派生查询而非 exa 神经 findSimilar（README 预期管理）。
    Similar {
        /// 参照页 URL（提取 host 与 path 末段关键词；纯域名退化 site: 查询）
        url: String,
        #[arg(long, default_value_t = 3, value_parser = clap::builder::RangedI64ValueParser::<usize>::from(1..=100))]
        limit: usize,
        /// 输出默认 JSON（AI-first 契约）；此 flag 切人读文本。
        #[arg(long, default_value_t = false)]
        human: bool,
        /// 兼容占位：JSON 已是默认输出，此 flag 解析但无效果（存量脚本零破坏）。
        #[arg(long, hide = true, default_value_t = false)]
        json: bool,
    },
    /// 查 GitHub latest release 与本地版本比对；只给指引，不做自替换
    Update,
}
#[derive(Args, Debug)]
struct SearchArgs {
    /// 一到多个查询串：单查询 = 原行为（searxng → Google 回退链）；
    /// 多查询 = batch 模式（并发 searxng、单条失败不阻塞、禁浏览器回退——浏览器单例不可并发）。
    #[arg(required = true, num_args = 1..)]
    query: Vec<String>,
    /// 1..=100——SearXNG 单查最多 10 页×10 条，更大的值只会翻页白耗时（实测 10000→18s）
    #[arg(long, default_value_t = 10, value_parser = clap::builder::RangedI64ValueParser::<usize>::from(1..=100))]
    limit: usize,
    /// 时间过滤：只看 day/week/month/year 内的结果。SearXNG 加 time_range，Google SERP 加 tbs=qdr。
    /// `site:` 等查询语法原样透传，无专属参数。
    #[arg(long, value_enum)]
    recency: Option<RecencyArg>,
    /// `--read N`（FixG10 L-1）：从搜索结果中只取前 N 条的 snippet 进输出（纯 HTTP 路径，不启动浏览器）。
    /// 原 `--read N` 的"启动 Chrome 读网页正文"语义改名为 `--browse N`；本 flag 不触发浏览器、纯 snippet。
    /// 与 `--open / --dl / --browse` 互斥（clap group "post"）；1..——0 视为非法。
    #[arg(long, group = "post", value_parser = clap::builder::RangedI64ValueParser::<usize>::from(1..))]
    read: Option<usize>,
    /// `--browse N`（FixG10 L-1：原 `--read N` 启浏览器读网页正文的语义改名到此）：
    /// 用 Chrome 渲染前 N 条结果的 URL、取页面正文（AdaptiveRead），仍走浏览器 launch 链。
    /// 与 `--open / --dl / --read` 互斥；1..。
    #[arg(long, group = "post", value_parser = clap::builder::RangedI64ValueParser::<usize>::from(1..))]
    browse: Option<usize>,
    #[arg(long, group = "post")]
    dl: Option<usize>,
    #[arg(long, group = "post")]
    open: Option<usize>,
    /// 跳过搜索前的 warmup（Wikipedia/GitHub/HN 随机访问 + 滚动）+ 指纹补丁。
    /// isatty 自动档（盲测六拍板）：stdout 是 TTY（人）默认开；管道/agent 调用默认关（快档，
    /// 实测省 80s+）。显式 --humanize / --no-humanize 恒覆盖自动档。
    #[arg(long, action = clap::ArgAction::SetTrue, overrides_with = "no_humanize")]
    humanize: bool,
    /// 显式关闭 humanize（覆盖 TTY 自动档；管道/agent 调用自动档已是关，通常无需传）。
    #[arg(long, action = clap::ArgAction::SetTrue, overrides_with = "humanize")]
    no_humanize: bool,
    #[arg(long, default_value_t = false)]
    full: bool,
    /// 输出默认 JSON（AI-first 契约）；此 flag 切回人读文本。
    #[arg(long, default_value_t = false)]
    human: bool,
    /// 兼容占位：JSON 已是默认输出，此 flag 解析但无效果（存量脚本零破坏）。
    #[arg(long, hide = true, default_value_t = false)]
    json: bool,
    /// JSON 结果 snippet 截断长度（按字符）；默认 160。
    #[arg(long, default_value_t = 160, value_parser = clap::builder::RangedI64ValueParser::<usize>::from(1..=100000))]
    snippet_len: usize,
    /// `--browse N --from K`：摘要段从第 K 段开始（1-based；默认 0 = 从首段）
    #[arg(long)]
    from: Option<usize>,
    /// `--browse N --headings-only`：只输出目录（最省 token fast path）
    #[arg(long, default_value_t = false)]
    headings_only: bool,
    /// `--browse N --excerpt N`——paragraph_index 每项附该段前 N 字符实际文本（--json 生效，
    /// 受 read_max_chars 总 cap 约束）。与 --full/--headings-only 互斥；默认不启用（输出逐键不变）。
    #[arg(long, conflicts_with_all = ["full", "headings_only"])]
    excerpt: Option<usize>,
    /// batch --json 输出信封形态。v2 = 顶层 {meta,results}（批统计一次，元素不带 meta）；
    /// 默认裸数组（存量 agent 零破坏）。单查询模式忽略此 flag。
    #[arg(long, value_enum)]
    envelope: Option<EnvelopeArg>,
    /// meta 压缩到少量字段（query/truncated/provider/elapsed_ms/recency）。
    /// 默认关（13 字段全量）；--verbose debug 或 GSEARCH_LOG=debug 时强制全量（排障现场保留）。
    #[arg(long, default_value_t = false)]
    compact_meta: bool,
    /// `--dl N -o DIR`：把下载文件落到 DIR 下（按 URL 末段命名）；DIR 缺省落 CWD。
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = BrowserArg::Auto)]
    /// 选择浏览器：auto = Chrome 优先缺则 Edge，强制选 chrome/edge。
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
        Command::Browse { url, full, human, from, headings_only, compact_meta: _, markdown, browser, allow_private, max_chars, .. } => {
            let opts = general::BrowseOpts {
                full,
                // 3gw：--json 已是默认，--human 才切人读
                json: !human,
                from: from.unwrap_or(0),
                headings_only,
                markdown,
                browser: browser.into(),
                proxy: proxy.clone(),
                allow_private,
                max_chars,
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
        Command::Fetch { url, human, allow_private, include, markdown, max_chars, timeout, retry, json_keys, .. } => {
            fetch::cmd_fetch(&url, &fetch::FetchOpts {
                json: !human,
                proxy: proxy.clone(),
                allow_private,
                include,
                markdown,
                max_chars,
                timeout_secs: timeout,
                retry,
                json_keys,
                anchor_pad_lines: 0,
            }).await
        }
        Command::Similar { url, limit, human, .. } => cmd_similar(url, limit, human).await,
        Command::Update => update::cmd_update(proxy.clone()).await,
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

/// 空串/纯空白 query 前置拒绝——单查与 batch 两入口共用（cmd_search 开头统一拦），
/// 不发起任何网络。返回首个非法 query 供报错（与 qbw similar 闸同型一行报错）。
fn first_blank_query(queries: &[String]) -> Option<&str> {
    queries.iter().map(String::as_str).find(|q| q.trim().is_empty())
}

/// P1 盲测八 site: 静默吞掉修复（H 实锤：site:github.com/... whitelist 引擎 0 命中，
/// 错误只说"查询无结果"无解释）。检测 query 含 site: 限定符时返回可行动建议；
/// 缺席 = 无 site:（语义与 proxy/recency 一致，键整体缺席不污染正常查询）。
fn site_warn_for(query: &str) -> Option<String> {
    if !contains_site_qualifier(query) {
        return None;
    }
    Some(
        "查询含 site: 限定符——白名单 SearXNG 引擎常忽略或仅特定引擎支持；\
         建议拆词（site:github.com → 加 inurl: 限定或换纯词）或用 -site test 排查引擎命中".to_string(),
    )
}

/// 检测 site: 限定符（前导 word boundary 简单匹配，不解析完整搜索语法）。
/// 也支持 site: 含子域路径形态（如 site:github.com/tokio-rs/tokio）。
fn contains_site_qualifier(query: &str) -> bool {
    // 简易 token 化：以空白/引号切词，找 site: 起首的 token
    let bytes = query.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // 跳过空白
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        // 跳引号
        if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
            let q = bytes[i];
            i += 1;
            while i < bytes.len() && bytes[i] != q {
                i += 1;
            }
            i += 1;
            continue;
        }
        // 找 token 结尾
        let start = i;
        while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' && bytes[i] != b'"' && bytes[i] != b'\'' {
            i += 1;
        }
        let token_bytes = &bytes[start..i];
        // site: 限定符（含前缀 - 排除形态）
        if token_bytes.len() > 5
            && token_bytes[..5].eq_ignore_ascii_case(b"site:")
        {
            return true;
        }
        if token_bytes.len() > 6
            && token_bytes[0] == b'-'
            && token_bytes[1..6].eq_ignore_ascii_case(b"site:")
        {
            return true;
        }
    }
    false
}

/// 盲测六 isatty 拍板：humanize 生效档 = 显式 flag 优先，都未传时跟随 stdout 是否 TTY——
/// 人（TTY）保留 warmup 档，管道/agent 自动快档。纯函数，三态单测锁。
fn resolve_humanize(explicit_on: bool, explicit_off: bool, stdout_is_tty: bool) -> bool {
    if explicit_on {
        true
    } else if explicit_off {
        false
    } else {
        stdout_is_tty
    }
}

impl SearchArgs {
    /// 运行时生效档（--humanize/--no-humanize 显式传参恒覆盖 isatty 自动档）。
    fn humanize_effective(&self) -> bool {
        resolve_humanize(self.humanize, self.no_humanize, std::io::stdout().is_terminal())
    }
}

async fn cmd_search(args: SearchArgs, proxy: Option<String>) -> Result<ExitCode> {
    // o1p：空 query 在客户端可知即非法——前置拒绝，不再白烧 6-11s 完整回退链
    if let Some(q) = first_blank_query(&args.query) {
        eprintln!("error: query 不能为空或纯空白（收到 {q:?}）；请给出搜索词");
        return Ok(ExitCode::from(2));
    }
    // batch 多查询：并发 searxng、单条失败不阻塞、禁浏览器回退（issue gsearch-rs-doh）
    if args.query.len() > 1 {
        return cmd_search_batch(args).await;
    }
    // d3u：--read / --browse N 的静态可判越界（N > --limit）在发起搜索前拒绝——结果数 ≤ limit 恒成立，
    // N > limit 必越界，参数校验阶段 rc=2，零网络零浏览器。运行时越界（N ≤ limit 但返回不足）
    // 仍在搜索完成后、浏览器 launch 前校验（post 块）。
    if let Some(n) = args.read
        && n > args.limit
    {
        eprintln!("error: --read {n} 越界：结果数上限为 --limit {}，请求前即可判定", args.limit);
        return Ok(ExitCode::from(2));
    }
    if let Some(n) = args.browse
        && n > args.limit
    {
        eprintln!("error: --browse {n} 越界：结果数上限为 --limit {}，请求前即可判定", args.limit);
        return Ok(ExitCode::from(2));
    }
    let started = std::time::Instant::now();
    let browser_kind = browser_arg_to_kind(args.browser);
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
    if let gsearch::search::SearxngAttempt::CircuitBroken(reason) = &searxng {
        if json_mode {
            // stderr 诊断行已由 try_searxng 打（豁免未来任何静默策略）
            emit_searxng_degraded_json(&query, &args, proxy.clone(), recency, started.elapsed().as_millis(), reason);
        }
        // 人读模式 stdout 不打假结果；退出码 2 = 无结果语义族
        return Ok(ExitCode::from(2));
    }
    // yq6 + o1p：记下主源状态——FallbackGoogle(Some) = SearXNG 故障（回退空 → searxng_degraded）；
    // FallbackGoogle(None)/HealthyEmpty = SearXNG 源健康零结果（回退空 → filtered_empty/no_results）。
    let searxng_fallback_reason = match &searxng {
        gsearch::search::SearxngAttempt::FallbackGoogle(r) => r.clone(),
        _ => None,
    };
    let searxng_source_healthy = matches!(
        searxng,
        gsearch::search::SearxngAttempt::HealthyEmpty
            | gsearch::search::SearxngAttempt::FallbackGoogle(None)
    );
    // 9gb：NotConfigured = 裸环境（没配 SearXNG）——落到 Google 直爬时给一行配置出口提示
    let searxng_not_configured = matches!(&searxng, gsearch::search::SearxngAttempt::NotConfigured);
    let (mut results, captcha_solved, provider) = match searxng {
        gsearch::search::SearxngAttempt::Results(
            gsearch::search::SearchOutcome::Results { results, captcha_solved, provider },
        ) => (results, captcha_solved, provider),
        // NotConfigured（未配 SearXNG）/ FallbackGoogle（预检通过，回退 warn 已打）→ Google 直爬
        _ => {
        // 9gb（④打回轮1）：hint 前移到回退决策点——用户/agent 在 Chrome 启动等待期就能看到，
        // 而不是运行结束才出现
        if searxng_not_configured {
            eprintln!("[hint] SearXNG 未配置，已回退 Google 直爬（可配 GSEARCH_SEARXNG_URL 提速；agent 高频建议 --no-humanize）");
        }
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
            let page = gsearch::browser::open_page(&browser).await?;
            if args.humanize_effective() {
                // 9gb：isatty 自动档下这只在用户显式 --humanize 且非交互管道时出现——提示耗时代价
                if !std::io::stdout().is_terminal() {
                    eprintln!("[hint] humanize 已显式开启（非交互管道）：warmup + CAPTCHA 处理会显著增加耗时；高频调用去掉 --humanize 走快档");
                }
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
                    emit_captcha_timeout_json(&query, &args, proxy.clone(), recency, started.elapsed().as_millis());
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
    // yq6 + o1p：主源降级且回退也空 → searxng_degraded（真因透传）；主源健康空 →
    // filtered_empty（recency 过滤后）/ no_results（查询无果）——防把「没新鲜结果」当基础设施故障。
    let degraded = results.is_empty() && searxng_fallback_reason.is_some();
    let healthy_empty = results.is_empty() && searxng_source_healthy;
    let (run_status, run_message) = if degraded {
        let reason = searxng_fallback_reason.as_deref().unwrap_or_default();
        (gsearch::types::RunStatus::SearxngDegraded, gsearch::search::circuit_diag(reason))
    } else if healthy_empty {
        let (s, m) = gsearch::search::SearxFail::empty_status(recency);
        (s, m.to_string())
    } else if captcha_solved {
        (gsearch::types::RunStatus::Ok, "本次搜索经过了人工 CAPTCHA 验证".into())
    } else {
        (gsearch::types::RunStatus::Ok, String::new())
    };
    // 0mf：--json + --browse 时 read 产物并入单一 JSON 文档——envelope 延后装配，stdout 只出
    // 一份可解析 JSON（旧行为 envelope 先打 + read raw 追加 = json.loads 崩）。
    // b95：--headings-only 连 SERP 集都不进输出（text 模式同样跳过 print_text）。
    // FixG10 L-1：`--read N` 不再触发浏览器，改为纯 snippet 截断；原语义改名为 `--browse N`。
    let read_n = args.read;
    let browse_n = args.browse;
    // --read N 越界前置拒绝（不依赖浏览器 launch）——与 --browse 同型校验
    if let Some(n) = read_n
        && n > results.len()
    {
        eprintln!("error: --read {n} 越界（结果数 {}）", results.len());
        return Ok(ExitCode::from(2));
    }
    if let Some(n) = browse_n
        && n > results.len()
    {
        eprintln!("error: --browse {n} 越界（结果数 {}）", results.len());
        return Ok(ExitCode::from(2));
    }
    // FixG10 L-1：--read N 命中时直接 truncate 结果集到前 N（snippet-only，不启浏览器）。
    // envelope + stdout 在 truncate 之后打，输出只含 top N 条 snippet。
    if let Some(n) = read_n {
        results.truncate(n);
    }
    let browse_solo_json = json_mode && browse_n.is_some();
    let headings_solo = browse_n.is_some() && args.headings_only;
    let meta = gsearch::types::MetaOutput {
        tool: "gsearch",
        version: env!("CARGO_PKG_VERSION"),
        query: query.clone(),
        profile: gsearch::browser::profile_name_only(),
        proxy: proxy.clone(),
        humanize: args.humanize_effective(),
        limit: args.limit,
        elapsed_ms: started.elapsed().as_millis(),
        truncated: results.len() >= args.limit,
        truncated_at_offset: 0,
        provider: provider.into(),
        recency: recency.map(|r| r.as_str().into()),
        site_warn: site_warn_for(&query),
    };
    // o1p：stderr 诊断行与 run.status 同源——run move 进信封后仍需在空结果分支打 stderr
    let stderr_diag = (results.is_empty() && !run_message.is_empty()).then(|| run_message.clone());
    let run = gsearch::types::RunStatusInfo {
        // yq6 + o1p：degraded / filtered_empty / no_results 三态零结果语义分立（零结果 ≠ Ok）
        status: run_status,
        captcha_solved,
        message: run_message,
    };
    let envelope = gsearch::types::OutputEnvelope { meta, run, results: &results };
    // FixG10 L-1：--read N 已 truncate 结果，envelope 用 truncate 后的切片；与 --browse 共用同一份 envelope。
    // browse_solo_json 由 browse 触发（read 已是 snippet-only，不需要单独 envelope 装配）。
    if !browse_solo_json {
        if json_mode {
            gsearch::output::print_envelope_json(&envelope)?;
        } else if !headings_solo {
            gsearch::output::print_text(&results);
        }
    }
    // M4 后处理（PLAN §3.4）：clap ArgGroup "post" 保证 --open/--read/--browse/--dl 互斥；仅剩运行时分发。
    // FixG10 L-1：--read 已 truncate（无浏览器）；--browse 走原 read 路径（启 Chrome 读网页正文）。
    let post = async {
        if let Some(n) = args.open {
            postproc::open(&results, n)?;
        }
        if let Some(n) = browse_n {
            // M17 惰性：SearXNG 命中时浏览器尚未启动，--browse/--dl 首次用到才 launch(headless)
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
            if browse_solo_json {
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
    // 仅 Google 路径（或 --browse/--dl 惰性启动过）才有 browser 需要收尾
    if let Some(browser) = browser_opt.as_mut() {
        gsearch::browser::graceful_close(browser).await;
    }
    if let Err(e) = post {
        eprintln!("postproc 失败: {e}");
        if browse_solo_json {
            // w9y：browse 失败必须结构化可见——补打信封并顶层挂 read_error，
            // agent 不再把「SERP 正常 + 读失败」误读为全成功。
            let mut doc = serde_json::to_value(&envelope)?;
            if let Some(obj) = doc.as_object_mut() {
                obj.insert("read_error".into(), serde_json::Value::String(format!("{e:#}")));
            }
            println!("{doc}");
        }
        if browse_n.is_some() {
            // w9y：显式请求的 browse 失败不再静默 exit 0
            return Ok(ExitCode::from(1));
        }
    }
    if results.is_empty() {
        // o1p：stderr 诊断行与 run.status 同步（degraded/filtered_empty/no_results 各自一行）
        if let Some(d) = &stderr_diag {
            eprintln!("{d}");
        }
        eprintln!("未找到结果");
        Ok(ExitCode::from(2))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// `similar <url>`——启发式派生查询（title 关键词，SearXNG 单查 + 词重合/同域重排）。
/// JSON 信封 = 常规 envelope + 顶层 similar_of/note 注解（0mf to_value 手法，不动公共结构）。
async fn cmd_similar(url: String, limit: usize, human: bool) -> Result<ExitCode> {
    // qbw：非 URL 入参会退化成 site:<garbage> 派生查询白烧 token，发起前拒掉
    if !gsearch::search::looks_like_url(&url) {
        eprintln!("error: 输入应为 URL（如 https://example.com/page）：{url}");
        return Ok(ExitCode::from(2));
    }
    let started = std::time::Instant::now();
    let (mut hits, derived_query) =
        gsearch::search::similar(&url, limit).await.map_err(anyhow::Error::msg)?;
    // cw8 同款：snippet cap 装配层统一
    for h in &mut hits {
        h.hit.snippet = gsearch::output::truncate_snippet(&h.hit.snippet, 160);
    }
    if human {
        for (i, h) in hits.iter().enumerate() {
            println!("{}. {}", i + 1, h.hit.title);
            println!("   {}", h.hit.url);
            println!("   [similarity] {}", h.similarity);
            println!("   {}", h.hit.snippet);
        }
        return Ok(ExitCode::SUCCESS);
    }
    // 1az：快乐路径信封自洽——有结果且 rc=0 → status=ok；run.message 带 provider/结果数摘要
    //（原 default() 是 status=error，与 rc=0/results 非空三信号互相打架）。
    // hits 空不可达：searxng_collect Ok 恒非空（零结果走 Err 分支）。
    let run = gsearch::types::RunStatusInfo {
        status: gsearch::types::RunStatus::Ok,
        captcha_solved: false,
        // 先组装 message（derived_query 随后 move 进 meta.query）
        message: format!("searxng 派生查询命中 {} 条（查询: {derived_query}）", hits.len()),
    };
    let meta = gsearch::types::MetaOutput {
        tool: "gsearch",
        version: env!("CARGO_PKG_VERSION"),
        query: derived_query.clone(),
        profile: gsearch::browser::profile_name_only(),
        proxy: None,
        humanize: false,
        limit,
        elapsed_ms: started.elapsed().as_millis(),
        truncated: hits.len() >= limit,
        truncated_at_offset: 0,
        provider: "searxng".into(),
        recency: None,
        site_warn: site_warn_for(&derived_query),
    };
    let envelope = gsearch::types::OutputEnvelope { meta, run, results: &hits };
    let mut doc = serde_json::to_value(&envelope)?;
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("similar_of".into(), serde_json::Value::String(url.clone()));
        obj.insert(
            "note".into(),
            serde_json::Value::String("启发式派生查询（title 词重合 + 同域重排），非 exa 神经 findSimilar".into()),
        );
    }
    println!("{doc}");
    Ok(ExitCode::SUCCESS)
}

/// batch 多查询：并发走 SearXNG，单条失败不阻塞其他条目。
/// 刻意边界：禁浏览器回退（浏览器单例不可并发）；--read/--dl/--open 不参与 batch。
/// 输出：--json 为裸 BatchEntry 数组；人读模式逐条分隔标题。退出码：全成功 0 / 部分失败 1 / 全部失败 2。
async fn cmd_search_batch(args: SearchArgs) -> Result<ExitCode> {
    if args.read.is_some() || args.browse.is_some() || args.dl.is_some() || args.open.is_some() {
        return Err(anyhow!(
            "batch 多查询暂不支持 --read/--browse/--dl/--open（batch 无浏览器参与）；请对单查询使用"
        ));
    }
    // 3gw：JSON 默认，--human 切人读
    let json_mode = !args.human;
    let started = std::time::Instant::now();
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
                // o1p：源健康零结果按 recency 定态（filtered_empty/no_results），不再一律 error；
                // 故障真因仍按旧格式进 error message（存量契约不破）
                Err(fail) => match &fail {
                    gsearch::search::SearxFail::HealthyEmpty => {
                        let (s, m) = gsearch::search::SearxFail::empty_status(recency);
                        (s, m.to_string(), vec![])
                    }
                    gsearch::search::SearxFail::SourceError(reason) => (
                        gsearch::types::RunStatus::Error,
                        format!("SearXNG 查询失败（{reason}）；batch 模式禁浏览器回退"),
                        vec![],
                    ),
                },
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
                // batch 全程纯 HTTP 直连局域网 searxng（searxng.rs 固定 no_proxy），代理字段恒空
                proxy: None,
                humanize: args.humanize_effective(),
                limit: args.limit,
                elapsed_ms,
                truncated: results.len() >= args.limit,
                truncated_at_offset: 0,
                provider: "searxng".into(),
                recency: recency.map(|r| r.as_str().into()),
                site_warn: site_warn_for(&query),
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
            } else if e.status == gsearch::types::RunStatus::Ok {
                gsearch::output::print_text(&e.results);
            } else {
                // o1p：三态非 Ok 状态（filtered_empty/no_results/degraded）透出诊断行
                println!("  {}", e.message);
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

/// CAPTCHA 超时时输出 status=captcha_timeout 的 JSON 信封。
fn emit_captcha_timeout_json(
    query: &str,
    args: &SearchArgs,
    proxy: Option<String>,
    recency: Option<gsearch::search::Recency>,
    elapsed_ms: u128,
) {
    let meta = gsearch::types::MetaOutput {
        tool: "gsearch",
        version: env!("CARGO_PKG_VERSION"),
        query: query.to_string(),
        profile: gsearch::browser::profile_name_only(),
        proxy,
        humanize: args.humanize_effective(),
        limit: args.limit,
        elapsed_ms,
        truncated: false,
        truncated_at_offset: 0,
        // CAPTCHA 超时只发生在 Google 直爬路径（searxng 不撞码）
        provider: "google".into(),
        recency: recency.map(|r| r.as_str().into()),
        site_warn: site_warn_for(query),
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

/// SearXNG 熔断输出——status=searxng_degraded（元审计硬约束值）、provider 照实标
/// searxng、results 空；message 带诊断行让 Agent 无需解析 stderr。
/// 真因（HTTP 错误码等）透传进 message，与 stderr 诊断行同源（circuit_diag）。
fn emit_searxng_degraded_json(
    query: &str,
    args: &SearchArgs,
    proxy: Option<String>,
    recency: Option<gsearch::search::Recency>,
    elapsed_ms: u128,
    reason: &str,
) {
    let meta = gsearch::types::MetaOutput {
        tool: "gsearch",
        version: env!("CARGO_PKG_VERSION"),
        query: query.to_string(),
        profile: gsearch::browser::profile_name_only(),
        proxy,
        humanize: args.humanize_effective(),
        limit: args.limit,
        elapsed_ms,
        truncated: false,
        truncated_at_offset: 0,
        // 熔断时结果确实来自 SearXNG 链路（provider 照实标注，非 google）
        provider: "searxng".into(),
        recency: recency.map(|r| r.as_str().into()),
        site_warn: site_warn_for(query),
    };
    let run = gsearch::types::RunStatusInfo {
        status: gsearch::types::RunStatus::SearxngDegraded,
        captcha_solved: false,
        message: gsearch::search::circuit_diag(reason),
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
/// --json 输出 {checks:[{name,status,message,value?}], elapsed_ms, fail_count, warn_count}
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
    let profile_res = gsearch::browser::profile_dir();
    let (st, msg, val) = match &profile_res {
        Ok(dir) => match test_profile_writable(dir) {
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
    let profile_dir = profile_res.ok();

    // 4) 出口 IP（明文 HTTP GET 80 端口，3s 超时；失败降 WARN）
    let (st, msg, val, ip_drift) = match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fetch_public_ip(),
    )
    .await
    {
        // 8lp③：值类检查——IP 进 value
        Ok(Ok(ip)) => {
            // 5a5 工具侧：与上次记录比对，漂移则附加一行 WARN（passwall 路由器层仍需人工）
            let drift = check_exit_ip_drift(&ip, profile_dir.as_deref());
            (DoctorStatus::Ok, String::new(), Some(ip), drift)
        }
        Ok(Err(e)) => (
            DoctorStatus::Warn,
            format!("出口 IP 不可达（撞码调试辅助；改用代理/VPN 后重试）: {e}"),
            None,
            None,
        ),
        Err(_) => (
            DoctorStatus::Warn,
            "出口 IP 检测超时（2s）".to_string(),
            None,
            None,
        ),
    };
    record_check(&mut checks, &mut fail, &mut warn, "exit_ip", st, msg, val);
    if let Some(drift) = ip_drift {
        record_check(&mut checks, &mut fail, &mut warn, "exit_ip_drift", DoctorStatus::Warn, drift, None);
    }

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

/// 5a5 工具侧：出口 IP 漂移检测。上次 IP 存 `<profile>/last_exit_ip`；
/// 首次无记录 → 记当前、不 WARN；与上次相同 → 静默；不同 → 返回 WARN 文案。
/// 记录写入失败只 warn 不影响 doctor 结果（漂移检测是增值项，不是健康门）。
fn check_exit_ip_drift(ip: &str, profile_dir: Option<&std::path::Path>) -> Option<String> {
    let path = profile_dir?.join("last_exit_ip");
    let prev = std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    if let Err(e) = std::fs::write(&path, ip) {
        tracing::warn!("记录出口 IP 失败（漂移检测下次不可用）: {} ({e})", path.display());
    }
    match prev {
        None => None,
        Some(p) if p == ip => None,
        Some(p) => Some(format!(
            "出口 IP 自上次检查已变化（{p} → {ip}）——VPN/代理切换或 IP 信誉重置信号"
        )),
    }
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
    use super::check_exit_ip_drift;
    use super::resolve_humanize;
    use super::{contains_site_qualifier, first_blank_query, site_warn_for, Cli, Command, RecencyArg};
    use clap::Parser;

    /// P1 site: 检测：含 site: 限定符的 query 返回 Some(warn)；其他 None。
    /// H 盲测八 site:github.com/tokio-rs/tokio refactor 0 命中无解释。
    #[test]
    fn site_warn_for_returns_some_only_with_site_qualifier() {
        // 命中：site: 起首 token
        let w = site_warn_for("site:github.com/tokio-rs/tokio refactor");
        assert!(w.is_some(), "site: 起首应命中");
        let msg = w.unwrap();
        assert!(msg.contains("site:"), "warn 应提及 site:");
        // 不命中：普通查询
        assert!(site_warn_for("rust async").is_none());
        // 不命中：site 在词中（不是限定符）
        assert!(site_warn_for("website:foo bar").is_none(), "site 在词中不视作限定符");
        // 命中：大小写无关
        assert!(site_warn_for("Site:example.com foo").is_some());
        // 引号内 site: 大多数搜索引擎视为字面字符串而非限定符——不命中
        assert!(site_warn_for(r#""site:foo.com" bar"#).is_none(), "引号内视为字面字符串");
        // 不命中：相似前缀（sit:）
        assert!(site_warn_for("sit:foo bar").is_none());
    }

    /// contains_site_qualifier：基础边界——词形态、空白、引号、前后置。
    #[test]
    fn contains_site_qualifier_boundary_cases() {
        assert!(contains_site_qualifier("site:foo"));
        assert!(contains_site_qualifier("site:github.com/a/b refactor"));
        assert!(contains_site_qualifier("foo site:github.com"));
        assert!(contains_site_qualifier("a -site:test.com"));
        // 引号内 site: 视为字面字符串，不命中
        assert!(!contains_site_qualifier(r#""site:foo.com" bar"#));
        assert!(!contains_site_qualifier(""));
        assert!(!contains_site_qualifier("rust tokio"));
        assert!(!contains_site_qualifier("website:foo"));
        assert!(!contains_site_qualifier("sit:foo"));
        // 多词含 site: 也命中（site:bar 是合法限定符）
        assert!(contains_site_qualifier("foo site:bar baz qux"));
    }

    /// 5a5：首跑无记录→None 且落盘；同 IP→静默；换 IP→WARN 文案含新旧 IP。
    #[test]
    fn exit_ip_drift_three_states() {
        let dir = std::env::temp_dir().join(format!("gsearch_drift_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = Some(dir.as_path());
        // 首次：无记录，只显示 IP 不 WARN
        assert_eq!(check_exit_ip_drift("1.1.1.1", d), None);
        assert_eq!(std::fs::read_to_string(dir.join("last_exit_ip")).unwrap(), "1.1.1.1");
        // 相同：静默
        assert_eq!(check_exit_ip_drift("1.1.1.1", d), None);
        // 漂移：WARN 含 A→B
        let warn = check_exit_ip_drift("2.2.2.2", d).unwrap();
        assert!(warn.contains("1.1.1.1") && warn.contains("2.2.2.2"), "WARN 应含新旧 IP: {warn}");
        // 无 profile 目录：直接 None（不 panic）
        assert_eq!(check_exit_ip_drift("3.3.3.3", None), None);
        std::fs::remove_dir_all(&dir).ok();
    }


    /// o1p：空串/纯空白 query 前置拒绝——单查与 batch 两入口共用的入口校验纯函数。
    #[test]
    fn blank_query_gate_detects_whitespace_only() {
        assert_eq!(first_blank_query(&["".into()]), Some(""));
        assert_eq!(first_blank_query(&["   ".into(), "\t\n".into()]), Some("   "));
        assert_eq!(first_blank_query(&["rust async".into(), "  ".into()]), Some("  "));
        assert_eq!(first_blank_query(&["rust async".into(), "tokio".into()]), None);
    }

    /// 盲测六 isatty 拍板：--humanize / --no-humanize 两个显式 flag 都可解析。
    #[test]
    fn humanize_explicit_flags_parse_both_directions() {
        let cli = Cli::try_parse_from(["gsearch", "search", "test", "--humanize"]).unwrap();
        let Command::Search(args) = cli.cmd else { panic!("expected search") };
        assert!(args.humanize && !args.no_humanize);
        let cli2 = Cli::try_parse_from(["gsearch", "search", "test", "--no-humanize"]).unwrap();
        let Command::Search(args2) = cli2.cmd else { panic!("expected search") };
        assert!(!args2.humanize && args2.no_humanize);
    }

    /// 盲测六 isatty 拍板：生效档纯函数三态——显式覆盖恒优先；都未传跟随 TTY。
    #[test]
    fn resolve_humanize_three_states() {
        // 显式 --humanize 覆盖非 TTY（管道里强制慢档）
        assert!(resolve_humanize(true, false, false));
        // 显式 --no-humanize 覆盖 TTY（终端里强制快档）
        assert!(!resolve_humanize(false, true, true));
        // 默认：跟随 stdout——人（TTY）开，管道/agent 关
        assert!(resolve_humanize(false, false, true));
        assert!(!resolve_humanize(false, false, false));
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

    /// M12 互斥：--open/--read/--browse/--dl 四个 flag 在 clap 解析阶段就拒绝。
    /// FixG10 L-1：--read 是 snippet-only、--browse 启浏览器；两者不能同时传。
    #[test]
    fn post_flags_mutually_exclusive() {
        let r = Cli::try_parse_from(["gsearch", "search", "x", "--open", "1", "--read", "1"]);
        assert!(r.is_err(), "--open + --read 应在 clap 阶段被拒绝");
        let r = Cli::try_parse_from(["gsearch", "search", "x", "--read", "1", "--dl", "1"]);
        assert!(r.is_err(), "--read + --dl 应在 clap 阶段被拒绝");
        // L-1：--read + --browse 也互斥（同一 post 组）
        let r = Cli::try_parse_from(["gsearch", "search", "x", "--read", "1", "--browse", "1"]);
        assert!(r.is_err(), "--read + --browse 应在 clap 阶段被拒绝");
        let r = Cli::try_parse_from(["gsearch", "search", "x", "--browse", "1", "--dl", "1"]);
        assert!(r.is_err(), "--browse + --dl 应在 clap 阶段被拒绝");
        // 单用 OK
        let r = Cli::try_parse_from(["gsearch", "search", "x", "--read", "1"]);
        assert!(r.is_ok(), "--read 单用应通过");
        let r = Cli::try_parse_from(["gsearch", "search", "x", "--browse", "1"]);
        assert!(r.is_ok(), "--browse 单用应通过");
    }

    /// browse --full 与 --headings-only clap 阶段互斥；单用通过。
    #[test]
    fn browse_full_headings_only_mutually_exclusive() {
        let r = Cli::try_parse_from(["gsearch", "browse", "https://example.com", "--full", "--headings-only"]);
        assert!(r.is_err(), "--full + --headings-only 应在 clap 阶段被拒绝");
        let r = Cli::try_parse_from(["gsearch", "browse", "https://example.com", "--full"]);
        assert!(r.is_ok(), "--full 单用应通过");
    }

    /// --compact-meta 在 search / browse 上可解析、默认关。
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

    /// doctor 默认 JSON 输出；--human 切人读；--json 存量兼容（noop）。
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

    /// verify 子命令解析；默认 JSON、--human 切人读、--json 存量 noop。
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

    /// search 默认 JSON（human=false）、--human 翻转、--json 存量 noop、snippet 默认 160；
    /// --limit 1..=100、--read 1.. 越界值 clap 阶段拒绝。
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
        // l6o：--read 0 拒绝（不再白起浏览器）；L-1：--read 是 snippet-only，0 仍拒
        assert!(Cli::try_parse_from(["gsearch", "search", "x", "--read", "0"]).is_err());
        assert!(Cli::try_parse_from(["gsearch", "search", "x", "--read", "1"]).is_ok());
        // L-1：--browse 0 同理拒
        assert!(Cli::try_parse_from(["gsearch", "search", "x", "--browse", "0"]).is_err());
        assert!(Cli::try_parse_from(["gsearch", "search", "x", "--browse", "1"]).is_ok());
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

    /// similar 子命令解析——默认 limit 3、--json 兼容占位、limit 越界拒绝。
    #[test]
    fn similar_subcommand_parses() {
        let cli = Cli::try_parse_from(["gsearch", "similar", "https://docs.rs/serde"]).unwrap();
        let Command::Similar { url, limit, human, json } = cli.cmd else { panic!("expected similar") };
        assert_eq!(url, "https://docs.rs/serde");
        assert_eq!(limit, 3, "默认 limit 3");
        assert!(!human && !json);
        let cli = Cli::try_parse_from(["gsearch", "similar", "https://docs.rs/serde", "--limit", "5"]).unwrap();
        let Command::Similar { limit, .. } = cli.cmd else { panic!("expected similar") };
        assert_eq!(limit, 5);
        // cxa 同款护栏：limit 0 拒绝
        assert!(Cli::try_parse_from(["gsearch", "similar", "https://docs.rs/serde", "--limit", "0"]).is_err());
        // 零位置参数拒绝
        assert!(Cli::try_parse_from(["gsearch", "similar"]).is_err());
    }

    /// help 文本不得泄漏内部 issue 代号（盲测七受试者可见 cxa/l6o/3gw/M9/6dp）。
    /// 渲染顶层级与全部子命令 help，断言已知代号零命中；泄漏时报出具体行便于定位。
    #[test]
    fn help_text_free_of_internal_issue_codes() {
        use clap::CommandFactory;
        let codes = [
            "cxa", "l6o", "3gw", "M9", "6dp", "n76", "xih", "pkp", "kda", "dsg",
            "nx4", "e1i", "cw8", "fve", "e7c", "q34", "i9a", "745", "ptb",
        ];
        let mut cmd = Cli::command();
        let mut texts = vec![cmd.render_help().to_string()];
        for sub in cmd.get_subcommands_mut() {
            texts.push(sub.render_help().to_string());
        }
        for text in &texts {
            for code in codes {
                let leak = text.lines().find(|l| l.contains(code));
                assert!(leak.is_none(), "help 泄漏内部代号 {code}: {:?}", leak);
            }
        }
    }

    /// FixG10 J-1/J-2：fetch 新 flag 解析——--timeout 1..=300、--retry 0..=3、--json-keys 逗号分隔。
    /// FixG10 L-1：fetch 命令解析不挂 --read/--browse（那是 search 的 flag）。
    #[test]
    fn fetch_new_flags_parse_with_ranges() {
        // --timeout 默认 10、范围 1..=300
        let cli = Cli::try_parse_from(["gsearch", "fetch", "https://e.test/"]).unwrap();
        let Command::Fetch { timeout, retry, json_keys, .. } = cli.cmd else { panic!("expected fetch") };
        assert_eq!(timeout, 10, "默认 timeout 10");
        assert_eq!(retry, 0, "默认 retry 0");
        assert!(json_keys.is_empty(), "默认 json-keys 空");
        // --timeout 30 + --retry 2 命中
        let cli = Cli::try_parse_from(["gsearch", "fetch", "https://e.test/", "--timeout", "30", "--retry", "2"]).unwrap();
        let Command::Fetch { timeout, retry, .. } = cli.cmd else { panic!("expected fetch") };
        assert_eq!(timeout, 30);
        assert_eq!(retry, 2);
        // 越界：--timeout 0 / 301 都拒
        assert!(Cli::try_parse_from(["gsearch", "fetch", "https://e.test/", "--timeout", "0"]).is_err());
        assert!(Cli::try_parse_from(["gsearch", "fetch", "https://e.test/", "--timeout", "301"]).is_err());
        // 越界：--retry 4 拒
        assert!(Cli::try_parse_from(["gsearch", "fetch", "https://e.test/", "--retry", "4"]).is_err());
        // --json-keys 逗号分隔多字段
        let cli = Cli::try_parse_from(["gsearch", "fetch", "https://e.test/", "--json-keys", "crate.max_version,crate.max_stable_version"]).unwrap();
        let Command::Fetch { json_keys, .. } = cli.cmd else { panic!("expected fetch") };
        assert_eq!(json_keys, vec!["crate.max_version".to_string(), "crate.max_stable_version".to_string()]);
    }
}
