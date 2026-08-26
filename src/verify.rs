//! M14-1A `verify <url>`：headless URL 健康检查（不启动 Chrome）。
//!
//! 传输层直接调系统 curl（Windows 10+ 自带 curl.exe，Schannel 后端 = OS 证书库真验证），
//! HTTP 语义在本模块手写：状态行解析、redirect 链提取、405→GET 回退、错误分类。
//! ponytail: 上游指令原文是「std::net 手写 HTTP」，但验收要求 https 的 ssl_valid=true——
//! 纯 TcpStream 无法完成 TLS 握手，依赖树亦无 rustls/native-tls；零新依赖约束下
//! curl.exe 是唯一能真验 TLS 的路径（doctor 的 fetch_public_ip 有同款先例）。

use std::process::{Command, ExitCode};
use std::time::Instant;

use anyhow::{Context, Result};

use crate::types::VerifyReport;

/// verify 总预算（含全部 redirect hop），对应 curl --max-time；轻量快查不拖累。
const VERIFY_TIMEOUT_SECS: u64 = 5;
/// redirect hop 上限，防 A→B→A 死循环把超时吃满。
const MAX_REDIRECT_HOPS: u32 = 10;
/// `-w` 输出分隔标记：stdout = 各 hop 响应头 dump + 本标记 + 最终 URL。
const FINAL_URL_MARKER: &str = "__GSEARCH_FINAL__";

/// `gsearch verify <url>`：HTTP HEAD（405 时回退 GET）→ 5 项报告 + 分类退出码。
/// 退出码：0=OK / 2=404 / 3=SSL 失败 / 4=DNS 失败 / 5=超时 / 1=其他。
pub fn cmd_verify(url: &str, json: bool, proxy: Option<&str>) -> Result<ExitCode> {
    let started = Instant::now();

    let (code, stdout) = {
        let (code, stdout, _) = run_curl(&curl_args(url, proxy, true))?;
        // 405 Method Not Allowed → 回退 GET 重测（spec 点名的 fallback）
        if code == 0 && final_status(&stdout) == Some(405) {
            let (code, stdout, _) = run_curl(&curl_args(url, proxy, false))?;
            (code, stdout)
        } else {
            (code, stdout)
        }
    };

    if code != 0 {
        return Ok(handle_transport_error(code, url, json, started));
    }

    let (headers, mut final_url) = split_final_url(&stdout);
    let Some((status, chain)) = report_from_headers(headers) else {
        anyhow::bail!("curl 输出无法解析为响应头: {stdout:?}");
    };
    if final_url.is_empty() {
        final_url = url.to_owned();
    }
    let report = VerifyReport {
        status,
        final_url,
        redirect_chain: chain,
        // https：握手+证书已由 curl/Schannel 验证通过；http：无握手即无异常。
        ssl_valid: true,
        latency_ms: started.elapsed().as_millis() as u64,
    };
    print_report(&report, json);
    Ok(ExitCode::from(exit_for_status(status)))
}

/// curl 传输失败（非 HTTP 响应）：分类退出码；SSL 失败仍出 ssl_valid=false 的报告。
/// 不透传 curl 原始 stderr：中文 Windows 上 Schannel 报错是 GBK 本地化文本，
/// 透传到 UTF-8 控制台会乱码；kind + curl exit code 已足够定位。
fn handle_transport_error(code: i32, url: &str, json: bool, started: Instant) -> ExitCode {
    let exit = classify_curl_exit(code);
    if exit == 3 {
        // spec：握手错误 ssl_valid=false。status=0 表示未拿到任何 HTTP 响应。
        let report = VerifyReport {
            status: 0,
            final_url: url.to_owned(),
            redirect_chain: Vec::new(),
            ssl_valid: false,
            latency_ms: started.elapsed().as_millis() as u64,
        };
        print_report(&report, json);
    }
    let kind = match exit {
        3 => "SSL 失败",
        4 => "DNS 失败",
        5 => "超时",
        _ => "网络错误",
    };
    eprintln!("verify {kind} (curl exit {code})");
    ExitCode::from(exit)
}

fn print_report(r: &VerifyReport, json: bool) {
    if json {
        match serde_json::to_string_pretty(r) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("JSON 序列化失败: {e}"),
        }
    } else {
        println!("status:      {}", r.status);
        println!("final_url:   {}", r.final_url);
        if r.redirect_chain.is_empty() {
            println!("redirects:   0");
        } else {
            println!("redirects:   {}", r.redirect_chain.join(" -> "));
        }
        println!("ssl_valid:   {}", r.ssl_valid);
        println!("latency_ms:  {}", r.latency_ms);
    }
}

fn curl_args(url: &str, proxy: Option<&str>, head: bool) -> Vec<String> {
    let mut args = vec![
        "-sS".to_owned(),
        "--max-time".to_owned(),
        VERIFY_TIMEOUT_SECS.to_string(),
        "-L".to_owned(),
        "--max-redirs".to_owned(),
        MAX_REDIRECT_HOPS.to_string(),
        "-D".to_owned(),
        "-".to_owned(),
        "-o".to_owned(),
        null_device().to_owned(),
        "-w".to_owned(),
        format!("\n{FINAL_URL_MARKER}%{{url_effective}}"),
    ];
    if head {
        args.push("--head".to_owned());
    }
    if let Some(p) = proxy {
        args.push("--proxy".to_owned());
        args.push(p.to_owned());
    }
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
        } else if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("location") {
                pending = Some(value.trim().to_owned());
            }
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

/// spec 仅特判 404→2；其余拿到响应即 0，状态码由 report 携带（agent 读 status 字段）。
fn exit_for_status(status: u16) -> u8 {
    if status == 404 { 2 } else { 0 }
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

    #[test]
    fn exit_for_status_maps_404_only() {
        assert_eq!(exit_for_status(200), 0);
        assert_eq!(exit_for_status(404), 2);
        assert_eq!(exit_for_status(500), 0);
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
}
