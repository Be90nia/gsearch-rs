//! bd gsearch-rs-eu8：`site:` 限定符被白名单引擎静默吞掉时的诚实诊断。
//!
//! 三条锁：
//!
//!   1. **客户端不吞**——假 SearXNG 收到的 `q` 参数原样含 `site:`（证明「静默吞掉」发生在
//!      引擎侧，不是 gsearch 把限定符丢了；引擎级 site: 支持差异客户端拿不到）。
//!   2. **含 site: 且零结果**——诊断行给出「部分引擎不支持 site:」+ 可执行下一步，
//!      且**不编造被忽略的引擎数**（`unresponsive_engines` 是「引擎没答上来」，不是「忽略了限定符」）。
//!   3. **不含 site: 的零结果**——诊断行与既有文案逐字节一致（正常查询的输出形态不变）。
//!
//! 零外网：本地回环假 SearXNG（仿 `tests/cli_golden_bytes.rs`）。
//! 零结果走 **batch 入口**（SearXNG-only、禁浏览器回退）——单查询入口的零结果会走
//! DDG/Google 回退链（真实外网 + 可能起浏览器），不可进测试。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{LazyLock, Mutex};

/// 既有零结果文案（`src/search.rs::NO_RESULTS_MSG`）——不含 site: 时必须逐字节一致。
const NO_RESULTS_MSG: &str = "查询无结果（SearXNG 源健康，非基础设施故障）；建议换词或缩短查询重试";

/// 有结果的 query——单查询入口用它验证「无 site: 时 stdout 不带任何 site: 文本」。
const Q_WITH_RESULTS: &str = "rust async";
/// 含 site: 的零结果 query。
const Q_SITE: &str = "site:github.com rust";
/// 不含 site: 的零结果 query。
const Q_PLAIN_EMPTY: &str = "kubernetes pdb yaml";

/// 假 SearXNG 收到的 q 参数（按到达顺序）——用于证明客户端原样透传 site:。
static RECEIVED_Q: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn received_q() -> &'static Mutex<Vec<String>> {
    &RECEIVED_Q
}

/// 本地回环假 SearXNG：`rust async` 给 1 条结果，其余 query 零结果。
/// 返回 base URL；收到的 q 参数记进 `received_q()`。
fn spawn_fake_searxng() -> String {
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
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 64 * 1024 {
                            break;
                        }
                    }
                }
            }
            let head = String::from_utf8_lossy(&buf);
            let target = head
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or_default();
            let raw_q = target
                .split_once('?')
                .map(|(_, q)| q)
                .unwrap_or("")
                .split('&')
                .find_map(|kv| kv.strip_prefix("q="))
                .unwrap_or("");
            received_q()
                .lock()
                .expect("锁 q 记录")
                .push(raw_q.to_string());
            // 只给 Q_WITH_RESULTS（"rust async"）结果，其余 query（含 site: 那个）零结果
            let body = if raw_q.contains("async") {
                r#"{"results":[{"title":"Rust async guide","url":"https://example.com/async","content":"async rust patterns","score":1.0}]}"#
            } else {
                r#"{"results":[]}"#
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        }
    });
    base
}

