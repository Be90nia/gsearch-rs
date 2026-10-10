//! M14-1A `verify <url>`：headless URL 健康检查（不启动 Chrome）。
//!
//! 传输层直接调系统 curl（Windows 10+ 自带 curl.exe，Schannel 后端 = OS 证书库真验证），
//! HTTP 语义在本模块手写：状态行解析、redirect 链提取、403/405→GET 回退、错误分类。
//! ponytail: 上游指令原文是「std::net 手写 HTTP」，但验收要求 https 的 ssl_valid=true——
//! 纯 TcpStream 无法完成 TLS 握手，依赖树亦无 rustls/native-tls；零新依赖约束下
//! curl.exe 是唯一能真验 TLS 的路径（doctor 的 fetch_public_ip 有同款先例）。

use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::types::VerifyReport;

/// verify 总预算（含全部 redirect hop），对应 curl --max-time；轻量快查不拖累。
/// 也是 `--timeout` 默认值（issue gsearch-rs-7qx：CDN 5-10s 抖动端点可调高）。
pub const VERIFY_TIMEOUT_SECS: u64 = 5;
/// redirect hop 上限，防 A→B→A 死循环把超时吃满。
const MAX_REDIRECT_HOPS: u32 = 10;
/// `-w` 输出分隔标记：stdout = 各 hop 响应头 dump + 本标记 + 最终 URL。
const FINAL_URL_MARKER: &str = "__GSEARCH_FINAL__";

/// `gsearch verify <url>...`：HTTP HEAD（403/405 回退 GET）→ 5 项报告 + 分类退出码。
/// 退出码：0=OK / 2=HTTP 4xx/5xx / 3=SSL 失败 / 4=DNS 失败 / 5=超时 / 1=其他；批量全 OK 0 / 否则 1。
pub fn cmd_verify(
    urls: &[String],
    json: bool,
    proxy: Option<&str>,
    timeout: u64,
    urls_file: Option<&Path>,
) -> Result<ExitCode> {
    let mut targets: Vec<String> = urls.to_vec();
    if let Some(f) = urls_file {
        let content =
            fs::read_to_string(f).with_context(|| format!("无法读取 urls 文件: {}", f.display()))?;
        targets.extend(
            content
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned),
        );
    }
    match targets.as_slice() {
        // 单 URL：退出码与人读输出与旧版一致；JSON 增加 verdict 分类（6a6/ih1，与批量元素同构）
        [one] => {
            let probe = probe_url(one, proxy, timeout)?;
            match probe.transport {
                // 传输层失败沿用旧行为：仅 SSL 出报告，其余只出 stderr 分类行
                Some((kind, curl_code)) => Ok(transport_exit(&probe, kind, curl_code, json)),
                None => {
                    print_probe(&probe, json);
                    Ok(ExitCode::from(probe.verdict))
                }
            }
        }
        [] => anyhow::bail!("verify: 未提供任何 URL（参数为空或 urls 文件无有效行）"),
        many => {
            let probes = many
                .iter()
                .map(|u| {
                    let p = probe_url(u, proxy, timeout)?;
                    if let Some((kind, code)) = p.transport {
                        eprintln!("verify {u}: {kind} (curl exit {code})");
                    }
                    Ok(p)
                })
                .collect::<Result<Vec<_>>>()?;
            print_batch(many, &probes, json);
            // 批量退出码对齐 batch search（main.rs cmd_search_batch）：全 OK 0 / 部分失败 1 / 全失败 2
            let all_ok = probes.iter().all(|p| p.verdict == 0);
            let any_ok = probes.iter().any(|p| p.verdict == 0);
            Ok(ExitCode::from(if all_ok {
                0
            } else if any_ok {
                1
            } else {
                2
            }))
        }
    }
}

