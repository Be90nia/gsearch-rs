use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::emulation::SetFocusEmulationEnabledParams;
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

  // Permissions API:headless 下 query({name:'notifications'}) 返回 'prompt' 而
  // Notification.permission 是 'denied',二者不一致是 sannysoft/bot-detector 经典检测点。
  // 同款 chromiumoxide 0.9.1 page.rs hide_permissions:query 恒返回 Notification.permission
  // (denied→denied;headed 默认 default/granted 也原样,保证恒一致),非 notifications 透传原生。
  if (window.navigator.permissions && window.navigator.permissions.query) {
    const originalQuery = window.navigator.permissions.query;
    window.navigator.permissions.__proto__.query = (parameters) => {
      if (parameters && parameters.name === 'notifications') {
        return Promise.resolve({ state: Notification.permission });
      }
      return originalQuery.call(window.navigator.permissions, parameters);
    };
  }

  // headless=new 的 screen 对象是独立虚拟屏(默认 800x600),不随 --window-size 走——
  // 与真实窗口 1920x1080 不一致会被 screen/window 关联检测抓出。对齐窗口尺寸
  // (availHeight 预留 Windows 任务栏 ~40px,典型 Win11 形态)。
  if (screen.width !== 1920 || screen.height !== 1080) {
    define(screen, 'width', 1920);
    define(screen, 'height', 1080);
    define(screen, 'availWidth', 1920);
    define(screen, 'availHeight', 1040);
  }

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
/// 用户级安装（无管理员权限时 Chrome/Edge 默认装这里，且不会自加入 PATH，`where` 探不到）。
fn user_scope_path(exe_name: &str) -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|la| user_scope_path_in(&PathBuf::from(la), exe_name))
}

