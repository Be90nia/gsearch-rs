//! `fetch <url>`：纯 reqwest GET 取网页正文，全程零浏览器（issue gsearch-rs-fetch）。
//! 存在理由：没装 Chrome 的机器上也能秒读静态页——换机可用性缺口。
//! HTTP 层与 searxng.rs 同款 reqwest 客户端构建；正文提取手写轻量状态机
//! （剥 script/style/noscript + 实体解码），不做完整 DOM 解析、不引新 crate。
//!
//! 安全门（Important-1 / 2）：
//! - 默认拒绝私网（loopback / RFC1918 / link-local / IPv6 ::1 + fc00::/7）；
//!   放行通过 `--allow-private` 或 `GSEARCH_FETCH_ALLOW_PRIVATE=1`。
//! - 重定向强制 https-only（builder.https_only(true)），禁止 https→http 降级与跨 scheme 转 SSRF。

use std::net::{IpAddr, Ipv6Addr, ToSocketAddrs};
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
/// 响应体硬上限（Important-3）：超过此字节数立即停下载，避免 OOM/zip-bomb。
const FETCH_BODY_LIMIT: usize = 10 * 1024 * 1024;
/// `fetch <url>` 选项集（与 general::BrowseOpts 同风格）。
#[derive(Debug, Clone, Default)]
pub struct FetchOpts {
    pub json: bool,
    /// 显式代理（--proxy / GSEARCH_PROXY）；None = 跟随环境代理（面向公网，与 searxng 的 no_proxy 相反）。
    pub proxy: Option<String>,
    /// 放行私网（loopback / RFC1918 / link-local）。默认 false：SSRF 门。
    pub allow_private: bool,
}

/// scheme 前缀校验（大小写不敏感）。非 http(s) 一律拒（fetch 子命令的契约定位 = 互联网只读）。
fn http_or_https_scheme(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// 解析 host（字面 IP 直读；域名走 ToSocketAddrs 取首个解析结果）。
/// 返回 Ok(取到的第一个 IP)，Err = 解析失败或 host 为空。
fn resolve_host(host: &str) -> Result<IpAddr> {
    if host.is_empty() {
        return Err(anyhow!("URL host 为空"));
    }
    // 字面 IP：直接解析，避免 DNS 走系统解析器
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ip);
    }
    // 域名：用 ToSocketAddrs（带 80 端口仅占位，host 解析后即返回；端口不影响 IP 判定）
    let mut addrs = (host, 80u16)
        .to_socket_addrs()
        .with_context(|| format!("host 解析失败: {host}"))?;
    addrs.next().map(|sa| sa.ip()).ok_or_else(|| anyhow!("host 解析为空: {host}"))
}

/// SSRF 私网门（Important-1）：命中以下任一即拒绝。
/// - IPv4: 0.0.0.0/8、127.0.0.0/8、10.0.0.0/8、172.16.0.0/12、192.168.0.0/16、169.254.0.0/16
/// - IPv6: ::1、fc00::/7（ULA）、fe80::/10（link-local，含 IPv4-mapped 形态）
///
/// 169.254.169.254（云 metadata）落在 169.254.0.0/16 内自动覆盖。
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_unspecified()                  // 0.0.0.0
                || v4.is_loopback()               // 127.0.0.0/8
                || v4.is_private()                // 10/172.16/192.168
                || v4.is_link_local()             // 169.254/16
                || v4.is_multicast()
        }
        IpAddr::V6(v6) => {
            v6.is_unspecified()
                || v6.is_loopback()               // ::1
                || is_ula_v6(v6)                  // fc00::/7
                || v6.segments()[0] == 0xfe80      // fe80::/10
                || v6.is_multicast()
        }
    }
}

/// ULA (Unique Local Address): fc00::/7 = 首字节 0xfc 或 0xfd。
fn is_ula_v6(v6: Ipv6Addr) -> bool {
    let first = v6.segments()[0];
    (first & 0xfe00) == 0xfc00
}