/// 单条探测结果：报告 + 分类判定 + 输出元数据（标注/表格用）。
struct Probe {
    report: VerifyReport,
    /// HEAD 被拒（403/405）后走了 GET 回退。
    get_fallback: bool,
    /// 分类退出码：0=OK / 2=404 / 3=SSL / 4=DNS / 5=超时 / 1=其他。
    verdict: u8,
    /// 传输层失败（未拿到 HTTP 响应）= Some((类别, curl exit code))。
    transport: Option<(&'static str, i32)>,
}

impl Probe {
    /// 探测方式标注；无回退时省略，保持旧输出结构不变。
    fn probe_tag(&self) -> Option<&'static str> {
        self.get_fallback.then_some("get-fallback")
    }

    /// 6a6：失败简述（ok=None）。http_error 带状态码；传输层失败带类别 + curl exit
    /// （与单条 stderr 分类行同文案，中文 Windows 下不透传 GBK stderr）。
    fn error_detail(&self) -> Option<String> {
        match self.verdict {
            0 => None,
            2 => Some(format!("HTTP {}", self.report.status)),
            _ => self
                .transport
                .map(|(kind, code)| format!("{kind} (curl exit {code})")),
        }
    }

    /// ih1：ssl_valid 的输出值。仅 https 有 TLS 参与 → Some(验证结果)；
    /// http（无握手）与其余传输失败 → None（JSON 整键缺席 / 人读 n/a），不再谎报。
    /// 成功路径按最终 URL 的 scheme 判定（重定向后落在 http 也算无 TLS）。
    fn ssl_output(&self) -> Option<bool> {
        match self.transport {
            Some(_) => (self.verdict == 3).then_some(false),
            None => scheme_is_https(&self.report.final_url).then_some(true),
        }
    }
}

/// URL 是否 https（大小写不敏感）；无 scheme 按 curl 默认视为 http。
fn scheme_is_https(url: &str) -> bool {
    url.trim_start().to_ascii_lowercase().starts_with("https://")
}

/// verdict 数值 → 分类名（与单条退出码 0/2/3/4/5 一一对应）。
fn verdict_name(v: u8) -> &'static str {
    match v {
        0 => "ok",
        2 => "http_error",
        3 => "ssl_error",
        4 => "dns_error",
        5 => "timeout",
        _ => "other",
    }
}

/// verify 的 JSON 输出元素。显式字段复刻 VerifyReport 的键序（status/final_url/redirect_chain/
/// ssl_valid/latency_ms，旧消费者按序可读），其后追加 verdict 分类（6a6）与可选标注。
/// 不走 `#[serde(flatten)]`——flatten 经 serde_json::Map 会按字典序重排键。
#[derive(Serialize)]
struct ProbeJson<'a> {
    status: u16,
    final_url: &'a str,
    redirect_chain: &'a [String],
    /// ih1：http 目标整键缺席（Option=None），不再谎报 true。
    #[serde(skip_serializing_if = "Option::is_none")]
    ssl_valid: Option<bool>,
    latency_ms: u64,
    /// 错误分类：ok/http_error/ssl_error/dns_error/timeout/other。
    verdict: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe: Option<&'static str>,
}

fn probe_json(p: &Probe) -> ProbeJson<'_> {
    ProbeJson {
        status: p.report.status,
        final_url: &p.report.final_url,
        redirect_chain: &p.report.redirect_chain,
        ssl_valid: p.ssl_output(),
        latency_ms: p.report.latency_ms,
        verdict: verdict_name(p.verdict),
        error_detail: p.error_detail(),
        probe: p.probe_tag(),
    }
}

/// HEAD 探测被 403/405 拒 → GET（Range: bytes=0-0）重测一次再判定：
/// Cloudflare 类反爬只拒 HEAD，连通本身正常，直接报状态不符是误判（issue gsearch-rs-02z）。
fn should_get_fallback(curl_code: i32, stdout: &str) -> bool {
    curl_code == 0 && matches!(final_status(stdout), Some(403) | Some(405))
}

