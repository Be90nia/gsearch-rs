//! M4 搜索结果后处理：`--open N` / `--read N` / `--dl N`（PLAN §3.4 / §3.5）。
//! 复用 cmd_search 已启动的同一 browser 实例（同 profile → 同 cookie），不另起 Chrome。
//!
//! M9：`--read N` 默认输出 AdaptiveRead（按文章结构自适应三段：目录/摘要/段落索引），
//! `--full` 拿纯 innerText 5000 字（兜底），`--json` 拿结构化 JSON，`--from K` 摘要偏移，
//! `--headings-only` 仅目录。

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow};
use chromiumoxide::browser::Browser;

use gsearch::search::is_captcha;
use gsearch::skeleton::{extract_adaptive, format_adaptive, format_headings_only, format_json};
use gsearch::types::SearchResult;
use gsearch::util::{b64_decode, filename_from_url};

const READ_FULL_MAX_CHARS: usize = 5000;
const PAGE_TIMEOUT_SECS: u64 = 30;
const DL_LARGE_BYTES: usize = 50_000_000;

/// `--{flag} N` 下标校验：1-based；0 或超出结果数报错（含结果数为 0 的情况）
fn pick<'a>(results: &'a [SearchResult], n: usize, flag: &str) -> Result<&'a str> {
    if n == 0 || n > results.len() {
        return Err(anyhow!("--{flag} {n} 越界（结果数 {}）", results.len()));
    }
    Ok(&results[n - 1].url)
}

/// `--open N`：默认浏览器开窗。
pub fn open(results: &[SearchResult], n: usize) -> Result<()> {
    let url = pick(results, n, "open")?;
    if let Err(e) = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .spawn()
    {
        tracing::warn!("打开默认浏览器失败（不影响输出）: {e}");
    }
    Ok(())
}

/// `--read N` 选项集。M9：默认 AdaptiveRead，`--full/--json/--headings-only/--from K` 互斥选择。
#[derive(Debug, Clone, Default)]
pub struct ReadOpts {
    pub full: bool,
    pub json: bool,
    pub headings_only: bool,
    pub from: usize,
}

/// `--read N`：M9 默认走 AdaptiveRead（按文章结构自适应）。opts 见 ReadOpts。
pub async fn read(
    browser: &Browser,
    results: &[SearchResult],
    n: usize,
    opts: &ReadOpts,
) -> Result<String> {
    let url = pick(results, n, "read")?;
    let page = browser.new_page("about:blank").await?;
    tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
        .await
        .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
        .map_err(|e| anyhow!("goto {url} 失败（浏览器被手关？）: {e}"))?;

    if is_captcha(&page.content().await.unwrap_or_default()) {
        return Err(anyhow!("{url} 遇 CAPTCHA，M4 后处理不支持人解，请重试或手动浏览器打开"));
    }

    if opts.full {
        return read_full_inner(&page, url).await;
    }

    let title = page
        .evaluate("document.title")
        .await?
        .into_value::<String>()
        .unwrap_or_default();
    let html = page.content().await.unwrap_or_default();
    let mut read = extract_adaptive(&html);
    read.url = url.to_string();
    read.title = title;

    let out = if opts.json {
        format_json(&read)
    } else if opts.headings_only {
        format_headings_only(&read)
    } else {
        format_adaptive(&read, opts.from)
    };
    println!("{out}");
    Ok(out)
}

/// `--read N --full` 兜底：纯 innerText 5000 字。
pub async fn read_full(browser: &Browser, results: &[SearchResult], n: usize) -> Result<String> {
    let url = pick(results, n, "read")?;
    let page = browser.new_page("about:blank").await?;
    tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
        .await
        .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
        .map_err(|e| anyhow!("goto {url} 失败（浏览器被手关？）: {e}"))?;

    if is_captcha(&page.content().await.unwrap_or_default()) {
        return Err(anyhow!("{url} 遇 CAPTCHA，M4 后处理不支持人解，请重试或手动浏览器打开"));
    }

    read_full_inner(&page, url).await
}

