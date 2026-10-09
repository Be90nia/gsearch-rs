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

use crate::postproc::{cap_chars, cap_chars_json};

/// fetch 总超时默认（FixG10 J-1：--timeout 默认值，封装在 FetchOpts::default，便于 main 复用）。
/// JS 壳判定阈值：剥标签后正文低于此字符数 → 大概率是渲染型页面。
const SHELL_MIN_CHARS: usize = 500;
/// redirect 跟随上限（reqwest 默认同款，显式声明防歧义）。
const MAX_REDIRECTS: usize = 10;
/// 响应体硬上限（Important-3）：超过此字节数立即停下载，避免 OOM/zip-bomb。
const FETCH_BODY_LIMIT: usize = 10 * 1024 * 1024;
/// fetch batch 并发上限（防目标站/出口压力）。
const FETCH_CONCURRENCY: usize = 5;

/// fetch 单请求超时上限（秒，FixG10 J-1：--timeout 上限拉宽，给 GitHub 抖动下重试留余地）
const FETCH_TIMEOUT_MAX_SECS: u64 = 300;
/// fetch 重试上限（次，FixG10 J-1：--retry 上限 3 次；backoff 1s/2s/4s）
const FETCH_RETRY_MAX: u32 = 3;

/// `fetch <url>` 选项集（与 general::BrowseOpts 同风格）。
#[derive(Debug, Clone)]
pub struct FetchOpts {
    pub json: bool,
    /// 显式代理（--proxy / GSEARCH_PROXY）；None = 跟随环境代理（面向公网，与 searxng 的 no_proxy 相反）。
    pub proxy: Option<String>,
    /// 放行私网（loopback / RFC1918 / link-local）。默认 false：SSRF 门。
    pub allow_private: bool,
    /// 逗号分隔 CSS selector（如 "main,article"）：命中时取所有命中容器的正文并跳过 JS 壳判定；
    /// 未命中回退全文提取，--json 在 meta.include_hit=false / include_hits=N 标注。
    /// FixG10 J-3：多选器累加（旧版只取首个，命中断章不再卡死）。
    pub include: Option<String>,
    /// 正文以 markdown 输出（表格/标题/链接保结构）。--json 下 text 字段换源为 markdown，
    /// meta.format="markdown" 标注；无 flag 时逐字节不变。
    pub markdown: bool,
    /// 正文字符预算（text 上限；超限截断，meta.truncated/omitted 如实标注）。CLI 默认 50000。
    pub max_chars: usize,
    /// FixG10 J-1：单请求超时（秒）；默认 10（与 FetchOpts::default 行为一致）；CLI 默认 10，范围 1..=300。
    pub timeout_secs: u64,
    /// FixG10 J-1：失败重试次数（不含首次）；默认 0 = 不重试（行为不变）；CLI 默认 0，范围 0..=3。
    /// backoff：1s, 2s, 4s（第 N 次等待 2^(N-1) 秒）。每次重试 stderr 一行提示「第 N/总 N 次重试」。
    pub retry: u32,
    /// FixG10 J-2：JSONPath 投影（例：`.crate.max_version,.crate.max_stable_version`）；
    /// 非空时 `--json` 输出只保留指定字段并打 `meta.truncated_by_json_keys: true`。
    pub json_keys: Vec<String>,
    /// FixG10 L-2：URL `#N-M` 锚点范围裁剪——命中时仅输出文本 [line_start, line_end]，
    /// 上下各扩 N 行作为上下文；meta.anchor_crop_range: [start, end]。
    pub anchor_pad_lines: usize,
}

