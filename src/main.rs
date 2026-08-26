//! gsearch-rs 入口：clap 子命令派发 + Windows 控制台 UTF-8 + tracing 初始化

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod general;
mod postproc;
mod shell;
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
}
#[derive(Args, Debug)]
struct SearchArgs {
    query: String,
    #[arg(long, default_value_t = 10)]
    limit: usize,
    #[arg(long)]
    read: Option<usize>,
    #[arg(long)]
    dl: Option<usize>,
    #[arg(long)]
    open: Option<usize>,
    #[arg(long, default_value_t = false)]
    /// 启用 fingerprint 补丁 + Google 搜索前 warmup（opt-in；新 profile/裸搜可能撞码时建议 off）。
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
    #[arg(long, value_enum, default_value_t = BrowserArg::Auto)]
    /// 选择浏览器：auto = Chrome 优先缺则 Edge，强制选 chrome/edge。M11。
    browser: BrowserArg,
 }

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    enable_utf8_console();
    init_tracing();

    let cli = Cli::parse();
    let result: Result<ExitCode> = match cli.cmd {
        Command::Search(args) => cmd_search(args).await,
        Command::Browse { url, full, json, from, headings_only, browser } => {
            let opts = general::BrowseOpts {
                full,
                json,
                from: from.unwrap_or(0),
                headings_only,
                browser: browser.into(),
            };
            general::cmd_browse(&url, &opts).await
        }
        Command::Login { url, browser } => general::cmd_login(&url, browser.into()).await,
        Command::Dl { url, output, browser } => general::cmd_dl(&url, output.as_deref(), browser.into()).await,
        Command::Shell => shell::run_shell().await,
        Command::Doctor => cmd_doctor().await,
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            for cause in e.chain().skip(1) {
                eprintln!("  原因: {cause}");
            }
            eprintln!("（设 RUST_LOG=debug 查详细）");
            ExitCode::from(1)
        }
    }
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
}
async fn cmd_search(args: SearchArgs) -> Result<ExitCode> {
    let browser_kind = browser_arg_to_kind(args.browser);
    let (mut browser, handler) = gsearch::browser::launch_with_kind(true, browser_kind)
        .await
        .context("启动 Chrome/Edge 失败：检查 GSEARCH_CHROME 是否指向 chrome.exe/msedge.exe，或 profile 被另一实例占用")?;
    let _h = gsearch::browser::spawn_handler(handler);
    let page = browser.new_page("about:blank").await?;
    if args.humanize {
        stealth::install_init_script(&page).await?;
        stealth::warmup(&page).await?;
    }
    let results = gsearch::search::run_search_on_page(
        &mut browser,
        gsearch::search::SearchConfig {
            query: args.query,
            limit: args.limit,
        },
        page,
    )
    .await?;
    if args.json {
        gsearch::output::print_json(&results)?;
    } else {
        gsearch::output::print_text(&results);
    }

    // M4 后处理（PLAN §3.4）：--open/--read/--dl 独立判断、可并存，复用同一 browser（同 profile cookie）
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
            postproc::dl(&browser, &results, n).await?;
        }
        anyhow::Ok(())
    }
    .await;

    if let Err(e) = browser.close().await {
        tracing::warn!("close browser 失败: {e}");
    }
    let _ = browser.wait().await;
    if let Err(e) = post {
        eprintln!("error: {e}");
        return Ok(ExitCode::from(2));
    }

    if results.is_empty() {
        eprintln!("未找到结果");
        Ok(ExitCode::from(2))
    } else {
        Ok(ExitCode::SUCCESS)
    }
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
    println!("gsearch doctor");
    let mut fail = 0;
    let mut warn = 0;

    // 1) Chrome 可用
    let chrome_ok = match gsearch::browser::BrowserKind::Chrome {
        k => find_with_kind_display(k),
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

    // 6) GSEARCH_PROFILE 路径检查
    match std::env::var("GSEARCH_PROFILE") {
        Ok(v) if !v.trim().is_empty() => {
            let p = std::path::PathBuf::from(v.trim());
            let exists = p.exists();
            if exists {
                println!("[ OK ] GSEARCH_PROFILE 已设置且存在: {}", p.display());
            } else {
                println!("[WARN] GSEARCH_PROFILE 已设置但路径不存在: {}（gsearch 会自动创建）", p.display());
                warn += 1;
            }
        }
        _ => println!("[ OK ] GSEARCH_PROFILE 未设置（默认 ~/.gsearch/profiles/default/）"),
    }

    if fail > 0 {
        println!("\n[{fail} 项 FAIL] 检查上面建议。");
        Ok(ExitCode::from(1))
    } else if warn > 0 {
        println!("\n[{warn} 项 WARN] 整体可用。");
        Ok(ExitCode::SUCCESS)
    } else {
        println!("\n所有检查通过 ✓");
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
