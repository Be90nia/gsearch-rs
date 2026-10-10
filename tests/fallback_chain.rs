//! P0-1/P0-2/P1-3/P1-4 回归（audit-tests-20261010）：本地回环假 SearXNG 驱动真实
//! `try_searxng` / `run_batch` 决策与翻页内核——零外网、零浏览器、确定性隔离。
//!
//! 隔离手段：`config::set_explicit_and_load(临时 gsearch.json → 本假服务器)`。
//! CONFIG 是进程级 OnceLock → 本文件（独立测试二进制）内只初始化一次，全部测试
//! 共享同一假服务器；`GSEARCH_SEARXNG_URL` 先移除防机器级 env 劫持（env > file）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use std::time::Duration;

use gsearch::config;
use gsearch::search::{run_batch, try_searxng, SearchConfig, SearchOutcome, SearxFail, SearxngAttempt};

/// 请求统计：(q, 是否 json 层) → 已收到的 pageno 序列（翻页次数/页码归因断言用）。
#[derive(Default)]
struct Stats {
    pagenos: Mutex<HashMap<(String, bool), Vec<u32>>>,
}

impl Stats {
    fn record(&self, q: &str, json: bool, pageno: u32) {
        self.pagenos
            .lock()
            .entry((q.to_string(), json))
            .or_default()
            .push(pageno);
    }
    fn pagenos_of(&self, q: &str, json: bool) -> Vec<u32> {
        self.pagenos
            .lock()
            .get(&(q.to_string(), json))
            .cloned()
            .unwrap_or_default()
    }
}

static STATS: LazyLock<Arc<Stats>> = LazyLock::new(|| Arc::new(Stats::default()));
static HARNESS: LazyLock<String> = LazyLock::new(|| {
    // 机器级 env 会盖过配置文件（env > file）；本假服务器是测试唯一合法出口
    unsafe { std::env::remove_var("GSEARCH_SEARXNG_URL") };
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回环端口");
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let stats = stats();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(sock) = stream else { continue };
            let stats = stats.clone();
            // 每连接一线程：慢查询的延迟只延迟自己，不阻塞同批其他请求
            std::thread::spawn(move || handle_conn(sock, stats));
        }
    });
    let dir = std::env::temp_dir().join(format!("gsearch-b39-harness-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时配置目录");
    let cfg_path = dir.join("gsearch.json");
    std::fs::write(&cfg_path, format!(r#"{{"searxng_url": "{base}"}}"#)).expect("写临时配置");
    config::set_explicit_and_load(cfg_path).expect("加载显式配置");
    base
});

fn stats() -> Arc<Stats> {
    STATS.clone()
}

/// 进程级一次性装配：本地假 SearXNG + 显式配置指过去 + 移除机器 env 覆盖。
/// 返回 base URL（CONFIG OnceLock 只允许初始化一次 → 全测试共享同一 server）。
fn harness() -> &'static str {
    HARNESS.as_str()
}

fn handle_conn(mut sock: std::net::TcpStream, stats: Arc<Stats>) {
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
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    if path != "/search" {
        return; // 非 SearXNG 请求直接断开（客户端报错，测试自身的断言会先失败）
    }
    let mut q = String::new();
    let mut json = false;
    let mut pageno = 1u32;
    for kv in query.split('&') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        match k {
            "q" => q = v.to_string(),
            "format" => json = v == "json",
            "pageno" => pageno = v.parse().unwrap_or(1),
            _ => {}
        }
    }
    stats.record(&q, json, pageno);
    let (status, ctype, body) = route(&q, json, pageno);
    let reason = if status == 200 { "OK" } else { "Internal Server Error" };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes());
    let _ = sock.flush();
}

