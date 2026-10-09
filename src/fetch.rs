//! `fetch <url>`：纯 reqwest GET 取网页正文，全程零浏览器（issue gsearch-rs-fetch）。
//! 存在理由：没装 Chrome 的机器上也能秒读静态页——换机可用性缺口。
//! HTTP 层与 searxng.rs 同款 reqwest 客户端构建；正文提取统一走 scraper 树内路径
//! （kda：与 skeleton::extract_adaptive 同一解析器。原手写剥标签状态机在属性值含
//! `>` 时提前截断标签、把属性尾部漏进正文，已删）。
//!
//! 安全门（Important-1 / 2）：
//! - 默认拒绝私网（loopback / RFC1918 / link-local / IPv6 ::1 + fc00::/7）；
//!   放行通过 `--allow-private` 或 `GSEARCH_FETCH_ALLOW_PRIVATE=1`（=1 值语义）。
//! - scheme 规则与私网门在重定向每跳由 Policy::custom 强制（9re，防重定向绕过）：
//!   公网强制 https；私网 http 仅显式放行时允许（内网端点常见 http-only）。

use std::net::{IpAddr, Ipv6Addr, ToSocketAddrs};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use scraper::{Html, Node, Selector};

use crate::postproc::cap_chars;

/// fetch 总超时（含 redirect 链）；纯 HTTP 无渲染，10s 足够。
const FETCH_TIMEOUT_SECS: u64 = 10;
/// JS 壳判定阈值：剥标签后正文低于此字符数 → 大概率是渲染型页面。
const SHELL_MIN_CHARS: usize = 500;
/// redirect 跟随上限（reqwest 默认同款，显式声明防歧义）。
const MAX_REDIRECTS: usize = 10;
/// 响应体硬上限（Important-3）：超过此字节数立即停下载，避免 OOM/zip-bomb。
const FETCH_BODY_LIMIT: usize = 10 * 1024 * 1024;
/// fetch batch 并发上限（防目标站/出口压力）。
const FETCH_CONCURRENCY: usize = 5;

/// `fetch <url>` 选项集（与 general::BrowseOpts 同风格）。
#[derive(Debug, Clone, Default)]
pub struct FetchOpts {
    pub json: bool,
    /// 显式代理（--proxy / GSEARCH_PROXY）；None = 跟随环境代理（面向公网，与 searxng 的 no_proxy 相反）。
    pub proxy: Option<String>,
    /// 放行私网（loopback / RFC1918 / link-local）。默认 false：SSRF 门。
    pub allow_private: bool,
    /// 逗号分隔 CSS selector（如 "main,article"）：命中时取首个命中容器的正文并跳过 JS 壳判定；
    /// 未命中回退全文提取，--json 在 meta.include_hit=false 标注（用户明确知道要什么，壳判定不适用）。
    pub include: Option<String>,
    /// 正文以 markdown 输出（表格/标题/链接保结构）。--json 下 text 字段换源为 markdown，
    /// meta.format="markdown" 标注；无 flag 时逐字节不变。
    pub markdown: bool,
    /// 正文字符预算（text 上限；超限截断，meta.truncated/omitted 如实标注）。CLI 默认 50000。
    pub max_chars: usize,
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

/// 放行判定：--allow-private flag 或 GSEARCH_FETCH_ALLOW_PRIVATE=1。env 走值语义（仅 "1"/"true"
/// 生效）——presence 语义会让 `=0`/空值静默开洞。
pub(crate) fn allow_private_requested(flag: bool) -> bool {
    flag
        || std::env::var("GSEARCH_FETCH_ALLOW_PRIVATE")
            .map(|v| v.trim() == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
}

/// URL → host → IP → 私网判定（pkp 拆分：无调用方文案的纯判定层）。
/// 返回 (host, ip, 是否私网)。私网性始终返回——调用方结合 allow_private 决定拒/放行，
/// 错误文案由调用方（fetch / browse 门）各自包装，避免 browse 场景打出「fetch 拒绝」。
pub(crate) fn classify_url(url: &str) -> Result<(String, IpAddr, bool)> {
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
    let private = is_private_ip(ip);
    Ok((host.to_string(), ip, private))
}

/// 私网门判定核心：URL → host → IP → 私网判定。返回 (host, ip, 是否私网)。
/// allow_private=true 时仍解析返回——调用方需要私网性决定是否放行内网明文 http。
pub(crate) fn gate_check(url: &str, allow_private: bool) -> Result<(String, IpAddr, bool)> {
    let (host, ip, private) = classify_url(url)?;
    if private && !allow_private {
        anyhow::bail!(
            "fetch 拒绝私网地址 {ip}（host={host}）。如确需内网，请传 --allow-private 或设置 GSEARCH_FETCH_ALLOW_PRIVATE=1"
        );
    }
    Ok((host, ip, private))
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

/// fetch/dl 共用客户端：UA + 总超时 + 重定向每跳 SSRF 门 + 可选显式代理。
/// 门逻辑与单条/批量/并发数无关——每个 client 自带 Policy::custom，批量下每 URL 每跳照走。
pub(crate) fn build_client(proxy: Option<&str>, allow: bool, timeout: Duration) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .user_agent(format!("gsearch/{}", env!("CARGO_PKG_VERSION")))
        .timeout(timeout)
        // 9re(c)：每跳 host 都过私网门 + scheme 规则，防重定向绕过（替代旧 https_only 全局开关）。
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.stop();
            }
            let u = attempt.url();
            // 无 host / 解析失败一律按私网处理（fail-closed）
            let host_priv = u
                .host_str()
                .map(|h| resolve_host(h).map(is_private_ip).unwrap_or(true))
                .unwrap_or(true);
            if host_priv && !allow {
                return attempt.error(
                    "重定向目标命中私网门（SSRF 防护）；放行请加 --allow-private 或设置 GSEARCH_FETCH_ALLOW_PRIVATE=1",
                );
            }
            match u.scheme() {
                "https" => attempt.follow(),
                "http" if allow && host_priv => attempt.follow(),
                _ => attempt.error("fetch 仅支持 https：明文 http 已禁用（重定向链同样禁止）"),
            }
        }));
    if let Some(p) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(p).context("代理 URL 无效")?);
    }
    builder.build().context("构建 HTTP 客户端失败")
}

