//! gsearch-rs 入口：clap 子命令派发 + Windows 控制台 UTF-8 + tracing 初始化

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod general;
mod postproc;
mod shell;

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
    },
    /// 有头窗人工登录，cookie 落 profile
    Login {
        url: String,
    },
    /// 带 profile 登录态下载（M6）
    Dl {
        url: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// 交互式 shell：起一次 Chrome 会话复用（M7 追加里程碑）
    Shell,
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
        Command::Browse { url, full, json, from, headings_only } => {
            let opts = general::BrowseOpts {
                full,
                json,
                from: from.unwrap_or(0),
                headings_only,
            };
            general::cmd_browse(&url, &opts).await
        }
        Command::Login { url } => general::cmd_login(&url).await,
        Command::Dl { url, output } => general::cmd_dl(&url, output.as_deref()).await,
        Command::Shell => shell::run_shell().await,
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

async fn cmd_search(args: SearchArgs) -> Result<ExitCode> {
    let (mut browser, handler) = gsearch::browser::launch(true)
        .await
        .context("启动 Chrome 失败：检查 GSEARCH_CHROME 是否指向 chrome.exe，或 profile 被另一实例占用")?;
    let _h = gsearch::browser::spawn_handler(handler);

    let results = gsearch::search::run_search(
        &mut browser,
        gsearch::search::SearchConfig {
            query: args.query,
            limit: args.limit,
        },
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