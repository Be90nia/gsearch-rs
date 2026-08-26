use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::handler::Handler;
use futures::StreamExt;

/// 标准 Chrome UA（无 HeadlessChrome 字样，遮 navigator.webdriver 配对的两个 bot 信号之一）。
/// 注意：每半年更新一次版本号；UA 过老本身也是反爬信号。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

const DEFAULT_CHROME: &str = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserKind {
    Chrome,
    Edge,
}

const DEFAULT_EDGE: &str = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe";
const DEFAULT_EDGE_64: &str = r"C:\Program Files\Microsoft\Edge\Application\msedge.exe";

/// 浏览器定位顺序（M11）：Chrome env → Chrome 默认安装路径 → Edge 默认安装路径 →
/// `where chrome.exe` / `where msedge.exe`。Edge 是 Chromium 内核，参数与 Chrome 兼容。
/// 返回的 `(path, kind)` 供 `launch()` 决定是否需要任何浏览器特定逻辑。
pub fn find_browser() -> Result<(PathBuf, BrowserKind)> {
    if let Ok(p) = std::env::var("GSEARCH_CHROME") {
        let path = PathBuf::from(p);
        if path.is_file() {
            let kind = if path.to_string_lossy().to_ascii_lowercase().contains("msedge") {
                BrowserKind::Edge
            } else {
                BrowserKind::Chrome
            };
            return Ok((path, kind));
        }
    }

    let default = PathBuf::from(DEFAULT_CHROME);
    if default.is_file() {
        return Ok((default, BrowserKind::Chrome));
    }

    for default in [DEFAULT_EDGE_64, DEFAULT_EDGE] {
        let p = PathBuf::from(default);
        if p.is_file() {
            return Ok((p, BrowserKind::Edge));
        }
    }

    for (exe_name, kind) in [("chrome.exe", BrowserKind::Chrome), ("msedge.exe", BrowserKind::Edge)] {
        let out = std::process::Command::new("where").arg(exe_name).output();
        if let Ok(o) = out
            && o.status.success()
            && let Some(first) = String::from_utf8_lossy(&o.stdout).lines().next()
        {
            let p = PathBuf::from(first.trim());
            if p.is_file() {
                return Ok((p, kind));
            }
        }
    }

    Err(anyhow!(
        "找不到 Chrome 或 Edge；请装 Chrome 到 {DEFAULT_CHROME}，或 Edge 到 {DEFAULT_EDGE}，或设 GSEARCH_CHROME env"
    ))
}

/// 给定 BrowserKind 查找对应路径；找不到返回 None（让 launch() 兑底到 find_browser）。
pub fn find_specific(kind: BrowserKind) -> Option<(PathBuf, BrowserKind)> {
    let exe_name = match kind {
        BrowserKind::Chrome => "chrome.exe",
        BrowserKind::Edge => "msedge.exe",
    };
    let defaults: &[&str] = match kind {
        BrowserKind::Chrome => &[DEFAULT_CHROME],
        BrowserKind::Edge => &[DEFAULT_EDGE_64, DEFAULT_EDGE],
    };
    for d in defaults {
        let p = PathBuf::from(d);
        if p.is_file() {
            return Some((p, kind));
        }
    }
    if let Ok(o) = std::process::Command::new("where").arg(exe_name).output()
        && o.status.success()
        && let Some(first) = String::from_utf8_lossy(&o.stdout).lines().next()
    {
        let p = PathBuf::from(first.trim());
        if p.is_file() {
            return Some((p, kind));
        }
    }
    None
}


/// 仅返回 Chrome 路径的便捷别名（M11 兼容旧调用方）。Chrome 不可用时回落到 Edge，
/// 但 launch(BrowserKind) 推荐显式接收 `(path, kind)`。
pub fn find_chrome() -> Result<PathBuf> {
    let (p, _kind) = find_browser()?;
    Ok(p)
}