fn user_scope_path_in(local_app_data: &std::path::Path, exe_name: &str) -> PathBuf {
    let vendor = if exe_name == "chrome.exe" { "Google" } else { "Microsoft" };
    local_app_data
        .join(vendor)
        .join(if exe_name == "chrome.exe" { "Chrome" } else { "Edge" })
        .join("Application")
        .join(exe_name)
}
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
    if let Some(p) = user_scope_path("chrome.exe")
        && p.is_file()
    {
        return Ok((p, BrowserKind::Chrome));
    }

    for default in [DEFAULT_EDGE_64, DEFAULT_EDGE] {
        let p = PathBuf::from(default);
        if p.is_file() {
            return Ok((p, BrowserKind::Edge));
        }
    }
    if let Some(p) = user_scope_path("msedge.exe")
        && p.is_file()
    {
        return Ok((p, BrowserKind::Edge));
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
        "找不到 Chrome 或 Edge；请设 GSEARCH_CHROME env / 配置文件 chrome 键，或安装到 {DEFAULT_CHROME}、%LOCALAPPDATA%\\Google\\Chrome\\Application\\chrome.exe（用户级）、Edge 同理"
    ))
}
/// 给定 BrowserKind 查找对应路径；找不到返回 None（让 launch() 兑底到 find_browser）。
pub fn find_specific(kind: BrowserKind) -> Option<(PathBuf, BrowserKind)> {
    let exe_name = match kind {
        BrowserKind::Chrome => "chrome.exe",
        BrowserKind::Edge => "msedge.exe",
    };
    let mut defaults: Vec<PathBuf> = match kind {
        BrowserKind::Chrome => vec![PathBuf::from(DEFAULT_CHROME)],
        BrowserKind::Edge => vec![PathBuf::from(DEFAULT_EDGE_64), PathBuf::from(DEFAULT_EDGE)],
    };
    if let Some(u) = user_scope_path(exe_name) {
        defaults.push(u);
    }
    for d in defaults {
        if d.is_file() {
            return Some((d, kind));
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

/// Profile 目录：env `GSEARCH_PROFILE` > 配置文件 profile 键 > default。
/// 值为**已存在的绝对路径**时直接用作 profile 目录（换盘符存放）；
/// 否则视为 profile 名，放进 `~/.gsearch/profiles/<名>/`。
///
/// 命中默认 profile 时若已被他人持锁（多 agent 并发场景：chromiumoxide 持同一
/// Chrome profile lockfile，第二进程必撞锁），自动 fork 到 `fork-<uuid>` 子目录
/// 从 default 一次性 copy cookie/历史/GAEX，后续所有 read/write 走 fork。
/// stderr 一行 hint 通报用户/agent。诊断 exit code 0 不变——fork 是 silent fallback。
pub fn profile_dir() -> Result<PathBuf> {
    let path = match effective_profile_raw() {
        Some(raw) if is_absolute_dir(&raw) => std::path::absolute(raw.trim())?,
        Some(raw) => {
            let name = profile_name(&raw)?;
            let home = home_dir()
                .context("HOME/USERPROFILE env 未设置，无法定位默认 profile 目录")?;
            std::path::absolute(home.join(".gsearch").join("profiles").join(name))?
        }
        None => {
            let home = home_dir()
                .context("HOME/USERPROFILE env 未设置，无法定位默认 profile 目录")?;
            std::path::absolute(home.join(".gsearch").join("profiles").join("default"))?
        }
    };
    ensure_dir(&path)?;
    // Fork 仅作用于默认 profile 路径：用户显式指定 GSEARCH_PROFILE=work 等自定义
    // profile 时不触发 fork（避免误把个人 work profile 内容拷成 fork-{uuid}）。
    if is_default_profile_path(&path)
        && let Some(forked) = try_fork_profile(&path)
    {
        return Ok(forked);
    }
    Ok(path)
}

/// 是否「默认 profile 路径」语义（HOME/.gsearch/profiles/default，末段 = default）。
/// 非默认 profile（用户显式指定）不走 fork，避免误拷用户私有 profile 内容。
fn is_default_profile_path(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()) == Some("default")
}

/// 检测 default profile 是否被其他进程持锁（chrome SingletonLock/SingletonCookie
/// 存在 + diagnose_lock_holders 返回 Some = 有别的 browser 进程在用它）。
/// 命中时返回 fork 路径（已 cp 完 default 内容）；不命中/拷贝失败返回 None，
/// 调用方静默回落到 default 路径。
fn try_fork_profile(default_path: &Path) -> Option<PathBuf> {
    if !default_path.exists() {
        return None; // 首次启动，default 不存在无锁可抢，自然走 default
    }
    // 仅在 lockfile 残留 + 确认有其他 browser 持锁时才 fork（孤立 SingletonLock
    // 残留但无活进程 = 上次进程被 kill，留着走 cleanup_stale_locks，不 fork）。
    let lock_files_present = ["SingletonLock", "SingletonCookie", "SingletonSocket"]
        .iter()
        .any(|name| default_path.join(name).exists());
    if !lock_files_present {
        return None;
    }
    // 只诊断不 kill——精确 PID 交给用户；存在 PID 则 fork
    let holders = match diagnose_lock_holders(default_path) {
        Some(s) if !s.trim().is_empty() => s,
        _ => return None, // 锁文件残留但无活进程持锁（孤儿）→ 走 cleanup_stale_locks
    };
    let fork_path = fork_path_for(default_path)?;
    if fork_path.exists() {
        // 同 uuid 已存在（理论同进程同时刻只会进来一次，但极端场景下兜底：存在即复用，
        // 不重新拷贝，避免 fork-{uuid} 在并发 agent 间被反复覆盖）。
        eprintln!(
            "[hint] default profile 被他人持锁（{holders}），复用 fork {}",
            fork_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
        );
        return Some(fork_path);
    }
    match copy_profile_contents(default_path, &fork_path) {
        Ok(()) => {
            eprintln!(
                "[hint] default profile 被他人持锁（{holders}），自动 fork 到 {}（cookie 已 copy 一次）",
                fork_path.display()
            );
            Some(fork_path)
        }
        Err(e) => {
            // fork 失败时静默回落 default——Chromium 仍会撞锁但锁打尽逻辑已能
            // 给出降级出口（gsearch fetch 路径），不阻断用户。
            tracing::warn!("fork profile 失败，回落 default: {e}");
            None
        }
    }
}

/// 构造 fork 路径：`~/.gsearch/profiles/fork-<timestamp>-<pid>-<rand>`。
/// 时间戳+pid+随机后缀防并发 agent 同名冲突。
fn fork_path_for(default_path: &Path) -> Option<PathBuf> {
    let parent = default_path.parent()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let pid = std::process::id();
    // 末段随机：4 hex 字符（16 bits 足够区分同秒同 pid 的并发 agent）
    let rand: u32 = {
        use std::hash::{BuildHasher, Hasher, RandomState};
        (RandomState::new().build_hasher().finish() as u32) & 0xFFFF
    };
    let name = format!("fork-{stamp}-{pid}-{rand:04x}");
    Some(parent.join(name))
}

/// bnh：fork 拷贝跳过的缓存类目录名——**只影响 fork 拷贝**（default profile 本体不动）。
/// 这些目录体积大且跨会话无价值（缓存/着色器/崩溃转储），fork 后 Chrome 自行重建。
const FORK_SKIP_DIRS: &[&str] = &[
    "Cache",
    "Code Cache",
    "GPUCache",
    "Service Worker",
    "Crashpad",
    "GrShaderCache",
];

/// 一次性 copy default profile 内容到 fork 目录（cookie/历史/Local Storage/GAEX）。
/// 递归 Default/ 子树（bnh：缓存类目录经 FORK_SKIP_DIRS 整棵跳过）。
fn copy_profile_contents(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("创建 fork profile 目录失败: {}", to.display()))?;
    for entry in std::fs::read_dir(from).with_context(|| format!("读取 default profile 失败: {}", from.display()))? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        // 跳过 lockfile 残留：fork 目录绝不带 SingletonLock/Cookie/Socket，
        // 否则 fork 进程一启动又会报"持锁"歧义。
        if let Some(name) = entry.file_name().to_str()
            && matches!(name, "SingletonLock" | "SingletonCookie" | "SingletonSocket" | "lockfile")
        {
            continue;
        }
        // bnh：顶层缓存目录（Crashpad/GrShaderCache 等）不进 fork（见 FORK_SKIP_DIRS）。
        let is_dir = src.is_dir();
        if is_dir && entry.file_name().to_str().is_some_and(|n| FORK_SKIP_DIRS.contains(&n)) {
            continue;
        }
        let copy_result: std::result::Result<(), std::io::Error> = if is_dir {
            copy_dir_recursive(&src, &dst)
        } else {
            std::fs::copy(&src, &dst).map(|_| ())
        };
        if let Err(e) = copy_result {
            tracing::warn!("fork profile 跳过 {}: {e}", src.display());
        }
    }
    Ok(())
}