/// Content-Type 判 PDF（含参数形态 `application/pdf; charset=binary`，大小写不敏感）。
/// 纯函数离线单测覆盖；fetch 侧据此拒抓，指引走 dl。
fn is_pdf_content_type(ct: &str) -> bool {
    // 剥参数段后精确匹配 mime 主类型，避免子串误伤（application/pdf+xml 之类复合类型不判 PDF）
    ct.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("application/pdf")
}

/// 其余二进制 Content-Type 前置拒绝（图像/音视频/字体/压缩包/可执行等）——
/// 原样当文本透传 = 乱码正文。文本类（text/*、+xml/+json/javascript 等）不在此判；
/// 无 Content-Type 头由调用方兜底按文本处理（既有契约）。大小写不敏感（调用方已
/// to_lowercase，双保险与 is_pdf_content_type 同款）。
fn is_binary_content_type(ct: &str) -> bool {
    let main = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    main.starts_with("image/")
        || main.starts_with("audio/")
        || main.starts_with("video/")
        || main.starts_with("font/")
        || main.starts_with("model/")
        || matches!(
            main.as_str(),
            "application/pdf"
                | "application/zip"
                | "application/gzip"
                | "application/x-gzip"
                | "application/x-tar"
                | "application/x-7z-compressed"
                | "application/x-rar-compressed"
                | "application/vnd.rar"
                | "application/x-iso9660-image"
                | "application/octet-stream"
                | "application/wasm"
                | "application/java-archive"
                | "application/x-elf"
                | "application/x-msdownload"
                | "application/x-shockwave-flash"
                | "application/msword"
                | "application/vnd.ms-excel"
                | "application/vnd.ms-powerpoint"
                | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        )
}

