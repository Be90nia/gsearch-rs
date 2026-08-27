use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::handler::Handler;
use futures::StreamExt;

/// 标准 Chrome UA（无 HeadlessChrome 字样，遮 navigator.webdriver 配对的两个 bot 信号之一）。
/// 注意：每半年更新一次版本号；UA 过老本身也是反爬信号。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

/// 指纹补丁脚本（M14-2A 自 bin stealth.rs 搬来作单一事实源，stealth.rs 留别名）：
/// webdriver / userAgent / languages / hardwareConcurrency / deviceMemory /
/// maxTouchPoints / plugins / mimeTypes / chrome.runtime / WebGL vendor /
/// domAutomationController 等 10 项。launch 层（chaser-stealth）与 --humanize 共用。
pub const STEALTH_INIT_SCRIPT: &str = r#"
(() => {
  const define = (target, key, value) => {
    try {
      Object.defineProperty(target, key, { configurable: true, get: () => value });
    } catch (_) {}
  };
  const defineGetter = (target, key, getter) => {
    try {
      Object.defineProperty(target, key, { configurable: true, get: getter });
    } catch (_) {}
  };

  define(navigator, 'webdriver', false);
  defineGetter(navigator, 'userAgent', () => 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36');
  define(navigator, 'languages', ['en-US', 'en']);
  define(navigator, 'hardwareConcurrency', 8);
  define(navigator, 'deviceMemory', 8);
  define(navigator, 'maxTouchPoints', 0);

  const pdf = {
    0: { name: 'PDF Viewer', filename: 'internal-pdf-viewer',
         description: 'Portable Document Format', length: 1 },
    length: 1,
    item: function (index) { return index === 0 ? this[0] : null; },
    namedItem: function (name) { return name === 'PDF Viewer' ? this[0] : null; }
  };
  defineGetter(Navigator.prototype, 'plugins', () => pdf);
  defineGetter(Navigator.prototype, 'mimeTypes', () => ({ length: 0 }));

  if (!window.chrome) window.chrome = {};
  defineGetter(window.chrome, 'runtime', () => ({
    connect: function () { return { onMessage: { addListener: function () {} } }; },
    sendMessage: function () { return Promise.resolve(); }
  }));

  const vendor = 'Intel Inc.';
  const renderer = 'Intel Iris OpenGL Engine';
  for (const prototype of [WebGLRenderingContext.prototype,
                           WebGL2RenderingContext.prototype]) {
    if (!prototype) continue;
    const original = prototype.getParameter;
    prototype.getParameter = function (parameter) {
      if (parameter === 37445) return vendor;
      if (parameter === 37446) return renderer;
      return original.call(this, parameter);
    };
  }
  // WebGL debug renderer info constants: UNMASKED_VENDOR_WEBGL=37445,
  // UNMASKED_RENDERER_WEBGL=37446.

  for (const key of Object.getOwnPropertyNames(window)) {
    if (key.toLowerCase().indexOf('cdc_') === 0 ||
        key === 'domAutomationController' ||
        key.toLowerCase().indexOf('__webdriver_') === 0) {
      define(window, key, undefined);
    }
  }
  for (const key of Object.getOwnPropertyNames(navigator)) {
    if (key === 'webdriver' || key.toLowerCase().indexOf('cdc_') === 0 ||
        key === 'domAutomationController' || key.toLowerCase().indexOf('__webdriver_') === 0) {
      define(navigator, key, undefined);
    }
  }
  define(window, 'domAutomationController', undefined);
  define(window, 'cdc_', undefined);

  // Chromium's broken-image placeholder is 16x16 by default; expose 0x0.
  for (const name of ['width', 'height']) {
    Object.defineProperty(HTMLImageElement.prototype, name, {
      configurable: true,
      get: function () { return this.naturalWidth === 0 ? 0 : this.naturalWidth; },
      set: function () {}
    });
  }
})();
"#;

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
    let env_or_cfg = std::env::var("GSEARCH_CHROME")
        .ok()
        .or_else(|| crate::config::load().chrome.clone());
    if let Some(p) = env_or_cfg {
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
        "找不到 Chrome 或 Edge；请装 Chrome 到 {DEFAULT_CHROME}，或 Edge 到 {DEFAULT_EDGE}，或设 GSEARCH_CHROME env / 配置文件 chrome 键"
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

/// Profile 目录：env `GSEARCH_PROFILE` > 配置文件 profile 键 > default，
/// 取末段名放进 `~/.gsearch/profiles/`。
pub fn profile_dir() -> Result<PathBuf> {
    let name = match effective_profile_raw() {
        Some(raw) => profile_name(&raw)?,
        None => "default".to_owned(),
    };
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME/USERPROFILE env 未设置，无法定位默认 profile 目录")?;
    let path = std::path::absolute(PathBuf::from(home).join(".gsearch").join("profiles").join(name))?;
    ensure_dir(&path)?;
    Ok(path)
}

/// 生效的 profile 原始值（env 优先，其次配置文件；两者都空返回 None）。
fn effective_profile_raw() -> Option<String> {
    if let Ok(raw) = std::env::var("GSEARCH_PROFILE")
        && !raw.trim().is_empty()
    {
        return Some(raw);
    }
    crate::config::load().profile.clone()
}
/// M14-1B：取 meta 头部用的 profile 末段名（不创建目录、纯查询）。
/// ponytail: profile_dir() 会 create_dir_all 在没设 env 时副作用意外；这里只读。
pub fn profile_name_only() -> String {
    match effective_profile_raw() {
        Some(raw) => profile_name(&raw).unwrap_or_else(|_| "default".into()),
        None => "default".into(),
    }
}

fn profile_name(raw: &str) -> Result<String> {
    let path = Path::new(raw.trim()).to_path_buf();
    let name = path.file_name().and_then(|part| part.to_str()).unwrap_or_default();
    if name.is_empty() || name == ".." || name == "." || name == "/" {
        return Err(anyhow!("profile 路径末段非法: {raw:?}"));
    }
    if is_windows_reserved(name) {
        return Err(anyhow!("profile 是 Windows 保留设备名: {raw:?}"));
    }
    Ok(name.to_owned())
}

/// Windows 保留设备名（CON/NUL/PRN/AUX/COM1-9/LPT1-9，含 `CON.txt` 带扩展形态）：
/// 作目录名在 Windows 上非法/行为未定义；profile 会 zip 换机携带，全平台统一拒绝。
fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    match stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT")) {
        Some(d) => d.len() == 1 && (b'1'..=b'9').contains(&d.as_bytes()[0]),
        None => matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL"),
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


/// 代理串凭据脱敏：`scheme://user:pass@host` 的 userinfo 段（含只有 user 的形态）
/// 整段替换为 `***`，无凭据原样返回。打日志前必走，防凭据进 CI/用户贴出的日志。
fn redact_proxy(proxy: &str) -> String {
    let Some(scheme_end) = proxy.find("://") else {
        return proxy.to_owned();
    };
    let auth_start = scheme_end + 3;
    // authority 段止于首个 '/'；密码里未编码的 '@' 按 URL 惯例取最后一个
    let auth_end = proxy[auth_start..]
        .find('/')
        .map_or(proxy.len(), |i| auth_start + i);
    match proxy[auth_start..auth_end].rfind('@') {
        Some(at) => format!("{}***{}", &proxy[..auth_start], &proxy[auth_start + at..]),
        None => proxy.to_owned(),
    }
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
        tracing::info!("代理: {}", redact_proxy(proxy));
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

    // M14-2A cfg 切换点：默认（feature off）走 chromiumoxide 0.9 原路径，零行为变化；
    // chaser-stealth on 时改走 launch 层 transport stealth（见 launch_with_stealth_transport）。
    #[cfg(feature = "chaser-stealth")]
    let (browser, handler) = launch_with_stealth_transport(config).await?;
    #[cfg(not(feature = "chaser-stealth"))]
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

/// chaser-stealth transport 补丁序列（launch 层逐 target 按序应用，顺序即检测面）。
/// Page.enable 先声明 page 会话，再挂指纹脚本（addScriptToEvaluateOnNewDocument），
/// 保证任何站点文档创建前补丁已就位。Page.enable→Runtime.enable 的顺序
/// chromiumoxide 0.9.1 内部 frame init_commands 已保证（vendored handler/frame.rs）；
/// SetAutoAttachParams 也未开 exposeNodeAccessorInWorker（vendored handler/target.rs，
/// CDP 默认 false），无需额外拦截——补这两条反而会破坏内部 attach 流程。
#[cfg(feature = "chaser-stealth")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StealthPatchStep {
    /// Page.enable —— 声明 page 会话
    PageEnable,
    /// Page.addScriptToEvaluateOnNewDocument —— 注入 STEALTH_INIT_SCRIPT 指纹补丁
    InitScript,
}

#[cfg(feature = "chaser-stealth")]
const STEALTH_PATCH_SEQUENCE: &[StealthPatchStep] =
    &[StealthPatchStep::PageEnable, StealthPatchStep::InitScript];

/// chaser-stealth on：launch 成功后对每个已存在 target 按 STEALTH_PATCH_SEQUENCE
/// 应用 transport 补丁；补丁失败只 warn 不 abort（stealth 是加固，不是正确性路径）。
/// 关键约束：此时 handler 还在调用方手里未 spawn，直接 await CDP 命令会因无人
/// poll 响应而永久挂起（实测 postproc_live 卡 60s+）。补丁期间用 select 手动驱动
/// handler 转发响应，完成后原样交还，调用方照常 spawn_handler。
/// ponytail: 调用方后续 new_page 的新 target 不在本层（--humanize 路径已注入）；
/// 全量 CDP 命令拦截是 chaser-oxide fork 本体价值，接入 fork 时替换本函数实现即可。
#[cfg(feature = "chaser-stealth")]
async fn launch_with_stealth_transport(
    config: BrowserConfig,
) -> Result<(Browser, Handler)> {
    let (browser, mut handler) = launch_with_retry(config).await?;
    // 缩窄 patches 作用域：循环结束 + drop 后再移动 browser。
    // ponytail: Box::pin 让 borrow 在块尾随 patches drop 一起结束，
    // 否则编译器看见 borrowing coroutine 跨 move（E0505）。
    {
        let mut patches = Box::pin(async {
            match browser.pages().await {
                Ok(pages) => {
                    for page in &pages {
                        apply_stealth_patches(page).await;
                    }
                    tracing::debug!("chaser-stealth: 已对 {} 个 launch 层 target 注入补丁", pages.len());
                }
                Err(e) => tracing::warn!("chaser-stealth: 枚举初始 target 失败，跳过 launch 层注入: {e}"),
            }
        });
        loop {
            tokio::select! {
                maybe = handler.next() => {
                    if maybe.is_none() {
                        (&mut patches).await;
                        break;
                    }
                }
                _ = &mut patches => break,
            }
        }
    }
    Ok((browser, handler))
}

/// 按 STEALTH_PATCH_SEQUENCE 顺序对单个 page 执行补丁；单项失败 warn 后继续。
#[cfg(feature = "chaser-stealth")]
async fn apply_stealth_patches(page: &chromiumoxide::Page) {
    use chromiumoxide::cdp::browser_protocol::page::EnableParams;
    for step in STEALTH_PATCH_SEQUENCE {
        let result = match step {
            StealthPatchStep::PageEnable => page
                .execute(EnableParams::default())
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
            StealthPatchStep::InitScript => page
                .add_init_script(STEALTH_INIT_SCRIPT)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
        };
        if let Err(e) = result {
            tracing::warn!("chaser-stealth: {step:?} 应用失败: {e}");
        }
    }
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
    use super::{profile_name, redact_proxy};

    #[test]
    fn profile_name_uses_last_path_component() {
        assert_eq!(profile_name("work").unwrap(), "work");
        assert_eq!(profile_name("D:/foo/bar/").unwrap(), "bar");
        assert!(profile_name("..").is_err());
        assert!(profile_name("/").is_err());
    }
    /// Windows 保留设备名作目录名非法（profile 会 zip 换机携带，全平台统一拒）。
    #[test]
    fn profile_name_rejects_windows_reserved_names() {
        for bad in ["CON", "con", "NUL", "nul", "PRN", "Aux", "COM1", "com9", "LPT1", "lpt9"] {
            assert!(profile_name(bad).is_err(), "应拒绝 Windows 保留名: {bad}");
        }
        assert!(profile_name("C:/x/CON.txt").is_err(), "带扩展名的保留名形态也应拒绝");
    }

    #[test]
    fn redact_proxy_masks_userinfo() {
        // 带凭据 / 只有 user：userinfo 段整段替换为 ***
        assert_eq!(redact_proxy("http://user:pass@proxy.example.com:8080"), "http://***@proxy.example.com:8080");
        assert_eq!(redact_proxy("socks5://alice@10.0.0.1:1080"), "socks5://***@10.0.0.1:1080");
        assert_eq!(redact_proxy("http://u:p@h:1/api"), "http://***@h:1/api");
    }

    #[test]
    fn redact_proxy_keeps_credential_free_input() {
        assert_eq!(redact_proxy("http://127.0.0.1:7890"), "http://127.0.0.1:7890");
    }
}

/// chaser-stealth feature on 时的补丁注入契约（M14-2A）。
#[cfg(all(test, feature = "chaser-stealth"))]
mod chaser_stealth_tests {
    use super::{STEALTH_INIT_SCRIPT, STEALTH_PATCH_SEQUENCE, StealthPatchStep};

    #[test]
    fn sequence_puts_page_enable_before_init_script() {
        let enable = STEALTH_PATCH_SEQUENCE
            .iter()
            .position(|s| *s == StealthPatchStep::PageEnable)
            .expect("序列必须含 PageEnable");
        let script = STEALTH_PATCH_SEQUENCE
            .iter()
            .position(|s| *s == StealthPatchStep::InitScript)
            .expect("序列必须含 InitScript");
        assert!(enable < script, "Page.enable 必须先于 addScriptToEvaluateOnNewDocument");
    }

    #[test]
    fn sequence_pins_transport_footprint() {
        // 补丁序列确定性：恰为这两条命令，不随实现漂移增加检测面
        assert_eq!(
            STEALTH_PATCH_SEQUENCE,
            &[StealthPatchStep::PageEnable, StealthPatchStep::InitScript]
        );
    }

    #[test]
    fn init_script_covers_fingerprint_surfaces() {
        // launch 层注入的脚本与 --humanize 路径同源，覆盖任务点名的指纹面
        for marker in [
            "navigator, 'webdriver'",
            "Navigator.prototype, 'plugins'",
            "navigator, 'languages'",
        ] {
            assert!(STEALTH_INIT_SCRIPT.contains(marker), "missing: {marker}");
        }
    }
}
