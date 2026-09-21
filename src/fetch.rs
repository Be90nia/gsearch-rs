//! `fetch <url>`：纯 reqwest GET 取网页正文，全程零浏览器（issue gsearch-rs-fetch）。
//! 存在理由：没装 Chrome 的机器上也能秒读静态页——换机可用性缺口。
//! HTTP 层与 searxng.rs 同款 reqwest 客户端构建；正文提取手写轻量状态机
//! （剥 script/style/noscript + 实体解码），不做完整 DOM 解析、不引新 crate。

use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};

use crate::postproc::{cap_chars, read_max_chars};

/// fetch 总超时（含 redirect 链）；纯 HTTP 无渲染，10s 足够。
const FETCH_TIMEOUT_SECS: u64 = 10;
/// JS 壳判定阈值：剥标签后正文低于此字符数 → 大概率是渲染型页面。
const SHELL_MIN_CHARS: usize = 500;
/// redirect 跟随上限（reqwest 默认同款，显式声明防歧义）。
const MAX_REDIRECTS: usize = 10;

/// `fetch <url>` 选项集（与 general::BrowseOpts 同风格）。
#[derive(Debug, Clone, Default)]
pub struct FetchOpts {
    pub json: bool,
    /// 显式代理（--proxy / GSEARCH_PROXY）；None = 跟随环境代理（面向公网，与 searxng 的 no_proxy 相反）。
    pub proxy: Option<String>,
}

/// `gsearch fetch <url>`：GET → 轻量正文提取 → 人读 / --json 输出。
/// 退出码：0 成功；1 JS 壳（需渲染）；HTTP 错误经 anyhow → main 统一打印退出 1。
pub async fn cmd_fetch(url: &str, opts: &FetchOpts) -> Result<ExitCode> {
    let started = Instant::now();
    let mut builder = reqwest::Client::builder()
        .user_agent(format!("gsearch/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS));
    if let Some(p) = &opts.proxy {
        builder = builder.proxy(reqwest::Proxy::all(p).context("代理 URL 无效")?);
    }
    let client = builder.build().context("构建 HTTP 客户端失败")?;

    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("请求失败: {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        // 验收契约：非零退出 + 错误信息含状态码（error 经 main 统一打印，退出码 1）
        return Err(anyhow!("HTTP {status}: {url}"));
    }
    let is_html = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.to_ascii_lowercase().contains("html"))
        .unwrap_or(false);
    let html = resp.text().await.with_context(|| format!("读取响应失败: {url}"))?;

    let limit = read_max_chars();
    let fetched = process_html(url, &html, is_html, limit);

    if is_html && looks_like_js_shell(&fetched.text, &html) {
        eprintln!("该页无服务端正文（JS 壳），需渲染：用 gsearch browse {url}");
        return Ok(ExitCode::from(1));
    }

    if opts.json {
        // 与 read/browse 同契约：载荷 + meta{truncated, omitted, content_untrusted:true}
        let v = serde_json::json!({
            "url": fetched.url,
            "title": fetched.title,
            "text": fetched.text,
            "meta": {
                "truncated": fetched.truncated,
                "omitted": fetched.omitted,
                "content_untrusted": true,
            },
        });
        println!("{v}");
    } else {
        if fetched.truncated {
            eprintln!("注意：正文超上限已截断（省略 {} 字符；--json 输出在 meta 字段标注）", fetched.omitted);
        }
        println!("=== {} | {} ===\n{}", fetched.url, fetched.title, fetched.text);
    }
    tracing::debug!("fetch 完成: {url} ({}ms)", started.elapsed().as_millis());
    Ok(ExitCode::SUCCESS)
}

/// 提取产物（url 原样带回，方便 --json 消费方对账）。
struct Fetched {
    url: String,
    title: String,
    text: String,
    truncated: bool,
    omitted: usize,
}

/// 纯函数：html → 提取 + 截断（limit 注入，离线单测不碰配置）。
/// 非 HTML（text/plain 等）不剥标签不判壳——markdown 源码里的 `Vec<u8>` 不是标签。
fn process_html(url: &str, html: &str, is_html: bool, limit: usize) -> Fetched {
    let title = if is_html { extract_title(html) } else { String::new() };
    let raw = if is_html {
        extract_text(html)
    } else {
        collapse_blank(decode_entities(html))
    };
    let (text, truncated, omitted) = cap_chars(&raw, limit);
    Fetched { url: url.to_string(), title, text, truncated, omitted }
}

/// JS 壳判定：正文 < 500 字符 **且** html 含 SPA 挂载点标记（root/app/__next 等）。
/// 双条件缺一不可：合法的小静态页（example.com 类）短但无挂载点，不判壳（压测抓到的假阳性）。
fn looks_like_js_shell(text: &str, html: &str) -> bool {
    if text.trim().chars().count() >= SHELL_MIN_CHARS {
        return false;
    }
    let lower = html.to_ascii_lowercase();
    ["id=\"root\"", "id=\"app\"", "id=root", "id=app", "__next"]
        .iter()
        .any(|m| lower.contains(m))
}

/// 轻量正文提取：script/style/noscript/template 连内容删除；注释删除；
/// 其余标签剥壳（块级边界 → 换行，行内 → 空格）；实体解码；空白规整。
fn extract_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..lt]));
        let after = &rest[lt..];
        if let Some(body) = after.strip_prefix("<!--") {
            // 注释：跳到 -->；未闭合则后面全是注释
            match body.find("-->") {
                Some(end) => rest = &body[end + 3..],
                None => return collapse_blank(out),
            }
            continue;
        }
        let Some(gt) = after.find('>') else {
            // 未闭合标签：丢弃尾巴
            break;
        };
        let tag = &after[1..gt];
        rest = &after[gt + 1..];
        let name = tag_name(tag);
        if !tag.starts_with('/') && matches!(name.as_str(), "script" | "style" | "noscript" | "template") {
            // 整块内容删除：跳到对应闭合标签（rest 停在 </xxx 处，下一轮当普通标签处理）
            let close = format!("</{name}");
            match rest.to_ascii_lowercase().find(&close) {
                Some(p) => rest = &rest[p..],
                None => return collapse_blank(out),
            }
        } else if is_block_boundary(&name) {
            out.push('\n');
        } else {
            out.push(' ');
        }
    }
    out.push_str(&decode_entities(rest));
    collapse_blank(out)
}