/// 单 URL 拉取 + 提取（不含输出）。批量与单条共用；每 URL 独立过 SSRF 门（含重定向每跳）。
async fn fetch_one(url: &str, opts: &FetchOpts) -> Result<FetchOne> {
    if !http_or_https_scheme(url) {
        return Err(anyhow!("fetch 仅支持 http/https URL（拒绝: {url}）"));
    }
    let allow = allow_private_requested(opts.allow_private);
    // SSRF 门先行（含 DNS 解析）：私网 URL 无论 scheme 默认在这里被拒（错误含「私网」）。
    let (_host, _ip, is_priv) = gate_check(url, allow)?;
    // 公网明文 http 一律拒（防降级 + 重定向中转 SSRF）；私网 http 仅在显式放行时允许
    // （内网端点常见 http-only，allow_private 即「自担风险进内网」的完整语义）。
    if url.trim_start().to_ascii_lowercase().starts_with("http://") && !(allow && is_priv) {
        // 7z0：这是 https 门不是私网门——--allow-private 对公网无效，文案不得误导推荐
        return Err(anyhow!(
            "fetch 仅支持 https：公网明文 http 已禁用（防降级与重定向 SSRF 中转）。--allow-private 仅放行内网 http，对公网地址无效；如该站有 https 地址请改用 https: {url}"
        ));
    }
    let client = build_client(opts.proxy.as_deref(), allow, Duration::from_secs(FETCH_TIMEOUT_SECS))?;

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
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.to_ascii_lowercase());
    // q34：application/pdf 不剥标签直接转文本 = 乱码——显式报错指引 dl 落盘 + 外部工具提取。
    if content_type.as_deref().is_some_and(is_pdf_content_type) {
        return Err(anyhow!(
            "PDF 二进制内容，fetch 不做本地解析；用 gsearch dl {url} 落盘后由外部工具提取文本"
        ));
    }
    // kda：其余二进制类型前置拒绝（下载 body 前），与 PDF 同语义。
    if content_type.as_deref().is_some_and(is_binary_content_type) {
        return Err(anyhow!(
            "二进制内容（{}），fetch 不做文本提取；用 gsearch dl {url} 落盘后处理",
            content_type.as_deref().unwrap_or_default()
        ));
    }
    let is_html = content_type
        .as_deref()
        .map(|ct| ct.contains("html"))
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

    let limit = opts.max_chars;
    let mut fetched = process_html(url, &html, is_html, limit);
    fetched.github_comment_hint = github_thread_comment_gap(url);
    // xih：--markdown 在剥标签前的原始 HTML 上转换（保表格/标题/链接结构），text 字段换源；
    // 非 HTML（text/plain / JSON / md 源文）本就是文本，原样保留。
    if opts.markdown && is_html {
        let (md, t, o) = cap_chars(&crate::convert::html_to_markdown(&html)?, limit);
        fetched.text = md;
        fetched.truncated = t;
        fetched.omitted = o;
        fetched.markdown = true;
    }
    // --include：命中容器则用容器内 HTML 重新提取正文（title 仍取页面级）；未命中回退全文提取。
    if let Some(include) = &opts.include {
        fetched.include_hit = Some(false);
        if is_html
            && let Some(inner) = extract_with_include(&html, include)?
        {
            let raw = if opts.markdown {
                crate::convert::html_to_markdown(&inner)?
            } else {
                extract_text(&inner)
            };
            let (text, t, o) = cap_chars(&raw, limit);
            fetched = Fetched {
                url: fetched.url,
                title: fetched.title,
                text,
                truncated: t,
                omitted: o,
                include_hit: Some(true),
                markdown: opts.markdown,
                github_comment_hint: fetched.github_comment_hint,
            };
        }
    }
    // 字节上限触发的截断：meta.truncated=true；omitted 仅作下界（实际丢多少未知，至少 FETCH_BODY_LIMIT 已读）
    if truncated {
        fetched.truncated = true;
        // 累计到已有 omitted 之上（区分两次截断：body 上限 vs cap_chars 正文字符上限）
        fetched.omitted = fetched.omitted.saturating_add(FETCH_BODY_LIMIT);
    }

    // 元审计约束：--include 命中 = 用户明确知道要什么，跳过 JS 壳判定。
    if fetched.include_hit != Some(true) && is_html && looks_like_js_shell(&fetched.text, &html) {
        return Ok(FetchOne::JsShell);
    }
    Ok(FetchOne::Done(fetched))
}