/// 临时配置文件的序号——每次调用一个唯一文件名（同名临时文件会被并行用例互删）。
static CFG_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 跑真实二进制（`search` 子命令，多 query = batch 入口）→ (stdout, stderr, 退出码)。
fn run_cli(base: &str, queries: &[&str], json: bool) -> (String, String, i32) {
    let seq = CFG_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let cfg_path = std::env::temp_dir().join(format!(
        "gsearch-site-diag-{}-{seq}.json",
        std::process::id()
    ));
    std::fs::write(&cfg_path, "{}").expect("写临时配置");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gsearch"));
    cmd.arg("--config").arg(&cfg_path).arg("search");
    for q in queries {
        cmd.arg(q);
    }
    if json {
        cmd.arg("--json");
    }
    cmd.env("GSEARCH_SEARXNG_URL", base);
    for k in [
        "GSEARCH_PROXY",
        "GSEARCH_PROFILE",
        "GSEARCH_LOG",
        "RUST_LOG",
    ] {
        cmd.env_remove(k);
    }
    let out = cmd.output().expect("跑 gsearch 二进制");
    let _ = std::fs::remove_file(&cfg_path);
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// eu8-1：客户端把 `site:` 原样透传给 SearXNG（吞限定符的不是 gsearch）。
#[test]
fn client_forwards_site_qualifier_verbatim() {
    let base = spawn_fake_searxng();
    let (stdout, _stderr, _code) = run_cli(&base, &[Q_SITE, Q_PLAIN_EMPTY], true);
    let seen = received_q().lock().expect("锁 q 记录").clone();
    assert!(
        seen.iter().any(|q| q.contains("site%3Agithub.com")),
        "SearXNG 侧收到的 q 必须原样含 site:（证明吞限定符发生在引擎侧）: {seen:?}"
    );
    assert!(
        seen.iter().any(|q| q.contains("kubernetes")),
        "对照 query 也应到达假服务器: {seen:?}"
    );
    assert!(!stdout.is_empty(), "batch --json 应有输出");
}

/// eu8-2：含 site: 且零结果 → 诊断行给出「部分引擎不支持 site:」+ 可执行下一步，且不编造引擎数。
#[test]
fn site_query_with_zero_results_gets_actionable_diagnosis() {
    let base = spawn_fake_searxng();
    let (stdout, _stderr, code) = run_cli(&base, &[Q_SITE, Q_PLAIN_EMPTY], true);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout 是合法 JSON 数组");
    let arr = v.as_array().expect("batch --json 是裸数组");
    let site_entry = arr
        .iter()
        .find(|e| e["query"] == Q_SITE)
        .expect("含 site: 的条目");
    let msg = site_entry["message"]
        .as_str()
        .expect("零结果条目须带 message（诊断行）");
    assert!(msg.contains("site:"), "诊断行须点名 site: 限定符: {msg}");
    assert!(msg.contains("建议"), "诊断行须给可执行的下一步: {msg}");
    assert!(
        msg.starts_with(NO_RESULTS_MSG),
        "既有零结果语义须保留（在其后追加解释）: {msg}"
    );
    assert!(
        !msg.contains("个引擎"),
        "不得编造被忽略的引擎数（客户端拿不到引擎级 site: 支持情况）: {msg}"
    );
    assert_eq!(
        site_entry["status"], "no_results",
        "退出码/状态语义不变（结果为空 ≠ 新错误态）: {stdout}"
    );
    assert_eq!(code, 2, "零结果退出码语义不变（rc=2）");
}

/// eu8-3：不含 site: 的零结果 → 诊断行逐字节不变（正常查询输出形态不动）。
#[test]
fn plain_query_zero_result_diagnosis_is_unchanged() {
    let base = spawn_fake_searxng();
    let (stdout, _stderr, code) = run_cli(&base, &[Q_SITE, Q_PLAIN_EMPTY], true);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout 是合法 JSON 数组");
    let arr = v.as_array().expect("batch --json 是裸数组");
    let plain = arr
        .iter()
        .find(|e| e["query"] == Q_PLAIN_EMPTY)
        .expect("不含 site: 的条目");
    assert_eq!(
        plain["message"].as_str(),
        Some(NO_RESULTS_MSG),
        "不含 site: 的零结果诊断行必须逐字节不变: {stdout}"
    );
    assert_eq!(code, 2, "零结果退出码语义不变（rc=2）");
}

/// eu8-4：有结果的普通查询 → stdout 不带任何 site: 文本、stderr 干净（正常态输出不动）。
#[test]
fn query_without_site_has_no_site_text_in_output() {
    let base = spawn_fake_searxng();
    let (stdout, stderr, code) = run_cli(&base, &[Q_WITH_RESULTS], true);
    assert_eq!(code, 0, "有结果 → rc=0");
    assert!(
        !stdout.contains("site:"),
        "不含 site: 的查询输出不得出现 site: 文本: {stdout}"
    );
    assert!(stderr.is_empty(), "正常态 stderr 必须干净: {stderr}");
}
