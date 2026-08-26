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
}