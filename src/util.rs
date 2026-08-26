//! 跨模块复用的小工具（M8 抽取）：原本 3 份 filename_from_url + 2 份 b64_decode 散落各处。

use anyhow::{Result, anyhow};

/// 文件名 = URL 路径最后一段（去 query/hash、剥 scheme://authority）；为空（裸 origin/尾斜杠）则 download.bin。
/// 原本 postproc.rs / general.rs / shell.rs 各一份（M4/M6/M7 各加的），现在统一。
pub fn filename_from_url(url: &str) -> String {
    let path = url.split(['#', '?']).next().unwrap_or(url);
    let path = match path.find("://") {
        Some(i) => path[i + 3..].find('/').map_or("", |j| &path[i + 3 + j..]),
        None => path,
    };
    let last = path.rsplit('/').next().unwrap_or("");
    if last.is_empty() {
        "download.bin".into()
    } else {
        last.into()
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
}