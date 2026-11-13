//! bd gsearch-rs-d8s B3：正常态 `--json` / md 的**真实 CLI stdout 逐字节**回归 + meta 键集固定。
//!
//! 走真实二进制（`env!("CARGO_BIN_EXE_gsearch")`）+ 本地回环假 SearXNG（仿 `tests/fallback_chain.rs`
//! 的 TcpListener 假服务器；零外网、零浏览器、确定性）。三条锁：
//!
//!   1. **逐字节**：stdout 归一（耗时字段）后与仓内 golden 逐字节相同——挡住「改键序 / 改输出
//!      形态 / 往正常态 meta 加键」类回归。
//!   2. **meta 键集固定**：正常态 meta/run/results 的键集与键序精确等于预期列表（缺席语义）。
//!   3. **B2 判定用未截断文本**：query 词元只落在 snippet 尾部（第 160 字之后）时，结果
//!      **不得**被判 off_topic（截断必须在判定之后）。
//!
//! golden 出处：`tests/fixtures/golden_search_json.txt` / `golden_search_md.txt` 是 **bd
//! gsearch-rs-d8s 引入前**的 release 二进制（commit ae1d595）对同一假 SearXNG 载荷的实跑
//! stdout（归一后）——故本测试同时证明「本特性未改正常态输出的字节」。
//!
//! 归一只动两项（换行符 CRLF→LF、耗时字段），其余全部逐字节参与比对；规则与来源见
//! `tests/fixtures/cli_golden_provenance.md`。
//!
//! 载荷（假 SearXNG 的固定响应）见 `tests/fixtures/cli_golden_payloads.json`，按 query 路由：
//! `rust async` → normal（相关结果）、`kubernetes pdb yaml` → off_topic（整集无关）、
//! `zeta omega` → tail_token（词元只在 snippet 尾部）。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

const PAYLOADS: &str = "tests/fixtures/cli_golden_payloads.json";
const GOLDEN_JSON: &str = "tests/fixtures/golden_search_json.txt";
const GOLDEN_MD: &str = "tests/fixtures/golden_search_md.txt";
const GOLDEN_JSON_OFF_TOPIC: &str = "tests/fixtures/golden_search_json_off_topic.txt";

const Q_NORMAL: &str = "rust async";
const Q_OFF_TOPIC: &str = "kubernetes pdb yaml";
const Q_TAIL: &str = "zeta omega";

/// 正常态 meta 键集/键序（缺席语义：proxy/recency/site_warn/truncated/truncated_detail/
/// off_topic 全部缺席）。
const NORMAL_META_KEYS: &[&str] = &[
    "tool",
    "version",
    "query",
    "profile",
    "humanize",
    "limit",
    "elapsed_ms",
    "provider",
];

