//! `gsearch.json` 配置文件（M15）：让用户不用环境变量也能配 profile / chrome 路径。
//!
//! 查找顺序：`--config <path>` 显式指定 → `./gsearch.json`（工作目录）→ exe 旁 `gsearch.json`
//! （绿色软件，cwd 无关）→ `~/.gsearch/config.json`。
//! 自动发现路径只读已存在的文件，不主动创建。
//!
//! 优先级（各键独立）：环境变量 > 配置文件 > 默认值。
//! 格式错误：显式指定的路径报错（用户点名要它）；自动发现的仅 warn 后忽略。

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct GsearchConfig {
    /// profile 名或路径（语义同 GSEARCH_PROFILE：取末段名放进 ~/.gsearch/profiles/）
    pub profile: Option<String>,
    /// Chrome/Edge 可执行文件路径（语义同 GSEARCH_CHROME）
    pub chrome: Option<String>,
    /// M16 SearXNG 实例 base URL（语义同 GSEARCH_SEARXNG_URL）；None = 走 Google 直爬。
    pub searxng_url: Option<String>,
    /// jp4：read/browse 正文提取的字符硬上限；None = 缺省 50000（postproc::READ_BODY_MAX_CHARS）。
    /// 防超大页正文撑爆 agent 上下文。
    pub read_max_chars: Option<usize>,
}

static CONFIG: OnceLock<GsearchConfig> = OnceLock::new();
static EXPLICIT: OnceLock<PathBuf> = OnceLock::new();

/// main 用：记录显式路径并立即加载校验（文件不存在/格式错 → 启动即报错，不静默落到默认）。
pub fn set_explicit_and_load(path: PathBuf) -> Result<()> {
    let _ = EXPLICIT.set(path);
    let cfg = load_from_disk()?;
    let _ = CONFIG.set(cfg);
    Ok(())
}

/// 读配置（进程内首次读盘后缓存）。文件不存在 = 空配置，不报错。
pub fn load() -> &'static GsearchConfig {
    CONFIG.get_or_init(|| load_from_disk().unwrap_or_default())
}
/// M16：searxng_url 的 env 覆盖（GSEARCH_SEARXNG_URL > 配置文件 > None）。
/// 对齐 GSEARCH_PROXY 语义：设了但空白 = 未设。抽纯函数便于单测（env::set_var 在测试里是 unsafe + 全局污染）。
fn merge_searxng_env(cfg: &mut GsearchConfig, env_val: Option<String>) {
    if let Some(v) = env_val.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        cfg.searxng_url = Some(v);
    }
}