/// 私网门：URL → host → IP → 私网判定；放行 = allow_private 或 GSEARCH_FETCH_ALLOW_PRIVATE=1。
fn gate_private(url: &str, allow_private: bool) -> Result<()> {
    if allow_private || std::env::var_os("GSEARCH_FETCH_ALLOW_PRIVATE").is_some() {
        return Ok(());
    }
    let scheme_end = url.find("://").ok_or_else(|| anyhow!("URL 无 scheme: {url}"))?;
    let after_scheme = &url[scheme_end + 3..];
    // host 提取：IPv6 字面量（[...]）vs 主机名（首个 '/?#:' 截断）
    let host = if let Some(rest) = after_scheme.strip_prefix('[') {
        // IPv6：到 ']' 止；'/' '?' '#' 若先于 ']' 出现则视为裸主机名
        let close = rest.find(']').ok_or_else(|| anyhow!("URL IPv6 host 未闭合: {url}"))?;
        &rest[..close]
    } else {
        // 普通 host：到首个 '/?#:'（端口分隔）止
        let host_end = after_scheme
            .find(['/', '?', '#', ':'])
            .unwrap_or(after_scheme.len());
        &after_scheme[..host_end]
    };
    let ip = resolve_host(host)?;
    if is_private_ip(ip) {
        return Err(anyhow!(
            "fetch 拒绝私网地址 {ip}（host={host}）。如确需内网，请传 --allow-private 或设置 GSEARCH_FETCH_ALLOW_PRIVATE=1"
        ));
    }
    Ok(())
}

/// 响应体累积（Important-3 字节上限）：把 chunk 接进 buf，超 limit 即截断到 limit 并报告 hit。
/// 纯函数（不碰 IO/异步），离线单测覆盖。
/// 返回 (新 buf, 是否撞上限)。撞上限时 buf.len() == limit；调用方后续停止拉流。
fn accumulate_chunk(mut buf: Vec<u8>, chunk: &[u8], limit: usize) -> (Vec<u8>, bool) {
    if buf.len() >= limit {
        return (buf, true);
    }
    let room = limit - buf.len();
    if chunk.len() > room {
        buf.extend_from_slice(&chunk[..room]);
        (buf, true)
    } else {
        buf.extend_from_slice(chunk);
        (buf, false)
    }
}