fn results_json(urls: &[String]) -> String {
    let items: Vec<String> = urls
        .iter()
        .map(|u| format!(r#"{{"title":"t","url":"{u}","content":"c"}}"#))
        .collect();
    format!(r#"{{"results":[{}]}}"#, items.join(","))
}

fn results_html(urls: &[String]) -> String {
    urls.iter()
        .map(|u| {
            format!(
                r#"<article class="result"><h3><a href="{u}">T</a></h3><p class="content">s</p></article>"#
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

/// 路由表：catch-all = 500（双层故障）。q 按测试场景一字一段。
fn route(q: &str, json: bool, pageno: u32) -> (u16, &'static str, String) {
    let ok_json = |urls: &[&str]| -> (u16, &'static str, String) {
        let owned: Vec<String> = urls.iter().map(|s| s.to_string()).collect();
        (200, "application/json", results_json(&owned))
    };
    let ok_html = |body: String| -> (u16, &'static str, String) { (200, "text/html", body) };
    let err500 = || -> (u16, &'static str, String) { (500, "text/plain", "boom".into()) };
    match (q, json) {
        ("provider", true) if pageno == 1 => ok_json(&["http://provider/1"]),
        ("slow", true) => {
            // 慢查询：给同批快查询留出确定性的完成时差（run_batch 保序回归的输入条件）
            std::thread::sleep(Duration::from_millis(250));
            ok_json(&["http://slow/1"])
        }
        ("fast1", true) => ok_json(&["http://fast1/1"]),
        ("fast2", true) => ok_json(&["http://fast2/1"]),
        // 页1 json 三条命中、页2 起双层故障 → 部分结果必须保留（有多少用多少）
        ("partial", true) if pageno == 1 => ok_json(&["http://p/1", "http://p/2", "http://p/3"]),
        // 页1 json 命中、页2 json 炸但 html 降级命中 → 降级结果按文档序并入
        ("partialhtml", true) if pageno == 1 => ok_json(&["http://u/1", "http://u/2"]),
        ("partialhtml", false) if pageno == 2 => ok_html(results_html(&["http://u/4".to_string()])),
        // json 层 200 空结果 + html 层 200 空 → 源健康 HealthyEmpty（degrade 分类以最终层为准）
        ("jsonempty", true) => (200, "application/json", r#"{"results":[]}"#.into()),
        ("jsonempty", false) => (200, "text/html", "<html><body>nothing here</body></html>".into()),
        // 各页都是同一 URL：去重耗尽后循环自然结束 → HealthyEmpty（源给过结果，非故障）
        ("dupall", true) => ok_json(&["http://dup/one"]),
        // 超采样远超 limit：第 1 页就该 break（凑满即停）+ truncate 收口
        ("trunc", true) => ok_json(&["http://t/1", "http://t/2", "http://t/3", "http://t/4", "http://t/5"]),
        // 每页两条新 URL 且 limit 永远凑不满：翻页必须打满 1..=MAX_PAGES 自然收口
        ("pages", true) => {
            let a = format!("http://pg/{pageno}/a");
            let b = format!("http://pg/{pageno}/b");
            ok_json(&[a.as_str(), b.as_str()])
        }
        // 页2 重复页1 首条 + 新条；页3 双层炸 → 去重保首、部分结果收口
        ("dupx", true) if pageno == 1 => ok_json(&["http://d/1", "http://d/2"]),
        ("dupx", true) if pageno == 2 => ok_json(&["http://d/1", "http://d/3"]),
        _ => err500(),
    }
}

fn ok_urls(out: &[(String, Result<SearchOutcome, SearxFail>)], idx: usize) -> Vec<String> {
    match &out[idx].1 {
        Ok(SearchOutcome::Results { results, .. }) => results.iter().map(|r| r.url.clone()).collect(),
        other => panic!("第 {idx} 条应得 Results，得 {other:?}"),
    }
}

// ---------- P0-1：回退链可达态（NotConfigured 门在 batch_not_configured 二进制锁） ----------

/// P0-1（可达态）：SearXNG 命中 → Results 且 provider="searxng"、captcha_solved=false。
/// 防回归：provider 标错源 / captcha 标记误置 / 命中路径误入回退分支。
#[tokio::test]
async fn try_searxng_hit_routes_provider_searxng() {
    let base = harness();
    assert!(base.starts_with("http://127.0.0.1"), "harness 必须是回环地址: {base}");
    let cfg = SearchConfig { query: "provider".into(), limit: 10, recency: None };
    match try_searxng(&cfg).await {
        SearxngAttempt::Results(SearchOutcome::Results { results, captcha_solved, provider }) => {
            assert_eq!(provider, "searxng");
            assert!(!captcha_solved);
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].url, "http://provider/1");
        }
        other => panic!("SearXNG 命中应得 Results，得 {other:?}"),
    }
}

// ---------- P1-4：run_batch slots 保序 ----------

/// P1-4：慢查询（250ms 延迟）+ 两条快查询并发 → buffer_unordered 完成序必为快-快-慢，
/// 输出仍必须与输入逐位对齐（slots 重排契约）。重排被删（直接收集流序）时本测试必红。
#[tokio::test]
async fn run_batch_preserves_input_order_under_out_of_order_completion() {
    harness();
    let queries = vec!["slow".to_string(), "fast1".to_string(), "fast2".to_string()];
    let out = run_batch(&queries, 10, None).await;
    assert_eq!(out.len(), 3);
    for (i, (q, r)) in out.iter().enumerate() {
        assert_eq!(q, &queries[i], "第 {i} 位必须对齐输入（slots 保序契约）");
        match r {
            Ok(SearchOutcome::Results { results, provider, .. }) => {
                assert_eq!(*provider, "searxng");
                assert_eq!(results[0].url, format!("http://{}/1", queries[i]), "结果须与自己的查询配对");
            }
            other => panic!("第 {i} 条应成功，得 {other:?}"),
        }
    }
}

// ---------- P0-2：searxng_collect 翻页/部分结果/健康归因内核 ----------

/// P0-2③：页1 三条命中、页2 起双层故障 → 已得部分结果必须保留。
/// 回归成 `return Err(reason)` 时页1 数据丢光（单查与 batch 共用内核 = 双丢数据）。
#[tokio::test]
async fn collect_keeps_partial_results_when_later_pages_fail() {
    harness();
    let out = run_batch(&["partial".to_string()], 10, None).await;
    assert_eq!(
        ok_urls(&out, 0),
        vec!["http://p/1", "http://p/2", "http://p/3"],
        "后续页故障不得丢光首页已得结果"
    );
}

/// P1-3：json 层故障 → HTML 降级层命中 → 降级结果并入并保持文档序。
#[tokio::test]
async fn collect_degrades_to_html_and_merges_results_in_order() {
    harness();
    let out = run_batch(&["partialhtml".to_string()], 10, None).await;
    assert_eq!(ok_urls(&out, 0), vec!["http://u/1", "http://u/2", "http://u/4"]);
}

/// P1-3：json+html 双层故障 → SourceError 真因链必须双层拼接
/// 「json 真因；HTML 层: html 真因」。缺层/顺序错/吞状态码全部必红。
#[tokio::test]
async fn collect_reports_full_reason_chain_when_both_layers_fail() {
    harness();
    let out = run_batch(&["chain".to_string()], 10, None).await;
    match &out[0].1 {
        Err(SearxFail::SourceError(msg)) => {
            assert!(msg.starts_with("SearXNG 返回错误状态: "), "json 层真因在前: {msg}");
            assert!(msg.contains("format=json&pageno=1"), "json 层 URL（含页码）应在链中: {msg}");
            assert!(
                msg.contains("；HTML 层: SearXNG HTML 返回错误状态: "),
                "html 层真因链格式「；HTML 层: 」: {msg}"
            );
        }
        other => panic!("双层故障应得 SourceError 全链，得 {other:?}"),
    }
}

/// P0-2：后续页与首页全重复（无新结果）→ 不得丢首页结果、不得报错，循环打满
/// MAX_PAGES 自然收口（truncate 后吐首页那条）。 HealthyEmpty 归因的真可达路径是
/// 「双层 200 空」（见 collect_json_and_html_both_empty_is_healthy_empty）。
#[tokio::test]
async fn collect_dup_later_pages_keep_first_seen_results() {
    harness();
    let q = ["dupall".to_string()];
    let out = run_batch(&q, 10, None);
    match &out.await[0].1 {
        Ok(SearchOutcome::Results { results, provider, .. }) => {
            assert_eq!(*provider, "searxng");
            let urls: Vec<&str> = results.iter().map(|r| r.url.as_str()).collect();
            assert_eq!(urls, vec!["http://dup/one"], "重复页不得复制/丢失首页结果");
        }
        other => panic!("全重复后续页应保留首页结果 Ok，得 {other:?}"),
    }
    let seen = stats().pagenos_of("dupall", true);
    assert_eq!(seen.len(), 10, "无新结果的重复页应翻满 MAX_PAGES=10 再收口: {seen:?}");
    assert!(
        stats().pagenos_of("dupall", false).is_empty(),
        "json 层每页都非空 → 不得触发 html 降级"
    );
}

/// P1-3/P0-2：json 层 HTTP 200 空 + html 层 HTTP 200 空 → HealthyEmpty。
/// degrade_html 的 `Ok(_) => HealthyEmpty` 若回归成 SourceError，健康实例的空查询会误打熔断诊断行。
#[tokio::test]
async fn collect_json_and_html_both_empty_is_healthy_empty() {
    harness();
    let out = run_batch(&["jsonempty".to_string()], 10, None).await;
    match &out[0].1 {
        Err(SearxFail::HealthyEmpty) => {}
        other => panic!("双层 200 空应归 HealthyEmpty，得 {other:?}"),
    }
}

/// P0-2：页1 五条、limit=3 → 首页即 break + truncate(limit)。锁两个分支：
/// 凑满即停（不再翻页）与 truncate 收口（多拿的不吐）。
#[tokio::test]
async fn collect_stops_first_page_and_truncates_to_limit() {
    harness();
    let out = run_batch(&["trunc".to_string()], 3, None).await;
    assert_eq!(ok_urls(&out, 0), vec!["http://t/1", "http://t/2", "http://t/3"]);
    assert_eq!(stats().pagenos_of("trunc", true), vec![1], "凑满 limit 后不得再翻页");
}

/// P0-2：结果持续供给且 limit 永远凑不满 → 翻页打满 1..=MAX_PAGES 自然收口：
/// 不死循环、不提前断（10 页 × 每页 2 条 = 20 条，页码严格递增）。
#[tokio::test]
async fn collect_caps_pagination_at_max_pages() {
    harness();
    let out = run_batch(&["pages".to_string()], 1000, None).await;
    let urls = ok_urls(&out, 0);
    assert_eq!(urls.len(), 20, "10 页 × 每页 2 条");
    assert_eq!(urls[0], "http://pg/1/a");
    assert_eq!(urls[19], "http://pg/10/b");
    assert_eq!(
        stats().pagenos_of("pages", true),
        (1..=10).collect::<Vec<u32>>(),
        "pageno 必须严格 1..=MAX_PAGES 递增"
    );
}

/// P0-2：跨页去重保首次（页2 的 d/1 是页1 重复，丢弃）；页3 双层故障 → 部分结果收口。
#[tokio::test]
async fn collect_dedups_across_pages_keeping_first_seen() {
    harness();
    let out = run_batch(&["dupx".to_string()], 10, None).await;
    assert_eq!(ok_urls(&out, 0), vec!["http://d/1", "http://d/2", "http://d/3"]);
}
