//! bd gsearch-rs-d8s 向后兼容硬约束（**信封序列化层**）：未触发可疑判定时 `--json` 输出的
//! 字段集、键序与字节形态固定。
//!
//! 本文件的 golden = **手写常量**（占位 URL `example.com` / `example.org`，非实跑捕获），
//! 锁的是 `OutputEnvelope` 的 serde 序列化结果——任何字段增删、键序漂移、格式变化都会红。
//! **真实 CLI stdout 的逐字节锁在 `tests/cli_golden_bytes.rs`**（跑真二进制 + 本地回环假
//! SearXNG，golden 取自本特性引入前的 commit `ae1d595` release 二进制实跑输出）——本文件
//! 不替代它，两者覆盖面不同（类型层 vs 装配+输出层）。
//!
//! 缺席语义（8lp）：`relevance` 只在整集判可疑时出现；正常态该键整体缺席 → golden 不变。

use gsearch::types::{MetaOutput, OutputEnvelope, RunStatus, RunStatusInfo, SearchResult};

/// 改动前的 `--json` 输出（逐字节 golden）。
const GOLDEN_JSON: &str = concat!(
    r#"{"meta":{"tool":"gsearch","version":"0.4.0","query":"rust async","profile":"default","humanize":false,"limit":10,"elapsed_ms":4,"provider":"searxng"},"run":{"status":"ok"},"results":"#,
    r#"[{"title":"Async in Rust - The Rust Programming Language","url":"https://example.com/rust-async","snippet":"Rust async await basics, futures and the Future trait.","score":3.0,"domain_class":"other"},"#,
    r#"{"title":"Tokio Tutorial - Async Rust","url":"https://example.org/tokio-async","snippet":"Building async services with tokio in Rust.","score":2.0,"domain_class":"other"}"#,
    r#"]}"#,
);

fn golden_envelope(results: &[SearchResult]) -> OutputEnvelope<&[SearchResult]> {
    let meta = MetaOutput {
        tool: "gsearch",
        version: "0.4.0",
        query: "rust async".into(),
        profile: "default".into(),
        proxy: None,
        humanize: false,
        limit: 10,
        elapsed_ms: 4,
        truncated: false,
        truncated_at_offset: 0,
        provider: "searxng".into(),
        recency: None,
        site_warn: None,
        truncated_detail: None,
        off_topic: None,
    };
    let run = RunStatusInfo {
        status: RunStatus::Ok,
        captcha_solved: false,
        message: String::new(),
    };
    OutputEnvelope { meta, run, results }
}

fn sample_results() -> Vec<SearchResult> {
    vec![
        SearchResult {
            title: "Async in Rust - The Rust Programming Language".into(),
            url: "https://example.com/rust-async".into(),
            snippet: "Rust async await basics, futures and the Future trait.".into(),
            score: Some(3.0),
            domain_class: "other",
            relevance: None,
        },
        SearchResult {
            title: "Tokio Tutorial - Async Rust".into(),
            url: "https://example.org/tokio-async".into(),
            snippet: "Building async services with tokio in Rust.".into(),
            score: Some(2.0),
            domain_class: "other",
            relevance: None,
        },
    ]
}

/// A4 code-level：正常态 `--json` 输出与改动前逐字节一致（golden 断言）。
#[test]
fn json_output_is_byte_identical_to_pre_change_golden() {
    let results = sample_results();
    let env = golden_envelope(&results);
    let json = serde_json::to_string(&env).expect("信封序列化");
    assert_eq!(
        json, GOLDEN_JSON,
        "--json 输出与改动前不一致（正常态必须逐字节相同）"
    );
    assert!(!json.contains("relevance"), "正常态不得出现 relevance 键: {json}");
}

/// 缺席语义：relevance = None 时该键整体缺席（不是 null）。
#[test]
fn relevance_key_absent_when_not_judged() {
    let results = sample_results();
    let json = serde_json::to_string(&golden_envelope(&results)).expect("序列化");
    assert!(!json.contains("\"relevance\""), "键应缺席: {json}");
}