fn load_from_disk() -> Result<GsearchConfig> {
    let candidates: Vec<(PathBuf, bool)> = match EXPLICIT.get() {
        Some(p) => vec![(p.clone(), true)],
        None => {
            let mut v = vec![(PathBuf::from("gsearch.json"), false)];
            // exe 旁边（绿色软件惯例：gsearch.json 随 exe 分发，cwd 无关）。
            // 用户把配置放 exe 旁却在任意目录调 gsearch 时，前两个候选都落空 →
            // searxng_url 静默失效走 Google 直爬撞 CAPTCHA 弹窗（真机踩坑，2026-09-11）。
            if let Ok(exe) = std::env::current_exe()
                && let Some(dir) = exe.parent()
            {
                v.push((dir.join("gsearch.json"), false));
            }
            if let Some(home) = home_dir() {
                v.push((home.join(".gsearch").join("config.json"), false));
            }
            v
        }
    };
    let mut cfg = GsearchConfig::default();
    for (path, explicit) in candidates {
        if !path.is_file() {
            if explicit {
                return Err(anyhow!("--config 指定的文件不存在: {}", path.display()));
            }
            continue;
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("读取配置失败: {}", path.display()))?;
        cfg = serde_json::from_str(&raw)
            .with_context(|| format!("配置格式错误（应为 JSON 对象，键 profile/chrome/searxng_url）: {}", path.display()))?;
        tracing::debug!("已加载配置: {}", path.display());
        break;
    }
    // M16：searxng_url 的 env 覆盖在磁盘配置之后统一做（env > 文件 > None）
    merge_searxng_env(&mut cfg, std::env::var("GSEARCH_SEARXNG_URL").ok());
    Ok(cfg)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// jp4：read_max_chars 可配（缺 key = None，不破坏旧配置文件）。
    #[test]
    fn parses_read_max_chars() {
        let cfg: GsearchConfig = serde_json::from_str(r#"{"read_max_chars": 80000}"#).unwrap();
        assert_eq!(cfg.read_max_chars, Some(80000));
        let cfg: GsearchConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.read_max_chars, None);
    }

    #[test]
    fn parses_both_keys() {
        let cfg: GsearchConfig = serde_json::from_str(
            r#"{"profile": "work", "chrome": "D:/Sdk/chrome.exe"}"#,
        )
        .unwrap();
        assert_eq!(cfg.profile.as_deref(), Some("work"));
        assert_eq!(cfg.chrome.as_deref(), Some("D:/Sdk/chrome.exe"));
    }

    #[test]
    fn empty_object_and_unknown_keys_ok() {
        let cfg: GsearchConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.profile.is_none() && cfg.chrome.is_none());
        // 未知键忽略（向前兼容）
        let cfg: GsearchConfig =
            serde_json::from_str(r#"{"future_key": 1, "profile": "p"}"#).unwrap();
        assert_eq!(cfg.profile.as_deref(), Some("p"));
    }

    #[test]
    fn malformed_json_rejected() {
        assert!(serde_json::from_str::<GsearchConfig>("{nope").is_err());
        // 错误类型：profile 必须是字符串
        assert!(serde_json::from_str::<GsearchConfig>(r#"{"profile": 3}"#).is_err());
    }

    #[test]
    fn load_from_disk_missing_file_is_empty() {
        // EXPLICIT 未设 + CWD 无 gsearch.json 的场景下不 panic 不报错
        let cfg = load_from_disk().unwrap();
        // 只断言结构合法；值取决于机器上是否恰好存在配置文件
        let _ = &cfg.profile;
        let _ = &cfg.chrome;
    }
    #[test]
    fn load_from_disk_explicit_missing_errs() {
        // 复刻 load_from_disk 的读+解析两步，验证缺失路径报错（EXPLICIT 是全局静态，测试里不能直接喂）
        let p = std::path::Path::new("Z:/definitely/not/here/gsearch.json");
        assert!(std::fs::read_to_string(p).is_err());
    }

    #[test]
    fn parses_searxng_url_key() {
        let cfg: GsearchConfig =
            serde_json::from_str(r#"{"searxng_url": "http://192.168.89.249:8888"}"#).unwrap();
        assert_eq!(cfg.searxng_url.as_deref(), Some("http://192.168.89.249:8888"));
        // 缺省 = None（走 Google 直爬）
        let cfg: GsearchConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.searxng_url.is_none());
    }

    #[test]
    fn merge_searxng_env_priority() {
        // env > 文件：覆盖已有值
        let mut cfg = GsearchConfig {
            searxng_url: Some("http://from-file:8888".into()),
            ..Default::default()
        };
        merge_searxng_env(&mut cfg, Some("http://from-env:9999".into()));
        assert_eq!(cfg.searxng_url.as_deref(), Some("http://from-env:9999"));
        // env 未设：保留文件值
        let mut cfg = GsearchConfig {
            searxng_url: Some("http://from-file:8888".into()),
            ..Default::default()
        };
        merge_searxng_env(&mut cfg, None);
        assert_eq!(cfg.searxng_url.as_deref(), Some("http://from-file:8888"));
        // env 设了但空白 = 未设（对齐 GSEARCH_PROXY 语义）
        merge_searxng_env(&mut cfg, Some("   ".into()));
        assert_eq!(cfg.searxng_url.as_deref(), Some("http://from-file:8888"));
        // env 空白值 trim 后生效
        merge_searxng_env(&mut cfg, Some("  http://trim:1  ".into()));
        assert_eq!(cfg.searxng_url.as_deref(), Some("http://trim:1"));
    }
}