/// 本地回环假 SearXNG：按 query 路由到固定载荷，返回 base URL。
fn spawn_fake_searxng() -> String {
    let raw = std::fs::read_to_string(PAYLOADS).expect("读载荷 fixture");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("解析载荷 fixture");
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
            let key = if raw_q.contains("kubernetes") {
                "off_topic"
            } else if raw_q.contains("zeta") {
                "tail_token"
            } else {
                "normal"
            };
            let body = serde_json::to_string(&v[key]).expect("序列化载荷");
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

/// 跑真实二进制（`search` 子命令）→ (stdout, stderr)。环境清干净：只留假 SearXNG 的 URL。
fn run_cli(base: &str, query: &str, json: bool) -> (String, String) {
    run_cli_bin(env!("CARGO_BIN_EXE_gsearch"), base, query, json)
}

/// 临时配置文件的序号——**每次调用一个唯一文件名**。
/// libtest 默认并行跑用例，同名临时文件会互相删：先跑完的用例把配置删了，还在使用它的
/// 用例于是拿到「配置文件不存在」的 rc=2 空输出 → golden 比对随机红（实测复现）。
static CFG_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 同上，但二进制路径可指定（捕获工具要跑「改动前」的二进制）。
fn run_cli_bin(binary: &str, base: &str, query: &str, json: bool) -> (String, String) {
    // 显式配置（空文件）→ 屏蔽机器级 ~/.gsearch/config.json，保证 profile 等字段可复现。
    let seq = CFG_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let cfg_path = std::env::temp_dir().join(format!(
        "gsearch-cli-golden-{}-{}-{seq}.json",
        std::process::id(),
        query.replace([' ', ':'], "-")
    ));
    std::fs::write(&cfg_path, "{}").expect("写临时配置");
    let mut cmd = Command::new(binary);
    cmd.args(["--config", cfg_path.to_str().expect("配置路径"), "search", query]);
    if json {
        cmd.arg("--json");
    }
    cmd.env("GSEARCH_SEARXNG_URL", base);
    for k in ["GSEARCH_PROXY", "GSEARCH_PROFILE", "GSEARCH_LOG", "RUST_LOG"] {
        cmd.env_remove(k);
    }
    let out = cmd.output().expect("跑 gsearch 二进制");
    let _ = std::fs::remove_file(&cfg_path);
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// 归一：**只**动两项——① 换行符 CRLF→LF；② 耗时字段。
/// 其余（键序 / 键集 / 浮点字面量 / 字符串 / 空白）全部参与逐字节比对。
///
/// ① 换行符：期望文件是某一次捕获的字节，落盘后可能被 git 的 `core.autocrlf` 改写
/// （LF→CRLF），而 stdout 是程序直接写的字节（Rust 不做 CRLF 翻译）⇒ 不归一则
/// Windows 检出的工作区与 Linux/macOS CI 上必红。两侧各自归一即可。
/// ② 耗时：JSON 的 `"elapsed_ms":<数字>` → `"elapsed_ms":0`；md meta 头的
/// `elapsed: <数字>.<数字>s` → `elapsed: 0.0s`（同一个 `meta.elapsed_ms` 的两种渲染）。
fn normalize(s: &str) -> String {
    let s = s.replace("\r\n", "\n");

    // JSON：`"elapsed_ms":<数字>` → `"elapsed_ms":0`
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find("\"elapsed_ms\":") {
        let marker = "\"elapsed_ms\":";
        out.push_str(&rest[..i + marker.len()]);
        let tail = &rest[i + marker.len()..];
        let digits = tail.chars().take_while(|c| c.is_ascii_digit()).count();
        out.push('0');
        rest = &tail[digits..];
    }
    out.push_str(rest);

    // md meta 头：`elapsed: <数字>.<数字>s` → `elapsed: 0.0s`
    let mut md = String::new();
    let mut tail = out.as_str();
    while let Some(i) = tail.find("elapsed: ") {
        md.push_str(&tail[..i + "elapsed: ".len()]);
        let after = &tail[i + "elapsed: ".len()..];
        let end = after.find('s').map(|p| p + 1).unwrap_or(0);
        md.push_str("0.0s");
        tail = &after[end..];
    }
    md.push_str(tail);
    md
}

/// 从 JSON 文本取指定成员的直属键（按出现顺序）——serde_json::Value 的 Map 是 BTreeMap
/// （键序被排序），要断言「键序固定」必须扫原始文本。
fn json_object_keys(json: &str, needle: &str) -> Vec<String> {
    let at = json
        .find(needle)
        .unwrap_or_else(|| panic!("JSON 文本缺 {needle}"));
    let bytes = json.as_bytes();
    let mut i = at + needle.len();
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    assert_eq!(bytes[i], b'{', "{needle} 后应是对象起始");
    let mut depth = 0i32;
    let mut keys = Vec::new();
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b'"' if depth == 1 => {
                let key_end = json[i + 1..]
                    .find('"')
                    .map(|p| i + 1 + p)
                    .unwrap_or(bytes.len());
                let key = json[i + 1..key_end].to_string();
                let mut j = key_end + 1;
                while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b':' {
                    keys.push(key);
                }
                i = key_end;
            }
            _ => {}
        }
        i += 1;
    }
    keys
}

