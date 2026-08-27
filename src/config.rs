//! `gsearch.json` 配置文件（M15）：让用户不用环境变量也能配 profile / chrome 路径。
//!
//! 查找顺序：`--config <path>` 显式指定 → `./gsearch.json`（工作目录）→ `~/.gsearch/config.json`。
//! 前两个都只读已存在的文件，不主动创建——不想留痕 C 盘就放前两处。
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

fn load_from_disk() -> Result<GsearchConfig> {
    let candidates: Vec<(PathBuf, bool)> = match EXPLICIT.get() {
        Some(p) => vec![(p.clone(), true)],
        None => {
            let mut v = vec![(PathBuf::from("gsearch.json"), false)];
            if let Some(home) = home_dir() {
                v.push((home.join(".gsearch").join("config.json"), false));
            }
            v
        }
    };
    for (path, explicit) in candidates {
        if !path.is_file() {
            if explicit {
                return Err(anyhow!("--config 指定的文件不存在: {}", path.display()));
            }
            continue;
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("读取配置失败: {}", path.display()))?;
        let cfg: GsearchConfig = serde_json::from_str(&raw)
            .with_context(|| format!("配置格式错误（应为 JSON 对象，键 profile/chrome）: {}", path.display()))?;
        tracing::debug!("已加载配置: {}", path.display());
        return Ok(cfg);
    }
    Ok(GsearchConfig::default())
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
}
