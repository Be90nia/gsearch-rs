//! gsearch-rs 入口：clap 子命令派发 + Windows 控制台 UTF-8 + tracing 初始化

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

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
    version,
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
#[derive(Subcommand, Debug)]
enum Command {
    /// Google 搜索（M2 实现；M9 `--read N` 默认 AdaptiveRead）
    Search(SearchArgs),
    /// 任意 URL → 渲染后页面正文（M1 主验收点；M9 默认 AdaptiveRead）
    Browse {
        url: String,
        #[arg(long, default_value_t = false)]
        full: bool,
        #[arg(long, default_value_t = false)]
        json: bool,
        #[arg(long)]
        from: Option<usize>,
        #[arg(long, default_value_t = false)]
        headings_only: bool,
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
    /// 带 profile 登录态下载（M6）
    Dl {
        url: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = BrowserArg::Auto)]
        /// 选择浏览器（M11）
        browser: BrowserArg,
    },
    /// 交互式 shell：起一次 Chrome 会话复用（M7 追加里程碑）
    Shell,
    /// 检测浏览器 / profile / 网络连通性 / 出口 IP（M11 doctor）
    Doctor,
    /// HEADless URL 健康检查：HEAD/GET + redirect 链 + SSL + 延迟（M14-1A，无需 Chrome）
    Verify {
        url: String,
        /// 输出结构化 JSON（默认人类可读 text）
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}
#[derive(Args, Debug)]
struct SearchArgs {
    query: String,
    #[arg(long, default_value_t = 10)]
    limit: usize,
    /// `--open / --read / --dl` 互斥：每次只能指定一个；不可同时传。
    #[arg(long, group = "post")]
    read: Option<usize>,
    #[arg(long, group = "post")]
    dl: Option<usize>,
    #[arg(long, group = "post")]
    open: Option<usize>,
    /// 启用 fingerprint 补丁 + Google 搜索前 warmup（opt-in；新 profile/裸搜可能撞码时建议 off）。
    #[arg(long, default_value_t = false)]
    humanize: bool,
    /// `--read N` 输出兜底纯 innerText（5000 字截断），覆盖默认 AdaptiveRead
    #[arg(long, default_value_t = false)]
    full: bool,
    /// `--read N` 输出 AdaptiveRead 结构化 JSON
    #[arg(long, default_value_t = false)]
    json: bool,
    /// `--read N --from K`：摘要段从第 K 段开始（1-based；默认 0 = 从首段）
    #[arg(long)]
    from: Option<usize>,
    /// `--read N --headings-only`：只输出目录（最省 token fast path）
    #[arg(long, default_value_t = false)]
    headings_only: bool,
    /// `--dl N -o DIR`：把下载文件落到 DIR 下（按 URL 末段命名）；DIR 缺省落 CWD。M13 修复两处不一致。
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = BrowserArg::Auto)]
    /// 选择浏览器：auto = Chrome 优先缺则 Edge，强制选 chrome/edge。M11。
    browser: BrowserArg,
}