fn probe_url(url: &str, proxy: Option<&str>, timeout: u64) -> Result<Probe> {
    let started = Instant::now();
    let (curl_code, stdout, get_fallback) = {
        let (code, out, _) = run_curl(&curl_args(url, proxy, timeout, true))?;
        if should_get_fallback(code, &out) {
            let (code, out, _) = run_curl(&curl_args(url, proxy, timeout, false))?;
            (code, out, true)
        } else {
            (code, out, false)
        }
    };

    if curl_code != 0 {
        let verdict = classify_curl_exit(curl_code);
        return Ok(Probe {
            report: VerifyReport {
                status: 0,
                final_url: url.to_owned(),
                redirect_chain: Vec::new(),
                // 结构体字段只存旧缺省；输出侧 ssl_valid 由 Probe::ssl_output() 派生（ih1：
                // 仅 SSL 失败显示 false，DNS/超时与 TLS 无关显示 n/a）
                ssl_valid: false,
                latency_ms: started.elapsed().as_millis() as u64,
            },
            get_fallback,
            verdict,
            transport: Some((transport_kind(verdict), curl_code)),
        });
    }

    let (headers, mut final_url) = split_final_url(&stdout);
    let Some((status, chain)) = report_from_headers(headers) else {
        anyhow::bail!("curl 输出无法解析为响应头: {stdout:?}");
    };
    if final_url.is_empty() {
        final_url = url.to_owned();
    }
    Ok(Probe {
        report: VerifyReport {
            status,
            final_url,
            redirect_chain: chain,
            // 结构体字段存旧缺省 true；输出侧由 Probe::ssl_output() 按最终 URL scheme 判定（ih1）
            ssl_valid: true,
            latency_ms: started.elapsed().as_millis() as u64,
        },
        get_fallback,
        verdict: exit_for_status(status),
        transport: None,
    })
}

/// curl 传输失败分类名：不透传原始 stderr——中文 Windows 上 Schannel 报错是 GBK 文本，
/// 透传到 UTF-8 控制台会乱码；kind + curl exit code 已足够定位。
fn transport_kind(verdict: u8) -> &'static str {
    match verdict {
        3 => "SSL 失败",
        4 => "DNS 失败",
        5 => "超时",
        _ => "网络错误",
    }
}

/// 单 URL 传输层失败：JSON 一律出结构化分类行（6a6，单条/批量同构）；
/// 人读仅 SSL 失败出报告（旧行为），其余只出 stderr 分类行。
fn transport_exit(probe: &Probe, kind: &'static str, curl_code: i32, json: bool) -> ExitCode {
    if json {
        print_json(&probe_json(probe));
    } else if probe.verdict == 3 {
        print_report(&probe.report, probe.ssl_output());
    }
    eprintln!("verify {kind} (curl exit {curl_code})");
    ExitCode::from(probe.verdict)
}

fn print_probe(p: &Probe, json: bool) {
    if json {
        print_json(&probe_json(p));
        return;
    }
    print_report(&p.report, p.ssl_output());
    if let Some(tag) = p.probe_tag() {
        println!("probe:       {tag}");
    }
}

/// 批量输出：--json 为报告数组（元素含 verdict 分类）；人读为对比表（含分类列，6a6）。
fn print_batch(urls: &[String], probes: &[Probe], json: bool) {
    if json {
        let rows: Vec<ProbeJson> = probes.iter().map(probe_json).collect();
        print_json(&rows);
        return;
    }
    let w = urls.iter().map(String::len).max().unwrap_or(3).max(3);
    println!(
        "{:<w$}  {:>6}  {:>5}  {:>11}  {:>10}",
        "url", "status", "ssl", "verdict", "latency_ms",
        w = w
    );
    for (u, p) in urls.iter().zip(probes) {
        println!(
            "{:<w$}  {:>6}  {:>5}  {:>11}  {:>10}",
            u,
            p.report.status,
            ssl_cell(p.ssl_output()),
            verdict_name(p.verdict),
            p.report.latency_ms,
            w = w
        );
    }
}

