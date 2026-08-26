//! M4 搜索结果后处理：`--open N` / `--read N` / `--dl N`（PLAN §3.4 / §3.5）。
//! 复用 cmd_search 已启动的同一 browser 实例（同 profile → 同 cookie），不另起 Chrome。

use std::time::Duration;

use anyhow::{Result, anyhow};
use chromiumoxide::browser::Browser;

use gsearch::search::is_captcha;
use gsearch::types::SearchResult;

const READ_MAX_CHARS: usize = 5000;
const PAGE_TIMEOUT_SECS: u64 = 30;
/// 提醒阈值：同源 fetch→base64 全程在 JS 字符串里搬运，几十 MB 起内存/CPU 吃紧
const DL_LARGE_BYTES: usize = 50_000_000;

/// `--{flag} N` 下标校验：1-based；0 或超出结果数报错（含结果数为 0 的情况）
fn pick<'a>(results: &'a [SearchResult], n: usize, flag: &str) -> Result<&'a str> {
    if n == 0 || n > results.len() {
        return Err(anyhow!("--{flag} {n} 越界（结果数 {}）", results.len()));
    }
    Ok(&results[n - 1].url)
}

/// `--open N`：默认浏览器开窗。PLAN §3.4：`cmd /c start "" url`（空标题参数防 url 被当窗口名）。
/// spawn 失败只 warn——不该阻挡已打印的搜索结果。
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

/// `--read N`：同一 browser 新开页签 goto → `document.body.innerText` 截 5000 字打印（PLAN §3.4）。
/// 返回截断后的正文（集成测试断言用；CLI 路径丢弃）。
pub async fn read(browser: &Browser, results: &[SearchResult], n: usize) -> Result<String> {
    let url = pick(results, n, "read")?;
    let page = browser.new_page("about:blank").await?;
    tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
        .await
        .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
        .map_err(|e| anyhow!("goto {url} 失败（浏览器被手关？）: {e}"))?;

    if is_captcha(&page.content().await.unwrap_or_default()) {
        // ponytail: M4 后处理不接 CAPTCHA 人解钩子（搜索正常时结果页极少撞码），撞到直接报错；
        // 需要时把 search.rs 的 poll_until_solved 提为 pub 复用（M6 范围）。
        return Err(anyhow!("{url} 遇 CAPTCHA，M4 后处理不支持人解，请重试或手动浏览器打开"));
    }

    let txt = page
        .evaluate("document.body.innerText")
        .await?
        .into_value::<String>()
        .unwrap_or_default();
    let txt: String = txt.chars().take(READ_MAX_CHARS).collect();
    println!("=== {url} ===\n{txt}");
    Ok(txt)
}

/// `--dl N`：走浏览器 cookie 的简化下载（PLAN §3.5 dl 的 M4/M6 最小交集）。
/// 先 goto 目标页建立同源环境，再页面内 `fetch(credentials:'include')` 取字节 → base64 回传落盘。
pub async fn dl(browser: &Browser, results: &[SearchResult], n: usize) -> Result<()> {
    let url = pick(results, n, "dl")?;
    let page = browser.new_page("about:blank").await?;
    // 直接文件 URL（PDF/zip 等）的 goto 可能因触发下载导航被 abort——不致命，同源 fetch 仍可尝试
    let _ = tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url)).await;
    // url 来自 SERP href，已 percent-encode，单引号注入基本不可能（ponytail：不额外转义）
    // 注意：必须用 `async function ()` 而非 `async () =>`——chromiumoxide 的函数探测
    // is_likely_js_function 不认 async 箭头（skip_args 只匹配原串 '(' 开头），会把箭头函数
    // 当 Expression 求值 → 返回函数对象被序列化成 {} → into_value::<String> 报 invalid type: map
    let js = format!(
        "async function () {{
            const r = await fetch('{url}', {{credentials: 'include'}});
            const b = await r.arrayBuffer();
            const u8 = new Uint8Array(b);
            let s = '';
            for (const x of u8) s += String.fromCharCode(x);
            return btoa(s);
        }}"
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

    let filename = filename_from_url(url);
    std::fs::write(&filename, &bytes).map_err(|e| anyhow!("写文件 {filename} 失败: {e}"))?;
    if bytes.len() > DL_LARGE_BYTES {
        tracing::warn!(
            "下载文件 {} MB，fetch→base64 路径吃内存，考虑 M6 Page download flow",
            bytes.len() / 1_000_000
        );
    }
    println!("已下载: {filename} ({} bytes)", bytes.len());
    Ok(())
}

/// 手写标准 base64 解码：输入来自页面 `btoa()`（标准字母表 + '=' padding，无空白）。
/// PLAN §1 依赖表无 base64 crate，这 20 行不值得破表加依赖。
fn b64_decode(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in s.chars() {
        if c == '=' {
            break;
        }
        let v = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return Err(anyhow!("base64 非法字符 {c:?}")),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

/// 文件名 = URL 路径最后一段（去 query/hash、剥 scheme://authority）；为空（裸 origin/尾斜杠）则 download.bin（PLAN §3.4）。
fn filename_from_url(url: &str) -> String {
    let path = url.split(['#', '?']).next().unwrap_or(url);
    let path = match path.find("://") {
        Some(i) => path[i + 3..].find('/').map_or("", |j| &path[i + 3 + j..]),
        None => path,
    };
    let last = path.rsplit('/').next().unwrap_or("");
    if last.is_empty() {
        "download.bin".to_string()
    } else {
        last.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_decode_known_vectors() {
        assert_eq!(b64_decode("").unwrap(), b"");
        assert_eq!(b64_decode("QQ==").unwrap(), b"A");
        assert_eq!(b64_decode("QUJD").unwrap(), b"ABC");
        assert_eq!(b64_decode("SGVsbG8sIFdvcmxkIQ==").unwrap(), b"Hello, World!");
        assert_eq!(b64_decode("/w==").unwrap(), vec![0xff]);
        assert!(b64_decode("aGk*").is_err());
    }

    #[test]
    fn filename_from_url_cases() {
        assert_eq!(filename_from_url("https://x.com/a/b/file.pdf?x=1#frag"), "file.pdf");
        assert_eq!(filename_from_url("https://x.com/"), "download.bin");
        assert_eq!(filename_from_url("https://x.com/a/"), "download.bin");
        assert_eq!(filename_from_url("https://x.com"), "download.bin");
    }

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
}

#[cfg(test)]
mod live_tests {
    //! 免 Google 集成测试（PM 决策 2026-08-26：IP 被 Google 临时封禁不可控，端到端留 PM S7 亲跑）。
    //! 真 Chrome + https://example.com 直测 postproc 三函数。

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

        // 越界：空结果集 read(1) → Err 含 "越界"
        let err = read(&browser, &[], 1).await.unwrap_err();
        assert!(err.to_string().contains("越界"), "got: {err}");

        // read：innerText 含 "Example Domain"
        let txt = read(&browser, &results, 1).await.unwrap();
        assert!(txt.contains("Example Domain"), "read got: {txt:?}");

        // dl：同源 fetch → base64 → 落盘 size>0 且内容正确（根路径 → download.bin）
        dl(&browser, &results, 1).await.unwrap();
        let bytes = std::fs::read("download.bin").unwrap();
        assert!(!bytes.is_empty());
        assert!(
            bytes.windows(14).any(|w| w == b"Example Domain"),
            "dl content head: {}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
        );
        std::fs::remove_file("download.bin").unwrap();
    }

    /// open：cmd /c start 机制 exit 0 + postproc::open 不报错（会弹一个默认浏览器窗口，PM 授权）
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