impl Default for FetchOpts {
    fn default() -> Self {
        Self {
            json: false,
            proxy: None,
            allow_private: false,
            include: None,
            markdown: false,
            max_chars: 50_000,
            timeout_secs: 10,
            retry: 0,
            json_keys: Vec::new(),
            anchor_pad_lines: 0,
        }
    }
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

/// 决定是否应该重试：纯函数（FixG10 J-1：把 retry 判定拆出便于单测）。
/// - 确定性错误（私网 / scheme / 二进制 Content-Type）→ false
/// - HTTP 4xx（除 408/429）→ false（客户端错不会因等待修复）
/// - 其余（含 5xx / 网络超时 / DNS 失败）→ true（重试 budget 允许时）
fn should_retry(err_msg: &str, has_budget: bool) -> bool {
    if !has_budget {
        return false;
    }
    let permanent = err_msg.contains("拒绝")
        || err_msg.contains("fetch 仅支持")
        || err_msg.contains("PDF")
        || err_msg.contains("二进制内容")
        || err_msg.contains("私网");
    if permanent {
        return false;
    }
    // 4xx（除 408/429）客户端错不重试
    if err_msg.contains("HTTP 4") && !err_msg.contains("HTTP 408") && !err_msg.contains("HTTP 429") {
        return false;
    }
    true
}

/// 单 URL 拉取 + 提取（不含输出）。批量与单条共用；每 URL 独立过 SSRF 门（含重定向每跳）。
/// FixG10 J-1：失败重试 loop——仅对网络/超时错误重试；HTTP 4xx 立即放弃（5xx 也重试，
/// 因为 502/503/504 服务端瞬时错误常抖几下就好）。每次重试 stderr 一行 backoff + 提示。
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
    // FixG10 J-1：--timeout 注入；clamp 上限 300s（防止误传 86400 把 fetch 跑挂）。
    let timeout_secs = opts.timeout_secs.clamp(1, FETCH_TIMEOUT_MAX_SECS);
    let retry = opts.retry.min(FETCH_RETRY_MAX);
    let total_attempts = retry + 1; // 含首次 = retry+1 次

    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..total_attempts {
        if attempt > 0 {
            // backoff：1s, 2s, 4s（第 N 次等待 2^(N-1) 秒）
            let delay_secs = 1u64 << (attempt - 1);
            // ponytail: backoff 写死 1s/2s/4s——任务约束第 N/总 N 次重试即可，更精细可改 jitter。
            eprintln!(
                "第 {attempt}/{total_attempts} 次重试（{delay_secs}s 后）: {url}"
            );
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;
        }
        match fetch_one_attempt(url, opts, timeout_secs, allow).await {
            Ok(r) => return Ok(r),
            Err(e) => {
                let msg = format!("{e:#}");
                let has_budget = attempt < total_attempts - 1;
                if !should_retry(&msg, has_budget) {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("fetch 重试耗尽")))
}

/// 单次 fetch 尝试（不含重试 loop）：抓响应 → 提取 → 组装 Fetched。
/// 拆出便于 J-1 重试 loop 包裹；其余逻辑（SSRF / scheme / Content-Type / 累积）保持不动。
async fn fetch_one_attempt(
    url: &str,
    opts: &FetchOpts,
    timeout_secs: u64,
    allow: bool,
) -> Result<FetchOne> {
    let client = build_client(opts.proxy.as_deref(), allow, Duration::from_secs(timeout_secs))?;
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
    let mut html = String::from_utf8_lossy(&buf).into_owned();
    let mut json_projected = false;
    // FixG10 J-2：--json-keys 在 process_html 前投影——先把 body 按 JSONPath 缩成小子集，
    // 再交给 cap_chars_json 走 brace 边界截断；不投影直接走 5000 字符截断会切到 JSON
    // 中段变非法（crates.io API 实测 395KB categories 后 max_version 永远拿不到）。
    // 仅非 HTML 且 body 是合法 JSON 时投影；HTML / 非 JSON / 路径错误走原路径。
    if !is_html && !opts.json_keys.is_empty() {
        match project_json_body(&html, &opts.json_keys) {
            Ok(Some(projected)) => {
                html = projected;
                json_projected = true;
            }
            Ok(None) => {} // body 非 JSON 或路径无命中：原样保留，让 cap_chars_json 走兜底
            Err(e) => {
                // 投影失败：把错误绑进 Fetched（后续 meta.truncated_by_json_keys 用不上时也透出）
                // 不强行报错——投影是 best-effort 加速，失败时退回到原始大文本。
                eprintln!("--json-keys 投影失败（{e:#}）；按原始 body 走");
            }
        }
    }

    let limit = opts.max_chars;
    let mut fetched = process_html(url, &html, is_html, limit);
    fetched.truncated_by_json_keys = json_projected;
    fetched.github_comment_hint = github_thread_comment_gap(url);
    // xih：--markdown 在剥标签前的原始 HTML 上转换（保表格/标题/链接结构），text 字段换源；
    // 非 HTML（text/plain / JSON / md 源文）本就是文本，原样保留。
    if opts.markdown && is_html {
        let (md, t, o, off) = cap_chars(&crate::convert::html_to_markdown(&html)?, limit);
        fetched.text = md;
        fetched.truncated = t;
        fetched.omitted = o;
        fetched.truncated_at_offset = t.then_some(off);
        fetched.markdown = true;
    }
    // --include：命中容器则用容器内 HTML 重新提取正文（title 仍取页面级）；未命中回退全文提取。
    if let Some(include) = &opts.include {
        fetched.include_hit = Some(false);
        if is_html
            && let Some((inner, hits)) = extract_with_include(&html, include)?
        {
            let raw = if opts.markdown {
                crate::convert::html_to_markdown(&inner)?
            } else {
                extract_text(&inner)
            };
            let (text, t, o, off) = cap_chars(&raw, limit);
            fetched = Fetched {
                url: fetched.url,
                title: fetched.title,
                text,
                truncated: t,
                omitted: o,
                truncated_at_offset: t.then_some(off),
                include_hit: Some(true),
                include_hits: Some(hits),
                markdown: opts.markdown,
                github_comment_hint: fetched.github_comment_hint,
                anchor_crop_range: None,
                truncated_by_json_keys: false,
            };
        }
    }
    // FixG10 L-2：URL `#N-M` 锚点裁剪——命中时按行号裁 text + 扩 pad 行上下文，meta 记实际范围。
    // 必须在 cap_chars 之后（用最终 text 行号）且在 cap_chars_json 之前（裁后才做 cap；裁后短文无需 cap）。
    if let Some((start, end)) = parse_anchor_range(url) {
        let pad = opts.anchor_pad_lines;
        let (cropped, actual) = crop_text_lines(&fetched.text, start, end, pad);
        fetched.text = cropped;
        fetched.anchor_crop_range = Some(actual);
        // anchor crop 不引入新截断（crop 本身就是用户精确请求的子集）
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
                flush_stdout();
                tracing::debug!("fetch 完成: {url} ({}ms)", started.elapsed().as_millis());
                Ok(ExitCode::SUCCESS)
            }
            Ok(FetchOne::JsShell) => {
                eprintln!("该页无服务端正文（JS 壳），需渲染：用 gsearch browse {url}");
                flush_stdout();
                Ok(ExitCode::from(1))
            }
            Err(e) => Err(e),
        };
    }
    let code = cmd_fetch_batch(urls, opts).await?;
    flush_stdout();
    Ok(code)
}