/// ssl 列单元格——None = http 无 TLS 参与，显示 n/a。
fn ssl_cell(ssl: Option<bool>) -> &'static str {
    match ssl {
        Some(true) => "true",
        Some(false) => "false",
        None => "n/a",
    }
}

/// JSON 走 compact 单行——输出契约面向 agent 消费，pretty 纯耗 token。
fn print_json<T: Serialize>(v: &T) {
    match serde_json::to_string(v) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("JSON 序列化失败: {e}"),
    }
}

fn print_report(r: &VerifyReport, ssl: Option<bool>) {
    println!("status:      {}", r.status);
    println!("final_url:   {}", r.final_url);
    if r.redirect_chain.is_empty() {
        println!("redirects:   0");
    } else {
        println!("redirects:   {}", r.redirect_chain.join(" -> "));
    }
    // ih1：http 目标无 TLS 参与 → n/a，不继承结构体缺省值谎报
    println!("ssl_valid:   {}", ssl.map(|b| b.to_string()).unwrap_or_else(|| "n/a".into()));
    println!("latency_ms:  {}", r.latency_ms);
}

fn curl_args(url: &str, proxy: Option<&str>, timeout: u64, head: bool) -> Vec<String> {
    let mut args = vec![
        "-sS".to_owned(),
        "--max-time".to_owned(),
        timeout.to_string(),
        "-L".to_owned(),
        "--max-redirs".to_owned(),
        MAX_REDIRECT_HOPS.to_string(),
        // cgp L4：URL 含 [] {} 时禁 curl globbing（不加工请求语义）
        "--globoff".to_owned(),
        "-D".to_owned(),
        "-".to_owned(),
        "-o".to_owned(),
        null_device().to_owned(),
        "-w".to_owned(),
        format!("\n{FINAL_URL_MARKER}%{{url_effective}}"),
    ];
    if head {
        args.push("--head".to_owned());
    } else {
        // GET 探测只取首字节防大文件拉满；不认 Range 的站点由 -o NUL 兜底
        args.push("-H".to_owned());
        args.push("Range: bytes=0-0".to_owned());
    }
    if let Some(p) = proxy {
        args.push("--proxy".to_owned());
        args.push(p.to_owned());
    }
    // cgp L4：`--` 终结选项解析——来自不可信内容的 URL 以 `-` 开头不得被 curl 当选项
    args.push("--".to_owned());
    args.push(url.to_owned());
    args
}

fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