/// Profile 目录：env `GSEARCH_PROFILE` 取末段名放进 `~/.gsearch/profiles/`，未设时用 `default`。
pub fn profile_dir() -> Result<PathBuf> {
    let name = match std::env::var("GSEARCH_PROFILE") {
        Ok(raw) if !raw.trim().is_empty() => profile_name(&raw)?,
        _ => "default".to_owned(),
    };
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME/USERPROFILE env 未设置，无法定位默认 profile 目录")?;
    let path = std::path::absolute(PathBuf::from(home).join(".gsearch").join("profiles").join(name))?;
    ensure_dir(&path)?;
    Ok(path)
}
fn profile_name(raw: &str) -> Result<String> {
    let path = Path::new(raw.trim()).to_path_buf();
    let name = path.file_name().and_then(|part| part.to_str()).unwrap_or_default();
    if name.is_empty() || name == ".." || name == "." || name == "/" {
        return Err(anyhow!("GSEARCH_PROFILE 路径末段非法: {raw:?}"));
    }
    Ok(name.to_owned())
}

/// M14-1B：取 meta 头部用的 profile 末段名（不创建目录、纯查询）。
/// ponytail: profile_dir() 会 create_dir_all 在没设 env 时副作用意外；这里只读。
pub fn profile_name_only() -> String {
    match std::env::var("GSEARCH_PROFILE") {
        Ok(raw) if !raw.trim().is_empty() => profile_name(&raw).unwrap_or_else(|_| "default".into()),
        _ => "default".into(),
    }
}

