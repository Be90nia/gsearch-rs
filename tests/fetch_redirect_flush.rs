//! bd gsearch-rs-0qw：`fetch … > file` 重定向的产物必须完整落盘（与管道路径一致）。
//!
//! 锁三条：
//!
//!   1. **重定向 ≠ 少字节**：stdout 重定向到文件时，产物字节数与管道路径（`| tee`）一致，
//!      且非空（盲测八 H 实锤「0 字节产出」）。
//!   2. **产物完整**：落盘文件含正文的**首尾**标记（不是只落了开头一段）。
//!   3. **JSON 路径同理**：`--json` 输出是单行长文本（行缓冲下最容易卡在缓冲区尾部），
//!      重定向与管道的归一后字节一致。
//!
//! 零外网：本地回环 HTTP 服务器伺服大 body（`--allow-private` 放行 loopback）。
//! 归一只动耗时字段（`elapsed: 0.1s` / `"elapsed_ms":12`）——两次运行的耗时不必然相同，
//! 其余字节（正文 / 键序 / 空白）全部参与比对。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};

const HEAD_MARKER: &str = "GSEARCH-FETCH-HEAD-MARKER";
const TAIL_MARKER: &str = "GSEARCH-FETCH-TAIL-MARKER";

/// 伺服大 body 的本地回环服务器——返回带首尾标记的 HTML（正文远大于 8KB 缓冲容量）。
fn spawn_big_server() -> String {
    let mut body = String::from("<html><head><title>Big</title></head><body>");
    body.push_str(&format!("<p>{HEAD_MARKER}</p>\n"));
    for i in 0..700 {
        body.push_str(&format!(
            "<p>paragraph {i} lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor</p>\n"
        ));
    }
    body.push_str(&format!("<p>{TAIL_MARKER}</p>\n"));
    body.push_str("</body></html>");
    let body = body.into_bytes();

    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回环端口");
    let base = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("取回环端口").port()
    );
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut sock) = stream else { continue };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match sock.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                }
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.write_all(&body);
            let _ = sock.flush();
        }
    });
    base
}

/// 归一：只动耗时字段（md 的 `elapsed: 0.1s` 与 JSON 的 `"elapsed_ms":12`）。
fn normalize(s: &str) -> String {
    let mut out = String::new();
    let mut rest: &str = s;
    while let Some(i) = rest.find("elapsed: ") {
        out.push_str(&rest[..i + "elapsed: ".len()]);
        let after = &rest[i + "elapsed: ".len()..];
        let end = after.find('s').map(|p| p + 1).unwrap_or(0);
        out.push_str("0.0s");
        rest = &after[end..];
    }
    out.push_str(rest);

    let mut json = String::new();
    let mut tail = out.as_str();
    while let Some(i) = tail.find("\"elapsed_ms\":") {
        let marker = "\"elapsed_ms\":";
        json.push_str(&tail[..i + marker.len()]);
        let after = &tail[i + marker.len()..];
        let digits = after.chars().take_while(|c| c.is_ascii_digit()).count();
        json.push('0');
        tail = &after[digits..];
    }
    json.push_str(tail);
    json
}

/// 临时配置文件序号——每次调用一个唯一文件名（libtest 并行跑用例，同名会互删）。
static CFG_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn cfg_path() -> std::path::PathBuf {
    let seq = CFG_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!(
        "gsearch-fetch-redirect-{}-{seq}.json",
        std::process::id()
    ));
    std::fs::write(&p, "{}").expect("写临时配置");
    p
}

/// 清掉可能劫持回环请求的代理环境（fetch 客户端默认跟随系统代理；SearXNG 客户端已 no_proxy）。
fn strip_proxy_env(cmd: &mut Command) {
    for k in [
        "GSEARCH_PROXY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        cmd.env_remove(k);
    }
}