/// FixG10 J-2：把 body 按 --json-keys 路径投影成只含指定字段的 JSON。
/// 返回 Ok(Some(投影后 JSON 串)) / Ok(None)（body 非 JSON 走原路径）/ Err（路径错误）。
/// 失败不强行 bail——调用方按 best-effort 走 stderr 一行 + 原 body。
fn project_json_body(body: &str, keys: &[String]) -> Result<Option<String>> {
    let parsed: serde_json::Value = match serde_json::from_str(body) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    let projected = project_json_paths(&parsed, keys)?;
    let serialized = serde_json::to_string(&projected).context("投影结果序列化失败")?;
    Ok(Some(serialized))
}

/// P1 fetch redirect 0 字节修：stdout 在 redirect（`> file`）下走全缓冲而非
/// 行缓冲——若 fetch 在 println 之后早返回，缓冲未及时 flush 会让下游看到
/// 0 字节（H 盲测八实锤：`fetch URL > file 2> err` 拿到 0B，`2>&1 | tee file`
/// 拿到全量；根因是 redirect 阻塞 stdout 写出）。这里在每条结果 println 后
/// 显式 flush stdout，确保 redirect 也吃到全部字节。tee/管道本身是 line-buffered
/// 或自己 drain，无副作用。
fn flush_stdout() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
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
    flush_stdout();
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
/// FixG10 J-2：非空 --json-keys 时顶层只保留指定字段并打 `meta.truncated_by_json_keys: true`；
/// J-3：--include 多选器命中数写入 meta.include_hits；
/// L-2：URL `#N-M` 锚点裁剪范围写入 meta.anchor_crop_range。
fn fetched_json(f: &Fetched) -> serde_json::Value {
    let mut meta = serde_json::json!({
        "truncated": f.truncated,
        "omitted": f.omitted,
        "content_untrusted": true,
    });
    if let Some(hit) = f.include_hit {
        meta["include_hit"] = serde_json::json!(hit);
    }
    if let Some(n) = f.include_hits {
        meta["include_hits"] = serde_json::json!(n);
    }
    if f.markdown {
        meta["format"] = serde_json::json!("markdown");
    }
    if let Some(hint) = &f.github_comment_hint {
        meta["github_comments_missing"] = serde_json::json!(true);
        meta["github_comments_hint"] = serde_json::json!(hint);
    }
    if let Some((start, end)) = f.anchor_crop_range {
        meta["anchor_crop_range"] = serde_json::json!([start, end]);
    }
    // FixG10 J-2：--json-keys 投影信号（仅用过 flag 才出键；默认输出结构不变）。
    if f.truncated_by_json_keys {
        meta["truncated_by_json_keys"] = serde_json::json!(true);
    }
    // 9jx：截断字节偏移（truncated=true 时填，缺席=未截断）。
    if let Some(off) = f.truncated_at_offset {
        meta["truncated_at_offset"] = serde_json::json!(off);
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
    /// 9jx：截断点在源里的字节偏移（与 truncated/omitted 同语义——truncated=true 时才有值，
    /// 缺席 = 未截断；与 read/browse 路径的 meta.truncated_at_offset 一致）。
    truncated_at_offset: Option<usize>,
    /// --include 状态：None=未用 --include；Some(true)=selector 命中容器；Some(false)=未命中回退全文。
    include_hit: Option<bool>,
    /// FixG10 J-3：--include 多选器累计命中容器数；未用 --include 时为 None（默认输出逐键不变）。
    include_hits: Option<usize>,
    /// text 字段是否已是 markdown（--json 据此写 meta.format）。
    markdown: bool,
    /// GitHub issue/PR 页的评论区缺失信号（None = 非 thread 页，键缺席）。
    github_comment_hint: Option<String>,
    /// FixG10 L-2：URL `#N-M` 锚点裁剪范围（1-based 行号，含端点）；未命中锚点 None。
    anchor_crop_range: Option<(usize, usize)>,
    /// FixG10 J-2：--json-keys 命中标记；true 时 meta.truncated_by_json_keys 出现。
    truncated_by_json_keys: bool,
}

/// 单 URL fetch 结果：Done = 正文已提取；JsShell = JS 壳需渲染（单条 exit 1 / 批量记 error）。
enum FetchOne {
    Done(Fetched),
    JsShell,
}

/// FixG10 L-2：从 URL fragment 提取 `#N-M` 行号范围。返回 (start, end) 1-based inclusive。
/// 仅匹配纯数字范围形态（`#2709-2714` / `#2709`）；命名锚点 / 单行号（无范围）返回 None。
/// 盲测九 L 实测 docs.rs 锚点形态：`https://docs.rs/.../de.rs.html#2709-2714`。
fn parse_anchor_range(url: &str) -> Option<(usize, usize)> {
    let hash = url.rfind('#')?;
    let frag = &url[hash + 1..];
    // 仅在 fragment 是 `N-M` 或 `N` 形态时尝试；其它字符（含命名锚点、混合）一律不裁剪
    // （agent 用 fragment 命名锚点是另一类用法，不该误伤）。
    let dash = frag.find('-')?;
    let start_s = &frag[..dash];
    let end_s = &frag[dash + 1..];
    let start: usize = start_s.parse().ok()?;
    let end: usize = end_s.parse().ok()?;
    if start == 0 || end < start {
        return None;
    }
    Some((start, end))
}

/// FixG10 L-2：按行号范围裁剪文本——保留 [start-pad, end+pad]（clamp 到文本边界）；
/// 范围超过文本行数则裁到末尾。返回 (裁后文本, (实际起, 实际终))。
/// pad=0 即严格端点；pad>0 给上下扩 N 行作为上下文（盲测九 L 上下 N 行作参考）。
fn crop_text_lines(text: &str, start: usize, end: usize, pad: usize) -> (String, (usize, usize)) {
    let lines: Vec<&str> = text.split('\n').collect();
    let total = lines.len();
    let s0 = start.saturating_sub(1 + pad); // 1-based → 0-based，再扣 pad
    let s1 = (end + pad).min(total);
    if s0 >= total {
        return (String::new(), (start.min(total), end.min(total)));
    }
    let actual_start = s0 + 1; // 回 1-based 给 meta 标注
    let actual_end = s1;
    (lines[s0..s1].join("\n"), (actual_start, actual_end))
}

/// FixG10 J-2：JSONPath 风格路径解析 + 投影。
/// 支持 `.field` 段与 `[N]` 段（数组下标），不支持复杂查询（递归 `..` / 通配符 / 过滤）。
/// 解析失败 → 整体失败（Err），不静默吞掉（agent 拼错了要报错）。
/// 空 paths → 原样返回（不做投影）。
fn project_json_paths(v: &serde_json::Value, paths: &[String]) -> Result<serde_json::Value> {
    let mut out = serde_json::Map::new();
    for path in paths {
        let segments = parse_json_path(path)?;
        let cur = v;
        let mut node: &serde_json::Value = cur;
        for seg in &segments {
            node = match seg {
                Segment::Field(name) => node.get(name).ok_or_else(|| {
                    anyhow!("json-keys 路径无此字段: {path:?}（在 {name:?} 处失败）")
                })?,
                Segment::Index(i) => node.get(*i).ok_or_else(|| {
                    anyhow!("json-keys 数组下标越界: {path:?}（[{i}] 失败）")
                })?,
            };
        }
        // 把路径末段名作为 key（数组下标则用 `[N]` 形态）
        let key = match segments.last().expect("至少一段") {
            Segment::Field(n) => n.clone(),
            Segment::Index(i) => format!("[{i}]"),
        };
        out.insert(key, node.clone());
    }
    Ok(serde_json::Value::Object(out))
}

enum Segment {
    Field(String),
    Index(usize),
}

/// 解析 `crate.max_version` / `.crate.max_version` / `[0]` / `versions[5].tag` 等形态。
/// 顶段 `.field` / `[N]` / 裸字段名均可；空段 / 不支持语法 → Err。
fn parse_json_path(path: &str) -> Result<Vec<Segment>> {
    let mut segs: Vec<Segment> = Vec::new();
    let mut s = path.trim();
    if s.is_empty() {
        anyhow::bail!("json-keys 路径不能为空");
    }
    // 主循环：每轮处理一段（首段允许裸字段名，无 `.` 前缀；后续段必须有 `.` / `[` 前缀）。
    loop {
        if s.starts_with('[') {
            let close = s[1..].find(']').map(|c| c + 1)
                .ok_or_else(|| anyhow!("json-keys 路径 `[` 未闭合: {path:?}"))?;
            let n: usize = s[1..close].parse()
                .map_err(|_| anyhow!("json-keys 数组下标必须为非负整数: {path:?}"))?;
            segs.push(Segment::Index(n));
            s = &s[close + 1..];
        } else {
            // 字段段：剥前导 `.`（首段可省略），字段名到下一个 `.` / `[` / 末尾
            let after_dot = s.strip_prefix('.');
            let (rest, required_dot) = match after_dot {
                Some(r) => (r, true),
                None => {
                    if !segs.is_empty() {
                        anyhow::bail!("json-keys 路径段必须以 `.` 或 `[` 起首: {path:?}");
                    }
                    (s, false)
                }
            };
            let end = rest.find(['.', '[']).unwrap_or(rest.len());
            let name = &rest[..end];
            if name.is_empty() {
                if required_dot {
                    anyhow::bail!("json-keys 路径空字段段（连续 `.`）: {path:?}");
                }
                anyhow::bail!("json-keys 路径首段字段名为空: {path:?}");
            }
            segs.push(Segment::Field(name.to_string()));
            s = &rest[end..];
        }
        if s.is_empty() {
            return Ok(segs);
        }
    }
}

/// 纯函数：html → 提取 + 截断（limit 注入，离线单测不碰配置）。
/// 非 HTML（text/plain 等）不剥标签不判壳也不实体解码——markdown/JSON 源文保真，
/// 字面 `&`/`&#20013;` 原样保留（agent 取原文场景）。
fn process_html(url: &str, html: &str, is_html: bool, limit: usize) -> Fetched {
    // kda：HTML 解析一次，title 与正文同出树内（字符串扫描版 extract_title 对
    // 属性值含 `>` 的标签同样漏片段）；非 HTML 源文保真，不碰解析器。
    let (title, raw) = if is_html {
        let doc = Html::parse_document(html);
        (tree_title(&doc), collapse_blank(tree_text(&doc)))
    } else {
        (String::new(), collapse_blank(html.to_string()))
    };
    // P1 fetch JSON 截断修复：GitHub API 等 JSON 源按字节截断会出半截 JSON，
    // json.loads 直接 UnclosedBraceError（G 盲测八实锤）。检测文本以 `{`/`[`
    // 开头 → 截断时回退到最后一个完整 `}`/`]` 边界；非 JSON 形态走原 cap_chars。
    let (text, truncated, omitted, off) = cap_chars_json(&raw, limit);
    Fetched {
        url: url.to_string(),
        title,
        text,
        truncated,
        omitted,
        truncated_at_offset: truncated.then_some(off),
        include_hit: None,
        include_hits: None,
        markdown: false,
        github_comment_hint: None,
        anchor_crop_range: None,
        truncated_by_json_keys: false,
    }
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

/// --include：逗号分隔 selector 依序试（scraper 解析），FixG10 J-3 累加所有命中容器（多选器全要），
/// 返回 (拼接后的 inner_html, 命中数)；全未命中 → (None, 0)。命中容器间以 `\n\n---\n\n` 分隔，
/// 让 agent 能识别不同 selector 块的边界。
/// selector 语法错误 → Err：用户显式输入拼错了要报错，静默跳过会伪装成"未命中回退全文"。
fn extract_with_include(html: &str, include: &str) -> Result<Option<(String, usize)>> {
    let doc = Html::parse_document(html);
    let mut collected: Vec<String> = Vec::new();
    for sel in include.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        // day：scraper 的 Display 泄漏内部变体名（EmptySelector/Please report...），映射成用户可操作的文案
        let selector = Selector::parse(sel).map_err(|_| {
            anyhow!("CSS selector 无效: {sel:?}（语法错误，应为合法 CSS 选择器如 \"#main, article\"）")
        })?;
        for el in doc.select(&selector) {
            collected.push(el.inner_html());
        }
    }
    if collected.is_empty() {
        return Ok(None);
    }
    Ok(Some((collected.join("\n\n---\n\n"), collected.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// --include：命中容器取 inner_html；逗号分隔依序试；未命中 None；selector 语法错误 Err。
    #[test]
    fn extract_with_include_hits_and_falls_back() {
        let html = "<html><head><title>T</title></head><body>\
                    <nav>菜单 链接</nav><main><h1>正文标题</h1><p>第一段</p></main></body></html>";
        // article 不存在 → main 命中容器：只取容器内正文
        let (got, hits) = extract_with_include(html, "article,main").unwrap().unwrap();
        assert_eq!(hits, 1, "article 不存在、main 命中 1 个");
        assert!(got.contains("正文标题") && got.contains("第一段"), "got: {got}");
        assert!(!got.contains("菜单"), "nav 内容不应混入: {got}");
        // 全未命中 → None
        assert!(extract_with_include(html, "article,aside").unwrap().is_none());
        // selector 语法错误 → Err（不伪装成"未命中"）
        assert!(extract_with_include(html, "main[").is_err());
    }

    /// FixG10 J-3：--include 多选器累加——同一 selector 多个命中按文档序拼接，不同 selector 也累加，
    /// 命中容器间以 `\n\n---\n\n` 分隔。盲测九 J 实测：releases 列表页 `.release-entry` 仅取首个断章。
    #[test]
    fn extract_with_include_multi_selector_accumulates() {
        // 多 release entries + nav：nav 不会被命中、3 个 release 全部进入 text
        let html = "<html><body><nav>菜单</nav>\
                    <section class='release-entry'><h2>1.5.0</h2><p>首次发布</p></section>\
                    <section class='release-entry'><h2>1.4.0</h2><p>上一个版本</p></section>\
                    <section class='release-entry'><h2>1.3.0</h2><p>远古版本</p></section>\
                    </body></html>";
        let (got, hits) = extract_with_include(html, ".release-entry").unwrap().unwrap();
        assert_eq!(hits, 3, "3 个 release entries 应全部命中: got_hits={hits}");
        assert!(got.contains("1.5.0") && got.contains("1.4.0") && got.contains("1.3.0"), "got: {got}");
        assert!(!got.contains("菜单"), "nav 不应混入: {got}");
        // 拼接分隔符：让 agent 能识别 selector 块边界
        assert!(got.contains("\n\n---\n\n"), "多命中应加分隔符: {got}");
        // 跨 selector 累加：releases + markdown-body 都命中，拼接成两段
        let html2 = "<html><body>\
                     <article class='markdown-body'>正文</article>\
                     <section class='release-entry'><h2>1.0.0</h2></section>\
                     <section class='release-entry'><h2>0.9.0</h2></section>\
                     </body></html>";
        let (got2, hits2) = extract_with_include(html2, ".markdown-body,.release-entry").unwrap().unwrap();
        assert_eq!(hits2, 3, "1 markdown-body + 2 release-entry");
        assert!(got2.contains("正文") && got2.contains("1.0.0") && got2.contains("0.9.0"));
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
            truncated_at_offset: None,
            include_hit: None,
            include_hits: None,
            markdown: false,
            github_comment_hint: github_thread_comment_gap("https://github.com/tokio-rs/tokio/issues/7787"),
            anchor_crop_range: None,
            truncated_by_json_keys: false,
        };
        let v = fetched_json(&f);
        assert_eq!(v["meta"]["github_comments_missing"], serde_json::json!(true));
        assert!(v["meta"]["github_comments_hint"].as_str().unwrap().contains("api.github.com"));

        let f2 = Fetched { url: "https://e.test/".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false };
        let v2 = fetched_json(&f2);
        assert!(v2["meta"].get("github_comments_missing").is_none(), "非 thread 页不得出键");
        assert!(v2["meta"].get("github_comments_hint").is_none());
    }

    /// FixG10 J-2：truncated_by_json_keys=true 时 meta 出键；默认输出结构不变。
    #[test]
    fn fetched_json_truncated_by_json_keys_flag() {
        // true：键出
        let f = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: true };
        let v = fetched_json(&f);
        assert_eq!(v["meta"]["truncated_by_json_keys"], serde_json::json!(true));
        // false：键缺席
        let f2 = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false };
        let v2 = fetched_json(&f2);
        assert!(v2["meta"].get("truncated_by_json_keys").is_none(), "默认不得出键");
    }

    /// 9jx：fetch meta.truncated_at_offset——truncated=true 时出键且值非零，None 时键缺席。
    /// 与 read/browse 路径同契约（缺席 = 未截断，offset 是 cap_chars 返回的截断字节位置）。
    #[test]
    fn fetched_json_truncated_at_offset_emits_when_truncated() {
        // truncated=true + Some(off) → meta 出键
        let f = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: true, omitted: 100, truncated_at_offset: Some(3000), include_hit: None, include_hits: None, markdown: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false };
        let v = fetched_json(&f);
        assert_eq!(v["meta"]["truncated_at_offset"], serde_json::json!(3000));
        // truncated=true + None → 键缺席（不与 truncated 键语义重叠）
        let f2 = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: true, omitted: 100, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false };
        let v2 = fetched_json(&f2);
        assert!(v2["meta"].get("truncated_at_offset").is_none());
        // truncated=false → 键缺席
        let f3 = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false };
        let v3 = fetched_json(&f3);
        assert!(v3["meta"].get("truncated_at_offset").is_none());
    }

    /// 9jx：cap_chars 触发的 truncated_at_offset 真实流转——process_html 走 cap_chars_json
    /// 路径，超限时 Fetched.truncated_at_offset 应为 Some(non-zero)。
    #[test]
    fn process_html_threads_truncated_at_offset() {
        // JSON 源：超限 → cap_chars_json 走 brace 边界回退；offset 应等于截后文本字节长度
        let long_json = r#"{"items":[1,2,3,4,5,6,7,8,9,10],"junk":"x"}"#;
        let f = process_html("https://e.test/a", long_json, false, 8);
        assert!(f.truncated, "应被 cap_chars_json 截断");
        let off = f.truncated_at_offset.expect("truncated=true 时 truncated_at_offset 应为 Some");
        assert!(off > 0, "offset 必须非零: {off}");
        // HTML 源：超限 → cap_chars_json 走非 JSON 兜底（仍以 `{` 起首但不裁 brace）走硬截，
        // offset 仍非零
        let html = "<html><body><p>一段很长的中文文本用于触发 cap_chars 截断。".repeat(50)
            + "</p></body></html>";
        let f2 = process_html("https://e.test/b", &html, true, 30);
        assert!(f2.truncated);
        assert!(f2.truncated_at_offset.is_some());
    }

    /// FixG10 J-2：project_json_body 投影 raw body；非 JSON 静默返回 None。
    #[test]
    fn project_json_body_returns_projection_or_none() {
        // 命中：投影
        let body = r#"{"crate":{"max_version":"1.40.0","max_stable_version":"1.39.2","junk":[1,2,3]}}"#;
        let out = project_json_body(body, &["crate.max_stable_version".into()]).unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["max_stable_version"], serde_json::json!("1.39.2"));
        // 非 JSON：None
        let out2 = project_json_body("<html>not json</html>", &["crate.max_stable_version".into()]).unwrap();
        assert!(out2.is_none());
        // 路径错误：Err（best-effort 由调用方打 stderr）
        assert!(project_json_body(body, &["crate.nonexistent".into()]).is_err());
    }

    /// FixG10 L-2：parse_anchor_range 解析 URL `#N-M` 数字范围；命名锚点 / 单值无 `-` / 颠倒起终 → None。
    #[test]
    fn parse_anchor_range_handles_numeric_range_only() {
        // 命中
        assert_eq!(parse_anchor_range("https://docs.rs/x/y.html#2709-2714"), Some((2709, 2714)));
        assert_eq!(parse_anchor_range("https://docs.rs/x/y.html#1-1"), Some((1, 1)));
        // 颠倒起终（end < start）→ None
        assert_eq!(parse_anchor_range("https://docs.rs/x.html#10-5"), None);
        // 无 anchor
        assert_eq!(parse_anchor_range("https://docs.rs/x/y.html"), None);
        // 命名锚点（如 GitHub 主题 #foo）→ None（不该误伤）
        assert_eq!(parse_anchor_range("https://github.com/o/r/issues/1#issuecomment-123"), None);
        // 起 0 → None（行号 1-based）
        assert_eq!(parse_anchor_range("https://x.com/y#0-5"), None);
    }

    /// FixG10 L-2：crop_text_lines 按 1-based 范围裁切 + pad 上下文。
    #[test]
    fn crop_text_lines_crops_with_padding() {
        let text = "a\nb\nc\nd\ne\nf\ng"; // 7 行
        // pad=0：第 3-4 行精确取
        let (crop, range) = crop_text_lines(text, 3, 4, 0);
        assert_eq!(crop, "c\nd");
        assert_eq!(range, (3, 4));
        // pad=1：扩上下 1 行（2-5）
        let (crop, range) = crop_text_lines(text, 3, 4, 1);
        assert_eq!(crop, "b\nc\nd\ne");
        assert_eq!(range, (2, 5));
        // 越界：end 超过总行数 → 截到末尾
        let (crop, range) = crop_text_lines(text, 5, 100, 0);
        assert_eq!(crop, "e\nf\ng");
        assert_eq!(range, (5, 7));
        // 起始越界：start 超过总行数 → 空
        let (crop, _range) = crop_text_lines(text, 100, 200, 0);
        assert!(crop.is_empty());
    }

    /// FixG10 J-2：project_json_paths 投影到指定字段；支持 .field 与 [N]；路径错误 Err。
    #[test]
    fn project_json_paths_filters_to_selected_fields() {
        let payload = serde_json::json!({
            "crate": {
                "max_version": "1.40.0",
                "max_stable_version": "1.39.2",
                "newest_version": "1.40.0",
                "categories": ["a","b","c","d","e","f","g","h","i","j"],
                "downloads": 99999999,
            },
            "versions": [
                {"num": "1.39.2", "yanked": false},
                {"num": "1.39.1", "yanked": true},
            ],
        });
        // 裸首段（无前导 .）：合法
        let out = project_json_paths(&payload, &["crate.max_stable_version".into()]).unwrap();
        assert_eq!(out["max_stable_version"], serde_json::json!("1.39.2"));
        assert_eq!(out.as_object().unwrap().len(), 1);
        // 多字段 + 数组下标
        let out = project_json_paths(&payload, &["crate.max_version".into(), "versions[0].num".into()]).unwrap();
        assert_eq!(out["max_version"], serde_json::json!("1.40.0"));
        // versions[0].num 的末段是 Field("num") → key 名为 "num"
        assert_eq!(out["num"], serde_json::json!("1.39.2"));
        // 仅数组下标（直接 `[0]`）→ key 用 `[0]` 形态
        let out2 = project_json_paths(&payload, &["versions[0]".into()]).unwrap();
        assert_eq!(out2["[0]"], serde_json::json!({"num": "1.39.2", "yanked": false}));
        // 字段不存在 → Err（不静默吞掉）
        assert!(project_json_paths(&payload, &["crate.nonexistent".into()]).is_err());
        // 数组下标越界 → Err
        assert!(project_json_paths(&payload, &["versions[99].num".into()]).is_err());
        // 前导 . 形式仍兼容
        let out3 = project_json_paths(&payload, &[".crate.max_version".into()]).unwrap();
        assert_eq!(out3["max_version"], serde_json::json!("1.40.0"));
    }

    /// FixG10 J-1：should_retry 决策矩阵——确定性错误 / 4xx / 5xx / 网络错 / budget 用尽。
    #[test]
    fn should_retry_decision_matrix() {
        // 确定性错误（私网门拒）：不重试
        assert!(!should_retry("fetch 拒绝私网地址", true));
        assert!(!should_retry("fetch 仅支持 https", true));
        assert!(!should_retry("PDF 二进制内容", true));
        assert!(!should_retry("二进制内容", true));
        // 4xx 客户端错（除 408/429）：不重试
        assert!(!should_retry("HTTP 404 Not Found", true));
        assert!(!should_retry("HTTP 403 Forbidden", true));
        // 4xx 但 408/429：仍可重试
        assert!(should_retry("HTTP 408 Request Timeout", true));
        assert!(should_retry("HTTP 429 Too Many Requests", true));
        // 5xx：重试
        assert!(should_retry("HTTP 502 Bad Gateway", true));
        assert!(should_retry("HTTP 503 Service Unavailable", true));
        assert!(should_retry("HTTP 504 Gateway Timeout", true));
        // 网络错（无 HTTP 字样）：重试
        assert!(should_retry("请求失败: connection refused", true));
        assert!(should_retry("error sending request", true));
        // budget 用尽：不重试
        assert!(!should_retry("HTTP 503 Service Unavailable", false));
        assert!(!should_retry("请求失败: timeout", false));
    }
}