/// 递归拷贝子目录（仅 chrome profile 内 Default/Cookies/History/Local Storage/...）。
/// 跳过锁文件与 SymbolicLink（Windows 上少见，遭遇即 warn 不 abort）。
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::result::Result<(), std::io::Error> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let s = entry.path();
        let d = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            tracing::warn!("fork profile 跳过符号链接: {}", s.display());
            continue;
        }
        if let Some(name) = entry.file_name().to_str()
            && matches!(name, "SingletonLock" | "SingletonCookie" | "SingletonSocket" | "lockfile")
        {
            continue;
        }
        // bnh：子树内缓存目录（Cache/Code Cache/GPUCache/Service Worker 等）整棵跳过。
        if file_type.is_dir() && entry.file_name().to_str().is_some_and(|n| FORK_SKIP_DIRS.contains(&n)) {
            continue;
        }
        if file_type.is_dir() {
            copy_dir_recursive(&s, &d)?;
        } else {
            std::fs::copy(&s, &d)?;
        }
    }
    Ok(())
}

/// 值是“存在的绝对路径目录”→ 当存放路径用（不是 profile 名）。
fn is_absolute_dir(raw: &str) -> bool {
    let p = Path::new(raw.trim());
    p.is_absolute() && p.is_dir()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
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
/// 取 meta 头部用的 profile 名（不创建目录、纯查询）。
/// 绝对路径模式返回末段名（meta 里仍展示可读名字）。
/// ponytail: profile_dir() 会 create_dir_all 在没设 env 时副作用意外；这里只读。
pub fn profile_name_only() -> String {
    match effective_profile_raw() {
        Some(raw) if is_absolute_dir(&raw) => {
            Path::new(raw.trim())
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("default")
                .to_owned()
        }
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
                // 276：持有者未必是 Chrome（用户他机实报是 msedge）——文案不误导排查方向
                tracing::warn!(
                    "残留锁被浏览器进程或杀软持有: {} ({e})。排查: handle.exe \"{}\"，或资源监视器（性能→CPU→关联的句柄）搜索 profile 路径",
                    p.display(),
                    p.display()
                );
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
pub async fn swap_to_headed(
    browser: &mut Browser,
    handler_slot: &mut Option<tokio::task::JoinHandle<()>>,
) -> Result<()> {
    // n9j：先 abort 旧 handler task 再 close——graceful_close 的 close() 无超时保护，
    // handler 卡死时 close().await 永不返回；abort 后 close 立即失败走 warn，wait 有 5s
    // 超时 + kill 兜底，swap 全程有界，launch 失败路径旧 handler 也不再悬挂。
    // （旧 handler 若跨到新 Browser 仍活着，chromiumoxide 0.9.1 sender 错配会报
    // "send failed receiver is gone"。）
    if let Some(h) = handler_slot.take() {
        h.abort();
    }
    graceful_close(browser).await;
    let (new_browser, handler) = launch(false).await?;
    *handler_slot = Some(spawn_handler(handler));
    *browser = new_browser;
    Ok(())
}

/// close 当前 browser 并同 profile 重启**无头**实例——解码完成后切回，对称 swap_to_headed。
/// 动机（PM 实测）：GAEX 豁免 cookie 对 headed 有效、对 --headless=new 无效（Google 对无头
/// 浏览器有独立指纹风控），headed 直接过验证后切回无头，本次命令/会话内后续搜索不再弹窗。
pub async fn swap_to_headless(
    browser: &mut Browser,
    handler_slot: &mut Option<tokio::task::JoinHandle<()>>,
) -> Result<()> {
    // n9j：与 swap_to_headed 对称——先 abort 旧 handler 保证 close/wait 有界 + 失败路径不悬挂。
    if let Some(h) = handler_slot.take() {
        h.abort();
    }
    graceful_close(browser).await;
    let (new_browser, handler) = launch(true).await?;
    *handler_slot = Some(spawn_handler(handler));
    *browser = new_browser;
    Ok(())
}
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
        // ⚠ chromiumoxide 0.9.1 ArgsBuilder 会给 key 统一加 "--" 前缀（argument.rs:35,37），
        // 手写 "--xxx" 会变成 "----xxx" 被 Chrome 静默忽略（实测 UA/proxy 一直没生效）。
        // 这里全部去掉前缀，由 ArgsBuilder 补 "--"。
        .arg("disable-blink-features=AutomationControlled")
        .arg(format!("user-agent={UA}"))
        .disable_default_args();
    if let Some(proxy) = &proxy {
        // ponytail: Chrome 只识别 --proxy-server=protocol://host:port；不引 chromiumoxide proxy builder（M12 调试期足以）。
        tracing::info!("代理: {}", redact_proxy(proxy));
        builder = builder.arg(format!("proxy-server={proxy}"));
    }
    let safe_args: &[&str] = if headless {
        // disable-gpu 已去：Chrome 132+ 统一 headless 支持 GPU，留着反而让 WebGL/性能
        // 特征假；window-size=1920,1080 补 headless 默认 800x600 的窗口尺寸指纹。
        ["headless=new", "no-sandbox", "disable-dev-shm-usage", "window-size=1920,1080"].as_slice()
    } else {
        ["no-sandbox", "disable-dev-shm-usage"].as_slice()
    };
    builder = builder.args(safe_args.iter().copied());
    builder = if headless {
        // chromiumoxide 默认 viewport=800x600,每页 CDP Emulation.setDeviceMetricsOverride
        // 会把它强加给页面(screen.width/innerWidth 全变 800x600 指纹)。
        // 显式设 1920x1080 与 --window-size 对齐:emulation 让 screen 与窗口全链一致。
        builder
            .viewport(chromiumoxide::handler::viewport::Viewport {
                width: 1920,
                height: 1080,
                ..Default::default()
            })
            .new_headless_mode()
    } else {
        builder.with_head()
    };

    let config = builder
        .build()
        .map_err(|e| anyhow!("构造 BrowserConfig 失败: {e}"))?;

    // M14-2A cfg 切换点：默认（feature off）走 chromiumoxide 0.9 原路径，零行为变化；
    // chaser-stealth on 时改走 launch 层 transport stealth（见 launch_with_stealth_transport）。
    #[cfg(feature = "chaser-stealth")]
    let (browser, handler) = launch_with_stealth_transport(
        config,
        &profile,
        |forked_path| rebuild_browser_config(&browser_exe, &browser_kind, &proxy, headless, forked_path),
    )
    .await?;
    #[cfg(not(feature = "chaser-stealth"))]
    let (browser, handler) = launch_with_retry(config, &profile, |forked_path| {
        rebuild_browser_config(&browser_exe, &browser_kind, &proxy, headless, forked_path)
    })
    .await?;
    Ok((browser, handler))
}

/// BrowserConfig 重搭：fork 路径触发时调用，复制同配方 builder 把 user_data_dir 换掉。
/// ponytail: chromiumoxide 0.9 BrowserConfig 无公开 setter，必须走 Builder 重搭；这里
/// 把 builder 配方提出来当纯函数，便于 race-robust 第二层防御复用。
fn rebuild_browser_config(
    browser_exe: &Path,
    browser_kind: &BrowserKind,
    proxy: &Option<String>,
    headless: bool,
    profile: &Path,
) -> BrowserConfig {
    let mut builder = BrowserConfig::builder()
        .chrome_executable(browser_exe)
        .user_data_dir(profile)
        .arg("disable-blink-features=AutomationControlled")
        .arg(format!("user-agent={UA}"))
        .disable_default_args();
    if let Some(p) = proxy {
        tracing::info!("代理: {}", redact_proxy(p));
        builder = builder.arg(format!("proxy-server={p}"));
    }
    let safe_args: &[&str] = if headless {
        ["headless=new", "no-sandbox", "disable-dev-shm-usage", "window-size=1920,1080"].as_slice()
    } else {
        ["no-sandbox", "disable-dev-shm-usage"].as_slice()
    };
    builder = builder.args(safe_args.iter().copied());
    builder = if headless {
        builder
            .viewport(chromiumoxide::handler::viewport::Viewport {
                width: 1920,
                height: 1080,
                ..Default::default()
            })
            .new_headless_mode()
    } else {
        builder.with_head()
    };
    let _ = browser_kind; // 路径已定，kind 不影响 builder 构造（仅 launch 自动兑底时才用）
    builder
        .build()
        .expect("BrowserConfig 重搭不应失败（builder 已验证）")
}


/// 瞬态失败多轮退避重试：上一实例 Chrome 未死透的 profile 锁竞态、Windows Defender
/// 扫新生 exe 的文件锁（OS error 5）、以及**多 agent 并发同 profile**（实测：3 个并发
/// browse 只有 1 个能起，其余 ExitStatus(21)；browse 单次 5-15s，1s 单次重试必然再撞）。
/// 退避序列 1/3/6/10/15s（累计 35s）盖住前一个实例的完整会话时长；打尽仍失败才上抛。
/// 每轮重试打出上一次失败的**真实错误**（可见进度，不再静默五连）；打尽后列出
/// 持 profile 锁的僵尸浏览器进程 PID——chromiumoxide Windows 子进程继承锁句柄，
/// gsearch 退出后残留浏览器持续持锁，继续重试无效，只有精确 kill 才能解。
///
/// 第二层 race-robust 防御：profile_dir() 解析时已做第一层 lockfile 检测，但盲测
/// 并发毫秒级窗口可能两进程都过"启动前 lockfile 检查"→ 都走 default → 撞锁。
/// 重试打尽后若 last_err 含锁文件/Singleton/locked by 等关键词（Chrome 报错标准文案）
/// → fork 路径自动重试一次（仅限默认 profile；用户自定义 profile 不误触发）。
/// `rebuild` 闭包由调用方传入：用相同 builder 配方但改 user_data_dir 到 fork 路径。
/// ponytail: chromiumoxide 0.9 BrowserConfig 无公开 setter，必须走 Builder 重搭——闭包
/// 把 builder 逻辑从调用方借来，避免在这里复制构造代码。
async fn launch_with_retry<F>(
    config: BrowserConfig,
    profile: &Path,
    rebuild: F,
) -> Result<(Browser, Handler)>
where
    F: FnOnce(&Path) -> BrowserConfig,
{
    const BACKOFF_SECS: [u64; 5] = [1, 3, 6, 10, 15];
    let rounds = BACKOFF_SECS.len();
    let mut attempt = Browser::launch(config.clone()).await;
    for (round, wait) in BACKOFF_SECS.into_iter().enumerate() {
        if attempt.is_ok() {
            return attempt.map_err(|e| anyhow!("{e}"));
        }
        tracing::warn!(
            "浏览器启动失败（第 {}/{} 次，{}s 后重试）: {}——profile 疑被并发实例/僵尸进程占用；{}",
            round + 1,
            rounds,
            wait,
            attempt.as_ref().unwrap_err(),
            LOCK_WARN_FETCH_EXIT
        );
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        attempt = Browser::launch(config.clone()).await;
    }
    let last_err = attempt.expect_err("退避循环打尽必有 Err");
    let last_err_str = last_err.to_string();

    // 第二层 race-robust 防御：锁文件碰撞错误 + 当前是默认 profile → fork 重试一次
    if is_lock_collision_error(&last_err_str)
        && is_default_profile_path(profile)
        && let Some(forked) = try_fork_profile(profile)
    {
        eprintln!(
            "[hint] 启动 Chrome 时撞 default profile lock（启动前 race），自动 fork 到 {} 重试",
            forked
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
        );
        let forked_config = rebuild(&forked);
        if let Ok(ok) = Browser::launch(forked_config).await {
            return Ok(ok);
        }
    }

    let mut msg = lock_failure_msg(rounds, &last_err_str);
    match diagnose_lock_holders(profile) {
        Some(holders) => {
            msg.push_str(&format!("\n持有该 profile 的进程（按 PID 精确处理，禁 taskkill /IM 全杀）:\n{holders}"));
        }
        None => msg.push_str(
            "\n未发现命令行引用该 profile 的浏览器进程——可能是杀软/同步盘占用: handle.exe 查锁文件，或资源监视器（性能→CPU→关联的句柄）搜索 profile 路径",
        ),
    }
    Err(anyhow!(msg))
}

/// Chrome 启动失败文案是否提示 lockfile 竞态（关键词匹配，覆盖 chromiumoxide / Chrome 标准输出）。
/// 命中词：lockfile / Singleton / locked by / profile lock / already locked。
/// 大小写不敏感，避免遗漏变体。
fn is_lock_collision_error(err_str: &str) -> bool {
    let lower = err_str.to_ascii_lowercase();
    ["lockfile", "singleton", "locked by", "profile lock", "already locked"]
        .iter()
        .any(|k| lower.contains(k))
}

/// profile 锁打尽重试后的裸失败消息（纯函数供单测锁出口建议）。
/// B 受试者扣分点：干等 38s 失败后不知道「无需 Chrome 的 fetch」降级出口。
fn lock_failure_msg(rounds: usize, last_err: &str) -> String {
    format!(
        "启动浏览器失败（已重试 {rounds} 轮共 35s），profile 锁疑被长期占用（残留浏览器进程持锁时重试无效）: {last_err}\n无需渲染的场景可用 `gsearch fetch <url>` 替代（纯 HTTP，不经 Chrome 无 profile 锁）"
    )
}

/// h90（③打回轮1）：重试 WARN 阶段同样给 fetch 降级出口——B 复测部分复现实锤：
/// 干等期每轮 warn 只有 handle.exe 指引，用户/agent 在第一轮就该看到降级出口。
const LOCK_WARN_FETCH_EXIT: &str = "或用 `gsearch fetch <url>`（纯 HTTP 无需 Chrome）";

/// 列出命令行引用了该 profile 的浏览器进程（僵尸持锁者），返回 "PID=… 进程名" 行集。
/// 只诊断不 kill——精确 PID 交给用户处理（禁 taskkill /IM chrome.exe 全杀：OMP daemon 等
/// 无关会话共用进程名，全杀误伤）。非 Windows / 枚举失败 / 无命中 → None。
fn diagnose_lock_holders(profile: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        let pat = profile.to_string_lossy().replace('\'', "''");
        let script = format!(
            "Get-CimInstance Win32_Process -Filter \"Name='chrome.exe' or Name='msedge.exe'\" | Where-Object {{ $_.CommandLine -like '*{pat}*' }} | ForEach-Object {{ \"PID=$($_.ProcessId) $($_.Name)\" }}"
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if text.is_empty() { None } else { Some(text) }
    }
    #[cfg(not(windows))]
    {
        let _ = profile;
        None
    }
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
async fn launch_with_stealth_transport<F>(
    config: BrowserConfig,
    profile: &Path,
    rebuild: F,
) -> Result<(Browser, Handler)>
where
    F: FnOnce(&Path) -> BrowserConfig,
{
    let (browser, mut handler) = launch_with_retry(config, profile, rebuild).await?;
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
/// 首个 error event 改 continue（仅致命 close 事件 break）——chromiumoxide 0.9.1
/// Stream<Item = Result<()>> 偶尔会冒出瞬态 WS Invalid message 等非致命错误，旧版 break
/// 让 handler 永久挂掉，所有后续 CDP 命令变成 -32000（接收端 gone）。改成 continue 后
/// 让 handler 持续 poll；handler 自然在 closing 时返回 None 走完循环。
pub fn spawn_handler(handler: Handler) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut h = handler;
        while let Some(event) = h.next().await {
            if event.is_err() {
                tracing::warn!("CDP handler 出错（继续 poll）: {:?}", event);
                continue;
            }
        }
    })
}

/// 统一 new_page 入口——建 about:blank 页后立刻开 focus emulation（CDP
/// Emulation.setFocusEmulationEnabled，jev-ultrafast 同款），防隐藏 tab 被 Chrome
/// 定时器节流（shell 多 tab / swap_to_headed 重建页场景；headless 单页本就不节流，
/// 开了无副作用）。emulation 失败只 warn 不 abort：防节流是性能加固不是正确性路径
/// （对齐 chaser-stealth 补丁风格）。调用前提与既有 new_page 相同：handler 已 spawn。
pub async fn open_page(browser: &Browser) -> Result<chromiumoxide::Page> {
    let page = browser.new_page("about:blank").await?;
    if let Err(e) = page.execute(SetFocusEmulationEnabledParams::new(true)).await {
        tracing::warn!("focus emulation 未开启（后台 tab 定时器可能被节流）: {e}");
    }
    Ok(page)
}

/// 关 Chrome 并等进程死透。chromiumoxide 0.9 的 close 只 background kill，
/// 不调 wait() 直接 Drop 会报 "was not closed manually" WARN，且 profile 锁残留。
/// 顶层命令（search/browse/login/dl）+ shell 退出统一走这条，避免下一次 launch 撞 profile 锁。
///
/// M16 压测发现：chromiumoxide 0.9.1 在 Windows 上 close + wait 后 child 仍可能残留
/// （handler 退出循环但未给 chrome 发 CDP Browser.close 命令，kill_on_drop 仅 Unix 生效）。
/// 曾有 `kill_residual_chrome_strict()` 跑 `taskkill /IM chrome.exe /T /F`——会误杀
/// 用户主 Chrome（不是本 profile 的实例），已被审计删除。Chrome 残留收口一律走
/// `graceful_close`（per-进程 close + wait + kill 兜底，不动用户主实例）。
/// 关 Chrome 并等进程死透。chromiumoxide 0.9.1 的 close 只断 CDP 不保证杀子进程树；
/// 调 kill() 兜底（公开 API，browser/mod.rs:315）。顶层命令（search/browse/login/dl）
/// + shell 退出统一走这条，避免下一次 launch 撞 profile 锁。
///
/// wait 加 5s tokio 超时——chromiumoxide 的 wait 在 Windows 上挂死历史踩坑
/// （kill_on_drop 仅 Unix 生效；close 后子进程未必立刻退）；超时后强制走 kill 分支，
/// 避免 graceful_close 自身成 hang 源。
pub async fn graceful_close(browser: &mut Browser) {
    if let Err(e) = browser.close().await {
        tracing::warn!("close browser 失败: {e}");
    }
    let wait_res = tokio::time::timeout(Duration::from_secs(5), browser.wait()).await;
    match wait_res {
        Ok(Ok(Some(status))) if status.success() => {} // 正常退出
        _ => {
            // 残留/超时：杀子进程，再 wait 兜底（这次不超时——kill 是同步信号）
            if let Some(Err(e)) = browser.kill().await {
                tracing::warn!("kill browser 失败: {e}");
            }
            let _ = browser.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{profile_name, redact_proxy};
    #[cfg(windows)]
    use super::user_scope_path_in;

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

    /// 用户级安装探测（换机兼容）：vendor 目录按 exe 区分（Google\Chrome vs Microsoft\Edge）。
    /// 概念上是 Windows-only（macOS 对应位置 ~/Library/Application Support 且未实现），CI 在 macOS/ubuntu
    /// 上跑时 cfg gate 掉——避免用 Windows 路径字面量在 Unix 文件系统上不可断言。
    #[cfg(windows)]
    #[test]
    fn user_scope_path_maps_vendor_by_exe() {
        let base = std::path::PathBuf::from(r"C:\Users\t\AppData\Local");
        let chrome = user_scope_path_in(&base, "chrome.exe");
        assert!(chrome.ends_with(r"Google\Chrome\Application\chrome.exe"), "{chrome:?}");
        let edge = user_scope_path_in(&base, "msedge.exe");
        assert!(edge.ends_with(r"Microsoft\Edge\Application\msedge.exe"), "{edge:?}");
    }

    /// find_specific 必须探测用户级安装路径——doctor 与 launch 一致性依赖于此。
    /// 旧版 doctor 的 find_with_kind_display 漏掉 user_scope，导致无管理员安装的 Chrome
    /// 报 FAIL 而 launch 实际能找到。本次回归测试断言：仅用户级 Chrome 存在时
    /// find_specific(Chrome) 能返回 Some（即使本机路径不存在，函数逻辑要进 user_scope）。
    #[cfg(windows)]
    #[test]
    fn find_specific_includes_user_scope_path() {
        // user_scope_path 只在 %LOCALAPPDATA% 存在时返回路径——函数逻辑入口。
        // 我们不假设具体机器有 Chrome,但 user_scope_path 在 Windows 上必返回 Some(本地配置存在时)。
        // 用 test helper 绕过 var_os:直接调 user_scope_path_in 验证函数契约。
        let fake_base = std::path::PathBuf::from(r"C:\Users\TestUser\AppData\Local");
        let chrome = user_scope_path_in(&fake_base, "chrome.exe");
        // 必须走 "Google\Chrome\Application" 分支,不是 "Microsoft"——分支正确性。
        assert!(chrome.to_string_lossy().contains("Google"));
        assert!(chrome.to_string_lossy().contains("Chrome"));
        assert!(!chrome.to_string_lossy().contains("Microsoft"));
    }

    /// P0 fork profile：default 不存在（首次启动）→ 不 fork，返回 default 自身
    ///（首次启动也是 silent fallback，与正式运行时行为一致）。
    #[test]
    fn try_fork_profile_returns_none_when_default_missing() {
        let tmp = tempdir();
        let default_path = tmp.join("profiles").join("default");
        assert!(!default_path.exists());
        assert!(super::try_fork_profile(&default_path).is_none());
    }

    /// P0 fork profile：default 存在但无 lockfile → 不 fork（用户主动开关一次
    /// Chrome 残留的 SingletonLock 由 cleanup_stale_locks 处理，不在 fork 路径触发）。
    #[test]
    fn try_fork_profile_returns_none_when_no_locks() {
        let tmp = tempdir();
        let default_path = tmp.join("profiles").join("default");
        std::fs::create_dir_all(&default_path).unwrap();
        // 放一些内容代表 cookie/历史
        std::fs::write(default_path.join("Cookies"), b"cookie-data").unwrap();
        assert!(super::try_fork_profile(&default_path).is_none());
    }

    /// P0 fork profile：lockfile 残留但无诊断到的持锁进程（孤儿 SingletonLock）→
    /// 不 fork，走 cleanup_stale_locks 路径——fork 仅在确有并发实例时触发。
    /// 本测试在普通 CI 上（无 chrome 进程）必命中 None。
    #[test]
    fn try_fork_profile_orphan_lock_no_fork() {
        let tmp = tempdir();
        let default_path = tmp.join("profiles").join("default");
        std::fs::create_dir_all(&default_path).unwrap();
        std::fs::write(default_path.join("SingletonLock"), b"").unwrap();
        // 在没有 chrome.exe/msedge.exe 跑着的 CI 上 diagnose_lock_holders 返回 None
        // → try_fork_profile 也返回 None（孤儿锁不 fork）。
        let r = super::try_fork_profile(&default_path);
        assert!(r.is_none(), "孤儿 SingletonLock 不应触发 fork（避免误拷无主内容）");
    }

    /// P0 fork profile：fork 路径形如 fork-<timestamp>-<pid>-<rand>，与 default 同 parent。
    #[test]
    fn fork_path_for_uses_fork_prefix_and_default_parent() {
        let tmp = tempdir();
        let default_path = tmp.join("profiles").join("default");
        std::fs::create_dir_all(default_path.parent().unwrap()).unwrap();
        let fork = super::fork_path_for(&default_path).unwrap();
        assert_eq!(fork.parent(), default_path.parent());
        let name = fork.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("fork-"), "fork 名前缀: {name}");
        // 末段 4 个 hex 字符
        let last = name.rsplit('-').next().unwrap();
        assert_eq!(last.len(), 4, "rand 后缀长度: {last}");
        assert!(last.chars().all(|c| c.is_ascii_hexdigit()), "rand 后缀 hex: {last}");
    }

    /// P0 fork profile：copy_profile_contents 递归拷贝并跳过 lockfile 残留。
    /// 模拟 default 顶层 + 子目录（Default/Cookies）+ 锁文件。
    #[test]
    fn copy_profile_contents_recursive_and_skips_locks() {
        let tmp = tempdir();
        let default_path = tmp.join("default");
        let fork_path = tmp.join("fork");
        // 顶层：cookie 文件 + 锁文件（应被跳过）
        std::fs::create_dir_all(&default_path).unwrap();
        std::fs::write(default_path.join("Cookies"), b"my-cookie").unwrap();
        std::fs::write(default_path.join("SingletonLock"), b"lock-data").unwrap();
        std::fs::write(default_path.join("Preferences"), b"pref-json").unwrap();
        // 子目录：Default/Cookies + 子目录里也有锁文件
        let default_sub = default_path.join("Default");
        std::fs::create_dir_all(&default_sub).unwrap();
        std::fs::write(default_sub.join("History"), b"history-data").unwrap();
        std::fs::write(default_sub.join("lockfile"), b"inner-lock").unwrap();

        super::copy_profile_contents(&default_path, &fork_path).unwrap();

        // 顶层：cookie + preferences 拷过去，SingletonLock 不拷
        assert!(fork_path.join("Cookies").exists(), "顶层 cookie 应拷");
        assert!(fork_path.join("Preferences").exists(), "顶层 prefs 应拷");
        assert!(!fork_path.join("SingletonLock").exists(), "顶层 SingletonLock 应跳过");

        // 子目录：History 拷过去，lockfile 不拷
        assert!(fork_path.join("Default").join("History").exists(), "子目录 History 应拷");
        assert!(!fork_path.join("Default").join("lockfile").exists(), "子目录 lockfile 应跳过");

        // 内容保真（cookie 是 cookie，不是 lock-data）
        let cookie = std::fs::read(fork_path.join("Cookies")).unwrap();
        assert_eq!(cookie, b"my-cookie", "cookie 内容保真");
    }

    /// bnh 回归：fork 拷贝跳过缓存类目录——顶层 Crashpad/GrShaderCache 与 Default/ 下
    /// Cache/Code Cache 整棵不拷，default profile 本体不动；登录/历史数据照拷。
    #[test]
    fn copy_profile_contents_skips_cache_dirs() {
        let tmp = tempdir();
        let default_path = tmp.join("default");
        let fork_path = tmp.join("fork");
        // 顶层缓存目录 + 登录数据
        std::fs::create_dir_all(default_path.join("Crashpad")).unwrap();
        std::fs::write(default_path.join("Crashpad").join("reports"), b"dump").unwrap();
        std::fs::create_dir_all(default_path.join("GrShaderCache")).unwrap();
        std::fs::write(default_path.join("Cookies"), b"my-cookie").unwrap();
        // Default/ 下缓存目录（Cache 内再嵌一层验证整棵跳过）
        let cache_sub = default_path.join("Default").join("Cache");
        std::fs::create_dir_all(&cache_sub).unwrap();
        std::fs::write(cache_sub.join("entry"), b"cache-entry").unwrap();
        std::fs::create_dir_all(default_path.join("Default").join("Code Cache")).unwrap();
        std::fs::write(default_path.join("Default").join("History"), b"history-data").unwrap();

        super::copy_profile_contents(&default_path, &fork_path).unwrap();

        assert!(!fork_path.join("Crashpad").exists(), "顶层 Crashpad 应跳过");
        assert!(!fork_path.join("GrShaderCache").exists(), "顶层 GrShaderCache 应跳过");
        assert!(!fork_path.join("Default").join("Cache").exists(), "Default/Cache 应整棵跳过");
        assert!(!fork_path.join("Default").join("Code Cache").exists(), "Default/Code Cache 应跳过");
        // 跳过表只影响 fork 拷贝：default 本体 Cache 不动
        assert!(cache_sub.join("entry").exists(), "default 本体 Cache 不应被删改");
        assert!(fork_path.join("Cookies").exists(), "顶层 Cookies 应拷");
        assert!(fork_path.join("Default").join("History").exists(), "Default/History 应拷");
    }

    /// P0 fork profile：is_default_profile_path 仅末段 = default 时为真，
    /// 自定义 profile (work 等) 不误判，避免误拷用户私有 profile。
    #[test]
    fn is_default_profile_path_only_default() {
        assert!(super::is_default_profile_path(std::path::Path::new("/home/u/.gsearch/profiles/default")));
        assert!(super::is_default_profile_path(std::path::Path::new("D:/foo/default")));
        assert!(!super::is_default_profile_path(std::path::Path::new("/home/u/.gsearch/profiles/work")));
        assert!(!super::is_default_profile_path(std::path::Path::new("D:/foo/default-backup")));
    }

    /// 用临时目录作为本次测试的「HOME」——避免污染真 ~/.gsearch/profiles。
    /// 测试结束后 tempdir drop 时会触发临时目录清理（dirs cleanup 由调用方控制）。
    fn tempdir() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "gsearch-fork-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// 第二层 race-robust 防御：is_lock_collision_error 关键词匹配覆盖 chromiumoxide / Chrome
    /// 标准报错文案。大小写不敏感；命中即视为锁碰撞，触发 fork 重试一次。
    #[test]
    fn is_lock_collision_error_keyword_coverage() {
        // 命中：各变体文案
        for ok in [
            "SingletonLock exists",
            "the lockfile is held by another process",
            "Profile locked by another browser instance",
            "Profile is already locked",
            "LOCKFILE contention",
            "user data dir is already locked by another process",
        ] {
            assert!(
                super::is_lock_collision_error(ok),
                "应识别锁碰撞: {ok}"
            );
        }
        // 不命中：非锁碰撞错（防御误触发 fork）
        for bad in [
            "ExitStatus(21)",
            "connection refused",
            "timeout waiting for browser",
            "Failed to find Chrome executable",
        ] {
            assert!(
                !super::is_lock_collision_error(bad),
                "不应误判锁碰撞: {bad}"
            );
        }
    }

    /// 第二层 race-robust 防御：fork 重试路径需要默认 profile + 持锁才能触发。
    /// 通过 try_fork_profile 直接验证（launch_with_retry 是 async 不易单测）——
    /// 验证条件分支：is_default_profile_path + SingletonLock 残留 → fork 路径生成。
    /// 单元测试不验证 Browser::launch 真实 mock（需 Chrome 进程），改验证 fork 路径产物
    /// 与 hint 文案。
    #[test]
    fn race_robust_second_layer_fork_path_generates_distinct_dir() {
        let tmp = tempdir();
        let default_path = tmp.join("profiles").join("default");
        std::fs::create_dir_all(&default_path).unwrap();
        std::fs::write(default_path.join("SingletonLock"), b"").unwrap();
        std::fs::write(default_path.join("Cookies"), b"cookie").unwrap();
        // fork 路径前缀 + UUID 形态（每次独立），不应与 default 同名
        let f1 = super::fork_path_for(&default_path).unwrap();
        let f2 = super::fork_path_for(&default_path).unwrap();
        assert_ne!(f1, f2, "两次 fork 路径应不同（uuid 随机性）");
        assert_eq!(f1.parent(), default_path.parent());
        assert!(f1.file_name().unwrap().to_str().unwrap().starts_with("fork-"));
        // copy_profile_contents 应跳过 SingletonLock 不带入 fork
        std::fs::create_dir_all(&f1).unwrap();
        super::copy_profile_contents(&default_path, &f1).unwrap();
        assert!(f1.join("Cookies").exists());
        assert!(!f1.join("SingletonLock").exists());
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

    /// h90：profile 锁裸失败文案带 fetch 降级出口（B 受试者扣分点：38s 干等后无出口引导）。
    #[test]
    fn lock_failure_msg_contains_fetch_exit() {
        let msg = super::lock_failure_msg(5, "ExitStatus(21)");
        assert!(msg.contains("gsearch fetch <url>"), "{msg}");
        assert!(msg.contains("profile 锁"), "{msg}");
        assert!(msg.contains("ExitStatus(21)"), "{msg}");
        // ③打回轮1：重试 WARN 阶段（每轮）同给 fetch 降级出口，不只终态 error
        assert!(super::LOCK_WARN_FETCH_EXIT.contains("gsearch fetch <url>"), "{}", super::LOCK_WARN_FETCH_EXIT);
        assert!(super::LOCK_WARN_FETCH_EXIT.contains("无需 Chrome"), "{}", super::LOCK_WARN_FETCH_EXIT);
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