fn init_tracing(level: &str) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
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
    let result: Result<ExitCode> = match cli.cmd {
        Command::Search(args) => cmd_search(args, proxy.clone()).await,
        Command::Browse { url, full, json, from, headings_only, browser } => {
            let opts = general::BrowseOpts {
                full,
                json,
                from: from.unwrap_or(0),
                headings_only,
                browser: browser.into(),
                proxy: proxy.clone(),
            };
            general::cmd_browse(&url, &opts).await
        }
        Command::Login { url, browser } => general::cmd_login(&url, browser.into(), proxy.clone()).await,
        Command::Dl { url, output, browser } => general::cmd_dl(&url, output.as_deref(), browser.into(), proxy.clone()).await,
        Command::Shell => shell::run_shell().await,
        Command::Doctor => cmd_doctor().await,
        Command::Verify { url, json } => gsearch::verify::cmd_verify(&url, json, proxy.as_deref()),
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
    let started = std::time::Instant::now();
    // M14-1B：早解析浏览器路径 → meta 头部字段（与 launch 实际选用的 kind 一致）。
    let browser_kind = browser_arg_to_kind(args.browser);
    let (browser_path, resolved_kind) = match browser_kind {
        Some(k) => gsearch::browser::find_specific(k)
            .or_else(|| gsearch::browser::find_browser().ok())
            .unwrap_or_else(|| {
                // 兑底：连 find_browser 都失败 → 留空让下面 launch 自己报错。
                (std::path::PathBuf::new(), k)
            }),
        None => gsearch::browser::find_browser().unwrap_or_else(|_| {
            (std::path::PathBuf::new(), gsearch::browser::BrowserKind::Chrome)
        }),
    };
    let (mut browser, handler) = gsearch::browser::launch_with_kind_proxy(true, browser_kind, proxy.clone())
        .await
        .context("启动 Chrome/Edge 失败：检查 GSEARCH_CHROME 是否指向 chrome.exe/msedge.exe，或 profile 被另一实例占用")?;
    let _h = gsearch::browser::spawn_handler(handler);
    let page = browser.new_page("about:blank").await?;
    if args.humanize {
        stealth::install_init_script(&page).await?;
        stealth::warmup(&page).await?;
    }
    let outcome = gsearch::search::run_search_on_page(
        &mut browser,
        gsearch::search::SearchConfig {
            query: args.query.clone(),
            limit: args.limit,
        },
        page,
    )
    .await?;
    // 解构 outcome：M15 透明等待协议按结局分支（不再统一 anyhow 退出）
    let (results, captcha_solved) = match outcome {
        gsearch::search::SearchOutcome::Results { results, captcha_solved } => (results, captcha_solved),
        gsearch::search::SearchOutcome::CaptchaTimeout => {
            // 输出 captcha_timeout JSON（Agent 看到 status 字段就知道等人解超时）
            if args.json {
                emit_captcha_timeout_json(&args, &browser_path, &resolved_kind, proxy.clone(), started.elapsed().as_millis());
            } else {
                eprintln!("error: CAPTCHA 亲解超时（{}s）；profile 已养熟，再次执行会跳过 CAPTCHA",
                    gsearch::search::CAPTCHA_TIMEOUT_SECS);
            }
            if let Err(e) = browser.close().await { tracing::warn!("close browser 失败: {e}"); }
            let _ = browser.wait().await;
            return Ok(ExitCode::from(3)); // 3 = CAPTCHA 超时（区别于 1=错误 / 2=无结果）
        }
    };
    if args.json {
        let meta = gsearch::types::MetaOutput {
            tool: "gsearch",
            version: env!("CARGO_PKG_VERSION"),
            query: args.query.clone(),
            profile: gsearch::browser::profile_name_only(),
            browser_kind: format!("{resolved_kind:?}"),
            browser_path: browser_path.to_string_lossy().into_owned(),
            proxy: proxy.clone(),
            humanize: args.humanize,
            limit: args.limit,
            elapsed_ms: started.elapsed().as_millis(),
            results_count: results.len(),
            truncated: results.len() >= args.limit,
        };
        let run = gsearch::types::RunStatusInfo {
            status: gsearch::types::RunStatus::Ok,
            captcha_solved,
            message: if captcha_solved { "本次搜索经过了人工 CAPTCHA 验证".into() } else { String::new() },
        };
        let envelope = gsearch::types::OutputEnvelope { meta, run, results: &results };
        gsearch::output::print_envelope_json(&envelope)?;
    } else {
        gsearch::output::print_text(&results);
    }
    // M4 后处理（PLAN §3.4）：clap ArgGroup \"post\" 保证 --open/--read/--dl 互斥；仅剩运行时分发。
    let post = async {
        if let Some(n) = args.open {
            postproc::open(&results, n)?;
        }
        if let Some(n) = args.read {
            let opts = postproc::ReadOpts {
                full: args.full,
                json: args.json,
                headings_only: args.headings_only,
                from: args.from.unwrap_or(0),
            };
            if opts.full {
                postproc::read_full(&browser, &results, n).await?;
            } else {
                postproc::read(&browser, &results, n, &opts).await?;
            }
        }
        if let Some(n) = args.dl {
            postproc::dl(&browser, &results, n, args.output.as_deref()).await?;
        }
        anyhow::Ok(())
    }
    .await;

    if let Err(e) = browser.close().await {
        tracing::warn!("close browser 失败: {e}");
    }
    let _ = browser.wait().await;
    if let Err(e) = post {
        eprintln!("postproc 失败: {e}");
    }
    if results.is_empty() {
        eprintln!("未找到结果");
        Ok(ExitCode::from(2))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// M15：CAPTCHA 超时时输出 status=captcha_timeout 的 JSON 信封。
fn emit_captcha_timeout_json(
    args: &SearchArgs,
    browser_path: &std::path::Path,
    resolved_kind: &gsearch::browser::BrowserKind,
    proxy: Option<String>,
    elapsed_ms: u128,
) {
    let meta = gsearch::types::MetaOutput {
        tool: "gsearch",
        version: env!("CARGO_PKG_VERSION"),
        query: args.query.clone(),
        profile: gsearch::browser::profile_name_only(),
        browser_kind: format!("{resolved_kind:?}"),
        browser_path: browser_path.to_string_lossy().into_owned(),
        proxy,
        humanize: args.humanize,
        limit: args.limit,
        elapsed_ms,
        results_count: 0,
        truncated: false,
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

fn browser_arg_to_kind(arg: BrowserArg) -> Option<gsearch::browser::BrowserKind> {
    match arg {
        BrowserArg::Auto => None,
        BrowserArg::Chrome => Some(gsearch::browser::BrowserKind::Chrome),
        BrowserArg::Edge => Some(gsearch::browser::BrowserKind::Edge),
    }
}

/// doctor 总耗时 <3s；不启动 Chrome。每项输出 `[OK] 描述 + 路径 / [WARN] ... / [FAIL] ...`。
/// 全部 OK 退出 0；任意 FAIL 退出 1；仅 WARN 退出 0。
async fn cmd_doctor() -> Result<ExitCode> {
    let started = std::time::Instant::now();
    println!("gsearch doctor");
    let mut fail = 0;
    let mut warn = 0;

    // 1) Chrome 可用
    let chrome_ok = {
        let k = gsearch::browser::BrowserKind::Chrome;
        find_with_kind_display(k)
    };
    match &chrome_ok {
        Some(_) => println!("[ OK ] Chrome: {}", chrome_ok.as_ref().unwrap()),
        None => {
            println!("[FAIL] Chrome 不可用（chrome.exe 未找到）");
            fail += 1;
        }
    }

    // 2) Edge 可用
    match find_with_kind_display(gsearch::browser::BrowserKind::Edge) {
        Some(p) => println!("[ OK ] Edge:   {p}"),
        None => {
            println!("[WARN] Edge 不可用（msedge.exe 未找到；仅 Chrome 可跑）");
            warn += 1;
        }
    }

    // 3) profile 可写
    match gsearch::browser::profile_dir() {
        Ok(dir) => match test_profile_writable(&dir) {
            Ok(()) => println!("[ OK ] profile 可写: {}", dir.display()),
            Err(e) => {
                println!("[FAIL] profile 不可写: {} ({e})", dir.display());
                fail += 1;
            }
        },
        Err(e) => {
            println!("[FAIL] profile 解析失败: {e}");
            fail += 1;
        }
    }

    // 4) 出口 IP（明文 HTTP GET 80 端口，3s 超时；失败降 WARN）
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fetch_public_ip(),
    )
    .await
    {
        Ok(Ok(ip)) => println!("[ OK ] 出口 IP: {ip}"),
        Ok(Err(e)) => {
            println!("[WARN] 出口 IP 不可达（撞码调试辅助；改用代理/VPN 后重试）: {e}");
            warn += 1;
        }
        Err(_) => {
            println!("[WARN] 出口 IP 检测超时（2s）");
            warn += 1;
        }
    }

    // 5) 网络连通（TCP connect google.com:443，2s 超时）
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(("www.google.com", 443)),
    )
    .await
    {
        Ok(Ok(_)) => println!("[ OK ] 网络连通 (www.google.com:443)"),
        Ok(Err(e)) => {
            println!("[FAIL] 网络不可达 (www.google.com:443): {e}");
            fail += 1;
        }
        Err(_) => {
            println!("[FAIL] 网络连接超时 (www.google.com:443)");
            fail += 1;
        }
    }

    // 6) 生效 profile 来源检查（env > 配置文件 > default）
    if let Ok(v) = std::env::var("GSEARCH_PROFILE")
        && !v.trim().is_empty()
    {
        let p = std::path::PathBuf::from(v.trim());
        if p.exists() {
            println!("[ OK ] GSEARCH_PROFILE 已设置且存在: {}", p.display());
        } else {
            println!("[WARN] GSEARCH_PROFILE 已设置但路径不存在: {}（gsearch 会自动创建）", p.display());
            warn += 1;
        }
    } else if let Some(p) = gsearch::config::load().profile.clone() {
        println!("[ OK ] profile 来自配置文件: {p}");
    } else {
        println!("[ OK ] GSEARCH_PROFILE 未设置（默认 ~/.gsearch/profiles/default/）");
    }

    let elapsed_ms = started.elapsed().as_millis();
    if fail > 0 {
        println!("\n[{fail} 项 FAIL] 检查上面建议。（耗时 {elapsed_ms}ms）");
        Ok(ExitCode::from(1))
    } else if warn > 0 {
        println!("\n[{warn} 项 WARN] 整体可用。（耗时 {elapsed_ms}ms）");
        Ok(ExitCode::SUCCESS)
    } else {
        println!("\n所有检查通过 ✓（耗时 {elapsed_ms}ms）");
        Ok(ExitCode::SUCCESS)
    }
}

fn find_with_kind_display(kind: gsearch::browser::BrowserKind) -> Option<String> {
    use gsearch::browser::*;
    let exe_name = match kind {
        BrowserKind::Chrome => "chrome.exe",
        BrowserKind::Edge => "msedge.exe",
    };
    let defaults: &[&str] = match kind {
        BrowserKind::Chrome => &[r"C:\Program Files\Google\Chrome\Application\chrome.exe"],
        BrowserKind::Edge => &[
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        ],
    };
    for d in defaults {
        let p = std::path::PathBuf::from(d);
        if p.is_file() {
            return Some(p.display().to_string());
        }
    }
    if let Ok(o) = std::process::Command::new("where").arg(exe_name).output()
        && o.status.success()
        && let Some(first) = String::from_utf8_lossy(&o.stdout).lines().next()
    {
        let p = std::path::PathBuf::from(first.trim());
        if p.is_file() {
            return Some(p.display().to_string());
        }
    }
    None
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
    use super::{Cli, Command};
    use clap::Parser;

    #[test]
    fn humanize_defaults_to_false() {
        let cli = Cli::try_parse_from(["gsearch", "search", "test"]).unwrap();
        let Command::Search(args) = cli.cmd else { panic!("expected search") };
        assert!(!args.humanize);
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

    /// M14-1A：verify 子命令 + --json flag 正常解析。
    #[test]
    fn verify_subcommand_parses_with_json_flag() {
        let cli = Cli::try_parse_from(["gsearch", "verify", "https://example.com", "--json"]).unwrap();
        let Command::Verify { url, json } = cli.cmd else { panic!("expected verify") };
        assert_eq!(url, "https://example.com");
        assert!(json);
        // 不带 flag 默认 text
        let cli = Cli::try_parse_from(["gsearch", "verify", "https://example.com"]).unwrap();
        let Command::Verify { json, .. } = cli.cmd else { panic!("expected verify") };
        assert!(!json);
    }
}