/// 跑 fetch：stdout 重定向到文件 → 落盘字节。
fn run_redirected(url: &str, json: bool) -> Vec<u8> {
    let cfg = cfg_path();
    let out_file = std::env::temp_dir().join(format!(
        "gsearch-fetch-redirect-out-{}-{}.txt",
        std::process::id(),
        CFG_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let file = std::fs::File::create(&out_file).expect("建落盘文件");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gsearch"));
    cmd.args(["--config", cfg.to_str().expect("配置路径"), "fetch", url]);
    cmd.args(["--max-chars", "100000", "--allow-private"]);
    if json {
        cmd.arg("--json");
    }
    strip_proxy_env(&mut cmd);
    cmd.stdout(Stdio::from(file));
    cmd.stderr(Stdio::null());
    let status = cmd.status().expect("跑 gsearch 二进制");
    assert!(status.success(), "fetch 应成功退出: {status}");
    let bytes = std::fs::read(&out_file).expect("读落盘文件");
    let _ = std::fs::remove_file(&out_file);
    let _ = std::fs::remove_file(&cfg);
    bytes
}

/// 跑 fetch：stdout 走管道（`2>&1 | tee` 那条路的等价形态）→ 捕获字节。
fn run_piped(url: &str, json: bool) -> Vec<u8> {
    let cfg = cfg_path();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gsearch"));
    cmd.args(["--config", cfg.to_str().expect("配置路径"), "fetch", url]);
    cmd.args(["--max-chars", "100000", "--allow-private"]);
    if json {
        cmd.arg("--json");
    }
    strip_proxy_env(&mut cmd);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let out = cmd.output().expect("跑 gsearch 二进制");
    assert!(out.status.success(), "fetch 应成功退出: {}", out.status);
    let _ = std::fs::remove_file(&cfg);
    out.stdout
}

/// 0qw-1：重定向到文件的产物字节数 == 管道路径，且非空（盲测八 H 的「0 字节」回归锁）。
#[test]
fn redirected_stdout_has_same_bytes_as_piped_stdout() {
    let base = spawn_big_server();
    let url = format!("{base}/big.html");
    let redirected = run_redirected(&url, false);
    let piped = run_piped(&url, false);
    assert!(
        !redirected.is_empty(),
        "重定向落盘产物不得为空（0 字节回归）"
    );
    assert!(
        redirected.len() > 10_000,
        "落盘产物应是大页面全文，不是片段: {} 字节",
        redirected.len()
    );
    assert_eq!(
        normalize(&String::from_utf8_lossy(&redirected)),
        normalize(&String::from_utf8_lossy(&piped)),
        "重定向与管道的 stdout 产物必须一致（归一只动耗时字段）"
    );
}

/// 0qw-2：落盘产物完整——首尾标记都在（不是只落了缓冲区里先到的开头一段）。
#[test]
fn redirected_stdout_contains_full_body() {
    let base = spawn_big_server();
    let url = format!("{base}/big.html");
    let redirected = String::from_utf8_lossy(&run_redirected(&url, false)).to_string();
    assert!(
        redirected.contains(HEAD_MARKER),
        "落盘产物缺正文首部标记: {} 字节",
        redirected.len()
    );
    assert!(
        redirected.contains(TAIL_MARKER),
        "落盘产物缺正文尾部标记（内容被截断/未刷出）: {} 字节",
        redirected.len()
    );
}

/// 0qw-3：JSON 路径（单行长文本）同样一致——行缓冲下尾部最容易卡在缓冲区里。
#[test]
fn redirected_json_stdout_has_same_bytes_as_piped_stdout() {
    let base = spawn_big_server();
    let url = format!("{base}/big.json");
    let redirected = run_redirected(&url, true);
    let piped = run_piped(&url, true);
    assert!(!redirected.is_empty(), "重定向落盘产物不得为空");
    let text = String::from_utf8_lossy(&redirected).to_string();
    let v: serde_json::Value = serde_json::from_str(&text).expect("落盘 JSON 合法");
    assert_eq!(
        v["status"], "ok",
        "落盘 JSON 须是完整信封（不是半截）: {text:.200}"
    );
    assert!(
        v["text"].as_str().is_some_and(|t| t.contains(TAIL_MARKER)),
        "落盘 JSON 的正文须含尾部标记: {text:.200}"
    );
    assert_eq!(
        normalize(&text),
        normalize(&String::from_utf8_lossy(&piped)),
        "JSON 路径：重定向与管道的产物必须一致"
    );
}