/// B3-1：正常态 `--json` 的真实 stdout 与本特性引入前**逐字节一致**。
#[test]
fn normal_state_json_stdout_is_byte_identical_to_pre_feature_golden() {
    let base = spawn_fake_searxng();
    let (stdout, stderr) = run_cli(&base, Q_NORMAL, true);
    assert!(stderr.is_empty(), "正常态 stderr 必须干净（告警噪音=不过）: {stderr}");
    let want = std::fs::read_to_string(GOLDEN_JSON).expect("读 golden");
    assert_eq!(
        normalize(&stdout),
        normalize(&want),
        "正常态 --json stdout 与引入本特性前不一致"
    );
}

/// B3-2：正常态 md（默认输出）的真实 stdout 与引入本特性前逐字节一致。
#[test]
fn normal_state_md_stdout_is_byte_identical_to_pre_feature_golden() {
    let base = spawn_fake_searxng();
    let (stdout, stderr) = run_cli(&base, Q_NORMAL, false);
    assert!(stderr.is_empty(), "正常态 stderr 必须干净（告警噪音=不过）: {stderr}");
    let want = std::fs::read_to_string(GOLDEN_MD).expect("读 golden");
    assert_eq!(
        normalize(&stdout),
        normalize(&want),
        "正常态 md stdout 与引入本特性前不一致"
    );
}

/// B3-3：正常态 meta/run/results 的键集与键序固定（往 meta 加键/改键序必红）。
#[test]
fn normal_state_meta_key_set_is_fixed() {
    let base = spawn_fake_searxng();
    let (stdout, stderr) = run_cli(&base, Q_NORMAL, true);
    assert!(stderr.is_empty(), "正常态 stderr 必须干净（告警噪音=不过）: {stderr}");
    assert_eq!(
        json_object_keys(&stdout, "\"meta\":"),
        NORMAL_META_KEYS,
        "正常态 meta 键集/键序必须固定"
    );
    assert_eq!(
        json_object_keys(&stdout, "\"run\":"),
        vec!["status"],
        "正常态 run 键集固定（空 message / captcha_solved=false 缺席）"
    );
    assert_eq!(
        json_object_keys(&stdout, "\"results\":["),
        vec!["title", "url", "snippet", "score", "domain_class"],
        "正常态结果键集固定（relevance 缺席）"
    );
}