/// 共享 innerText 5000 字截断 + 打印。
async fn read_full_inner(page: &chromiumoxide::Page, url: &str) -> Result<String> {
    let txt = page
        .evaluate("document.body.innerText")
        .await?
        .into_value::<String>()
        .unwrap_or_default();
    let txt: String = txt.chars().take(READ_FULL_MAX_CHARS).collect();
    println!("=== {url} ===\n{txt}");
    Ok(txt)
}
/// `--dl N [-o DIR]`：走浏览器 cookie 的简化下载（M13 加 `-o`）。
/// `-o DIR` 把文件落到 DIR 下（按 URL 末段命名）；缺省落 CWD（M4 历史行为）。
/// ponytail: `general::cmd_dl` 已对（同名 `output: Option<&Path>` + `dir.join(filename_from_url(url))`），
/// 此处只把 `postproc::dl` 改一致，不动 general。
pub async fn dl(
    browser: &Browser,
    results: &[SearchResult],
    n: usize,
    output: Option<&Path>,
) -> Result<()> {
    let url = pick(results, n, "dl")?;
    let page = browser.new_page("about:blank").await?;
    let _ = tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url)).await;
    let js = format!(
        "async function () {{
            const r = await fetch({}, {{credentials: 'include'}});
            const b = await r.arrayBuffer();
            const u8 = new Uint8Array(b);
            let s = '';
            for (const x of u8) s += String.fromCharCode(x);
            return btoa(s);
        }}",
        serde_json::to_string(url)?
    );
    let b64 = page
        .evaluate(js)
        .await
        .map_err(|e| anyhow!(
            "下载失败（{url}）：M4 同源 fetch 受 CORS 限制，未放行的站会在此报错，待 M6 升级为 Page download flow。原因: {e}"
        ))?
        .into_value::<String>()
        .map_err(|e| anyhow!("fetch 返回值非字符串（{url}）: {e}"))?;
    let bytes = b64_decode(&b64)?;
    if bytes.is_empty() {
        return Err(anyhow!("下载内容为空（{url}）"));
    }

    let dir = std::path::absolute(output.unwrap_or_else(|| Path::new(".")))?;
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("创建下载目录 {} 失败: {e}", dir.display()))?;
    let path = dir.join(filename_from_url(url));
    std::fs::write(&path, &bytes).map_err(|e| anyhow!("写文件 {} 失败: {e}", path.display()))?;
    if bytes.len() > DL_LARGE_BYTES {
        tracing::warn!(
            "下载文件 {} MB，fetch→base64 路径吃内存，考虑 M6 Page download flow",
            bytes.len() / 1_000_000
        );
    }
    println!("已下载: {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_bounds() {
        let r = [SearchResult {
            title: "t".into(),
            url: "u".into(),
            snippet: "s".into(),
        }];
        assert!(pick(&r, 0, "read").is_err());
        assert!(pick(&r, 2, "read").is_err());
        assert_eq!(pick(&r, 1, "read").unwrap(), "u");
        assert!(pick(&[], 1, "read").is_err());
    }

    /// PM 契约：postproc.rs tests 加 skeleton_extract_cases（h1/h2/h3 各提取 + 段数 + first_chars）。
    /// M9 升级为：直接验证 AdaptiveRead 三个核心行为（短文 / 中等 / 长文自适应）。
    /// 测试 HTML 通过 in-memory 字符串驱动，不启 Chrome → CI 安全。
    #[test]
    fn skeleton_extract_cases() {
        use gsearch::skeleton::extract_adaptive;
        let html = r#"<!doctype html><html><head><title>T</title></head><body>
<h1>One</h1>
<p>p1 alpha.</p>
<h2>Two</h2>
<p>p2 bravo charlie delta echo.</p>
<h3>Three</h3>
<p>p3.</p>
</body></html>"#;
        let r = extract_adaptive(html);
        assert_eq!(r.headings.len(), 3);
        assert_eq!(r.headings[0].level, 1);
        assert_eq!(r.headings[0].text, "One");
        assert_eq!(r.headings[1].level, 2);
        assert_eq!(r.headings[2].level, 3);
        assert_eq!(r.paragraph_index.len(), 3);
        assert!(r.paragraph_index[0].char_count > 0);
    }
}

#[cfg(test)]
mod live_tests {
    //! 免 Google 集成测试。
    //! 起真 Chrome 且独占 profile：并行会因 profile 锁竞态挂（ExitStatus(21)），
    //! 必须串行跑 `cargo test -- --test-threads=1`（CI 已 skip live，不受影响）。

    use super::*;

    fn fixture() -> Vec<SearchResult> {
        vec![SearchResult {
            title: "t".into(),
            url: "https://example.com/".into(),
            snippet: "s".into(),
        }]
    }

    /// 单一 async 测试独占 profile 锁，避免并行启两个 Chrome 抢锁。
    #[tokio::test]
    async fn postproc_live() {
        let (browser, handler) = gsearch::browser::launch(true).await.unwrap();
        let _h = gsearch::browser::spawn_handler(handler);
        let results = fixture();

        let err = read(&browser, &[], 1, &ReadOpts::default()).await.unwrap_err();
        assert!(err.to_string().contains("越界"), "got: {err}");

        let txt = read(&browser, &results, 1, &ReadOpts::default()).await.unwrap();
        assert!(txt.contains("[目录]"), "default read missing [目录]: {txt:?}");
        assert!(txt.contains("[摘要"), "default read missing [摘要]: {txt:?}");
        assert!(txt.contains("Example Domain"), "default read got: {txt:?}");

        let txt = read_full(&browser, &results, 1).await.unwrap();
        assert!(txt.contains("Example Domain"), "read_full got: {txt:?}");

        dl(&browser, &results, 1, None).await.unwrap();
        let bytes = std::fs::read("download.bin").unwrap();
        assert!(!bytes.is_empty());
        assert!(
            bytes.windows(14).any(|w| w == b"Example Domain"),
            "dl content head: {}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
        );
        std::fs::remove_file("download.bin").unwrap();
    }

    #[test]
    fn open_mechanism() {
        let st = std::process::Command::new("cmd")
            .args(["/c", "start", "", "https://example.com"])
            .status()
            .unwrap();
        assert!(st.success());
        assert!(open(&fixture(), 1).is_ok());
    }
}