/// 跑一次 curl，返回 (exit code, stdout, stderr)。被信号杀死（code=None）按「其他」处理。
fn run_curl(args: &[String]) -> Result<(i32, String, String)> {
    let out = Command::new("curl")
        .args(args)
        .output()
        .context("curl 不可用（verify 依赖系统 curl，Windows 10+ 自带）")?;
    Ok((
        out.status.code().unwrap_or(1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// stdout = header dump + `\n{MARKER}` + 最终 URL；标记缺失时 final_url 为空由调用方兜底。
fn split_final_url(raw: &str) -> (&str, String) {
    match raw.split_once(FINAL_URL_MARKER) {
        Some((h, u)) => (h, u.trim().to_owned()),
        None => (raw, String::new()),
    }
}

fn final_status(stdout: &str) -> Option<u16> {
    report_from_headers(split_final_url(stdout).0).map(|(s, _)| s)
}

/// 从 `-D -` 多 hop 响应头 dump 解析 (最终状态码, 已跟随的 redirect 链)。
/// 每个 "HTTP/" 行开一个 block；链 = 非最终 block 的 Location 值（原样，可能相对路径）。
/// 代理 CONNECT 产生的中间 200 block 无 Location，天然不入链。
fn report_from_headers(headers: &str) -> Option<(u16, Vec<String>)> {
    let mut status: Option<u16> = None;
    let mut chain: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;
    for line in headers.lines() {
        let line = line.trim();
        if line.starts_with("HTTP/") {
            // 上一个 block 结束：它的 Location 属于已跟随的跳转
            if status.is_some() && let Some(loc) = pending.take() {
                chain.push(loc);
            }
            if let Some(code) = line.split_whitespace().nth(1).and_then(|t| t.parse().ok()) {
                status = Some(code);
            }
        } else if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("location")
        {
            pending = Some(value.trim().to_owned());
        }
    }
    status.map(|s| (s, chain))
}

/// curl exit code → gsearch verify 退出码。
/// SSL 集合 = curl 文档 TLS/证书类错误（35 握手 / 51 证书 / 58-60 证书链 / 66/77 引擎与 CA 载入等）。
fn classify_curl_exit(code: i32) -> u8 {
    match code {
        6 => 4,   // DNS 解析失败
        28 => 5,  // 超时（--max-time 触发）
        35 | 51 | 53 | 54 | 58 | 59 | 60 | 64 | 66 | 77 | 90 | 91 => 3, // SSL/TLS
        _ => 1,   // 其他（7 连接拒绝 / 56 接收错误 / 47 重定向过多 …）
    }
}

/// 2xx/3xx（curl -L 跟随后的终态）= ok；4xx/5xx = http_error（buq：5xx 曾误判 ok，
/// agent 把服务端故障当健康）。具体状态码由 report.status + error_detail 携带，不另设分类。
fn exit_for_status(status: u16) -> u8 {
    if status >= 400 { 2 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOP_DUMP: &str = "HTTP/1.1 301 Moved Permanently\r\n\
Location: https://a.com/\r\n\
Server: cf\r\n\
\r\n\
HTTP/1.1 302 Found\r\n\
location: /login\r\n\
\r\n\
HTTP/2 200\r\n\
content-type: text/html\r\n\
\r\n";

    /// 验收点名用例：多 hop dump → 最终 status + 已跟随 redirect 链。
    #[test]
    fn verify_report_from_response_headers() {
        let (status, chain) = report_from_headers(HOP_DUMP).unwrap();
        assert_eq!(status, 200);
        assert_eq!(chain, vec!["https://a.com/", "/login"]);
    }

    #[test]
    fn single_200_has_empty_chain() {
        let dump = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n";
        let (status, chain) = report_from_headers(dump).unwrap();
        assert_eq!(status, 200);
        assert!(chain.is_empty());
    }

    /// 最终 block 自身是 3xx（curl 未跟完）→ 其 Location 不算已跟随。
    #[test]
    fn trailing_redirect_location_excluded_from_chain() {
        let dump = "HTTP/1.1 301\r\nLocation: https://x/\r\n\r\nHTTP/1.1 302\r\nLocation: https://y/\r\n\r\n";
        let (status, chain) = report_from_headers(dump).unwrap();
        assert_eq!(status, 302);
        assert_eq!(chain, vec!["https://x/"]);
    }

    #[test]
    fn proxy_connect_block_does_not_pollute_chain() {
        // 走代理时 curl 会先 dump 一个 CONNECT 200 block（无 Location）
        let dump = "HTTP/1.1 200 Connection established\r\n\r\nHTTP/1.1 200 OK\r\n\r\n";
        let (status, chain) = report_from_headers(dump).unwrap();
        assert_eq!(status, 200);
        assert!(chain.is_empty());
    }

    #[test]
    fn split_final_url_extracts_url_after_marker() {
        let raw = format!("HTTP/1.1 200\r\n\r\n\n{FINAL_URL_MARKER}https://final/");
        let (h, u) = split_final_url(&raw);
        assert!(h.starts_with("HTTP/1.1 200"));
        assert_eq!(u, "https://final/");
    }

    #[test]
    fn curl_exit_codes_classified() {
        assert_eq!(classify_curl_exit(6), 4); // DNS
        assert_eq!(classify_curl_exit(28), 5); // 超时
        assert_eq!(classify_curl_exit(35), 3); // SSL 握手
        assert_eq!(classify_curl_exit(60), 3); // SSL 证书不受信
        assert_eq!(classify_curl_exit(7), 1); // 连接拒绝 → 其他
        assert_eq!(classify_curl_exit(1), 1);
    }

    /// buq：verdict 分类表锁——2xx/3xx=ok（curl -L 终态），4xx/5xx 一律 http_error，
    /// DNS/SSL/超时/other 分类不变（classify_curl_exit）。
    #[test]
    fn exit_for_status_classification_table() {
        assert_eq!(exit_for_status(200), 0);
        assert_eq!(exit_for_status(204), 0);
        assert_eq!(exit_for_status(301), 0);
        assert_eq!(exit_for_status(302), 0);
        assert_eq!(exit_for_status(307), 0);
        assert_eq!(exit_for_status(400), 2);
        assert_eq!(exit_for_status(403), 2);
        assert_eq!(exit_for_status(404), 2);
        assert_eq!(exit_for_status(429), 2);
        assert_eq!(exit_for_status(500), 2);
        assert_eq!(exit_for_status(502), 2);
        assert_eq!(exit_for_status(503), 2);
    }

    /// 验收点名用例：超时 → ExitCode 5。回环起一个不 accept 的 listener，
    /// curl 能完成 TCP 连接（backlog）但永远等不到响应，--max-time 1 触发 exit 28。
    /// 不出外网、不怕防火墙干扰，确定性复现超时路径。
    #[test]
    fn timeout_returns_exit_code_5() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}/");
        let args = vec![
            "-sS".to_owned(),
            "--max-time".to_owned(),
            "1".to_owned(),
            "-o".to_owned(),
            null_device().to_owned(),
            url,
        ];
        let (code, _, _) = run_curl(&args).expect("系统 curl 应可用");
        assert_eq!(code, 28, "回环不响应应在 1s 触发 --max-time，实际 curl exit {code}");
        assert_eq!(classify_curl_exit(code), 5);
    }

    #[test]
    fn curl_args_honor_timeout_flag() {
        let args = curl_args("https://x/", None, 10, true);
        let i = args.iter().position(|a| a == "--max-time").unwrap();
        assert_eq!(args[i + 1], "10");
        // 默认值常量与旧硬编码一致（向后兼容闸）
        assert_eq!(VERIFY_TIMEOUT_SECS, 5);
    }

    #[test]
    fn get_probe_sends_range_and_drops_head() {
        let get = curl_args("https://x/", None, 5, false);
        assert!(!get.iter().any(|a| a == "--head"));
        assert!(get.iter().any(|a| a == "Range: bytes=0-0"));
        let head = curl_args("https://x/", None, 5, true);
        assert!(head.iter().any(|a| a == "--head"));
        assert!(!head.iter().any(|a| a == "Range: bytes=0-0"));
    }

    /// cgp L4：--globoff 禁 URL globbing；`--` 终结选项解析（`-` 开头的不可信 URL
    /// 不得被 curl 当选项）。
    #[test]
    fn curl_args_ends_with_double_dash_before_url() {
        for url in ["-K/etc/passwd", "https://x/[1-10]", "https://example.com/"] {
            let args = curl_args(url, None, 5, false);
            assert!(args.iter().any(|a| a == "--globoff"), "缺 --globoff: {args:?}");
            let last = args.last().expect("URL 是最后参数");
            assert_eq!(last, url, "URL 必须是最后一个 argv");
            let sep = &args[args.len() - 2];
            assert_eq!(sep, "--", "URL 前必须有 -- 分隔: {args:?}");
        }
    }

    #[test]
    fn get_fallback_on_403_and_405_only() {
        let dump = |code: u16| format!("HTTP/1.1 {code}\r\n\r\n");
        assert!(should_get_fallback(0, &dump(403)));
        assert!(should_get_fallback(0, &dump(405)));
        assert!(!should_get_fallback(0, &dump(200)));
        assert!(!should_get_fallback(0, &dump(500)));
        // 传输层失败（curl exit != 0）没有状态可判，不回退
        assert!(!should_get_fallback(28, &dump(403)));
    }

    fn probe_fixture(get_fallback: bool) -> Probe {
        Probe {
            report: VerifyReport {
                status: 200,
                final_url: "https://x/".into(),
                redirect_chain: vec![],
                ssl_valid: true,
                latency_ms: 7,
            },
            get_fallback,
            verdict: 0,
            transport: None,
        }
    }

    /// 无回退时无 probe 标注 → print_probe 走旧 VerifyReport 直序列化路径（JSON 不变形）。
    #[test]
    fn no_fallback_probe_has_no_tag() {
        let p = probe_fixture(false);
        assert_eq!(p.probe_tag(), None);
    }

    #[test]
    fn probe_json_marks_get_fallback() {
        let p = probe_fixture(true);
        let s = serde_json::to_string(&probe_json(&p)).unwrap();
        assert!(s.contains(r#""probe":"get-fallback""#));
        assert!(s.contains(r#""status":200"#));
    }

    /// 6a6：分类名与单条退出码语义一一对应。
    #[test]
    fn verdict_names_match_exit_code_semantics() {
        assert_eq!(verdict_name(0), "ok");
        assert_eq!(verdict_name(2), "http_error");
        assert_eq!(verdict_name(3), "ssl_error");
        assert_eq!(verdict_name(4), "dns_error");
        assert_eq!(verdict_name(5), "timeout");
        assert_eq!(verdict_name(1), "other");
    }

    /// 6a6：ok 无 error_detail；4xx/5xx（buq）→ HTTP 状态码简述；传输层失败 → 类别 + curl exit。
    #[test]
    fn error_detail_classifies_failures() {
        let mut p = probe_fixture(false);
        assert_eq!(p.error_detail(), None);

        p.verdict = 2;
        p.report.status = 404;
        assert_eq!(p.error_detail().as_deref(), Some("HTTP 404"));

        let mut p = probe_fixture(false);
        p.verdict = 4;
        p.transport = Some(("DNS 失败", 6));
        assert_eq!(p.error_detail().as_deref(), Some("DNS 失败 (curl exit 6)"));
    }

    /// ih1：ssl_valid 输出值按 scheme/失败类别派生——https 成功 true、http n/a、
    /// SSL 失败 false、DNS 失败不谎报 false。
    #[test]
    fn ssl_output_never_lies_for_http() {
        let mut p = probe_fixture(false);
        assert_eq!(p.ssl_output(), Some(true));

        p.report.final_url = "http://x/".into();
        assert_eq!(p.ssl_output(), None);

        let mut p = probe_fixture(false);
        p.verdict = 4;
        p.transport = Some(("DNS 失败", 6));
        assert_eq!(p.ssl_output(), None);

        p.verdict = 3;
        p.transport = Some(("SSL 失败", 60));
        assert_eq!(p.ssl_output(), Some(false));

        assert!(scheme_is_https("HTTPS://X/"));
        assert!(!scheme_is_https("http://x/"));
        assert!(!scheme_is_https("x/"));
    }

    /// ih1/6a6：JSON 元素——http 目标 ssl_valid 整键缺席，verdict 分类在场。
    #[test]
    fn probe_json_omits_ssl_valid_for_http() {
        let mut p = probe_fixture(false);
        p.report.final_url = "http://x/".into();
        let s = serde_json::to_string(&probe_json(&p)).unwrap();
        assert!(!s.contains("ssl_valid"));
        assert!(s.contains(r#""verdict":"ok""#));

        p.verdict = 4;
        p.transport = Some(("DNS 失败", 6));
        let s = serde_json::to_string(&probe_json(&p)).unwrap();
        assert!(s.contains(r#""verdict":"dns_error""#));
        assert!(s.contains(r#""error_detail""#));
    }
}
