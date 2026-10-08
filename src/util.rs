//! 跨模块复用的小工具（M8 抽取）：原本 3 份 filename_from_url + 2 份 b64_decode 散落各处。

use anyhow::{Result, anyhow};

/// 文件名 = URL 路径最后一段（去 query/hash、剥 scheme://authority）；为空（裸 origin/尾斜杠）则 download.bin。
///
/// 安全门（I9，cmd_dl -o 路径安全）：URL 派生的文件名是潜在注入面——
/// agent 流程里 URL 可能来自搜索/抓取结果，若未过滤：
///   - 路径分隔符 `/` `\` → 写到 dir 外
///   - Windows 保留字符 `: * ? " < > |` + 设备名 `CON/PRN/AUX/NUL/COM1-9/LPT1-9` → 写入失败或越权
///   - `..` 段 → 路径穿越
///   - 控制字符 / 过长 → 文件系统拒绝 / 静默截断
///
/// 策略：把可疑字符替换为 `_`，整段为 `..`/纯分隔/过长（>200 字节）则落到 `download.bin`。
/// 原本 postproc.rs / general.rs / shell.rs 各一份（M4/M6/M7 各加的），现在统一。
pub fn filename_from_url(url: &str) -> String {
    let path = url.split(['#', '?']).next().unwrap_or(url);
    let path = match path.find("://") {
        Some(i) => path[i + 3..].find('/').map_or("", |j| &path[i + 3 + j..]),
        None => path,
    };
    let last = path.rsplit('/').next().unwrap_or("");
    if last.is_empty() {
        return "download.bin".into();
    }
    let safe = sanitize_filename(last);
    if safe.is_empty() || safe == ".." {
        "download.bin".into()
    } else {
        safe
    }
}

/// I9：URL 末段字符过滤——禁路径分隔符 / Win 保留字符 / `..`，禁控制字符，封顶 200 字节。
/// 不引依赖（PLAN §1）；这函数可能受 Windows 设备名影响单测覆盖。
fn sanitize_filename(raw: &str) -> String {
    // Win 保留字符 + 路径分隔符 + 控制字符 → '_'
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_control()
                || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    // 折叠连续 '_'（防 '////' 这种 URL 经 raw 末段解析后被过滤成 '____'）
    let mut collapsed = String::with_capacity(cleaned.len());
    let mut prev_us = false;
    for c in cleaned.chars() {
        if c == '_' && prev_us {
            continue;
        }
        prev_us = c == '_';
        collapsed.push(c);
    }
    // 截首尾空白与 '.'（Windows 拒绝末尾 '.')；超 200 字节截断（按字符边界）
    let trimmed = collapsed.trim_matches(|c: char| c.is_whitespace() || c == '.');
    if trimmed.is_empty() || trimmed == ".." {
        return String::new();
    }
    if trimmed.chars().count() > 200 {
        trimmed.chars().take(200).collect()
    } else {
        trimmed.to_string()
    }
}