/// B3-4：可疑态 `--json` 逐字节 golden —— 锁标签渲染 + 新状态 + meta 计数键（S3）。
#[test]
fn off_topic_state_json_stdout_matches_golden() {
    let base = spawn_fake_searxng();
    let (stdout, stderr) = run_cli(&base, Q_OFF_TOPIC, true);
    assert!(stderr.is_empty(), "可疑态 stderr 必须干净（告警噪音=不过）: {stderr}");
    let want = std::fs::read_to_string(GOLDEN_JSON_OFF_TOPIC).expect("读 golden");
    assert_eq!(
        normalize(&stdout),
        normalize(&want),
        "可疑态 --json stdout 与 golden 不一致"
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout 是合法 JSON");
    assert_eq!(v["run"]["status"], "off_topic_results", "新状态名: {stdout}");
    assert_eq!(v["meta"]["off_topic"], 3, "meta 带零交集计数（S3）: {stdout}");
    for r in v["results"].as_array().expect("results 数组") {
        assert_eq!(r["relevance"], "off_topic", "逐条标签: {stdout}");
    }
}

/// B2：判定基于**未截断**的 title+snippet —— 词元只落在 snippet 第 160 字之后时不得判 off_topic。
#[test]
fn judgment_uses_untruncated_snippet() {
    let base = spawn_fake_searxng();
    let (stdout, stderr) = run_cli(&base, Q_TAIL, true);
    assert!(stderr.is_empty(), "正常态 stderr 必须干净（告警噪音=不过）: {stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout 是合法 JSON");
    assert_eq!(
        v["run"]["status"], "ok",
        "词元在 snippet 尾部时不得判 off_topic（截断须在判定之后）: {stdout}"
    );
    for r in v["results"].as_array().expect("results 数组") {
        assert!(
            r.get("relevance").is_none(),
            "正常态结果不得带 relevance 标签: {stdout}"
        );
    }
}

/// 换行归一的**独立断言**：把期望文件的换行换成另一种形态后，比对**仍**应通过。
///
/// 证明两件事：① 归一生效（CRLF 与 LF 两种期望文件都等价）；② 正常态输出**没有别的
/// 隐含平台差异**（键序 / 键集 / 浮点字面量 / 字符串全等，否则任一形态都会红）。
///
/// 为什么需要：期望文件落盘后可能被 git 的 `core.autocrlf` 改写（LF→CRLF），
/// 而 stdout 是程序直接写的字节——不归一则 Windows 工作区与 Linux/macOS CI 上必红。
#[test]
fn newline_normalization_makes_crlf_and_lf_equivalent() {
    let base = spawn_fake_searxng();
    let (stdout, stderr) = run_cli(&base, Q_NORMAL, true);
    assert!(stderr.is_empty(), "正常态 stderr 必须干净（告警噪音=不过）: {stderr}");
    let golden = std::fs::read_to_string(GOLDEN_JSON).expect("读 golden");
    let lf = golden.replace("\r\n", "\n");
    let crlf = lf.replace('\n', "\r\n");
    assert_ne!(lf, crlf, "两种换行形态必须真的不同（否则本断言无判别力）");
    assert_eq!(
        normalize(&stdout),
        normalize(&lf),
        "LF 形态的期望文件应比对通过"
    );
    assert_eq!(
        normalize(&stdout),
        normalize(&crlf),
        "CRLF 形态的期望文件应比对通过（换行归一生效）"
    );
}

/// 一次性捕获工具（人工执行，常规 CI 不跑——`#[ignore]`）：
///
/// ```bash
/// GSEARCH_PRE_BINARY=<改动前的 release 二进制> cargo test --test cli_golden_bytes -- --ignored capture_goldens
/// ```
///
/// 用**改动前**的二进制产出正常态期望文件（`golden_search_json.txt` / `golden_search_md.txt`），
/// 并断言它与**当前**二进制的输出**逐字节相同**——这条断言就是「本特性未改正常态输出字节」的
/// 证据（可审计：基准 sha + 本命令 + 生成脚本三者齐备，见 `tests/fixtures/cli_golden_provenance.md`）。
/// 可疑态期望文件（`golden_search_json_off_topic.txt`）由当前二进制产出（改动前没有该状态）。
#[test]
#[ignore = "一次性捕获工具：需 GSEARCH_PRE_BINARY 指向改动前的二进制"]
fn capture_goldens() {
    let pre = std::env::var("GSEARCH_PRE_BINARY")
        .expect("需设 GSEARCH_PRE_BINARY=<改动前的 release 二进制>");
    let base = spawn_fake_searxng();
    for (query, as_json, out) in [
        (Q_NORMAL, true, GOLDEN_JSON),
        (Q_NORMAL, false, GOLDEN_MD),
    ] {
        let (pre_out, pre_err) = run_cli_bin(&pre, &base, query, as_json);
        assert!(pre_err.is_empty(), "基准二进制 stderr 必须干净: {pre_err}");
        let (cur_out, cur_err) = run_cli(&base, query, as_json);
        assert!(cur_err.is_empty(), "当前二进制 stderr 必须干净: {cur_err}");
        assert_eq!(
            normalize(&pre_out),
            normalize(&cur_out),
            "正常态输出与改动前不一致（逐字节）"
        );
        std::fs::write(out, normalize(&pre_out)).expect("写 golden");
        println!("已写 {out}");
    }
    let (cur_out, cur_err) = run_cli(&base, Q_OFF_TOPIC, true);
    assert!(cur_err.is_empty(), "当前二进制 stderr 必须干净: {cur_err}");
    std::fs::write(GOLDEN_JSON_OFF_TOPIC, normalize(&cur_out)).expect("写 golden");
    println!("已写 {GOLDEN_JSON_OFF_TOPIC}");
}