/// `gsearch fetch <url>...`：GET → 轻量正文提取 → 人读 / --json 输出。
/// 单 URL = 原行为；多 URL = batch 并发（上限 FETCH_CONCURRENCY、单条失败不阻塞、
/// 退出码 0 全成功 / 1 部分失败 / 2 全失败，对齐 search batch）。
/// 退出码：0 成功；1 JS 壳（需渲染）/ 私网门拒（由 main 统一打印，HTTP 错误经 anyhow → exit 1）。
pub async fn cmd_fetch(urls: &[String], opts: &FetchOpts) -> Result<ExitCode> {
    let started = Instant::now();
    if let [url] = urls {
        return match fetch_one(url, opts).await {
            Ok(FetchOne::Done(fetched)) => {
                if opts.json {
                    println!("{}", fetched_json(&fetched));
                } else {
                    if fetched.truncated {
                        eprintln!("注意：正文超上限已截断（省略 {} 字符；--json 输出在 meta 字段标注）", fetched.omitted);
                    }
                    println!("=== {} | {} ===\n{}", fetched.url, fetched.title, fetched.text);
                }
                tracing::debug!("fetch 完成: {url} ({}ms)", started.elapsed().as_millis());
                Ok(ExitCode::SUCCESS)
            }
            Ok(FetchOne::JsShell) => {
                eprintln!("该页无服务端正文（JS 壳），需渲染：用 gsearch browse {url}");
                Ok(ExitCode::from(1))
            }
            Err(e) => Err(e),
        };
    }
    cmd_fetch_batch(urls, opts).await
}

/// 批量：并发上限 5（防目标站/出口压力），buffered 保持输入序；每条独立过 SSRF 门（含重定向每跳）。
async fn cmd_fetch_batch(urls: &[String], opts: &FetchOpts) -> Result<ExitCode> {
    use futures::stream::{StreamExt, iter};
    let fetched: Vec<(String, Result<FetchOne>)> = iter(urls.iter().cloned())
        .map(|u| async move {
            let r = fetch_one(&u, opts).await;
            (u, r)
        })
        .buffered(FETCH_CONCURRENCY)
        .collect()
        .await;

    let mut entries = Vec::with_capacity(fetched.len());
    let mut ok_count = 0usize;
    for (url, result) in &fetched {
        match result {
            Ok(FetchOne::Done(f)) => {
                ok_count += 1;
                let mut v = fetched_json(f);
                v["status"] = serde_json::json!("ok");
                entries.push(v);
            }
            Ok(FetchOne::JsShell) => entries.push(serde_json::json!({
                "url": url,
                "status": "error",
                // 8lp②：元素 url 键已携带地址，message 不再重复整段 URL
                "message": "该页无服务端正文（JS 壳），需渲染：用 gsearch browse",
            })),
            Err(e) => {
                let status = if e.to_string().contains("私网") { "private_blocked" } else { "error" };
                entries.push(serde_json::json!({ "url": url, "status": status, "message": format!("{e:#}") }));
            }
        }
    }

    let total = fetched.len();
    if opts.json {
        // 对齐 search batch：裸数组、无外层信封，单条失败不阻塞数组整体。
        // 8lp：compact 单行——输出契约面向 agent 消费。
        println!("{}", serde_json::to_string(&entries)?);
    } else {
        for (i, (url, result)) in fetched.iter().enumerate() {
            match result {
                Ok(FetchOne::Done(f)) => println!("=== [{i}/{total}] {} | {} ===\n{}", url, f.title, f.text),
                Ok(FetchOne::JsShell) => println!(
                    "=== [{i}/{total}] {url} ===\n出错: 该页无服务端正文（JS 壳），需渲染：用 gsearch browse {url}"
                ),
                Err(e) => println!("=== [{i}/{total}] {url} ===\n出错: {e:#}"),
            }
        }
    }
    eprintln!("batch 完成：{ok_count}/{total} 条成功");
    let code = if ok_count == total { 0 } else if ok_count == 0 { 2 } else { 1 };
    Ok(ExitCode::from(code))
}

/// GitHub issue/PR 页的评论区不在 SSR HTML 里（JS 动态加载），fetch 纯 HTTP 输出
/// 必缺评论区——meta 打缺失信号 + 可行动建议（盲测七 wqm：输出自称完整，agent 据
/// 此误判「无讨论」，靠 api.github.com 交叉验证才识破）。非 GitHub thread 页 None。
fn github_thread_comment_gap(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let path = rest.split(['?', '#']).next()?;
    let mut seg = path.split('/');
    let owner = seg.next().filter(|s| !s.is_empty())?;
    let repo = seg.next().filter(|s| !s.is_empty())?;
    let number = match (seg.next()?, seg.next()?) {
        ("issues", n) | ("pull", n) if !n.is_empty() => n,
        _ => return None,
    };
    Some(format!(
        "评论区由 JS 动态加载，未包含在本输出中（勿据本文判断有无讨论）；完整讨论：\
         gsearch browse {url} --markdown，或 GET https://api.github.com/repos/{owner}/{repo}/issues/{number}/comments"
    ))
}