/// 标签名：剥可能的闭合斜杠，取前导 ASCII 字母数字，小写化。
fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// 块级边界标签 → 换行（开闭都算，连续换行由 collapse_blank 压平）。
fn is_block_boundary(name: &str) -> bool {
    matches!(
        name,
        "br" | "p" | "div" | "li" | "tr" | "td" | "th" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            | "section" | "article" | "header" | "footer" | "table" | "ul" | "ol" | "blockquote" | "pre"
    )
}

/// 空白规整：行内连续空白 → 单空格；换行 → 单换行；去首尾。
fn collapse_blank(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_nl = false;
    let mut pending_sp = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if c == '\n' {
                pending_nl = true;
            } else {
                pending_sp = true;
            }
        } else {
            if pending_nl {
                if !out.is_empty() {
                    out.push('\n');
                }
            } else if pending_sp && !out.is_empty() {
                out.push(' ');
            }
            // pending 统一在此消费/丢弃（前导空白不留存，防泄漏到后续字符）
            pending_nl = false;
            pending_sp = false;
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// `<title>` 文本（实体解码 + trim）；无 title 标签 → 空串。定位用小写副本，取值用原文。
fn extract_title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(start) = lower.find("<title") else { return String::new() };
    let Some(gt) = lower[start..].find('>') else { return String::new() };
    let body_from = start + gt + 1;
    let Some(end) = lower[body_from..].find("</title") else { return String::new() };
    decode_entities(html[body_from..body_from + end].trim())
}