/// `gsearch fetch <url>`：GET → 轻量正文提取 → 人读 / --json 输出。
/// 退出码：0 成功；1 JS 壳（需渲染）/ 私网门拒（由 main 统一打印，HTTP 错误经 anyhow → exit 1）。
pub async fn cmd_fetch(url: &str, opts: &FetchOpts) -> Result<ExitCode> {
    let started = Instant::now();
    // 明文 http 在下方 https_only(true) 会被 reqwest 以 builder error 拒掉——报错难懂，
    // 前置拦截给出人话（含禁用理由）。
    if url.trim_start().to_ascii_lowercase().starts_with("http://") {
        return Err(anyhow!(
            "fetch 仅支持 https：明文 http 已禁用（防降级与重定向 SSRF 中转）。如确需内网 http 页面，请用 gsearch browse（需浏览器）: {url}"
        ));
    }
    if !http_or_https_scheme(url) {
        return Err(anyhow!("fetch 仅支持 http/https URL（拒绝: {url}）"));
    }
    gate_private(url, opts.allow_private)?;
    let mut builder = reqwest::Client::builder()
        .user_agent(format!("gsearch/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        // Important-2：禁止 https→http 降级，阻止跨 scheme 重定向中转 SSRF
        .https_only(true);
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

    // Important-3：bytes_stream 边读边累加，超过 FETCH_BODY_LIMIT 立即停下载（OOM/zip-bomb 同治）。
    let mut buf: Vec<u8> = Vec::new();
    let mut truncated = false;
    let mut stream = resp.bytes_stream();
    use futures::stream::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("读取响应失败: {url}"))?;
        let (new_buf, hit_limit) = accumulate_chunk(buf, &chunk, FETCH_BODY_LIMIT);
        buf = new_buf;
        if hit_limit {
            truncated = true;
            break;
        }
    }
    drop(stream);
    let html = String::from_utf8_lossy(&buf).into_owned();

    let limit = read_max_chars();
    let mut fetched = process_html(url, &html, is_html, limit);
    // 字节上限触发的截断：meta.truncated=true；omitted 仅作下界（实际丢多少未知，至少 FETCH_BODY_LIMIT 已读）
    if truncated {
        fetched.truncated = true;
        // 累计到已有 omitted 之上（区分两次截断：body 上限 vs cap_chars 正文字符上限）
        fetched.omitted = fetched.omitted.saturating_add(FETCH_BODY_LIMIT);
    }

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
/// 非 HTML（text/plain 等）不剥标签不判壳也不实体解码——markdown/JSON 源文保真，
/// 字面 `&amp;`/`&#20013;` 原样保留（agent 取原文场景）。
fn process_html(url: &str, html: &str, is_html: bool, limit: usize) -> Fetched {
    let title = if is_html { extract_title(html) } else { String::new() };
    let raw = if is_html {
        extract_text(html)
    } else {
        collapse_blank(html.to_string())
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

    /// Minor-b：非 HTML（text/plain / JSON / markdown 源文）不做实体解码——`&amp;` 原样保留。
    #[test]
    fn process_html_non_html_preserves_entities() {
        let raw = r#"{"name":"A&amp;B","cn":"&#20013;&#x6587;"}"#;
        let fetched = process_html("https://e.test/a.json", raw, false, 50_000);
        assert!(fetched.text.contains("A&amp;B"), "&amp; 必须保留: {}", fetched.text);
        assert!(fetched.text.contains("&#20013;"), "数字实体必须保留: {}", fetched.text);
        assert!(fetched.text.contains("&#x6587;"), "十六进制实体必须保留: {}", fetched.text);
        assert!(!fetched.text.contains('中'), "非 HTML 路径不应解码为中文字符");
    }

    /// Important-1：SSRF 私网门覆盖 IPv4 全谱 + IPv6 ULA/link-local/loopback + 云 metadata。
    #[test]
    fn ssrf_gate_rejects_private_addresses() {
        // 字面 IP：loopback / RFC1918 / link-local / 云 metadata / unspecified
        for bad in [
            "http://127.0.0.1/admin",
            "http://127.0.0.1:8080/x",
            "http://10.0.0.5/x",
            "http://172.16.0.1/x",
            "http://172.31.255.254/x",
            "http://192.168.1.1/admin",
            "http://169.254.169.254/latest/meta-data/",   // AWS / GCP metadata
            "http://0.0.0.0/x",
            "http://[::1]/admin",
            "http://[fc00::1]/x",
            "http://[fd00::1]/x",
            "http://[fe80::1]/x",
        ] {
            let err = gate_private(bad, false).unwrap_err();
            assert!(err.to_string().contains("拒绝"), "应拒绝 {bad}: {err}");
        }
        // 放行公网 IP 字面量（不发起请求，纯函数验证）
        for ok in [
            "http://8.8.8.8/x",
            "https://1.1.1.1/x",
            "https://93.184.216.34/x",
        ] {
            gate_private(ok, false).expect(ok);
        }
    }

    /// Important-1：allow_private=true 放行所有 IP（保留 SSRF 风险由用户承担）。
    #[test]
    fn ssrf_gate_allow_private_bypasses() {
        // 直接传字面 IP 不走 DNS；allow_private=true 全放行
        for url in [
            "http://127.0.0.1/x",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/admin",
        ] {
            gate_private(url, true).expect(url);
        }
    }

    /// Important-1：is_private_ip 覆盖各 IPv4 / IPv6 段（含 ::1 与 fc00::/7 边界）。
    #[test]
    fn is_private_ip_cases() {
        use std::net::Ipv4Addr;
        assert!(is_private_ip("0.0.0.0".parse::<Ipv4Addr>().unwrap().into()));
        assert!(is_private_ip("127.0.0.1".parse::<Ipv4Addr>().unwrap().into()));
        assert!(is_private_ip("10.0.0.1".parse::<Ipv4Addr>().unwrap().into()));
        assert!(is_private_ip("172.16.0.1".parse::<Ipv4Addr>().unwrap().into()));
        assert!(is_private_ip("172.31.255.254".parse::<Ipv4Addr>().unwrap().into()));
        assert!(!is_private_ip("172.32.0.1".parse::<Ipv4Addr>().unwrap().into()));
        assert!(is_private_ip("192.168.1.1".parse::<Ipv4Addr>().unwrap().into()));
        assert!(is_private_ip("169.254.169.254".parse::<Ipv4Addr>().unwrap().into()));
        assert!(!is_private_ip("8.8.8.8".parse::<Ipv4Addr>().unwrap().into()));
        // IPv6
        assert!(is_private_ip("::1".parse().unwrap()));
        assert!(is_private_ip("fc00::1".parse().unwrap()));
        assert!(is_private_ip("fd12:3456::1".parse().unwrap()));  // fd00::/8 = ULA
        assert!(is_private_ip("fe80::1".parse().unwrap()));
        assert!(!is_private_ip("2001:4860:4860::8888".parse().unwrap()));  // Google DNS IPv6
    }

    /// Important-1：非 http(s) scheme 同样拒绝（fetch 子命令定位 = 互联网只读）。
    #[test]
    fn http_or_https_scheme_rejects_other_schemes() {
        assert!(!http_or_https_scheme("file:///etc/passwd"));
        assert!(!http_or_https_scheme("javascript:alert(1)"));
        assert!(!http_or_https_scheme("ftp://example.com/"));
        assert!(http_or_https_scheme("http://example.com"));
        assert!(http_or_https_scheme("HTTPS://example.com"));  // 大小写不敏感
        assert!(http_or_https_scheme("  https://example.com"));  // 前导空白允许
    }

    /// Important-3：响应体硬上限——多次 chunk 累积到 limit 立即停下载。
    #[test]
    fn accumulate_chunk_caps_at_limit() {
        // 单 chunk 超 limit：截断到 limit 并报告 hit
        let (buf, hit) = accumulate_chunk(Vec::new(), &[0u8; 1024], 512);
        assert!(hit);
        assert_eq!(buf.len(), 512);

        // 多 chunk 累加：未超 limit 不 hit
        let (buf, hit) = accumulate_chunk(Vec::new(), b"hello", 100);
        assert!(!hit);
        assert_eq!(buf, b"hello");

        // 多 chunk 累加：最后一次 chunk 越过 limit 才 hit
        let (buf1, hit1) = accumulate_chunk(Vec::new(), b"AAAA", 10);
        assert!(!hit1);
        assert_eq!(buf1.len(), 4);
        let (buf2, hit2) = accumulate_chunk(buf1, b"BBBBBBBBB", 10);  // 4 + 9 = 13 > 10 → room=6
        assert!(hit2);
        assert_eq!(buf2.len(), 10);
        assert_eq!(&buf2[..4], b"AAAA");
        assert_eq!(&buf2[4..], b"BBBBBB");

        // 正好填满不算 hit（恰好等于 limit）
        let (buf, hit) = accumulate_chunk(Vec::new(), &[0u8; 10], 10);
        assert!(!hit);
        assert_eq!(buf.len(), 10);

        // 已满 buf 再喂任何 chunk 立即 hit（不再扩 buf）
        let full = vec![0u8; 10];
        let (buf, hit) = accumulate_chunk(full.clone(), b"more", 10);
        assert!(hit);
        assert_eq!(buf.len(), 10);
    }
}