/// 单条 JSON 载荷（url/title/text/meta{truncated, omitted, content_untrusted}）。
/// meta.include_hit 仅在用过 --include 时出现——不带 flag 的老输出结构不变。
fn fetched_json(f: &Fetched) -> serde_json::Value {
    let mut meta = serde_json::json!({
        "truncated": f.truncated,
        "omitted": f.omitted,
        "content_untrusted": true,
    });
    if let Some(hit) = f.include_hit {
        meta["include_hit"] = serde_json::json!(hit);
    }
    if f.markdown {
        meta["format"] = serde_json::json!("markdown");
    }
    if let Some(hint) = &f.github_comment_hint {
        meta["github_comments_missing"] = serde_json::json!(true);
        meta["github_comments_hint"] = serde_json::json!(hint);
    }
    serde_json::json!({
        "url": f.url,
        "title": f.title,
        "text": f.text,
        "meta": meta,
    })
}

/// 提取产物（url 原样带回，方便 --json 消费方对账）。
struct Fetched {
    url: String,
    title: String,
    text: String,
    truncated: bool,
    omitted: usize,
    /// --include 状态：None=未用 --include；Some(true)=selector 命中容器；Some(false)=未命中回退全文。
    include_hit: Option<bool>,
    /// text 字段是否已是 markdown（--json 据此写 meta.format）。
    markdown: bool,
    /// GitHub issue/PR 页的评论区缺失信号（None = 非 thread 页，键缺席）。
    github_comment_hint: Option<String>,
}

/// 单 URL fetch 结果：Done = 正文已提取；JsShell = JS 壳需渲染（单条 exit 1 / 批量记 error）。
enum FetchOne {
    Done(Fetched),
    JsShell,
}

/// 纯函数：html → 提取 + 截断（limit 注入，离线单测不碰配置）。
/// 非 HTML（text/plain 等）不剥标签不判壳也不实体解码——markdown/JSON 源文保真，
/// 字面 `&amp;`/`&#20013;` 原样保留（agent 取原文场景）。
fn process_html(url: &str, html: &str, is_html: bool, limit: usize) -> Fetched {
    // kda：HTML 解析一次，title 与正文同出树内（字符串扫描版 extract_title 对
    // 属性值含 `>` 的标签同样漏片段）；非 HTML 源文保真，不碰解析器。
    let (title, raw) = if is_html {
        let doc = Html::parse_document(html);
        (tree_title(&doc), collapse_blank(tree_text(&doc)))
    } else {
        (String::new(), collapse_blank(html.to_string()))
    };
    let (text, truncated, omitted) = cap_chars(&raw, limit);
    Fetched { url: url.to_string(), title, text, truncated, omitted, include_hit: None, markdown: false, github_comment_hint: None }
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

/// 树内正文提取（kda）：与 extract_adaptive 同一 scraper 解析器。script/style/noscript/
/// template 子树整跳；注释/doctype 非 Text 节点天然剔除；块级边界 → 换行、行内 → 空格
/// （与旧手写状态机的输出约定一致）；实体由 html5ever 解析期解码；空白规整交 collapse_blank。
fn tree_text(doc: &Html) -> String {
    let mut out = String::with_capacity(4096);
    // 显式栈 DFS，children 逆序入栈保持文档序（ego_tree 未被 scraper re-export，类型全程推断）
    let mut stack: Vec<_> = doc.tree.root().children().rev().collect();
    while let Some(node) = stack.pop() {
        match node.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(el) => {
                if matches!(el.name(), "script" | "style" | "noscript" | "template") {
                    continue;
                }
                let sep = if is_block_boundary(el.name()) { '\n' } else { ' ' };
                out.push(sep);
                stack.extend(node.children().rev());
                out.push(sep);
            }
            _ => {}
        }
    }
    out
}

/// `<title>` 文本（解析期已解码实体，trim 后取用）；无 title → 空串。
fn tree_title(doc: &Html) -> String {
    let sel = Selector::parse("title").expect("静态选择器必然合法");
    doc.select(&sel)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
        .unwrap_or_default()
}