/// 手写标准 base64 解码：输入来自页面 `btoa()`（标准字母表 + '=' padding，无空白）。
/// PLAN §1 依赖表无 base64 crate，这 20 行不值得破表加依赖。
/// postproc.rs / shell.rs 同款逻辑（原 M4 写、M7 抄过来），合并一处。
pub fn b64_decode(s: &str) -> Result<Vec<u8>> {
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

/// nw4：SERP 来源类型标注——纯 host/后缀启发式，零网络请求、保守不猜（未命中一律 other）。
/// 值域即 JSON `domain_class` 字段：docs/github/wikipedia/blog/forum/video/news/qa/other。
pub fn domain_class(url: &str) -> &'static str {
    // host 提取：剥 scheme → 截到第一个 / ? # → 去端口 → 去 www. → 小写
    let rest = match url.find("://") {
        Some(i) => &url[i + 3..],
        None => url,
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.split(':').next().unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    let host = host.to_ascii_lowercase();
    // site 匹配 = host 全等，或 `site` 前一位是 `.`（子域覆盖 en.wikipedia.org / old.reddit.com）
    fn host_matches(host: &str, site: &str) -> bool {
        host == site
            || (host.len() > site.len()
                && host.ends_with(site)
                && host.as_bytes()[host.len() - site.len() - 1] == b'.')
    }
    if host_matches(&host, "github.com") {
        return "github";
    }
    if host_matches(&host, "wikipedia.org") || host_matches(&host, "wikidata.org") {
        return "wikipedia";
    }
    // docs：docs.rs、doc./docs. 前缀（docs.python.org / doc.rust-lang.org）、readthedocs、MDN、MS Learn
    if host == "docs.rs"
        || host.starts_with("doc.")
        || host_matches(&host, "readthedocs.io")
        || host == "developer.mozilla.org"
        || host == "learn.microsoft.com"
    {
        return "docs";
    }
    // qa：Stack 全家 + Quora / 知乎
    if ["stackoverflow.com", "stackexchange.com", "superuser.com", "serverfault.com", "askubuntu.com", "quora.com", "zhihu.com"]
        .iter()
        .any(|s| host_matches(&host, s))
    {
        return "qa";
    }
    // video
    if ["youtube.com", "youtu.be", "vimeo.com", "bilibili.com"]
        .iter()
        .any(|s| host_matches(&host, s))
    {
        return "video";
    }
    // forum：Reddit / HN / V2EX
    if host == "news.ycombinator.com"
        || ["reddit.com", "v2ex.com"].iter().any(|s| host_matches(&host, s))
    {
        return "forum";
    }
    // news
    if ["nytimes.com", "bbc.com", "bbc.co.uk", "reuters.com", "theguardian.com", "cnn.com", "cnbc.com", "bloomberg.com", "wsj.com", "theverge.com", "arstechnica.com", "36kr.com"]
        .iter()
        .any(|s| host_matches(&host, s))
    {
        return "news";
    }
    // blog：托管平台 + 个人博客（github.io 是个人博客；与 github.com 官方仓库页不冲突）
    if host_matches(&host, "medium.com")
        || host_matches(&host, "dev.to")
        || host_matches(&host, "habr.com")
        || ["substack.com", "github.io", "wordpress.com", "blogspot.com"]
            .iter()
            .any(|s| host_matches(&host, s))
    {
        return "blog";
    }
    "other"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn b64_decode_known_vectors() {
        assert_eq!(b64_decode("").unwrap(), b"");
        assert_eq!(b64_decode("QQ==").unwrap(), b"A");
        assert_eq!(b64_decode("QUJD").unwrap(), b"ABC");
        assert_eq!(b64_decode("SGVsbG8sIFdvcmxkIQ==").unwrap(), b"Hello, World!");
        assert_eq!(b64_decode("/w==").unwrap(), vec![0xff]);
    }

    #[test]
    fn filename_from_url_cases() {
        assert_eq!(filename_from_url("https://x.com/a/b/file.pdf?x=1#f"), "file.pdf");
        assert_eq!(filename_from_url("https://example.com/"), "download.bin");
        assert_eq!(filename_from_url("https://example.com"), "download.bin");
        assert_eq!(filename_from_url("https://example.com/index.html"), "index.html");
    }

    /// nw4：分类器——host 后缀匹配、www/端口/大小写归一、未命中 other。
    #[test]
    fn domain_class_cases() {
        assert_eq!(domain_class("https://github.com/tokio-rs/tokio"), "github");
        assert_eq!(domain_class("https://gist.github.com/x/1"), "github");
        assert_eq!(domain_class("https://en.wikipedia.org/wiki/Rust"), "wikipedia");
        assert_eq!(domain_class("https://docs.rs/tokio/latest/tokio/"), "docs");
        assert_eq!(domain_class("https://doc.rust-lang.org/book/"), "docs");
        assert_eq!(domain_class("https://tokio.readthedocs.io/en/latest/"), "docs");
        assert_eq!(domain_class("https://stackoverflow.com/q/123"), "qa");
        assert_eq!(domain_class("https://rust-lang.stackexchange.com/q/1"), "qa");
        assert_eq!(domain_class("https://www.youtube.com/watch?v=1"), "video");
        assert_eq!(domain_class("https://youtu.be/abc"), "video");
        assert_eq!(domain_class("https://old.reddit.com/r/rust/"), "forum");
        assert_eq!(domain_class("https://news.ycombinator.com/item?id=1"), "forum");
        assert_eq!(domain_class("https://www.nytimes.com/2026/10/08/x.html"), "news");
        assert_eq!(domain_class("https://alice.github.io/post/"), "blog");
        assert_eq!(domain_class("https://medium.com/@x/post"), "blog");
        // 归一与边界
        assert_eq!(domain_class("https://WWW.GitHub.COM/x"), "github");
        assert_eq!(domain_class("https://192.168.89.249:8888/search?q=x"), "other");
        // 近似域名不误标：notgithub.io ≠ github.io，dockerns ≠ docs
        assert_eq!(domain_class("https://notgithub.io/post/"), "other");
        assert_eq!(domain_class("https://docker.com/"), "other");
        assert_eq!(domain_class("https://example.com/foo"), "other");
        assert_eq!(domain_class("not a url"), "other");
    }

    /// I9：URL 派生的文件名安全门——禁路径分隔符、Win 保留字符、路径穿越 `..`、控制字符。
    /// 这层过滤是 cmd_dl 的最后一道防线，文件名直接进 std::fs::write。
    #[test]
    fn filename_from_url_sanitizes_unsafe_chars() {
        // 路径分隔符：被滤为 _，结果不含 / \
        assert!(!filename_from_url("https://x.com/a/../../../etc/passwd").contains('/'));
        assert!(!filename_from_url("https://x.com/a\\..\\windows\\system32").contains('\\'));
        // Win 保留字符：: * ? " < > | 全部 _ 替
        let bad = filename_from_url("https://x.com/a:b*c?d\"e<f>g|h.bin");
        assert!(!bad.contains([':', '*', '?', '"', '<', '>', '|']));
        // 整段 = ".." 或纯分隔 → download.bin
        assert_eq!(filename_from_url("https://x.com/a/.."), "download.bin");
        assert_eq!(filename_from_url("https://x.com/a/../.."), "download.bin");
        // 控制字符（0x00/0x01/0x1f）→ _，不出文件名
        let ctrl = filename_from_url("https://x.com/a/file\u{0000}\u{0001}\u{001f}name.txt");
        assert!(!ctrl.chars().any(|c| c.is_control()));
        // 长末段 → 200 字符封顶
        let long = "a".repeat(500);
        let capped = filename_from_url(&format!("https://x.com/b/{long}"));
        assert!(capped.chars().count() <= 200);
        // 正常 case 不变
        assert_eq!(filename_from_url("https://x.com/release notes v2.zip"), "release notes v2.zip");
    }

    /// M13 三处下载路径一致性基线：filename_from_url + Path::join + std::fs::write
    /// 与 postproc::dl / shell::dl_in_page / general::cmd_dl 都走相同 shape。
    fn fresh_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gsearch-dl-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn cleanup(p: &PathBuf) {
        let _ = fs::remove_dir_all(p);
    }

    #[test]
    fn dl_join_root_url_picks_download_bin() {
        let dir = fresh_dir("root");
        let url = "https://example.com/";
        let path = dir.join(filename_from_url(url));
        assert_eq!(path, dir.join("download.bin"));
        fs::write(&path, b"x").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"x");
        cleanup(&dir);
    }
    #[test]
    fn dl_join_relative_output_dir() {
        let dir = fresh_dir("rel");
        let url = "https://x.com/a/b/file.pdf";
        let path = dir.join(filename_from_url(url));
        assert_eq!(path, dir.join("file.pdf"));
        fs::write(&path, b"x").unwrap();
        cleanup(&dir);
    }
    #[test]
    fn dl_join_path_with_spaces() {
        let dir = fresh_dir("space subdir");
        assert!(dir.to_string_lossy().contains(' '), "dir 自身含空格才能验");
        let url = "https://cdn.example.com/release notes v2.zip";
        let filename = filename_from_url(url);
        assert_eq!(filename, "release notes v2.zip", "URL 末段天然支持空格");
        let path = dir.join(&filename);
        assert!(path.to_string_lossy().contains(' '));
        fs::write(&path, b"x").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), 1);
        cleanup(&dir);
    }
    #[test]
    fn dl_join_overwrites_existing_same_name() {
        let dir = fresh_dir("overwrite");
        let url = "https://example.com/file.pdf";
        let path = dir.join(filename_from_url(url));
        fs::write(&path, b"old").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"old");
        fs::write(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        cleanup(&dir);
    }

    /// I5 回归：1MB 伪随机字节经 base64 encode→b64_decode roundtrip 无损。
    /// 覆盖 u32 累积器在大输入下的溢出/截断；编码器与 JS `btoa()` 同字母表（标准 + '=' padding）。
    #[test]
    fn b64_decode_roundtrip_1mb() {
        // xorshift64 确定性伪随机，不引 rand 依赖
        let mut seed: u64 = 0x243F_6A88_85A3_08D3;
        let raw: Vec<u8> = (0..1_000_000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect();
        const TBL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut enc = String::with_capacity(raw.len().div_ceil(3) * 4);
        for chunk in raw.chunks(3) {
            let n = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            enc.push(TBL[(n >> 18) as usize] as char);
            enc.push(TBL[(n >> 12 & 0x3f) as usize] as char);
            enc.push(if chunk.len() > 1 { TBL[(n >> 6 & 0x3f) as usize] as char } else { '=' });
            enc.push(if chunk.len() > 2 { TBL[(n & 0x3f) as usize] as char } else { '=' });
        }
        assert_eq!(b64_decode(&enc).unwrap(), raw);
    }

}