/// 常见命名实体 + 十/十六进制数字实体解码；未知实体原样保留（不破坏正文）。
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        // 实体最长 &#x10FFFF;：分号超出 10 字符即按普通文本
        let semi = match after.find(';') {
            Some(p) if p <= 10 => p,
            _ => {
                out.push('&');
                rest = &after[1..];
                continue;
            }
        };
        let ent = &after[1..semi];
        let decoded = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => ent
                .strip_prefix('#')
                .and_then(|num| match num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => num.parse::<u32>().ok(),
                })
                .and_then(char::from_u32)
                // 控制字符（&#0; 等）不落正文
                .filter(|c| !c.is_control()),
        };
        match decoded {
            Some(c) => out.push(c),
            None => out.push_str(&after[..semi + 1]),
        }
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// extract_text：script/style 连内容删除、标签剥壳、块级换行、空白规整。
    #[test]
    fn extract_text_strips_and_keeps_body() {
        let html = "<html><head><style>.x{color:red}</style>\
                    <script>var a = '<div>not text</div>';</script></head>\
                    <body><h1>标题</h1><p>第一段 &amp; 符号</p>\
                    <span>行内</span><noscript>无 JS 提示</noscript></body></html>";
        let text = extract_text(html);
        assert!(text.contains("标题"), "got: {text}");
        assert!(text.contains("第一段 & 符号"), "got: {text}");
        assert!(text.contains("行内"), "got: {text}");
        assert!(!text.contains("color:red"), "style 内容应删除: {text}");
        assert!(!text.contains("var a"), "script 内容应删除: {text}");
        assert!(!text.contains("无 JS 提示"), "noscript 内容应删除: {text}");
        assert!(!text.contains('<'), "不应残留标签: {text}");
    }

    /// extract_text：注释删除 + 大写标签名 + 未闭合 script 兜底（删到结尾不 panic）。
    #[test]
    fn extract_text_comments_and_case() {
        let text = extract_text("<!-- 注释 <b>不在正文</b> --><P>段落</P>");
        assert_eq!(text, "段落");
        let text = extract_text("<SCRIPT>未闭合一直删</SCRIPT>保底");
        assert_eq!(text, "保底");
        // 回归：标签剥壳产生的前导空白不得泄漏到正文中间（collapse_blank pending 泄漏 bug）
        let text = extract_text("<i> </i>前后不留痕");
        assert_eq!(text, "前后不留痕");
        let text = extract_text("<script>根本没闭合");
        assert_eq!(text, "");
    }

    /// decode_entities：命名实体 + 数字实体 + 未知实体原样 + 控制字符过滤。
    #[test]
    fn decode_entities_cases() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt; &quot;q&quot; &apos;s&apos;"), "a & b <c> \"q\" 's'");
        assert_eq!(decode_entities("&nbsp;X"), "\u{a0}X");
        assert_eq!(decode_entities("&#20013;&#x6587;"), "中文");
        assert_eq!(decode_entities("&unknown; &notanentity"), "&unknown; &notanentity");
        // 控制字符实体：解码失败 → 原样保留实体文本（不伪装丢内容）
        assert_eq!(decode_entities("&#0;&#x1F;"), "&#0;&#x1F;");
        // 无实体快路径
        assert_eq!(decode_entities("plain"), "plain");
    }

    /// looks_like_js_shell：短正文 + SPA 挂载点才判 true；小静态页（example.com 类）不判壳。
    #[test]
    fn js_shell_detection() {
        let spa = "<html><body><div id=\"root\"></div><script src=\"app.js\"></script>\
                   <noscript>需要 JS</noscript></body></html>";
        let fetched = process_html("https://e.test/", spa, true, 50_000);
        assert!(looks_like_js_shell(&fetched.text, spa));

        // 短正文但无挂载点 = 合法小静态页（example.com 形态），不判壳（压测回归）
        let tiny_static = "<html><head><title>Example</title></head><body><div>\
                           <h1>Example Domain</h1><p>This domain is for use in illustrative examples.</p>\
                           </div></body></html>";
        let tiny = process_html("https://e.test/", tiny_static, true, 50_000);
        assert!(!looks_like_js_shell(&tiny.text, tiny_static));

        let body = "正".repeat(600);
        assert!(!looks_like_js_shell(&body, spa));
        assert!(looks_like_js_shell(&"正".repeat(499), "<div id=\"app\"></div>"));
        assert!(!looks_like_js_shell(&"正".repeat(500), "<div id=\"app\"></div>"));
        // 短正文 + __next（Next.js）也判壳
        assert!(looks_like_js_shell("", "<div id=\"__next\"></div>"));
    }

    /// process_html：截断逻辑（limit 注入）+ 非 HTML 不剥标签（markdown 源码的 `Vec<u8>` 不是标签）。
    #[test]
    fn process_html_truncates_and_respects_plain_text() {
        let fetched = process_html("https://e.test/", &"x".repeat(100), false, 30);
        assert!(fetched.truncated && fetched.omitted == 70);
        assert_eq!(fetched.text.chars().count(), 30);

        let md = "# Title\nRust Vec<u8> keeps angle brackets";
        let fetched = process_html("https://e.test/a.md", md, false, 50_000);
        assert!(!fetched.truncated);
        assert!(fetched.text.contains("Vec<u8>"), "纯文本不应剥标签: {}", fetched.text);
    }
}