fn extract_text(html: &str) -> String {
    collapse_blank(tree_text(&Html::parse_document(html)))
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

/// --include：逗号分隔 selector 依序试（scraper 解析），返回首个命中元素的 inner_html；全未命中 → None。
/// selector 语法错误 → Err：用户显式输入拼错了要报错，静默跳过会伪装成"未命中回退全文"。
fn extract_with_include(html: &str, include: &str) -> Result<Option<String>> {
    let doc = Html::parse_document(html);
    for sel in include.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        // day：scraper 的 Display 泄漏内部变体名（EmptySelector/Please report...），映射成用户可操作的文案
        let selector = Selector::parse(sel).map_err(|_| {
            anyhow!("CSS selector 无效: {sel:?}（语法错误，应为合法 CSS 选择器如 \"#main, article\"）")
        })?;
        if let Some(el) = doc.select(&selector).next() {
            return Ok(Some(el.inner_html()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// --include：命中容器取 inner_html；逗号分隔依序试；未命中 None；selector 语法错误 Err。
    #[test]
    fn extract_with_include_hits_and_falls_back() {
        let html = "<html><head><title>T</title></head><body>\
                    <nav>菜单 链接</nav><main><h1>正文标题</h1><p>第一段</p></main></body></html>";
        // article 不存在 → 依序命中 main：只取容器内正文
        let got = extract_with_include(html, "article,main").unwrap().unwrap();
        assert!(got.contains("正文标题") && got.contains("第一段"), "got: {got}");
        assert!(!got.contains("菜单"), "nav 内容不应混入: {got}");
        // 全未命中 → None（调用方回退全文）
        assert!(extract_with_include(html, "article,aside").unwrap().is_none());
        // selector 语法错误 → Err（不伪装成"未命中"）
        assert!(extract_with_include(html, "main[").is_err());
    }

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

    /// 回归：属性值含 `>` 的标签不漏片段进正文（旧手写剥标签在 title="a>b" 处提前
    /// 截断标签，把 `b">` 起的尾巴漏进正文/title）。树内路径由解析器按引号正确处理。
    #[test]
    fn extract_text_attr_gt_no_leak() {
        let html = r#"<html><head><title foo="a>b">标题</title></head>
                      <body><div data-x="p>q">正文甲</div><span title="a>b">正文乙</span></body></html>"#;
        let fetched = process_html("https://e.test/", html, true, 50_000);
        assert_eq!(fetched.title, "标题", "title 不得含属性尾巴: {}", fetched.title);
        assert!(
            fetched.text.contains("正文甲") && fetched.text.contains("正文乙"),
            "got: {}",
            fetched.text
        );
        // 旧实现泄漏形态：attr 尾 `q">正文甲` 与 title 处 `b">标题`
        assert!(!fetched.text.contains("q\""), "attr 尾巴泄漏: {}", fetched.text);
        assert!(!fetched.text.contains("\">"), "引号闭合片段泄漏: {}", fetched.text);
        assert!(!fetched.text.contains('<'), "不应残留标签: {}", fetched.text);
    }

    /// 二进制 Content-Type 门——压缩包/图像/音视频/字体/文档判真，文本与结构化判假。
    #[test]
    fn binary_content_type_detection() {
        for bin in [
            "application/zip",
            "application/zip; charset=binary",
            "image/png",
            "video/mp4",
            "audio/ogg",
            "font/woff2",
            "application/octet-stream",
            "application/gzip",
            "application/pdf",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "APPLICATION/ZIP", // 大小写不敏感（fetch_one 已 to_lowercase，双保险）
        ] {
            assert!(is_binary_content_type(bin), "应判二进制: {bin}");
        }
        for text_ct in [
            "text/html; charset=utf-8",
            "text/plain",
            "text/markdown",
            "application/json",
            "application/xhtml+xml",
            "application/xml",
            "application/javascript",
        ] {
            assert!(!is_binary_content_type(text_ct), "不应判二进制: {text_ct}");
        }
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
            let err = gate_check(bad, false).unwrap_err();
            assert!(err.to_string().contains("拒绝"), "应拒绝 {bad}: {err}");
        }
        // 放行公网 IP 字面量（不发起请求，纯函数验证）
        for ok in [
            "http://8.8.8.8/x",
            "https://1.1.1.1/x",
            "https://93.184.216.34/x",
        ] {
            gate_check(ok, false).expect(ok);
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
            gate_check(url, true).expect(url);
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

    /// Content-Type 判 PDF——裸类型/带参数命中，复合类型与 html 不误伤。
    #[test]
    fn pdf_content_type_detection() {
        assert!(is_pdf_content_type("application/pdf"));
        assert!(is_pdf_content_type("application/pdf; charset=binary"));
        assert!(is_pdf_content_type("APPLICATION/PDF")); // 大小写不敏感（fetch_one 已 to_lowercase，双保险）
        assert!(!is_pdf_content_type("text/html; charset=utf-8"));
        assert!(!is_pdf_content_type("application/pdf+xml")); // 复合类型不判
        assert!(!is_pdf_content_type("text/plain"));
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

    /// --max-chars 字符预算——正文超限截断、meta.truncated/omitted 如实标注；
    /// 未超限时不截断。JSON 载荷 meta 键同步（agent 可按键判断是否放大预算重取）。
    #[test]
    fn max_chars_caps_text_and_flags_truncated() {
        let html = "<html><head><title>t</title></head><body><p>你好世界，测试正文。</p></body></html>";
        // 未超限：不截断；全文长度作 omitted 基准（提取文本含折叠空白，不从源串硬数）
        let full = process_html("https://e.test/a", html, true, 50_000);
        assert!(!full.truncated);
        assert_eq!(full.omitted, 0);
        // 超限：截到 3 字符，omitted 如实
        let f = process_html("https://e.test/a", html, true, 3);
        assert!(f.truncated);
        assert_eq!(f.text.chars().count(), 3);
        assert_eq!(f.omitted, full.text.chars().count() - 3);
        // JSON 载荷 meta.truncated 标注同步
        let v = fetched_json(&f);
        assert_eq!(v["meta"]["truncated"], serde_json::json!(true));
        assert!(v["meta"]["omitted"].as_u64().unwrap() > 0);
    }

    /// GitHub issue/PR 页评论区不在 SSR HTML 里（盲测七 wqm），meta 必打缺失信号 +
    /// 可行动建议（api.github.com comments / browse --markdown）；非 thread 页键缺席。
    #[test]
    fn github_thread_meta_signals_missing_comments() {
        // issue / PR 页命中，hint 含 api.github.com 端点与编号
        let hint = github_thread_comment_gap("https://github.com/tokio-rs/tokio/issues/7787")
            .expect("issue 页应命中");
        assert!(hint.contains("api.github.com/repos/tokio-rs/tokio/issues/7787/comments"), "hint: {hint}");
        assert!(hint.contains("browse"), "hint 应给 browse 出口: {hint}");
        let hint = github_thread_comment_gap("https://github.com/tokio-rs/tokio/pull/7788")
            .expect("PR 页应命中");
        assert!(hint.contains("/issues/7788/comments"), "PR 对话走 issues 端点: {hint}");
        // query/hash 不影响判定
        assert!(github_thread_comment_gap("https://github.com/o/r/issues/1?foo=bar#top").is_some());
        // 非 thread 页不出信号
        assert!(github_thread_comment_gap("https://github.com/tokio-rs/tokio").is_none());
        assert!(github_thread_comment_gap("https://github.com/tokio-rs/tokio/issues").is_none());
        assert!(github_thread_comment_gap("https://github.com/tokio-rs/tokio/blob/main/Cargo.toml").is_none());
        assert!(github_thread_comment_gap("https://docs.rs/serde_json").is_none());
        assert!(github_thread_comment_gap("http://github.com/o/r/issues/1").is_none());

        // JSON 载荷：thread 页带两键；非 thread 页键缺席（默认输出结构不变）
        let f = Fetched {
            url: "https://github.com/tokio-rs/tokio/issues/7787".into(),
            title: "t".into(),
            text: "body".into(),
            truncated: false,
            omitted: 0,
            include_hit: None,
            markdown: false,
            github_comment_hint: github_thread_comment_gap("https://github.com/tokio-rs/tokio/issues/7787"),
        };
        let v = fetched_json(&f);
        assert_eq!(v["meta"]["github_comments_missing"], serde_json::json!(true));
        assert!(v["meta"]["github_comments_hint"].as_str().unwrap().contains("api.github.com"));

        let f2 = Fetched { url: "https://e.test/".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, include_hit: None, markdown: false, github_comment_hint: None };
        let v2 = fetched_json(&f2);
        assert!(v2["meta"].get("github_comments_missing").is_none(), "非 thread 页不得出键");
        assert!(v2["meta"].get("github_comments_hint").is_none());
    }
}
