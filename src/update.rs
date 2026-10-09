//! `update`（issue gsearch-rs-745）：GitHub latest release 版本比对，学 gh CLI 的 opt-in 克制——
//! 只比对 + 给升级指引，**不做自替换**：Windows 运行中 exe 有文件锁，原地替换需 rename 技巧
//! 且易被杀软误报（gh 同样不自替换）。升级走 release 页资产或 cargo install。

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;

const RELEASE_API: &str = "https://api.github.com/repos/Be90nia/gsearch-rs/releases/latest";
/// API 请求总预算：版本检查应快速可见结果，不吃 dl 级 300s。
const UPDATE_TIMEOUT_SECS: u64 = 10;
/// 无 token GitHub API 限流 60 次/h/IP——403 时指引里说明。
const REPO_URL: &str = "https://github.com/Be90nia/gsearch-rs";

#[derive(serde::Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
}

/// "v0.2.10" / "0.2.10-rc1" / "0.2.10+build" → (0, 2, 10)；剥离 v 前缀与 -pre/+build 后缀，
/// 核心三段必须全数字，否则 None（调用方降级为"无法比对"）。
fn parse_semver3(s: &str) -> Option<(u64, u64, u64)> {
    let t = s.trim();
    let core = t.strip_prefix('v').unwrap_or(t).split(['-', '+']).next()?;
    let mut it = core.split('.');
    Some((
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    ))
}

pub async fn cmd_update(proxy: Option<String>) -> Result<ExitCode> {
    let local = env!("CARGO_PKG_VERSION");
    eprintln!("查询 GitHub latest release…");
    // 与 fetch/dl 同源客户端：UA gsearch/x.y.z + 重定向每跳 SSRF 门；GSEARCH_PROXY 透传。
    let client = crate::fetch::build_client(proxy.as_deref(), false, Duration::from_secs(UPDATE_TIMEOUT_SECS))?;
    let rel: LatestRelease = client
        .get(RELEASE_API)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .context("查询 GitHub releases/latest 失败（网络/DNS）")?
        .error_for_status()
        .context("GitHub API 返回非 2xx（403 多为无 token 限流 60 次/h/IP）")?
        .json()
        .await
        .context("解析 GitHub release JSON 失败")?;

    let tag = rel.tag_name.trim();
    let (Some(remote), Some(local_v)) = (parse_semver3(tag), parse_semver3(local)) else {
        println!("无法比对版本：远端 tag '{tag}' 或本地 '{}' 不是 semver 三段式", local);
        println!("release 页: {}", rel.html_url);
        return Ok(ExitCode::SUCCESS);
    };
    if local_v >= remote {
        // 本地 == 远端为常态；本地 > 远端（dev 提前发）也不建议降级
        println!("已是最新（本地 v{local}，远端 {tag}）");
    } else {
        println!("有新版 {tag}（本地 v{local}）");
        println!("下载 URL: {}", rel.html_url);
        println!("或: cargo install --git {REPO_URL} --locked");
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::parse_semver3;

    #[test]
    fn semver3_parses_v_prefix_and_pre_build_suffixes() {
        assert_eq!(parse_semver3("v0.2.10"), Some((0, 2, 10)));
        assert_eq!(parse_semver3("0.2.9"), Some((0, 2, 9)));
        assert_eq!(parse_semver3(" v1.2.3-rc1 "), Some((1, 2, 3)));
        assert_eq!(parse_semver3("1.2.3+build.7"), Some((1, 2, 3)));
        assert_eq!(parse_semver3("1.2"), None);
        assert_eq!(parse_semver3("abc"), None);
        assert_eq!(parse_semver3(""), None);
    }

    #[test]
    fn semver3_numeric_compare_not_lexicographic() {
        assert!((0, 2, 10) > (0, 2, 9));
        assert!(parse_semver3("v0.2.10").unwrap() > parse_semver3("0.2.9").unwrap());
    }
}
