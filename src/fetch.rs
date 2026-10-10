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

/// fetch 总超时默认（FixG10 J-1 + FixG11：--timeout 默认值封装在 FetchOpts::default，便于 main 复用；
/// FixG11 把 10s→30s 给 GitHub 抖动自愈留余量）。
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
    /// FixG19 JJ：--raw 逃生门——text = HTTP 响应体原样（零提取/零清洗，meta.format="raw"）。
    /// 精确性自证用：对比提取器改了什么全靠它。提取漏斗全部跳过（json_keys 投影/summary 剥除/
    /// 正文提取/markdown/host 路由/include/锚点裁剪/JS 壳判定）；--max-chars 仍截断（meta 如实）；
    /// 与 --markdown/--include/--json-keys 互斥（clap 拒）。PDF/二进制拒与 SSRF 门不绕（共享 GET 路径）。
    pub raw: bool,
    /// 正文字符预算（text 上限；超限截断，meta.truncated/omitted 如实标注）。CLI 默认 50000。
    pub max_chars: usize,
    /// FixG10 J-1 + FixG11：单请求超时（秒）；CLI 默认 30（盲测十 P0-3 GitHub 抖动自愈），
    /// 范围 1..=300。改默认是 breaking：理由是 GitHub .diff / tag 页偶发握手失败吃满 10s 才退，
    /// 真实场景 fetch 公网静态页几乎不会真有 30s 慢响应——放宽预算换抖动自愈。
    pub timeout_secs: u64,
    /// FixG10 J-1 + FixG11：失败重试次数（不含首次）；CLI 默认 1（盲测十 P0-3 公网抖动 1 次自愈），
    /// 范围 0..=3。backoff：1s, 2s, 4s（第 N 次等待 2^(N-1) 秒）。每次重试 stderr 一行提示。
    /// 改默认是 breaking：旧默认 0 等价「失败立即返回」——用户首次碰到 GitHub 5xx 必挂，
    /// 1 次重试 + 抖动 1s 几乎无感（确定性错误私网门拒 / scheme / PDF / 二进制仍不重试）。
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
            raw: false,
            max_chars: 50_000,
            timeout_secs: 30,
            retry: 1,
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
/// - IPv6: ::1、fc00::/7（ULA）、fe80::/10（link-local）；::ffff:x.y.z.w 先转 V4 再判（qmg）
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
            // qmg：IPv4-mapped（::ffff:a.b.c.d）先转 V4 判私网——v4 私网换 v6 伪装不得绕门
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
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
/// qmg：host 提取走 url crate（reqwest::Url 再导出，与连接层同一 parser）——手工切片
/// 不认 userinfo（`http://a.com:80@192.168.1.1/` 门判公网、实连私网），reqwest 实际
/// 连接的 host 与本门判定的 host 由此保证零分歧；IPv6 字面量也从解析结果直取。
pub(crate) fn classify_url(url: &str) -> Result<(String, IpAddr, bool)> {
    let parsed = reqwest::Url::parse(url).with_context(|| format!("URL 解析失败: {url}"))?;
    let host_str = parsed.host_str().ok_or_else(|| anyhow!("URL host 为空: {url}"))?;
    // IPv6 字面量序列化带方括号（[::1]）：成对剥除后再走字面量/DNS 判定
    let host = host_str
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host_str);
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