fn ensure_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)
            .with_context(|| format!("创建 profile 目录失败: {}", path.display()))?;
        tracing::info!(
            "新 profile 已创建: {}（首次搜索可能弹 CAPTCHA，解一次后养熟）",
            path.display()
        );
    }
    Ok(())
}
/// 清理上次进程被 kill 留下的 Singleton 锁；缺失 ignore，**占用错误也 ignore**（Python 版同语义）。
/// Chrome 后台 kill 未死透时 lockfile 被活进程持有，强删会 os error 32；只 warn 不 abort。
pub fn cleanup_stale_locks(dir: &Path) -> Result<()> {
    const NAMES: &[&str] = &[
        "SingletonLock",
        "SingletonCookie",
        "SingletonSocket",
        "lockfile",
    ];
    for name in NAMES {
        let p = dir.join(name);
        match std::fs::remove_file(&p) {
            Ok(()) => tracing::debug!("已清理残留锁: {}", p.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            // os error 32 = Windows "另一个程序正在使用此文件"（锁文件被活进程持有）
            // M3 swap_to_headed 二次 launch 撞到该路径；容错为 warn 不 abort
            #[cfg(windows)]
            Err(e) if e.raw_os_error() == Some(32) => {
                tracing::warn!("残留锁被活 Chrome 持有（等 1s 后 retry 也可能失败）: {} ({e})", p.display());
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                tracing::warn!("残留锁仍被占用（上一 Chrome 未死透，跳过）: {} ({e})", p.display());
            }
            Err(e) => {
                return Err(anyhow!("清理残留锁失败 {}: {e}", p.display()));
            }
        }
    }
    Ok(())
}

/// 启动浏览器，默认走自动兑底（Chrome 优先，回落 Edge）；兼容旧调用方。
/// handler 是 Stream<Item = Result<()>>，必须 spawn 到独立 task 持续 poll，否则 CDP 通信会卡死。
pub async fn launch(headless: bool) -> Result<(Browser, Handler)> {
    launch_with_kind(headless, None).await
}

/// close 当前 browser 并同 profile 起重起**有头**实例。
/// CAPTCHA 双模式（M3）核心：cookie 落盘保留（Playwright 做不到热切换，同款方案）。
/// 等价 plsearch AppContext.reveal_for_captcha（main.py:133-137）。
pub async fn swap_to_headed(browser: &mut Browser) -> Result<()> {
    browser.close().await.map_err(|e| anyhow!("close 当前 browser 失败: {e}"))?;
    let (new_browser, handler) = launch(false).await?;
    *browser = new_browser;
    spawn_handler(handler);
    Ok(())
}

/// 启动浏览器并返回 (Browser, Handler)。`kind = None` 自动兑底；
/// `kind = Some(_)` 时若指定浏览器不可用则兑底到第一个可用浏览器（不报错）。
pub async fn launch_with_kind(headless: bool, kind: Option<BrowserKind>) -> Result<(Browser, Handler)> {
    let proxy = std::env::var("GSEARCH_PROXY").ok().filter(|s| !s.is_empty());
    launch_with_kind_proxy(headless, kind, proxy).await
}

/// 启动浏览器并返回 (Browser, Handler)。`kind = None` 自动兑底；`proxy = None` 不走代理。
/// ponytail: 拆出 `proxy` 参数主要为了让上层调用不关心 env 细节（CLI 也走同一路径）。
pub async fn launch_with_kind_proxy(
    headless: bool,
    kind: Option<BrowserKind>,
    proxy: Option<String>,
) -> Result<(Browser, Handler)> {
    let (browser_exe, browser_kind) = match kind {
        Some(requested) => match find_specific(requested) {
            Some(found) => found,
            None => find_browser()?,
        },
        None => find_browser()?,
    };
    tracing::info!("使用浏览器: {browser_kind:?} -> {}", browser_exe.display());
    let profile = profile_dir()?;
    cleanup_stale_locks(&profile)?;
    // disable_default_args：chromiumoxide 默认参数与持久 profile 组合在 Windows 上
    // 触发 Chrome ExitStatus(21)（实测复现）；关掉后只加显式安全子集。
    let mut builder = BrowserConfig::builder()
        .chrome_executable(&browser_exe)
        .user_data_dir(&profile)
        .arg("--disable-blink-features=AutomationControlled")
        .arg(format!("--user-agent={UA}"))
        .disable_default_args();
    if let Some(proxy) = &proxy {
        // ponytail: Chrome 只识别 --proxy-server=protocol://host:port；不引 chromiumoxide proxy builder（M12 调试期足以）。
        tracing::info!("代理: {proxy}");
        builder = builder.arg(format!("--proxy-server={proxy}"));
    }
    let safe_args: &[&str] = if headless {
        ["--headless=new", "--disable-gpu", "--no-sandbox", "--disable-dev-shm-usage"].as_slice()
    } else {
        ["--no-sandbox", "--disable-dev-shm-usage"].as_slice()
    };
    builder = builder.args(safe_args.iter().copied());
    // HeadlessMode 枚举在 chromiumoxide 0.9 未对外 re-export；用 builder 自身的便捷方法切模式
    builder = if headless {
        builder.new_headless_mode()
    } else {
        builder.with_head()
    };

    let config = builder
        .build()
        .map_err(|e| anyhow!("构造 BrowserConfig 失败: {e}"))?;

    let (browser, handler) = launch_with_retry(config).await?;
    Ok((browser, handler))
}


/// 瞬态失败重试一次（≤1s 退避）：上一实例 Chrome 后台 kill 未死透的 profile 锁竞态、
/// Windows Defender 扫新生 exe 的文件锁（OS error 5）等；二次失败才上抛。
async fn launch_with_retry(config: BrowserConfig) -> Result<(Browser, Handler)> {
    // ponytail: 只重试 1 次盖住已知瞬态族；系统性失败（路径错/profile 活占用）二次也失败
    let mut attempt = Browser::launch(config.clone()).await;
    if attempt.is_err() {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        attempt = Browser::launch(config).await;
    }
    attempt.map_err(|e| anyhow!("启动 Chrome 失败，profile 可能被另一实例占用: {e}"))
}

/// 在独立 task 里持续 poll handler（chromiumoxide 要求，否则 CDP 通道会卡住）
pub fn spawn_handler(handler: Handler) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut h = handler;
        while let Some(event) = h.next().await {
            if event.is_err() {
                tracing::warn!("CDP handler 出错: {:?}", event);
                break;
            }
        }
    })
}

/// 关 Chrome 并等进程死透。chromiumoxide 0.9 的 close 只 background kill，
/// 不调 wait() 直接 Drop 会报 "was not closed manually" WARN，且 profile 锁残留。
/// 顶层命令（search/browse/login/dl）+ shell 退出统一走这条，避免下一次 launch 撞 profile 锁。
pub async fn graceful_close(browser: &mut Browser) {
    if let Err(e) = browser.close().await {
        tracing::warn!("close browser 失败: {e}");
    }
    let _ = browser.wait().await;
}

#[cfg(test)]
mod tests {
    use super::profile_name;

    #[test]
    fn profile_name_uses_last_path_component() {
        assert_eq!(profile_name("work").unwrap(), "work");
        assert_eq!(profile_name("D:/foo/bar/").unwrap(), "bar");
        assert!(profile_name("..").is_err());
        assert!(profile_name("/").is_err());
    }
}