/// uvi：单命令一次构建 client——(proxy, allow, timeout) 三参数在单条命令生命周期内恒定
/// （FixG10 J-1 的 --timeout clamp 也收敛在这唯一一处）；重定向 SSRF 门是 builder 级
/// Policy::custom，复用同一 client 不受影响。
fn command_client(opts: &FetchOpts) -> Result<reqwest::Client> {
    let allow = allow_private_requested(opts.allow_private);
    let timeout_secs = opts.timeout_secs.clamp(1, FETCH_TIMEOUT_MAX_SECS);
    build_client(opts.proxy.as_deref(), allow, Duration::from_secs(timeout_secs))
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
/// uvi：client 由命令入口构建一次传入（三参数命令内恒定），重试 loop 不再逐次重建。
async fn fetch_one(url: &str, opts: &FetchOpts, client: &reqwest::Client) -> Result<FetchOne> {
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
        match fetch_one_attempt(url, opts, client).await {
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
/// uvi：client 复用命令入口的构建（timeout/proxy/allow 已烘焙进 client），不再每 attempt 重建。
async fn fetch_one_attempt(
    url: &str,
    opts: &FetchOpts,
    client: &reqwest::Client,
) -> Result<FetchOne> {
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
    let limit = opts.max_chars;
    // FixG19 JJ：--raw 逃生门——body 原样进 text，提取漏斗全部跳过（含 JS 壳判定：
    // 用户显式要原始字节时「正文太短需渲染」的提示无意义）。body 硬上限截断照常如实进 meta。
    if opts.raw {
        return Ok(FetchOne::Done(raw_fetched(url, &html, limit, truncated)));
    }
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

    // FixG12：<summary> 是折叠控件按钮文本（docs.rs 的 "Expand description" 拼进签名行），
    // 提取漏斗单点剥除——host 路由（docs.rs <main> / GitHub 容器）与 --include 都从原始 html
    // 重新提取正文，会绕过 process_html 内的同款剥除，必须在漏斗处先剥；非 HTML 源文保真不碰。
    if is_html {
        html = strip_summary_elements(&html);
    }
    let mut fetched = process_html(url, &html, is_html, limit);
    fetched.truncated_by_json_keys = json_projected;
    fetched.github_comment_hint = github_thread_comment_gap(url);
    // xih：--markdown 在剥标签前的原始 HTML 上转换（保表格/标题/链接结构），text 字段换源；
    // 非 HTML（text/plain / JSON / md 源文）本就是文本，原样保留。
    if opts.markdown && is_html {
        let (md, t, o, off) =
            cap_chars(&crate::convert::clean_markdown(crate::convert::html_to_markdown(&html)?), limit);
        fetched.text = md;
        fetched.truncated = t;
        fetched.omitted = o;
        fetched.truncated_at_offset = t.then_some(off);
        fetched.markdown = true;
    }
    // FixG11 host 路由：未传 --include 时按 host 给默认 selector——github 走 skeleton 容器链 +
    // nav 剥离，docs.rs 走 <main>（全站侧栏绕过）。必须在 markdown 转换之后（与 --include 同位置）。
    // FixG12：host 判定恒标注——用户显式 --include 不被覆盖，但 auto_include_applied 保留
    // host 想命中的 label，include_overridden_by_user=true 标注覆盖事实（消费方一眼可辨）。
    if let Some((label, overridden)) = host_route_decision(opts.include.as_deref(), url) {
        if overridden {
            fetched.auto_include_applied = Some(label.to_string());
            fetched.include_overridden_by_user = true;
        } else {
            let applied = match label {
                "github" => apply_github_host_route(&mut fetched, &html, opts.markdown, limit)?,
                "docs.rs" => apply_docsrs_host_route(&mut fetched, &html, is_html, opts.markdown, limit)?,
                _ => false,
            };
            if applied {
                fetched.auto_include_applied = Some(label.to_string());
            }
        }
    }
    // --include：命中容器则用容器内 HTML 重新提取正文（title 仍取页面级）；未命中回退全文提取。
    if let Some(include) = &opts.include {
        fetched.include_hit = Some(false);
        if is_html
            && let Some((blocks, hits)) = extract_with_include(&html, include)?
        {
            let raw = render_include_blocks(&blocks, opts.markdown)?;
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
                raw: false,
                github_comment_hint: fetched.github_comment_hint,
                anchor_crop_range: None,
                truncated_by_json_keys: false,
                auto_include_applied: fetched.auto_include_applied,
                include_overridden_by_user: fetched.include_overridden_by_user,
            };
        }
    }
    // FixG13：用户 --include 覆盖且未命中 selector（回退全文）→ 补走 host 路由剥离；
    // 命中用户 selector 时 include_hit=Some(true)，本调用是 no-op。
    if let Some((label, true)) = host_route_decision(opts.include.as_deref(), url) {
        apply_host_route_on_include_fallback(
            &mut fetched,
            label,
            &html,
            is_html,
            opts.markdown,
            limit,
        )?;
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

/// FixG19：--raw 逃生门的产物构造——body 逐字符原样进 text（cap_chars 截断 meta 如实），
/// body 硬上限截断（FETCH_BODY_LIMIT）照常累计进 truncated/omitted。
fn raw_fetched(url: &str, body: &str, limit: usize, body_truncated: bool) -> Fetched {
    let (text, t, o, off) = cap_chars(body, limit);
    let mut f = Fetched {
        url: url.to_string(),
        title: String::new(),
        text,
        truncated: t,
        omitted: o,
        truncated_at_offset: t.then_some(off),
        include_hit: None,
        include_hits: None,
        markdown: false,
        raw: true,
        github_comment_hint: None,
        anchor_crop_range: None,
        truncated_by_json_keys: false,
        auto_include_applied: None,
        include_overridden_by_user: false,
    };
    if body_truncated {
        f.truncated = true;
        f.omitted = f.omitted.saturating_add(FETCH_BODY_LIMIT);
    }
    f
}

/// `gsearch fetch <url>...`：GET → 轻量正文提取 → 人读 / --json 输出。
/// 单 URL = 原行为；多 URL = batch 并发（上限 FETCH_CONCURRENCY、单条失败不阻塞、
/// 退出码 0 全成功 / 1 部分失败 / 2 全失败，对齐 search batch）。
/// 退出码：0 成功；1 JS 壳（需渲染）/ 私网门拒（由 main 统一打印，HTTP 错误经 anyhow → exit 1）。
pub async fn cmd_fetch(urls: &[String], opts: &FetchOpts) -> Result<ExitCode> {
    let started = Instant::now();
    // uvi：单命令单 client——单条与 batch 共用（fetch_one_attempt 不再每 URL×attempt 重建）
    let client = command_client(opts)?;
    if let [url] = urls {
        return match fetch_one(url, opts, &client).await {
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
    let code = cmd_fetch_batch(urls, opts, &client).await?;
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
    let projected = match project_json_paths(&parsed, keys) {
        Ok(p) => p,
        Err(e) => {
            // 顶层数组误用裸字段路径是最常见错法（GitHub comments/releases API）——
            // 教可行动语法而非让 agent 拿全量回退自己猜（盲测十四 W）。已用 [*] 的失败
            // 是元素缺字段，再教 [*] 会误导，不加。
            if parsed.is_array() && !keys.iter().any(|k| k.contains('*')) {
                anyhow::bail!("{e:#}\n响应为顶层数组：请用 `0.field` 索引语法（如 `0.user.login`）或 `[*]` 通配（如 `[*].tag_name`）");
            }
            return Err(e);
        }
    };
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
/// uvi：client 由 cmd_fetch 构建一次传入，全 batch 共用。
async fn cmd_fetch_batch(urls: &[String], opts: &FetchOpts, client: &reqwest::Client) -> Result<ExitCode> {
    use futures::stream::{StreamExt, iter};
    let fetched: Vec<(String, Result<FetchOne>)> = iter(urls.iter().cloned())
        .map(|u| async move {
            let r = fetch_one(&u, opts, client).await;
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
                // status:"ok" 由 fetched_json 统一注入（FixG20 NN，单/批同源）
                entries.push(fetched_json(f));
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

/// GitHub issue/PR 页的评论区由 JS 动态加载，SSR HTML 最多带少量已渲染评论（无作者归属、
/// 不完整）——正文并非"零评论"但也不完整，hint 如实说"仅部分包含"并给升级路径（盲测七
/// wqm：输出自称完整，agent 据此误判「无讨论」；盲测十四 W："未包含"绝对断言与正文混入
/// 部分评论自相矛盾）。非 GitHub thread 页 None。
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
        "评论区仅部分包含在本输出正文中（可能缺作者归属且不完整，勿据本文判断完整讨论）；\
         完整讨论：gsearch browse {url} --markdown，或 GET https://api.github.com/repos/{owner}/{repo}/issues/{number}/comments"
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
    if f.raw {
        meta["format"] = serde_json::json!("raw");
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
    // FixG12：host 级默认 include 路由判定——host 命中即标注（含被用户显式 --include 覆盖时，
    // label 恒在）；host 未命中（未知 host / 裸 host）或命中但容器未匹配 → 键缺席（默认输出结构不变）。
    if let Some(label) = &f.auto_include_applied {
        meta["auto_include_applied"] = serde_json::json!(label);
    }
    // FixG12：host 判定被用户显式 --include 覆盖的标注（仅 true 出键，缺席 = 无覆盖）。
    if f.include_overridden_by_user {
        meta["include_overridden_by_user"] = serde_json::json!(true);
    }
    // 9jx：截断字节偏移（truncated=true 时填，缺席=未截断）。
    if let Some(off) = f.truncated_at_offset {
        meta["truncated_at_offset"] = serde_json::json!(off);
    }
    // FixG16：投影命中时 text 为原生 JSON Value（对象/数组原样，agent 免二次解析）；
    // 投影产物被字符预算截成非法 JSON 时回退 string。非投影路径恒为 string（老输出不变）。
    let text = if f.truncated_by_json_keys {
        serde_json::from_str::<serde_json::Value>(&f.text).unwrap_or_else(|_| f.text.clone().into())
    } else {
        f.text.clone().into()
    };
    // FixG17：投影命中且投影含 title 时顶层 title 回填投影值——顶层 title 原是页面级
    // <title>（JSON API 响应取不到，恒空串），与 text.title 并存造成首读困惑。
    // 无 title 路径 / 投影值非字符串 / 顶层数组 → 维持页面级 title（现状）。
    let title = match text.get("title") {
        Some(serde_json::Value::String(s)) => s.clone(),
        _ => f.title.clone(),
    };
    // FixG20 NN：顶层补 status:"ok"——单 URL 扁平形态与 batch 数组元素同键，消费方按
    // URL 数无需分支解析（NN 曾按 batch 形态解析单 URL 撞 KeyError）；只增不删。
    serde_json::json!({
        "url": f.url,
        "title": title,
        "text": text,
        "meta": meta,
        "status": "ok",
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
    /// FixG19：--raw 逃生门标记（--json 据此写 meta.format="raw"；与 markdown 互斥）。
    raw: bool,
    /// GitHub issue/PR 页的评论区缺失信号（None = 非 thread 页，键缺席）。
    github_comment_hint: Option<String>,
    /// FixG10 L-2：URL `#N-M` 锚点裁剪范围（1-based 行号，含端点）；未命中锚点 None。
    anchor_crop_range: Option<(usize, usize)>,
    /// FixG10 J-2：--json-keys 命中标记；true 时 meta.truncated_by_json_keys 出现。
    truncated_by_json_keys: bool,
    /// FixG11：host 级默认 include 路由标注——Some(label) 表示该 URL 命中 host 路由
    /// （label="github" 走 skeleton::github_comment_html 容器链+nav 剥离；"docs.rs" 走
    /// --include="main" 等价路径）。FixG12 语义：host 判定恒保留——用户显式 --include 时
    /// label 仍标注（host 想命中哪里），覆盖事实由 include_overridden_by_user 出键；
    /// None = host 无路由（未知 host / 裸 host）或 host 命中但容器未匹配且未用 --include。
    /// meta.auto_include_applied 仅在 Some 时出键，默认输出结构不变。
    auto_include_applied: Option<String>,
    /// FixG12：host 自动路由被用户显式 --include 覆盖时 true；meta 仅 true 出键
    /// （false / 缺席 = 无覆盖发生，默认输出结构不变）。
    include_overridden_by_user: bool,
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
/// 支持 `.field` 段 / `[N]` 段（数组下标）/ 裸数字段 / `[*]` 数组通配（FixG15），
/// 不支持复杂查询（递归 `..` / 过滤）。
/// 解析失败 → 整体失败（Err），不静默吞掉（agent 拼错了要报错）。
/// 空 paths → 原样返回（不做投影）。
/// FixG14：多路径末段名冲突（`items.0.title` / `items.1.title` 都要写 key "title"）时，
/// 冲突 key 改用全路径形态输出，不再静默 last-write-wins；单路径/无冲突保持末段短名。
fn project_json_paths(v: &serde_json::Value, paths: &[String]) -> Result<serde_json::Value> {
    // 先求值全部路径，再统一定 key 形态（边插边定无法回头改名）。
    let mut resolved: Vec<(String, serde_json::Value)> = Vec::with_capacity(paths.len());
    for path in paths {
        let segments = parse_json_path(path)?;
        resolved.push(eval_json_path(v, &segments, path)?);
    }
    // 冲突检测：路径数是个位数，O(n²) 两两比对足够
    let mut conflicted = vec![false; resolved.len()];
    for i in 0..resolved.len() {
        for j in 0..i {
            if resolved[i].0 == resolved[j].0 {
                conflicted[i] = true;
                conflicted[j] = true;
            }
        }
    }
    let mut out = serde_json::Map::new();
    for (i, (key, value)) in resolved.into_iter().enumerate() {
        // 冲突 key → 全路径形态（用户路径规范化：去首尾空白与前导点）
        let final_key = if conflicted[i] {
            paths[i].trim().trim_start_matches('.').to_string()
        } else {
            key
        };
        out.insert(final_key, value);
    }
    Ok(serde_json::Value::Object(out))
}

/// 单条路径求值，返回 (输出 key, 投影值)。无通配：现行为——key 取末段名（数组下标用
/// `[N]` 形态）。含 `[*]`（FixG15）：通配前段定位到数组，其余段对每个元素求值，值为数组
/// 形态、key 取通配后末段名（纯 `[*]` → `"[*]"`）；一条路径最多一个 `[*]`，作用于非数组
/// → Err（不静默跳过回退全量，盲测十四 V）。
fn eval_json_path(
    v: &serde_json::Value,
    segs: &[Segment],
    path: &str,
) -> Result<(String, serde_json::Value)> {
    let Some(wi) = segs.iter().position(|s| *s == Segment::Wildcard) else {
        let mut node: &serde_json::Value = v;
        for seg in segs {
            node = step_json_path(node, seg, path)?;
        }
        let key = match segs.last().expect("至少一段") {
            Segment::Field(n) => n.clone(),
            Segment::Index(i) => format!("[{i}]"),
            Segment::Wildcard => unreachable!("position 为 None 即无通配"),
        };
        return Ok((key, node.clone()));
    };
    if segs[wi + 1..].contains(&Segment::Wildcard) {
        anyhow::bail!("json-keys 一条路径最多一个 [*] 通配: {path:?}");
    }
    // 通配前段定位到数组节点（如 `data.items[*].tag_name` 的 `data.items`）
    let mut node: &serde_json::Value = v;
    for seg in &segs[..wi] {
        node = step_json_path(node, seg, path)?;
    }
    let arr = node
        .as_array()
        .ok_or_else(|| anyhow!("json-keys 通配符 [*] 要求当前节点为数组: {path:?}"))?;
    let mut vals = Vec::with_capacity(arr.len());
    for el in arr {
        let mut n = el;
        for seg in &segs[wi + 1..] {
            n = step_json_path(n, seg, path)?;
        }
        vals.push(n.clone());
    }
    let key = match segs.last().expect("至少一段") {
        Segment::Field(n) => n.clone(),
        Segment::Index(i) => format!("[{i}]"),
        Segment::Wildcard => "[*]".to_string(),
    };
    Ok((key, serde_json::Value::Array(vals)))
}

/// 单步求值：Field/Index 走既有语义；Wildcard 只在 eval_json_path 的扇出层消费，
/// 走到这里说明一条路径里有第二个 `[*]`。
fn step_json_path<'a>(
    node: &'a serde_json::Value,
    seg: &Segment,
    path: &str,
) -> Result<&'a serde_json::Value> {
    match seg {
        Segment::Field(name) => node.get(name).ok_or_else(|| {
            anyhow!("json-keys 路径无此字段: {path:?}（在 {name:?} 处失败）")
        }),
        Segment::Index(i) => node.get(*i).ok_or_else(|| {
            anyhow!("json-keys 数组下标越界: {path:?}（[{i}] 失败）")
        }),
        Segment::Wildcard => anyhow::bail!("json-keys 一条路径最多一个 [*] 通配: {path:?}"),
    }
}

#[derive(Debug, PartialEq)]
enum Segment {
    Field(String),
    Index(usize),
    /// `[*]` 数组通配（FixG15）：对其余段在当前数组的每个元素上求值。
    Wildcard,
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
            if &s[1..close] == "*" {
                segs.push(Segment::Wildcard);
            } else {
                let n: usize = s[1..close].parse()
                    .map_err(|_| anyhow!("json-keys 数组下标必须为非负整数: {path:?}"))?;
                segs.push(Segment::Index(n));
            }
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
            // FixG13：裸纯数字段 = 数组下标（GitHub comments API 顶层数组 `0.user.login`）；
            // 非纯数字（含 `-1`、`0abc`）维持字段名语义不变。
            if let Ok(n) = name.parse::<usize>() {
                segs.push(Segment::Index(n));
            } else {
                segs.push(Segment::Field(name.to_string()));
            }
            s = &rest[end..];
        }
        if s.is_empty() {
            return Ok(segs);
        }
    }
}

/// FixG11 host 路由默认 selector（盲测十 P0-1/P0-2）：
///
/// - github.com → skeleton 容器优先级链（markdown-body/issue-body/article/.markdown-body + 旧回退
///   .js-comment-body/.comment-body）；命中后再按 SEL_GITHUB_NAV 剥 nav（Platform/Solutions/Resources）
/// - docs.rs → `<main>`（rustdoc 文档主体）
///
/// 返回 (label, selector)；调用方按 label 决定是用 selector 走 extract_with_include 路径（docs.rs）
/// 还是直接调 skeleton::github_comment_html 预处理 HTML（github，因 nav 剥离无法用 selector 表达）。
///
/// 纯函数：只比对 host，不发起网络，离线单测覆盖。
fn host_default_include(url: &str) -> Option<(&'static str, &'static str)> {
    let lower = url.trim_start().to_ascii_lowercase();
    // host 前缀匹配（path 必须非空——裸 host 不算内容页，避免对首页误命中）；
    // 大小写不敏感（先 to_ascii_lowercase）。
    let (rest, label) = if let Some(r) = lower
        .strip_prefix("https://github.com/")
        .or_else(|| lower.strip_prefix("http://github.com/"))
    {
        (r, "github")
    } else {
        let r = lower
            .strip_prefix("https://docs.rs/")
            .or_else(|| lower.strip_prefix("http://docs.rs/"))?;
        (r, "docs.rs")
    };
    if rest.is_empty() {
        return None;
    }
    let selector = match label {
        "github" => GITHUB_CONTAINER_CHAIN,
        "docs.rs" => "main",
        _ => unreachable!("label 闭集：仅 github/docs.rs 两值"),
    };
    Some((label, selector))
}

/// GitHub 容器优先级链（与 skeleton::SEL_GITHUB_NEW 同款 + 旧回退）；host_default_include
/// 返回给调用方的 selector——但调用方对 github 走 skeleton::github_comment_html 预处理而非
/// 本 selector（nav 剥离无法用 CSS 选择器表达），此常量仅用于标注「host 路由想命中这里」。
const GITHUB_CONTAINER_CHAIN: &str =
    r#"[data-testid="markdown-body"], [data-testid="issue-body"], [role="article"], .markdown-body, .js-comment-body, .comment-body"#;

/// host 路由判定——Some((label, overridden))：host 命中已知默认值时恒返回判定结果；
/// overridden=true 表示用户显式 --include（判定保留但 body 按用户 selector 提取）。
/// None = host 无路由（未知 host / 裸 host）。抽出便于单测（FixG12 扩字段语义）。
fn host_route_decision(opts_include: Option<&str>, url: &str) -> Option<(&'static str, bool)> {
    let label = host_default_include(url).map(|(label, _)| label)?;
    Some((label, opts_include.is_some()))
}

/// FixG11 host 路由：github 走 skeleton::github_comment_html 同时获得容器链命中 + nav 剥离；
/// 返回 Ok(true) 表示改写了 fetched.text，Ok(false) 表示无容器命中（releases/blob/repo 首页等）
/// 退回 process_html 输出，None = 不设 auto_include_applied。
fn apply_github_host_route(
    fetched: &mut Fetched,
    html: &str,
    markdown: bool,
    limit: usize,
) -> Result<bool> {
    let cleaned = match gsearch::skeleton::github_comment_html(html) {
        Some(c) => c,
        None => return Ok(false),
    };
    let raw = if markdown {
        crate::convert::clean_markdown(crate::convert::html_to_markdown(&cleaned)?)
    } else {
        extract_text(&cleaned)
    };
    // FixG12：PR/issue 标题栏（.gh-header-title 等）在正文容器外——容器改写后的正文从描述起，
    // caller 不知在看哪条 PR。用页面级 title 兜底前缀；正文头部已含标题则不重复加。
    let raw = with_title_prefix(&fetched.title, raw);
    let (text, t, o, off) = cap_chars(&raw, limit);
    fetched.text = text;
    fetched.truncated = t;
    fetched.omitted = o;
    fetched.truncated_at_offset = t.then_some(off);
    Ok(true)
}

/// FixG13：用户 --include 覆盖且未命中 selector（include_hit≠true）→ 回退全文补走 host
/// 路由剥离——回退正文不该比无 --include 时更脏（盲测十二 P：覆盖回退正文混入
/// "Skip to content"/"Navigation Menu"）。命中用户 selector 时不调用本函数（用户明确要
/// 什么就是什么，selector 提取的容器原样保留）。返回 true = 已改写 text；此时 include_hit
/// 复位 Some(false)（命中标志只对应用户 selector，apply_docsrs 内部设的 true 撤回）。
fn apply_host_route_on_include_fallback(
    fetched: &mut Fetched,
    label: &'static str,
    html: &str,
    is_html: bool,
    markdown: bool,
    limit: usize,
) -> Result<bool> {
    if fetched.include_hit == Some(true) {
        return Ok(false);
    }
    let applied = match label {
        "github" => apply_github_host_route(fetched, html, markdown, limit)?,
        "docs.rs" => apply_docsrs_host_route(fetched, html, is_html, markdown, limit)?,
        _ => false,
    };
    if applied {
        fetched.include_hit = Some(false);
        fetched.include_hits = None;
    }
    Ok(applied)
}

/// FixG12：正文头部缺页面标题时补 `# {title}\n\n` 前缀（GitHub 页 title 形如
/// "fs: support io_uring by .. · Pull Request #7696 · tokio-rs/tokio"，含 PR/issue 标题）。
/// 以 title 前 24 字符在正文前 300 字符内是否出现判重；title 为空或已含 → 原样返回。
fn with_title_prefix(title: &str, body: String) -> String {
    let title = title.trim();
    if title.is_empty() {
        return body;
    }
    let probe: String = title.chars().take(24).collect();
    let head: String = body.chars().take(300).collect();
    if head.contains(probe.as_str()) {
        return body;
    }
    format!("# {title}\n\n{body}")
}

/// FixG11 host 路由：docs.rs 走 --include="main" 等价路径（extract_with_include 同款）；
/// 非 HTML 或无 <main> → 退回 process_html 输出。
fn apply_docsrs_host_route(
    fetched: &mut Fetched,
    html: &str,
    is_html: bool,
    markdown: bool,
    limit: usize,
) -> Result<bool> {
    if !is_html {
        return Ok(false);
    }
    let Some((blocks, hits)) = extract_with_include(html, "main")? else {
        return Ok(false);
    };
    let raw = render_include_blocks(&blocks, markdown)?;
    let (text, t, o, off) = cap_chars(&raw, limit);
    fetched.text = text;
    fetched.truncated = t;
    fetched.omitted = o;
    fetched.truncated_at_offset = t.then_some(off);
    fetched.include_hit = Some(true);
    fetched.include_hits = Some(hits);
    Ok(true)
}

/// FixG12：剥 `<summary>…</summary>` 整段。summary 语义就是折叠控件标题（`<details>` 的按钮），
/// 任何站点都不属正文（泛规则，非 docs.rs 专属）。字符串级扫描（scraper 树不支持删节点），
/// 仅按小写标签名扫描；未闭合 summary 不再剥、余下内容原样保留（残缺页不误伤）。
fn strip_summary_elements(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(pos) = rest.find("<summary") {
        let after_name = &rest[pos + "<summary".len()..];
        // 标签边界：`<summary>` / `<summary ...>`；`<summaryx` 是别的元素，原样放行
        if !after_name.starts_with('>') && !after_name.starts_with(char::is_whitespace) {
            out.push_str(&rest[..pos + "<summary".len()]);
            rest = after_name;
            continue;
        }
        out.push_str(&rest[..pos]);
        let Some(tag_end) = rest[pos..].find('>') else { break };
        let after_open = &rest[pos + tag_end + 1..];
        match after_open.find("</summary>") {
            Some(close) => rest = &after_open[close + "</summary>".len()..],
            None => {
                out.push_str(&rest[pos..pos + tag_end + 1]);
                rest = after_open;
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 纯函数：html → 提取 + 截断（limit 注入，离线单测不碰配置）。
/// 非 HTML（text/plain 等）不剥标签不判壳也不实体解码——markdown/JSON 源文保真，
/// 字面 `&`/`&#20013;` 原样保留（agent 取原文场景）。
fn process_html(url: &str, html: &str, is_html: bool, limit: usize) -> Fetched {
    // kda：HTML 解析一次，title 与正文同出树内（字符串扫描版 extract_title 对
    // 属性值含 `>` 的标签同样漏片段）；非 HTML 源文保真，不碰解析器。
    // FixG12：<summary> 折叠按钮文本不属正文（泛规则）；此处剥除同时是独立调用本函数
    // 路径的守门（fetch 漏斗处另有单点剥除，幂等）。
    let (title, raw) = if is_html {
        let doc = Html::parse_document(&strip_summary_elements(html));
        (tree_title(&doc), collapse_preserving_code(tree_text(&doc)))
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
        raw: false,
        github_comment_hint: None,
        anchor_crop_range: None,
        truncated_by_json_keys: false,
        auto_include_applied: None,
        include_overridden_by_user: false,
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
                // wbr = 零宽可断行点，浏览器渲染不占宽——注入分隔符会把 from_<wbr>str
                // 撕成 "from_ str"（盲测十二 R 游离空格实锤）
                if el.name() == "wbr" {
                    continue;
                }
                let sep = if is_block_boundary(el.name()) { '\n' } else { ' ' };
                if matches!(el.name(), "pre" | "code") {
                    // FixG13：pre/code 内空白是内容——子树文本原样输出，\0 哨兵段由
                    // collapse_preserving_code 跳过规整；整树收集不再递归入主栈。
                    // FixG14：边界分隔符必须放进哨兵段内——哨兵段外 prose 段首尾空白会被
                    // collapse_blank trim，放外面必丢（"byT"/"Result<T>where" 盲测十三实锤）。
                    let block = is_block_boundary(el.name()); // pre=块级, code=行内
                    // 前置：块级一律 '\n'（文档起始除外）；行内按 out 尾形态——块边界续
                    // '\n'、文本节点自带空白续 ' '、紧贴字符/哨兵不加（"by<code>"→"byT" 正确）
                    let lead = if block {
                        (!out.is_empty()).then_some('\n')
                    } else {
                        match out.chars().last() {
                            Some('\n') => Some('\n'),
                            Some(c) if c.is_whitespace() => Some(' '),
                            _ => None,
                        }
                    };
                    out.push('\u{0}');
                    if let Some(c) = lead {
                        out.push(c);
                    }
                    let mut sub: Vec<_> = node.children().rev().collect();
                    while let Some(n) = sub.pop() {
                        match n.value() {
                            Node::Text(t) => out.push_str(t),
                            Node::Element(e)
                                if !matches!(
                                    e.name(),
                                    "script" | "style" | "noscript" | "template"
                                ) =>
                            {
                                // FixG14：哨兵段内块级子元素（div.where/br）注入 '\n'——
                                // docs.rs 签名 "Result<T><div>where" 无文本换行可依赖
                                if is_block_boundary(e.name()) {
                                    out.push('\n');
                                }
                                sub.extend(n.children().rev());
                            }
                            _ => {}
                        }
                    }
                    if block {
                        out.push('\n');
                    } else {
                        // 行内后置：按下一可见节点形态补——紧贴正文（", for"）不加；空白起头
                        // 文本（" is"）或空白后续行内元素补 ' '；块级元素补 '\n'。真实 HTML
                        // 里这些空白落在后 prose 段首，collapse_blank 会 trim 掉。
                        let mut trail = None;
                        let mut ws_pending = false;
                        for n in stack.iter().rev() {
                            match n.value() {
                                Node::Text(t) => {
                                    if ws_pending {
                                        trail = Some(' ');
                                        break;
                                    }
                                    if t.starts_with(char::is_whitespace) {
                                        if t.trim_start().is_empty() {
                                            ws_pending = true; // 纯空白文本，继续看其后
                                        } else {
                                            trail = Some(' '); // " is"：同段续文
                                            break;
                                        }
                                    } else {
                                        break; // 紧贴正文（", for"）：正确渲染就是无分隔
                                    }
                                }
                                Node::Element(e)
                                    if matches!(
                                        e.name(),
                                        "script" | "style" | "noscript" | "template" | "wbr"
                                    ) => {}
                                Node::Element(e) => {
                                    trail = if is_block_boundary(e.name()) {
                                        Some('\n')
                                    } else if ws_pending {
                                        Some(' ')
                                    } else {
                                        None
                                    };
                                    break;
                                }
                                Node::Comment(_) => {} // 注释不贡献输出，穿透再探
                                _ => break,
                            }
                        }
                        if let Some(c) = trail {
                            out.push(c);
                        }
                    }
                    out.push('\u{0}');
                    continue;
                }
                out.push(sep);
                stack.extend(node.children().rev());
                out.push(sep);
            }
            _ => {}
        }
    }
    out
}

/// tree_text 输出的空白规整：\\0 哨兵段（pre/code 原样文本）逐字节保留，普通文本仍走
/// collapse_blank（FixG13：代码空白是内容，prose 空白是噪音）。无哨兵时输出与
/// collapse_blank(s) 逐字节一致——非代码页零行为变化。
fn collapse_preserving_code(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, seg) in s.split('\u{0}').enumerate() {
        if i % 2 == 1 {
            out.push_str(seg);
        } else {
            out.push_str(&collapse_blank(seg.to_string()));
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
    collapse_preserving_code(tree_text(&Html::parse_document(html)))
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
/// 返回 (命中块的 inner_html 列表, 命中数)；全未命中 → None。每块收尾 trim + 换行归一 LF
/// （源码缩进/CRLF 是提取层伪影，官方 HTML 为 LF 无缩进——盲测十四 X）；块间分隔由
/// render_include_blocks 在渲染层以 `\n\n---\n\n` 拼接（分隔符混在 inner_html 里过
/// extract_text 会沦为裸文本节点被空白规整压扁）。
/// selector 语法错误 → Err：用户显式输入拼错了要报错，静默跳过会伪装成"未命中回退全文"。
fn extract_with_include(html: &str, include: &str) -> Result<Option<(Vec<String>, usize)>> {
    let doc = Html::parse_document(html);
    let mut collected: Vec<String> = Vec::new();
    for sel in include.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        // day：scraper 的 Display 泄漏内部变体名（EmptySelector/Please report...），映射成用户可操作的文案
        let selector = Selector::parse(sel).map_err(|_| {
            anyhow!("CSS selector 无效: {sel:?}（语法错误，应为合法 CSS 选择器如 \"#main, article\"）")
        })?;
        for el in doc.select(&selector) {
            let inner = el.inner_html().replace("\r\n", "\n").replace('\r', "\n");
            collected.push(inner.trim().to_string());
        }
    }
    let hits = collected.len();
    if hits == 0 {
        return Ok(None);
    }
    Ok(Some((collected, hits)))
}

/// --include 命中块渲染成正文：每块独立走 text/markdown 提取并 trim，再以 `\n\n---\n\n` 拼接，
/// 与 --help/README 宣称的分隔符形态一致。拼接 inner_html 整体重解析会让分隔符沦为裸文本
/// 节点被压成 ` --- ` 胶连、块首缩进漏成前导空格（盲测十四 X）——分块提取是修法本体。
fn render_include_blocks(blocks: &[String], markdown: bool) -> Result<String> {
    let mut parts = Vec::with_capacity(blocks.len());
    for b in blocks {
        let raw = if markdown {
            crate::convert::clean_markdown(crate::convert::html_to_markdown(b)?)
        } else {
            // FixG17：非 markdown 路径同样剥 rustdoc 标题锚点 §（与 --markdown 的 clean_markdown 对称）
            crate::convert::clean_text_anchors(extract_text(b))
        };
        parts.push(raw.trim().to_string());
    }
    Ok(parts.join("\n\n---\n\n"))
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
        let (blocks, hits) = extract_with_include(html, "article,main").unwrap().unwrap();
        let got = blocks.join("\n");
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
        let (got_blocks, hits) = extract_with_include(html, ".release-entry").unwrap().unwrap();
        let got = got_blocks.join("\n");
        assert_eq!(hits, 3, "3 个 release entries 应全部命中: got_hits={hits}");
        assert!(got.contains("1.5.0") && got.contains("1.4.0") && got.contains("1.3.0"), "got: {got}");
        assert!(!got.contains("菜单"), "nav 不应混入: {got}");
        // 拼接分隔符：让 agent 能识别 selector 块边界（FixG15 起在渲染层拼接）
        let rendered = render_include_blocks(&got_blocks, false).unwrap();
        assert!(rendered.contains("\n\n---\n\n"), "多命中应加分隔符: {rendered}");
        // 跨 selector 累加：releases + markdown-body 都命中，拼接成两段
        let html2 = "<html><body>\
                     <article class='markdown-body'>正文</article>\
                     <section class='release-entry'><h2>1.0.0</h2></section>\
                     <section class='release-entry'><h2>0.9.0</h2></section>\
                     </body></html>";
        let (got2_blocks, hits2) = extract_with_include(html2, ".markdown-body,.release-entry").unwrap().unwrap();
        let got2 = got2_blocks.join("\n");
        assert_eq!(hits2, 3, "1 markdown-body + 2 release-entry");
        assert!(got2.contains("正文") && got2.contains("1.0.0") && got2.contains("0.9.0"));
    }

    /// FixG15：inner_html 收集即归一——首尾空白 trim、CRLF 归 LF（盲测十四 X 前导空格伪影）。
    #[test]
    fn extract_with_include_normalizes_block_whitespace() {
        let html = "<html><body><pre>\r\n  <code>pub fn f()</code>  \r\n</pre></body></html>";
        let (blocks, hits) = extract_with_include(html, "pre").unwrap().unwrap();
        assert_eq!(hits, 1);
        assert_eq!(blocks[0], "<code>pub fn f()</code>", "块首尾空白应剥除: {:?}", blocks[0]);
    }

    /// FixG15：块级渲染后分隔符保持 `\n\n---\n\n`、无前导空白、纯 LF——拼接 inner_html
    /// 整体重解析会把分隔符压成 ` --- ` 胶连（盲测十四 X），分块分别提取是修法本体。
    #[test]
    fn render_include_blocks_separator_exact_and_trimmed() {
        let html = "<html><body>\
                    <pre class='a'>  <code>pub fn f() -&gt; u32</code>  </pre>\
                    <p>间隔噪音</p>\
                    <pre class='b'>\r\n<code>use serde;</code></pre>\
                    </body></html>";
        let (blocks, hits) = extract_with_include(html, "pre").unwrap().unwrap();
        assert_eq!(hits, 2);
        let text = render_include_blocks(&blocks, false).unwrap();
        assert!(text.starts_with("pub fn"), "无前导空白: {text:?}");
        assert!(!text.contains('\r'), "纯 LF: {text:?}");
        assert!(text.contains("\n\n---\n\n"), "分隔符形态: {text:?}");
        let mut segs = text.split("\n\n---\n\n");
        assert!(segs.next().unwrap().starts_with("pub fn"));
        assert!(segs.next().unwrap().starts_with("use serde;"));
        assert!(!text.contains("间隔噪音"), "未选中容器不混入: {text:?}");
    }

    /// FixG17：--include 非 markdown 渲染剥行首 § 锚点（docs.rs 标题自链 `<a>§</a>标题`
    /// 经 extract_text 胶成 "§标题"，与 --markdown 的 clean_markdown 对称）；行中引用 § 保留。
    #[test]
    fn render_include_blocks_text_mode_strips_section_anchor() {
        let html = "<html><body>\
                    <h4 id=\"example\"><a class=\"doc-anchor\" href=\"#example\">§</a>Example</h4>\
                    <p>正文引用 §3.2 保持原样</p>\
                    </body></html>";
        let (blocks, _) = extract_with_include(html, "h4,p").unwrap().unwrap();
        let out = render_include_blocks(&blocks, false).unwrap();
        assert!(out.starts_with("Example"), "h4 行首锚点 § 应剥除: {out:?}");
        assert!(out.contains("正文引用 §3.2"), "行中引用 § 保留: {out:?}");
        // markdown 模式行为不变（clean_markdown 已剥 [§](#anchor) 形态）
        let md = render_include_blocks(&blocks, true).unwrap();
        assert!(!md.contains("[§]("), "markdown 模式锚点链接剥除: {md:?}");
    }

    /// FixG15：`[*]` 通配段解析——与 Field/Index 并存（`data.items[*].name`）。
    #[test]
    fn parse_json_path_wildcard_segment() {
        use Segment::{Field, Wildcard};
        assert_eq!(
            parse_json_path("[*].tag_name").unwrap(),
            vec![Wildcard, Field("tag_name".into())]
        );
        assert_eq!(parse_json_path("[*]").unwrap(), vec![Wildcard]);
        assert_eq!(
            parse_json_path("data.items[*].name").unwrap(),
            vec![Field("data".into()), Field("items".into()), Wildcard, Field("name".into())]
        );
    }

    /// FixG15：`[*]` 通配投影——顶层数组每元素取指定字段，值为数组形态（盲测十四 V
    /// "静默跳过返回全量"的修法本体）；纯 `[*]` 取整组元素。
    #[test]
    fn project_json_paths_wildcard_projects_all_elements() {
        let payload = serde_json::json!([
            {"tag_name": "v1.0.0", "body": "a"},
            {"tag_name": "v0.9.0", "body": "b"},
            {"tag_name": "v0.8.0", "body": "c"},
        ]);
        let out = project_json_paths(&payload, &["[*].tag_name".to_string()]).unwrap();
        assert_eq!(out, serde_json::json!({"tag_name": ["v1.0.0", "v0.9.0", "v0.8.0"]}));
        let out = project_json_paths(&payload, &["[*]".to_string()]).unwrap();
        assert_eq!(out, serde_json::json!({"[*]": payload.clone()}));
    }

    /// FixG15：`[*]` 与既有索引/字段段同次调用共存（GitHub comments 场景形状）。
    #[test]
    fn project_json_paths_wildcard_coexists_with_index_and_field() {
        let payload = serde_json::json!([
            {"user": {"login": "alice"}, "body": "first"},
            {"user": {"login": "bob"}, "body": "second"},
        ]);
        let out = project_json_paths(
            &payload,
            &["0.user.login".to_string(), "[*].body".to_string()],
        )
        .unwrap();
        assert_eq!(out, serde_json::json!({"login": "alice", "body": ["first", "second"]}));
    }

    /// FixG15：`[*]` 作用于非数组 → 显式 Err，不静默跳过回退全量。
    #[test]
    fn project_json_paths_wildcard_on_non_array_errors() {
        let payload = serde_json::json!({"tag_name": "v1.0.0"});
        let err = project_json_paths(&payload, &["[*].tag_name".to_string()]).unwrap_err();
        assert!(err.to_string().contains("数组"), "err: {err:#}");
    }

    /// FixG15：顶层数组 + 裸字段路径 → 错误信息教 `0.field` 索引与 `[*]` 语法（盲测十四 W）；
    /// 已用 `[*]` 的失败（元素缺字段）不再重复教学。
    #[test]
    fn project_json_body_array_error_teaches_syntax() {
        let body = r#"[{"user": {"login": "alice"}}]"#;
        let err = project_json_body(body, &["user.login".to_string()]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("顶层数组"), "err: {msg}");
        assert!(msg.contains("0.user.login"), "err: {msg}");
        assert!(msg.contains("[*]"), "err: {msg}");
        let err2 = project_json_body(body, &["[*].nope".to_string()]).unwrap_err();
        assert!(!format!("{err2:#}").contains("顶层数组"), "已用 [*] 不再教语法: {err2:#}");
    }

    /// FixG15：hint 文案诚实——"仅部分包含"取代"未包含"绝对断言（正文实际混入部分
    /// 已渲染评论，盲测十四 W 自相矛盾点）；api.github.com 升级路径保留。
    #[test]
    fn github_thread_hint_claims_partial_not_absent() {
        let hint = github_thread_comment_gap("https://github.com/tokio-rs/tokio/issues/6741")
            .expect("issue 页应命中");
        assert!(hint.contains("仅部分"), "hint: {hint}");
        assert!(!hint.contains("未包含"), "不得绝对断言缺失: {hint}");
        assert!(
            hint.contains("api.github.com/repos/tokio-rs/tokio/issues/6741/comments"),
            "升级路径保留: {hint}"
        );
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

    /// qmg：userinfo 混淆——`http://a.com:80@192.168.1.1/` 的真实连接目标是 @ 后的 host，
    /// 门判定与 reqwest 同走 url crate parser，必须按 @ 后 host 判私网拒掉。
    #[test]
    fn ssrf_gate_rejects_userinfo_obfuscation() {
        for bad in [
            "http://a.com:80@192.168.1.1/",
            "http://evil.example@10.0.0.5/x",
            "http://user:pass@169.254.169.254/latest/meta-data/",
            "http://x@[::1]/admin",                  // userinfo + IPv6 字面量
            "http://192.168.1.1\\@public.example/",  // 反斜杠：url crate 按 '/' 归一 → 实连 192.168.1.1
        ] {
            let err = gate_check(bad, false).unwrap_err();
            assert!(err.to_string().contains("拒绝"), "应拒绝 {bad}: {err}");
        }
    }

    /// qmg：IPv4-mapped IPv6（::ffff:x.y.z.w）先转 V4 判私网——v4 私网换 v6 伪装不得绕门，
    /// mapped 公网照常放行。
    #[test]
    fn ssrf_gate_rejects_ipv4_mapped_v6() {
        for (url, private) in [
            ("http://[::ffff:192.168.1.1]/", true),
            ("http://[::ffff:10.0.0.5]/x", true),
            ("http://[::ffff:169.254.169.254]/latest/meta-data/", true),
            ("http://[::ffff:8.8.8.8]/", false),     // mapped 公网放行
        ] {
            let (_host, _ip, is_priv) = classify_url(url).expect(url);
            assert_eq!(is_priv, private, "私网判定错误: {url}");
        }
        assert!(is_private_ip("::ffff:172.16.0.1".parse().unwrap()));
        assert!(!is_private_ip("::ffff:1.1.1.1".parse().unwrap()));
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

    /// FixG20 NN：单 URL 扁平形态与 batch 数组元素共有顶层 status 键（消费方按 URL 数
    /// 无需分支解析）——fetched_json 只在成功路径被调，恒 "ok"，只增不删。
    #[test]
    fn fetched_json_includes_status_ok() {
        let f = Fetched { url: "u".into(), title: "t".into(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false, auto_include_applied: None, include_overridden_by_user: false };
        let v = fetched_json(&f);
        assert_eq!(v["status"], serde_json::json!("ok"), "单/批两形态都应有顶层 status: {v}");
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
            raw: false,
            github_comment_hint: github_thread_comment_gap("https://github.com/tokio-rs/tokio/issues/7787"),
            anchor_crop_range: None,
            truncated_by_json_keys: false,
            auto_include_applied: None,
            include_overridden_by_user: false,
        };
        let v = fetched_json(&f);
        assert_eq!(v["meta"]["github_comments_missing"], serde_json::json!(true));
        assert!(v["meta"]["github_comments_hint"].as_str().unwrap().contains("api.github.com"));

        let f2 = Fetched { url: "https://e.test/".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false, auto_include_applied: None, include_overridden_by_user: false };
        let v2 = fetched_json(&f2);
        assert!(v2["meta"].get("github_comments_missing").is_none(), "非 thread 页不得出键");
        assert!(v2["meta"].get("github_comments_hint").is_none());
    }

    /// FixG10 J-2：truncated_by_json_keys=true 时 meta 出键；默认输出结构不变。
    #[test]
    fn fetched_json_truncated_by_json_keys_flag() {
        // true：键出
        let f = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: true, auto_include_applied: None, include_overridden_by_user: false };
        let v = fetched_json(&f);
        assert_eq!(v["meta"]["truncated_by_json_keys"], serde_json::json!(true));
        // false：键缺席
        let f2 = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false, auto_include_applied: None, include_overridden_by_user: false };
        let v2 = fetched_json(&f2);
        assert!(v2["meta"].get("truncated_by_json_keys").is_none(), "默认不得出键");
    }

    /// FixG16：投影命中时 text 为原生 JSON Value（对象原样，免二次解析）；投影产物被截成
    /// 非法 JSON 或非投影路径回退恒为 string（老输出不变）。
    #[test]
    fn fetched_json_text_native_value_on_projection_hit() {
        let mk = |truncated_by_json_keys: bool, text: &str| Fetched {
            url: "u".into(),
            title: String::new(),
            text: text.into(),
            truncated: false,
            omitted: 0,
            truncated_at_offset: None,
            include_hit: None,
            include_hits: None,
            markdown: false,
            raw: false,
            github_comment_hint: None,
            anchor_crop_range: None,
            truncated_by_json_keys,
            auto_include_applied: None,
            include_overridden_by_user: false,
        };
        let hit = fetched_json(&mk(true, r#"{"tag_name":"v1.53.2"}"#));
        assert_eq!(hit["text"], serde_json::json!({"tag_name": "v1.53.2"}), "投影命中应为原生对象: {hit}");
        // 投影产物被字符预算截断成非法 JSON → 回退 string
        let cut = fetched_json(&mk(true, r#"{"tag_name":"v1"#));
        assert!(cut["text"].is_string(), "非法 JSON 投影产物应回退 string: {cut}");
        // 非投影路径恒为 string
        let plain = fetched_json(&mk(false, r#"{"tag_name":"v1.53.2"}"#));
        assert!(plain["text"].is_string(), "非投影 text 恒为 string: {plain}");
    }

    /// FixG17：投影命中且投影含 title（字符串）→ 顶层 title 回填投影值（页面级 <title>
    /// 对 JSON API 恒空串，与 text.title 并存首读困惑）；投影无 title / 顶层数组 → 维持现状。
    #[test]
    fn fetched_json_backfills_title_from_projection() {
        let mk = |truncated_by_json_keys: bool, title: &str, text: &str| Fetched {
            url: "u".into(),
            title: title.into(),
            text: text.into(),
            truncated: false,
            omitted: 0,
            truncated_at_offset: None,
            include_hit: None,
            include_hits: None,
            markdown: false,
            raw: false,
            github_comment_hint: None,
            anchor_crop_range: None,
            truncated_by_json_keys,
            auto_include_applied: None,
            include_overridden_by_user: false,
        };
        // 对象投影含 title → 顶层回填投影值
        let hit = fetched_json(&mk(true, "", r#"{"title":"tokio issue","state":"open"}"#));
        assert_eq!(hit["title"], serde_json::json!("tokio issue"), "投影 title 应回填顶层: {hit}");
        // 投影无 title 路径 → 维持页面级 title（空串现状）
        let no_title = fetched_json(&mk(true, "", r#"{"state":"open"}"#));
        assert_eq!(no_title["title"], serde_json::json!(""), "无 title 路径维持现状: {no_title}");
        // 顶层数组投影（[*]）取不出字符串 title → 维持页面级
        let arr = fetched_json(&mk(true, "page title", r#"[{"title":"a"},{"title":"b"}]"#));
        assert_eq!(arr["title"], serde_json::json!("page title"), "数组投影不回填: {arr}");
    }

    /// 9jx：fetch meta.truncated_at_offset——truncated=true 时出键且值非零，None 时键缺席。
    /// 与 read/browse 路径同契约（缺席 = 未截断，offset 是 cap_chars 返回的截断字节位置）。
    #[test]
    fn fetched_json_truncated_at_offset_emits_when_truncated() {
        // truncated=true + Some(off) → meta 出键
        let f = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: true, omitted: 100, truncated_at_offset: Some(3000), include_hit: None, include_hits: None, markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false, auto_include_applied: None, include_overridden_by_user: false };
        let v = fetched_json(&f);
        assert_eq!(v["meta"]["truncated_at_offset"], serde_json::json!(3000));
        // truncated=true + None → 键缺席（不与 truncated 键语义重叠）
        let f2 = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: true, omitted: 100, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false, auto_include_applied: None, include_overridden_by_user: false };
        let v2 = fetched_json(&f2);
        assert!(v2["meta"].get("truncated_at_offset").is_none());
        // truncated=false → 键缺席
        let f3 = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false, auto_include_applied: None, include_overridden_by_user: false };
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

    // ── FixG11 host 路由 / auto_include_applied ────────────────────────────────

    /// FixG11 P0-1/P0-2：host_default_include 按 host 分发——github → 容器链 selector；
    /// docs.rs → "main"；裸 host（无路径）跳过；未知 host → None。host 比较大小写不敏感。
    #[test]
    fn host_default_include_dispatches_by_host() {
        // github 内容页：返回 (label=github, selector=容器链)
        let (label, sel) = host_default_include("https://github.com/tokio-rs/tokio/issues/8065").unwrap();
        assert_eq!(label, "github");
        assert!(sel.contains("markdown-body"), "容器链含 markdown-body 优先级: {sel}");
        assert!(sel.contains("js-comment-body"), "旧回退 selector 也在链里: {sel}");
        // 大小写不敏感
        assert_eq!(
            host_default_include("HTTPS://GITHUB.COM/o/r").map(|(l, _)| l),
            Some("github")
        );
        // docs.rs 内容页：返回 (label=docs.rs, selector=main)
        let (label, sel) = host_default_include("https://docs.rs/serde_json/latest/serde_json/fn.from_str.html").unwrap();
        assert_eq!(label, "docs.rs");
        assert_eq!(sel, "main");
        // 裸 host（https://github.com/）→ None（不算内容页）
        assert_eq!(host_default_include("https://github.com/"), None);
        assert_eq!(host_default_include("https://docs.rs/"), None);
        // 未知 host → None
        assert_eq!(host_default_include("https://example.com/x"), None);
        assert_eq!(host_default_include("https://crates.io/api/v1/crates/tokio"), None);
        // http 形态也命中（fetch 路径 http 公网会被拒，但 host 路由先于 scheme 门）
        assert_eq!(host_default_include("http://github.com/o/r").map(|(l, _)| l), Some("github"));
    }

    // ── FixG12：host 判定恒标注 / include_overridden_by_user / summary 剥除 / title 前缀 ──

    /// host 判定恒保留：显式 --include → (label, overridden=true)；未传 → (label, false)；
    /// 未知 host → None。fetch_one_attempt 据此写 auto_include_applied + include_overridden_by_user。
    #[test]
    fn host_route_decision_flags_override_when_user_includes() {
        // 用户显式 --include：host 判定保留 + 覆盖标注
        assert_eq!(
            host_route_decision(Some("pre"), "https://docs.rs/serde_json/latest/serde_json/fn.from_str.html"),
            Some(("docs.rs", true))
        );
        assert_eq!(host_route_decision(Some("article"), "https://github.com/o/r/pull/1"), Some(("github", true)));
        // 用户未传：host 路由照常生效（无覆盖）
        assert_eq!(host_route_decision(None, "https://github.com/o/r/issues/1"), Some(("github", false)));
        assert_eq!(host_route_decision(None, "https://docs.rs/serde_json"), Some(("docs.rs", false)));
        // 未知 host：无判定（有无 --include 都不出标注）
        assert_eq!(host_route_decision(Some("main"), "https://crates.io/api/v1/crates/x"), None);
        assert_eq!(host_route_decision(None, "https://crates.io/api/v1/crates/x"), None);
    }

    /// meta.include_overridden_by_user 仅 true 出键；false 缺席（默认输出逐键不变）。
    /// 覆盖发生时 auto_include_applied 恒在（label 保留）。
    #[test]
    fn fetched_json_include_overridden_by_user_emits_only_when_true() {
        let overridden = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: Some(true), include_hits: Some(1), markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false, auto_include_applied: Some("docs.rs".into()), include_overridden_by_user: true };
        let v = fetched_json(&overridden);
        assert_eq!(v["meta"]["auto_include_applied"], serde_json::json!("docs.rs"), "覆盖时 label 恒在");
        assert_eq!(v["meta"]["include_overridden_by_user"], serde_json::json!(true));
        let plain = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false, auto_include_applied: None, include_overridden_by_user: false };
        let v2 = fetched_json(&plain);
        assert!(v2["meta"].get("include_overridden_by_user").is_none(), "无覆盖不得出键");
        assert!(v2["meta"].get("auto_include_applied").is_none(), "host 未命中不得出键");
    }

    /// docs.rs 折叠按钮文本（<summary>Expand description</summary>）不进正文，
    /// 签名行不再拼噪声；summary 外正文与签名保留。
    #[test]
    fn process_html_strips_summary_button_text() {
        let html = "<html><head><title>from_str - serde_json</title></head><body>\
                    <main>\
                    <pre>pub fn from_str&lt;T&gt;(s: &amp;str) -&gt; Result&lt;T&gt;</pre>\
                    <details><summary>Expand description</summary><p>Parses a string as JSON.</p></details>\
                    </main></body></html>";
        let f = process_html("https://docs.rs/serde_json/latest/serde_json/fn.from_str.html", html, true, 50_000);
        assert!(!f.text.contains("Expand description"), "按钮文本剥离: {:?}", f.text);
        assert!(f.text.contains("Parses a string as JSON."), "summary 外正文保留: {:?}", f.text);
        assert!(f.text.contains("pub fn from_str"), "签名保留: {:?}", f.text);
    }

    /// strip_summary_elements 边界——多 summary 全剥、带属性剥、<summaryx 不动、未闭合不误伤。
    #[test]
    fn strip_summary_elements_edge_cases() {
        let html = "<div><summary>SUMA</summary><p>keep1</p>\
                    <summary class='x'>SUMB</summary><span>keep2</span>\
                    <summaryx>SUMX</summaryx></div>";
        let out = strip_summary_elements(html);
        assert!(!out.contains("SUMA"), "无属性 summary 剥除: {out}");
        assert!(!out.contains("SUMB"), "带属性 summary 剥除: {out}");
        assert!(out.contains("SUMX"), "<summaryx 不是 summary 标签，保留: {out}");
        assert!(out.contains("keep1") && out.contains("keep2"), "summary 外内容保留: {out}");
        let unclosed = "<div><summary>SUMA<p>keep3</p></div>";
        let out2 = strip_summary_elements(unclosed);
        assert!(out2.contains("keep3"), "未闭合 summary 不误伤余下内容: {out2}");
    }

    /// 非 HTML 源文不碰——字面 <summary> 原样保留（源文保真铁律）。
    #[test]
    fn process_html_non_html_keeps_summary_source_fidelity() {
        let raw = r#"{"note":"<summary>Expand description</summary>"}"#;
        let f = process_html("https://e.test/a.json", raw, false, 50_000);
        assert!(f.text.contains("<summary>Expand description</summary>"), "非 HTML 源文保真: {}", f.text);
    }

    /// GitHub PR 页容器改写后正文头部补页面 title 前缀（标题栏 .gh-header-title 在
    /// markdown-body 容器外，否则 caller 不知在看哪条 PR）；正文已含标题则不重复。
    #[test]
    fn apply_github_host_route_prefixes_missing_page_title() {
        let html = "<html><body>\
                    <div class='gh-header'><h1><bdi>fs: support io_uring</bdi></h1></div>\
                    <div data-testid='markdown-body'><p>Motivation paragraph.</p></div>\
                    </body></html>";
        let mut fetched = Fetched {
            url: "https://github.com/tokio-rs/tokio/pull/7696".into(),
            title: "fs: support io_uring by darksonn · Pull Request #7696".into(),
            text: "Motivation only".into(),
            truncated: false,
            omitted: 0,
            truncated_at_offset: None,
            include_hit: None,
            include_hits: None,
            markdown: false,
            raw: false,
            github_comment_hint: None,
            anchor_crop_range: None,
            truncated_by_json_keys: false,
            auto_include_applied: None,
            include_overridden_by_user: false,
        };
        let applied = apply_github_host_route(&mut fetched, html, false, 50_000).unwrap();
        assert!(applied, "容器命中");
        assert!(fetched.text.starts_with("# fs: support io_uring"), "title 前缀补到正文头: {}", fetched.text);
        assert!(fetched.text.contains("Motivation"), "正文保留: {}", fetched.text);
        // 正文头部已含标题（标题写进了容器）→ 不重复前缀
        let html2 = "<html><body><div data-testid='markdown-body'>\
                     <p>fs: support io_uring by darksonn — merged last week</p></div></body></html>";
        let mut f2 = Fetched {
            url: "https://github.com/tokio-rs/tokio/pull/7696".into(),
            title: "fs: support io_uring by darksonn · Pull Request #7696".into(),
            text: "x".into(),
            truncated: false,
            omitted: 0,
            truncated_at_offset: None,
            include_hit: None,
            include_hits: None,
            markdown: false,
            raw: false,
            github_comment_hint: None,
            anchor_crop_range: None,
            truncated_by_json_keys: false,
            auto_include_applied: None,
            include_overridden_by_user: false,
        };
        let applied2 = apply_github_host_route(&mut f2, html2, false, 50_000).unwrap();
        assert!(applied2, "容器命中");
        assert!(!f2.text.starts_with('#'), "正文已含标题不重复前缀: {}", f2.text);
    }

    /// FixG11 P0-1：github host 路由——命中 markdown-body 容器时按 SEL_GITHUB_NAV 剥 nav；
    /// P0-1 验收：text 不含 Platform / Solutions / Resources。命中后设 auto_include_applied="github"。
    #[test]
    fn apply_github_host_route_strips_nav_and_marks_label() {
        // 模拟盲测十实测的 GitHub issue 页（head 巨大 + nav 占 7KB + markdown-body 真容器）
        let junk = "x".repeat(20_000);
        let html = format!(
            "<html><head><meta>{junk}</meta></head><body>\
             <nav class='Header'><a>Platform</a><a>Solutions</a><a>Resources</a></nav>\
             <div data-testid='markdown-body'>\
             <p>actual issue content paragraph one</p>\
             <p>actual issue content paragraph two</p>\
             </div></body></html>"
        );
        let mut fetched = Fetched {
            url: "https://github.com/o/r/issues/1".into(),
            title: "Issue 1".into(),
            text: "原 process_html 输出（含 nav 噪声）".into(),
            truncated: false,
            omitted: 0,
            truncated_at_offset: None,
            include_hit: None,
            include_hits: None,
            markdown: false,
            raw: false,
            github_comment_hint: None,
            anchor_crop_range: None,
            truncated_by_json_keys: false,
            auto_include_applied: None,
            include_overridden_by_user: false,
        };
        let applied = apply_github_host_route(&mut fetched, &html, false, 50_000).unwrap();
        assert!(applied, "github 容器命中应改写 body");
        // 调用方（fetch_one_attempt）负责在 applied=true 时设 auto_include_applied；
        // 单测镜像调用方行为以锁定契约。
        if applied {
            fetched.auto_include_applied = Some("github".to_string());
        }
        assert_eq!(fetched.auto_include_applied.as_deref(), Some("github"));
        // nav 已剥离（盲测十 P0-1 验收）
        assert!(!fetched.text.contains("Platform"), "nav Platform 剥离: {}", fetched.text);
        assert!(!fetched.text.contains("Solutions"), "nav Solutions 剥离: {}", fetched.text);
        assert!(!fetched.text.contains("Resources"), "nav Resources 剥离: {}", fetched.text);
        assert!(!fetched.text.contains(&junk), "head 噪声不进 body: {}", fetched.text);
        // 正文保留
        assert!(fetched.text.contains("actual issue content paragraph"), "正文保留: {}", fetched.text);
    }

    /// FixG11 P0-1 反向：github 页面无容器命中（releases/blob/repo 首页）→ host 路由不改 body，
    /// auto_include_applied 保持 None（不误标）。
    #[test]
    fn apply_github_host_route_no_container_match_keeps_body() {
        let html = "<html><body><h1>Plain page</h1><p>no markdown-body here</p></body></html>";
        let mut fetched = Fetched {
            url: "https://github.com/o/r".into(),
            title: "Repo".into(),
            text: "process_html 原始输出".into(),
            truncated: false,
            omitted: 0,
            truncated_at_offset: None,
            include_hit: None,
            include_hits: None,
            markdown: false,
            raw: false,
            github_comment_hint: None,
            anchor_crop_range: None,
            truncated_by_json_keys: false,
            auto_include_applied: None,
            include_overridden_by_user: false,
        };
        let applied = apply_github_host_route(&mut fetched, html, false, 50_000).unwrap();
        assert!(!applied, "无容器命中 → applied=false");
        assert!(fetched.auto_include_applied.is_none(), "不改写 body → 不设 label");
        assert_eq!(fetched.text, "process_html 原始输出", "body 不被改写");
    }

    /// FixG11 P0-2：docs.rs host 路由——命中 <main> 时改写 body + 设 auto_include_applied="docs.rs"；
    /// 同时打 include_hit/include_hits（与 --include 行为对齐）。
    #[test]
    fn apply_docsrs_host_route_uses_main_and_marks_label() {
        // 模拟 docs.rs 函数页：<main> 内是函数签名 + 描述 + Errors；外层 nav 是全站侧栏
        let html = "<html><body>\
                    <nav>crates.io search box</nav>\
                    <aside>left sidebar with module list</aside>\
                    <main>\
                    <h1>Function from_str</h1>\
                    <pre><code>pub fn from_str&lt;'a, T&gt;(s: &amp;'a str) -&gt; Result&lt;T&gt;</code></pre>\
                    <p>Deserializes a string of JSON into T.</p>\
                    <h2>Errors</h2>\
                    <p>This function will return an error if the input is not valid JSON.</p>\
                    </main>\
                    <footer>site-wide footer</footer>\
                    </body></html>";
        let mut fetched = Fetched {
            url: "https://docs.rs/serde_json/latest/serde_json/fn.from_str.html".into(),
            title: "from_str".into(),
            text: "全站 nav + sidebar + footer + main 一起的输出（含噪声）".into(),
            truncated: false,
            omitted: 0,
            truncated_at_offset: None,
            include_hit: None,
            include_hits: None,
            markdown: false,
            raw: false,
            github_comment_hint: None,
            anchor_crop_range: None,
            truncated_by_json_keys: false,
            auto_include_applied: None,
            include_overridden_by_user: false,
        };
        let applied = apply_docsrs_host_route(&mut fetched, html, true, false, 50_000).unwrap();
        assert!(applied, "docs.rs <main> 命中应改写 body");
        // 调用方负责设 auto_include_applied（与 fetch_one_attempt 行为一致）
        if applied {
            fetched.auto_include_applied = Some("docs.rs".to_string());
        }
        assert_eq!(fetched.auto_include_applied.as_deref(), Some("docs.rs"));
        assert_eq!(fetched.include_hit, Some(true));
        assert_eq!(fetched.include_hits, Some(1));
        // 侧栏/页脚/导航不应混入
        assert!(!fetched.text.contains("search box"), "nav 不进 body: {}", fetched.text);
        assert!(!fetched.text.contains("sidebar"), "aside 不进 body: {}", fetched.text);
        assert!(!fetched.text.contains("site-wide footer"), "footer 不进 body: {}", fetched.text);
        // 主内容保留
        assert!(fetched.text.contains("from_str"), "main 标题保留: {}", fetched.text);
        assert!(fetched.text.contains("Errors"), "Errors 段保留: {}", fetched.text);
        assert!(fetched.text.contains("Deserializes"), "描述段保留: {}", fetched.text);
    }

    /// FixG11 P0-5：fetched_json 在 auto_include_applied=Some 时出 meta 键；
    /// None 时键缺席（默认输出结构不变，与 include_hit 同款缺席语义）。
    #[test]
    fn fetched_json_auto_include_applied_emits_when_set() {
        // Some("github")：meta 出键
        let f = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false, auto_include_applied: Some("github".into()), include_overridden_by_user: false };
        let v = fetched_json(&f);
        assert_eq!(v["meta"]["auto_include_applied"], serde_json::json!("github"));
        // Some("docs.rs")：meta 出键
        let f2 = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false, auto_include_applied: Some("docs.rs".into()), include_overridden_by_user: false };
        let v2 = fetched_json(&f2);
        assert_eq!(v2["meta"]["auto_include_applied"], serde_json::json!("docs.rs"));
        // None：键缺席（默认输出逐键不变）
        let f3 = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false, auto_include_applied: None, include_overridden_by_user: false };
        let v3 = fetched_json(&f3);
        assert!(v3["meta"].get("auto_include_applied").is_none(), "默认不得出键");
    }

    /// FixG11：FetchOpts::default() 改 timeout=30 retry=1（盲测十 P0-3：GitHub 抖动自愈）。
    /// 其他字段保持原 FixG10 默认（max_chars=50000、json=false、markdown=false 等）。
    #[test]
    fn fetch_opts_default_timeout_30_retry_1() {
        let opts = FetchOpts::default();
        assert_eq!(opts.timeout_secs, 30, "默认 timeout 30（FixG11 给 GitHub 抖动自愈留余量）");
        assert_eq!(opts.retry, 1, "默认 retry 1（FixG11 公网抖动 1 次自愈）");
        assert_eq!(opts.max_chars, 50_000);
        assert!(!opts.json);
        assert!(!opts.markdown);
        assert!(opts.include.is_none());
        assert!(opts.json_keys.is_empty());
        assert_eq!(opts.anchor_pad_lines, 0);
        assert!(opts.proxy.is_none());
        assert!(!opts.allow_private);
    }

    /// FixG13 R 扣分：pre/code 内空白是内容——where 行 4 空格缩进保留；元素边界
    ///（&lt;a&gt;str、println!&lt;/span&gt;(、&lt;wbr&gt;）不注入游离空格。fixture 取自
    /// docs.rs fn.from_str 页实测结构（pre.item-decl 签名 + wbr 撕词 + where div）。
    #[test]
    fn extract_text_preserves_pre_code_whitespace() {
        let html = "<html><head><title>from_str - serde_json</title></head><body>\
                    <h1>Function <span class=\"fn\">from_<wbr>str</span>&nbsp;<button>Copy item path</button></h1>\
                    <main>\
                    <pre class=\"rust item-decl\"><code>pub fn from_str&lt;'a, T&gt;(s: &amp;'a \
                    <a class=\"primitive\" href=\"/std/primitive.str.html\">str</a>) -&gt; \
                    <a href=\"type.Result.html\">Result</a>&lt;T&gt;\
                    <div class=\"where\">where\n    T: <a href=\"trait.Deserialize.html\">Deserialize</a>&lt;'a&gt;,</div>\
                    </code></pre>\
                    <pre class=\"rust example-wrap\"><code>\
                    <span class=\"kw\">let </span>j = <span class=\"string\">\"{}\"</span>;\n\
                    <span class=\"kw\">let </span>u: User = serde_json::from_str(j).unwrap();\n\
                    <span class=\"macro\">println!</span>(<span class=\"string\">\"{:#?}\"</span>, u);</code></pre>\
                    </main></body></html>";
        let text = extract_text(html);
        assert!(text.contains("where\n    T: Deserialize<'a>,"),
            "where 缩进 4 空格是内容，不得折叠: {text:?}");
        assert!(!text.contains("from_ str"), "wbr 不撕词: {text:?}");
        assert!(!text.contains("println! ("), "code 内元素边界不注空格: {text:?}");
        assert!(text.contains("unwrap();\n"), "code 内换行原样不被折叠: {text:?}");
        assert!(text.contains("let u: User = serde_json::from_str(j).unwrap();"),
            "code 内行内元素(span)边界不注空格: {text:?}");
        // prose 部分仍走 collapse_blank：h1 的 &nbsp; 折叠、按钮文本保留
        assert!(text.contains("Function from_str Copy item path"),
            "prose 空白规整不变: {text:?}");
    }

    /// FixG18 JJ：docs.rs host route 清洗后产物不再混入复制按钮文案与 NBSP——
    /// markdown 路径（clean_markdown）与文本路径（clean_text_anchors）同验；
    /// pre>code 签名经 sanitize_pre_blocks 保真，逐字符不受清洗影响。
    #[test]
    fn docsrs_route_strips_copy_button_and_nbsp_both_modes() {
        let html = "<html><head><title>from_str - serde_json</title></head><body>\
                    <main>\
                    <h1>Function <span class=\"fn\">from_str</span>&nbsp;<button>Copy item path</button></h1>\
                    <pre class=\"rust item-decl\"><code>pub fn from_str&lt;'a, T&gt;(s: &amp;'a str) -&gt; Result&lt;T&gt;</code></pre>\
                    <p>Deserializes\u{a0}any value.</p>\
                    </main></body></html>";
        let (blocks, _) = extract_with_include(html, "main").unwrap().unwrap();
        let md = render_include_blocks(&blocks, true).unwrap();
        assert!(!md.contains("Copy item path"), "markdown 按钮文案剥除: {md:?}");
        assert!(!md.contains('\u{a0}'), "markdown NBSP 归一: {md:?}");
        assert!(md.contains("# Function from_str"), "标题保留: {md:?}");
        assert!(md.contains("pub fn from_str<'a, T>(s: &'a str) -> Result<T>"),
            "签名逐字符保真: {md:?}");
        let text = render_include_blocks(&blocks, false).unwrap();
        assert!(!text.contains("Copy item path"), "文本路径按钮文案同剥: {text:?}");
        assert!(text.contains("pub fn from_str<'a, T>(s: &'a str) -> Result<T>"),
            "文本路径签名保真: {text:?}");
    }

    /// FixG13 零回归保证：无 \0 哨兵时 collapse_preserving_code 与 collapse_blank
    /// 逐字节一致（非代码页行为零变化）。
    #[test]
    fn collapse_preserving_code_matches_collapse_blank_without_sentinel() {
        for raw in [
            "普通 一段\n\n多行   文本",
            "  leading and trailing  ",
            "",
            "a\n\n\nb   c\td",
        ] {
            assert_eq!(
                collapse_preserving_code(raw.to_string()),
                collapse_blank(raw.to_string()),
                "无哨兵必须等价: {raw:?}"
            );
        }
    }

    /// FixG13：哨兵段（pre/code 原样文本）逐字节保留（缩进/连续空白/换行），
    /// 哨兵外 prose 段照常折叠；首尾哨兵段不被误 trim。
    #[test]
    fn collapse_preserving_code_keeps_code_segment_verbatim() {
        let mixed = "前置   折叠\n\u{0}where\n    T:   保持\u{0}\n后置  折叠";
        let out = collapse_preserving_code(mixed.to_string());
        assert!(out.contains("where\n    T:   保持"), "哨兵段逐字节: {out:?}");
        assert!(out.contains("前置 折叠"), "prose 段折叠: {out:?}");
        assert!(out.contains("后置 折叠"), "prose 段折叠: {out:?}");
        // 开头即哨兵（页面以 pre 起始）
        let head = "\u{0}a\n  b\u{0}尾  部";
        let out = collapse_preserving_code(head.to_string());
        assert!(out.contains("a\n  b"), "首哨兵段原样: {out:?}");
        assert!(out.contains("尾 部"), "尾部 prose 折叠: {out:?}");
    }

    /// FixG13 Q 扣分：裸纯数字段 = 数组下标（GitHub comments API `0.user.login`）；
    /// 非纯数字段维持字段名；`[0]` 显式语法与连续下标不回归。
    #[test]
    fn parse_json_path_numeric_segment_becomes_index() {
        use Segment::{Field, Index};
        assert_eq!(
            parse_json_path("0.user.login").unwrap(),
            vec![Index(0), Field("user".into()), Field("login".into())],
            "裸 0 → Index"
        );
        assert_eq!(
            parse_json_path("0").unwrap(),
            vec![Index(0)],
            "单段裸 0 → Index"
        );
        assert_eq!(
            parse_json_path("[0].user.login").unwrap(),
            vec![Index(0), Field("user".into()), Field("login".into())],
            "显式 [0] 语法不回归"
        );
        assert_eq!(
            parse_json_path("0.1").unwrap(),
            vec![Index(0), Index(1)],
            "连续下标"
        );
        assert_eq!(
            parse_json_path("0abc.def").unwrap(),
            vec![Field("0abc".into()), Field("def".into())],
            "非纯数字维持字段名"
        );
        assert_eq!(
            parse_json_path("-1").unwrap(),
            vec![Field("-1".into())],
            "负数不是下标（对象键语义）"
        );
        assert!(parse_json_path("").is_err(), "空路径仍报错");
    }

    /// FixG13：顶层数组响应按裸数字段投影——GitHub comments API 形态
    /// `--json-keys "0.user.login,0.body"`。
    #[test]
    fn project_json_paths_supports_top_level_array_index() {
        let payload = serde_json::json!([
            {"user": {"login": "alice"}, "body": "first comment"},
            {"user": {"login": "bob"}, "body": "second"}
        ]);
        let out = project_json_paths(
            &payload,
            &["0.user.login".to_string(), "0.body".to_string()],
        )
        .unwrap();
        assert_eq!(out["login"], serde_json::json!("alice"));
        assert_eq!(out["body"], serde_json::json!("first comment"));
        // 越界仍报错（不静默）
        let err = project_json_paths(&payload, &["9.user.login".to_string()]).unwrap_err();
        assert!(err.to_string().contains("越界"), "越界语义: {err}");
    }

    /// FixG13 P 扣分：用户 --include 覆盖且未命中 selector → 回退全文补走 host 路由
    /// 剥离（nav 不混入），include_hit 复位 Some(false)；命中用户 selector 时 no-op
    ///（用户明确要什么就是什么）。
    #[test]
    fn include_miss_fallback_applies_host_route_stripping() {
        let junk = "x".repeat(5_000);
        let html = format!(
            "<html><body><nav>Skip to content Navigation Menu Platform {junk}</nav>\
             <div data-testid='markdown-body'><p>Motivation paragraph.</p></div></body></html>"
        );
        let full_text = format!("Skip to content Navigation Menu Platform {junk} 正文");
        let mut fetched = Fetched {
            url: "https://github.com/o/r/issues/1".into(),
            title: "issue title".into(),
            text: full_text.clone(),
            truncated: false,
            omitted: 0,
            truncated_at_offset: None,
            include_hit: Some(false),
            include_hits: None,
            markdown: false,
            raw: false,
            github_comment_hint: None,
            anchor_crop_range: None,
            truncated_by_json_keys: false,
            auto_include_applied: Some("github".into()),
            include_overridden_by_user: true,
        };
        let applied =
            apply_host_route_on_include_fallback(&mut fetched, "github", &html, true, false, 50_000)
                .unwrap();
        assert!(applied, "未命中回退应改写 text");
        assert!(!fetched.text.contains("Skip to content"), "nav 剥离: {}", fetched.text);
        assert!(!fetched.text.contains("Navigation Menu"), "nav 剥离: {}", fetched.text);
        assert!(fetched.text.contains("Motivation paragraph."), "正文保留: {}", fetched.text);
        assert_eq!(fetched.include_hit, Some(false), "命中标志只对应用户 selector");
        assert_eq!(fetched.include_hits, None);

        // 命中用户 selector（include_hit=Some(true)）：no-op，用户容器原样
        let mut hit = Fetched {
            url: "https://github.com/o/r/issues/1".into(),
            title: "issue title".into(),
            text: full_text,
            truncated: false,
            omitted: 0,
            truncated_at_offset: None,
            include_hit: Some(true),
            include_hits: Some(1),
            markdown: false,
            raw: false,
            github_comment_hint: None,
            anchor_crop_range: None,
            truncated_by_json_keys: false,
            auto_include_applied: Some("github".into()),
            include_overridden_by_user: true,
        };
        let applied2 =
            apply_host_route_on_include_fallback(&mut hit, "github", &html, true, false, 50_000)
                .unwrap();
        assert!(!applied2, "命中用户 selector 不动");
        assert!(hit.text.contains("Skip to content"), "用户容器原样保留: {}", hit.text);
        assert_eq!(hit.include_hit, Some(true));
        assert_eq!(hit.include_hits, Some(1));
    }

    /// FixG14 bug1：行内 code 前置空格——文本节点自带的空格落在 prose 段尾会被
    /// collapse_blank trim，移进哨兵段后 "by <code>T</code>," 不再粘连成 "byT"。
    #[test]
    fn inline_code_leading_space_preserved() {
        let text =
            extract_text("<p>expected by <code>T</code>, for example</p>");
        assert!(text.contains("expected by T, for example"), "got: {text:?}");
        assert!(!text.contains("byT"), "got: {text:?}");
    }

    /// FixG14 bug1：行内 code 后置空格——"T is" 空白落在后 prose 段首会被 trim，
    /// 按 peek 下一节点形态补进哨兵段，"ifTis" 类粘连不再出现。
    #[test]
    fn inline_code_trailing_space_preserved() {
        let text = extract_text("<p>if <code>T</code> is required</p>");
        assert!(text.contains("if T is required"), "got: {text:?}");
        assert!(!text.contains("ifT"), "got: {text:?}");
        assert!(!text.contains("Tis"), "got: {text:?}");
    }

    /// FixG14：行内 code 紧贴标点/紧邻行内元素不注入多余空格（"T, for" 正确、
    /// `<code>a</code><code>b</code>` → "ab" 与浏览器渲染一致）。
    #[test]
    fn inline_code_tight_adjacency_no_extra_space() {
        let text = extract_text("<p>by <code>T</code>, for</p>");
        assert!(text.contains("T, for"), "got: {text:?}");
        assert!(!text.contains("T ,"), "got: {text:?}");
        let text2 = extract_text("<p><code>a</code><code>b</code> c</p>");
        assert!(text2.contains("ab c"), "got: {text2:?}");
    }

    /// FixG14：行内 code 紧跟块级边界——块 sep 进哨兵段后换行保留且不重复
    ///（"para\nT starts"，而非 "paraT…" 或 "para\n\nT…"）。
    #[test]
    fn inline_code_after_block_boundary_keeps_single_newline() {
        let text = extract_text("<p>para</p><p><code>T</code> starts</p>");
        assert!(text.contains("para\nT starts"), "got: {text:?}");
    }

    /// FixG14 bug2（边界半边）：pre 块前后换行移进哨兵段——不再被相邻 prose 段
    /// 首尾 trim 吃掉，代码块与正文正确分行。
    #[test]
    fn pre_block_boundaries_preserved() {
        let text = extract_text("<p>before</p><pre>line1\n  line2</pre><p>after</p>");
        assert!(text.contains("before\nline1\n  line2\nafter"), "got: {text:?}");
    }

    /// FixG14 bug2（核心半边）：pre 内块级子元素（div.where）注入 '\n'——docs.rs
    /// 签名真实结构 `Result&lt;T&gt;<div>where` 无文本换行可依赖，旧代码出
    /// "Result<T>where"（粘贴不可编译）。
    #[test]
    fn pre_inner_block_element_gets_newline() {
        let text = extract_text(
            "<pre><code>pub fn f() -&gt; Result&lt;T&gt;\
             <div class=\"where\">where\n    T: X</div></code></pre>",
        );
        assert!(text.contains("Result<T>\nwhere"), "got: {text:?}");
        assert!(!text.contains("Result<T>where"), "got: {text:?}");
    }

    /// FixG14 bug3：--json-keys 多路径末段同名——冲突 key 全路径化，两条都保留
    ///（旧代码静默 last-write-wins 丢数据）；单路径输出形态零变化。
    #[test]
    fn project_json_paths_conflicting_keys_use_full_path() {
        let payload = serde_json::json!({"items": [{"title": "a"}, {"title": "b"}]});
        let out = project_json_paths(
            &payload,
            &["items.0.title".into(), "items.1.title".into()],
        )
        .unwrap();
        assert_eq!(out["items.0.title"], serde_json::json!("a"));
        assert_eq!(out["items.1.title"], serde_json::json!("b"));
        assert_eq!(out.as_object().unwrap().len(), 2, "两条都保留");
        // 单路径/无冲突：末段短名形态与旧版一致
        let single = project_json_paths(&payload, &["items.0.title".into()]).unwrap();
        assert_eq!(single["title"], serde_json::json!("a"));
        assert_eq!(single.as_object().unwrap().len(), 1);
    }

    /// FixG19 JJ：--raw 逃生门——text = body 逐字符原样（标签/实体/NBSP 全保留，零提取零清洗）；
    /// --max-chars 照常约束 raw（截断 meta 如实）；body 硬上限（FETCH_BODY_LIMIT）截断照常累计。
    #[test]
    fn raw_fetched_keeps_body_verbatim() {
        let body = "<!DOCTYPE html>\n<html><body>  raw &amp; <b>tags</b>\u{a0}</body></html>";
        let f = raw_fetched("https://e.test/x", body, 50_000, false);
        assert_eq!(f.text, body, "raw 模式 text 必须逐字符等于响应体");
        assert!(f.raw && !f.markdown, "raw 置位、markdown 不置位");
        assert!(!f.truncated && f.omitted == 0 && f.truncated_at_offset.is_none(), "未截断时 meta 零标记");
        // --max-chars 照常约束 raw
        let f2 = raw_fetched("https://e.test/x", body, 10, false);
        assert!(f2.truncated && f2.omitted > 0 && f2.truncated_at_offset == Some(10), "cap 截断如实标注");
        assert_eq!(f2.text.chars().count(), 10, "text 截到预算内");
        // body 硬上限截断（字节级）如实累计进 meta
        let f3 = raw_fetched("https://e.test/x", body, 50_000, true);
        assert!(f3.truncated && f3.omitted >= FETCH_BODY_LIMIT, "body 硬上限截断累计: omitted={}", f3.omitted);
    }

    /// FixG19：--json 下 meta.format="raw"（与 format="markdown" 同键位；两者互斥由 clap 保证）。
    #[test]
    fn fetched_json_marks_raw_format() {
        let f = raw_fetched("https://e.test/x", "<html>hi</html>", 50_000, false);
        let v = fetched_json(&f);
        assert_eq!(v["meta"]["format"], serde_json::json!("raw"));
        assert_eq!(v["meta"]["content_untrusted"], serde_json::json!(true), "content_untrusted 恒在");
        // 非 raw 路径不得出 format="raw" 键（默认输出结构不变）
        let plain = Fetched { url: "u".into(), title: String::new(), text: "x".into(), truncated: false, omitted: 0, truncated_at_offset: None, include_hit: None, include_hits: None, markdown: false, raw: false, github_comment_hint: None, anchor_crop_range: None, truncated_by_json_keys: false, auto_include_applied: None, include_overridden_by_user: false };
        assert!(fetched_json(&plain)["meta"].get("format").is_none(), "非 raw 非 markdown 不得出 format 键");
    }
}
