//! bd gsearch-rs-b39：search.rs 回退链五态决策矩阵 + searxng_collect 翻页/部分结果/健康归因内核。
//!
//! 隔离手段：回环假 SearXNG（`std::net::TcpListener` 绑 127.0.0.1:0 随机端口）+ 进程级显式
//! 配置（`config::set_explicit_and_load` → 假实例 URL）。CONFIG 是进程级 OnceLock → 本文件
//! （独立测试二进制）内只初始化一次，全部用例共享同一假服务器；`GSEARCH_SEARXNG_URL` 先移除
//! 防机器级 env 劫持（env > file）。
//!
//! 零外网、零浏览器：被测请求全部打到回环假服务器。
//!
//! 五态矩阵（每态至少一用例）：
//!   ① JSON 命中          → provider="searxng"，且**不得**触发 HTML 降级（html 层零请求）
//!   ② JSON 故障/非 JSON  → 降级 HTML 端点命中，provider 仍是 "searxng"（降级不改来源标注）
//!   ③ JSON+HTML 双故障   → 故障分类 SourceError（带双层真因链）→ 回退决策携真因（非「源健康」归因）
//!   ④ 双层 200 空        → HealthyEmpty（源健康零结果）→ run.status=no_results/filtered_empty，
//!                          **不是** searxng_degraded（零结果 ≠ 基础设施故障）
//!   ⑤ 部分引擎/部分页失败 → 照常出已有结果（不误判整集失败）
//!
//! 内核三项（searxng_collect，经 `run_batch` 观测——batch_one 与单查共用同一内核）：
//!   翻页（跨页拼接 + 页上限，不越界多取）、部分结果（后续页失败不丢已得结果）、
//!   健康归因（故障真因链归到具体层；源健康零结果归因不含故障）。
//!
//! 已知边界（自动化盲区，见 reports/2026-11-17-fallback-matrix-tests.md）：
//!   「双层失败 → 走 Google 分支（provider="google"）」这一*分支选择*无法在本仓自动化——
//!   `try_searxng` 的 Err 分支直接调 `duckduckgo::collect`（公网硬编码 URL）与
//!   `google_fallback_precheck`（直连 www.google.com:443），二者都无注入边界，自动化只会
//!   打真外网且结果随网络环境漂移。本文件锁的是该分支的**输入与归因**（SourceError 双层
//!   真因链 → 回退决策必须携真因、不得归为源健康零结果），分支选择需先抽纯函数（见报告）。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;

use gsearch::config;
use gsearch::search::{
    circuit_diag, run_batch, try_searxng, Recency, SearchConfig, SearchOutcome, SearxFail,
    SearxngAttempt, FILTERED_EMPTY_MSG, NO_RESULTS_MSG,
};
use gsearch::types::{RunStatus, SearchResult};

/// 假服务器收到的请求（用于断言「走了哪一层 / 翻了几页 / 有没有多取」）。
#[derive(Clone, Debug)]
struct Req {
    q: String,
    /// true = JSON 层（请求带 format=json）；false = HTML 降级层。
    json: bool,
    pageno: u32,
}

static LOG: LazyLock<Arc<Mutex<Vec<Req>>>> = LazyLock::new(|| Arc::new(Mutex::new(Vec::new())));

/// 进程级一次性装配：本地假 SearXNG + 显式配置指过去 + 移除机器 env 覆盖。
static HARNESS: LazyLock<String> = LazyLock::new(|| {
    // env 优先于配置文件（env > file）——机器级 env 会把测试流量导向真实例
    unsafe { std::env::remove_var("GSEARCH_SEARXNG_URL") };
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回环端口");
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let log = LOG.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(sock) = stream else { continue };
            let log = log.clone();
            // 每连接一线程：单请求短命，不会互相阻塞
            std::thread::spawn(move || handle_conn(sock, log));
        }
    });
    let dir = std::env::temp_dir().join(format!("gsearch-fallback-matrix-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时配置目录");
    let cfg_path = dir.join("gsearch.json");
    std::fs::write(&cfg_path, format!(r#"{{"searxng_url": "{base}"}}"#)).expect("写临时配置");
    config::set_explicit_and_load(cfg_path.clone()).expect("加载显式配置");
    // 配置已进进程级 CONFIG（OnceLock），文件此后不再被读 → 立即清掉，测试不落残留
    let _ = std::fs::remove_file(&cfg_path);
    let _ = std::fs::remove_dir(&dir);
    base
});

fn harness() -> &'static str {
    HARNESS.as_str()
}

fn handle_conn(mut sock: std::net::TcpStream, log: Arc<Mutex<Vec<Req>>>) {
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
        return; // 非 SearXNG 请求直接断开（客户端报错，本文件的断言会先失败）
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
    log.lock().push(Req {
        q: q.clone(),
        json,
        pageno,
    });
    let (status, ctype, body) = route(&q, json, pageno);
    let reason = if status == 200 { "OK" } else { "Internal Server Error" };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes());
    let _ = sock.flush();
}

/// JSON 成功响应（字段与 SearXNG JSON API 一致）。
fn results_json(urls: &[&str]) -> String {
    let items: Vec<String> = urls
        .iter()
        .map(|u| format!(r#"{{"title":"t","url":"{u}","content":"c"}}"#))
        .collect();
    format!(r#"{{"results":[{}]}}"#, items.join(","))
}

/// JSON 成功响应 + 部分引擎挂掉（SearXNG 用 unresponsive_engines 回报失败引擎）。
fn results_json_dead_engine(urls: &[&str], dead: &str) -> String {
    let items: Vec<String> = urls
        .iter()
        .map(|u| format!(r#"{{"title":"t","url":"{u}","content":"c"}}"#))
        .collect();
    format!(
        r#"{{"results":[{}],"unresponsive_engines":[{{"name":"{dead}","error":"timeout"}}]}}"#,
        items.join(",")
    )
}

/// HTML 结果页（SearXNG 降级层形态：article.result 容器）。
fn results_html(urls: &[&str]) -> String {
    urls.iter()
        .map(|u| {
            format!(
                r#"<article class="result"><h3><a href="{u}">T</a></h3><p class="content">s</p></article>"#
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

/// 路由表：q 段一字一场景；catch-all = 500（双层故障）。
fn route(q: &str, json: bool, pageno: u32) -> (u16, &'static str, String) {
    match (q, json) {
        // ① JSON 命中：首页两条即凑满 limit（后续页不该被请求）
        ("s1json", true) if pageno == 1 => (
            200,
            "application/json",
            results_json(&["http://s1/1", "http://s1/2"]),
        ),
        ("s1json", _) => (500, "text/plain", "boom".into()),
        // ② JSON 层 HTTP 500 → 降级 HTML 层命中
        ("s2http", true) => (500, "text/plain", "boom".into()),
        ("s2http", false) => (200, "text/html", results_html(&["http://s2h/1"])),
        // ②b JSON 层 HTTP 200 但响应体非 JSON → 解析失败 → 降级 HTML 层命中
        ("s2notjson", true) => (200, "text/plain", "not json at all".into()),
        ("s2notjson", false) => (200, "text/html", results_html(&["http://s2n/1"])),
        // ③ 双层都 500 → SourceError 双层真因链
        ("s3both", true) => (500, "text/plain", "boom".into()),
        ("s3both", false) => (500, "text/plain", "boom".into()),
        // ④ 双层 200 空 → 源健康零结果（HealthyEmpty）
        ("s4empty", true) => (200, "application/json", r#"{"results":[]}"#.into()),
        ("s4empty", false) => (200, "text/html", "<html><body>nothing here</body></html>".into()),
        // ⑤ 部分引擎挂但其余引擎有结果 → 照常出结果
        ("s5engine", true) if pageno == 1 => (
            200,
            "application/json",
            results_json_dead_engine(&["http://s5/1", "http://s5/2"], "bing"),
        ),
        ("s5engine", _) => (500, "text/plain", "boom".into()),
        // 部分结果（内核）：首页 2 条命中、第 2 页双层故障 → 保留已得结果
        ("kpart", true) if pageno == 1 => (
            200,
            "application/json",
            results_json(&["http://kpart/1", "http://kpart/2"]),
        ),
        ("kpart", _) => (500, "text/plain", "boom".into()),
        // 翻页：每页两条新 URL（limit=5 → 第 3 页凑满后截断）
        ("kpages", true) => {
            let a = format!("http://kp/{pageno}/a");
            let b = format!("http://kp/{pageno}/b");
            (
                200,
                "application/json",
                results_json(&[a.as_str(), b.as_str()]),
            )
        }
        // 页上限：每页一条新 URL（limit 永远凑不满 → 打满页上限自然收口）
        ("kcap", true) => {
            let u = format!("http://kc/{pageno}");
            (200, "application/json", results_json(&[u.as_str()]))
        }
        _ => (500, "text/plain", "boom".into()),
    }
}

/// 某查询在 JSON 层被请求的页码序列（顺序 = 实际请求序）。
fn json_pagenos(q: &str) -> Vec<u32> {
    LOG.lock()
        .iter()
        .filter(|r| r.q == q && r.json)
        .map(|r| r.pageno)
        .collect()
}

/// 某查询在 HTML 降级层被请求的页码序列。
fn html_pagenos(q: &str) -> Vec<u32> {
    LOG.lock()
        .iter()
        .filter(|r| r.q == q && !r.json)
        .map(|r| r.pageno)
        .collect()
}

fn urls_of(results: &[SearchResult]) -> Vec<String> {
    results.iter().map(|r| r.url.clone()).collect()
}

/// 取 Results 态的结果集（provider / captcha 标记一并断言）。
fn expect_results(outcome: SearxngAttempt) -> (Vec<String>, &'static str, bool) {
    match outcome {
        SearxngAttempt::Results(SearchOutcome::Results {
            results,
            captcha_solved,
            provider,
        }) => (urls_of(&results), provider, captcha_solved),
        other => panic!("应得 SearXNG 结果态，得 {other:?}"),
    }
}

// ---------- 五态决策矩阵 ----------

/// ① JSON 端点成功 → provider="searxng"、captcha_solved=false，且**不触发** HTML 降级。
/// 防回归：provider 标错源 / captcha 标记误置 / 命中路径误入降级分支（多打一次 HTML 请求）。
#[tokio::test]
async fn state_01_json_hit_marks_provider_searxng_without_html_degrade() {
    let base = harness();
    assert!(base.starts_with("http://127.0.0.1"), "harness 必须是回环地址: {base}");
    let cfg = SearchConfig { query: "s1json".into(), limit: 2, recency: None };
    let (urls, provider, captcha_solved) = expect_results(try_searxng(&cfg).await);
    assert_eq!(provider, "searxng", "命中来源必须是 searxng");
    assert!(!captcha_solved, "SearXNG 路径不涉及人解验证码");
    assert_eq!(urls, vec!["http://s1/1", "http://s1/2"], "结果集须与 JSON 响应一致");
    assert_eq!(json_pagenos("s1json"), vec![1], "JSON 层只应请求首页");
    assert!(
        html_pagenos("s1json").is_empty(),
        "JSON 命中不得触发 HTML 降级请求: {:?}",
        html_pagenos("s1json")
    );
}

/// ② JSON 层 HTTP 故障 → 降级 HTML 端点命中 → provider 仍是 "searxng"（降级不改来源标注）。
/// 防回归：降级结果被标成 google/duckduckgo（agent 分流错）、或降级路径没被走到。
#[tokio::test]
async fn state_02_json_http_error_degrades_to_html_keeping_provider_searxng() {
    harness();
    let cfg = SearchConfig { query: "s2http".into(), limit: 1, recency: None };
    let (urls, provider, captcha_solved) = expect_results(try_searxng(&cfg).await);
    assert_eq!(provider, "searxng", "HTML 降级仍属 SearXNG 源，provider 不得漂移");
    assert!(!captcha_solved);
    assert_eq!(urls, vec!["http://s2h/1"], "须取到 HTML 层的结果");
    assert_eq!(json_pagenos("s2http"), vec![1], "JSON 层应被尝试过");
    assert_eq!(html_pagenos("s2http"), vec![1], "JSON 层故障后必须降级打 HTML 层");
}

/// ②b JSON 层 HTTP 200 但响应体不是 JSON → 解析失败 → 同样降级 HTML 层，provider 不变。
/// 防回归：非 JSON 响应被当成「零结果」（语义错）或直接报错不上降级。
#[tokio::test]
async fn state_02b_json_non_json_body_degrades_to_html_keeping_provider_searxng() {
    harness();
    let cfg = SearchConfig { query: "s2notjson".into(), limit: 1, recency: None };
    let (urls, provider, _) = expect_results(try_searxng(&cfg).await);
    assert_eq!(provider, "searxng");
    assert_eq!(urls, vec!["http://s2n/1"], "非 JSON 响应必须降级到 HTML 层取结果");
    assert_eq!(html_pagenos("s2notjson"), vec![1], "非 JSON 响应必须触发 HTML 降级");
}

/// ③ JSON+HTML 双层故障 → 归因为「故障」（SourceError）且真因链双层拼接、逐层归位；
/// 回退决策携带的真因（run.message/stderr 诊断行同源）不得丢层或吞掉状态。
/// 防回归：真因链缺层/串层；`fault` 无条件赋值（健康零结果被误标 degraded）。
#[tokio::test]
async fn state_03_both_layers_failed_is_fault_with_two_layer_cause_chain() {
    harness();
    let out = run_batch(&["s3both".to_string()], 10, None).await;
    let msg = match &out[0].1 {
        Err(SearxFail::SourceError(m)) => m.clone(),
        other => panic!("双层故障应得 SourceError，得 {other:?}"),
    };
    assert!(
        msg.contains("SearXNG 返回错误状态") && msg.contains("format=json"),
        "json 层真因须在链里: {msg}"
    );
    assert!(
        msg.contains("；HTML 层: SearXNG HTML 返回错误状态"),
        "html 层真因须以「；HTML 层: 」拼接: {msg}"
    );
    let html_part = msg
        .split_once("；HTML 层: ")
        .expect("链里必须有 HTML 段")
        .1;
    assert!(
        !html_part.contains("format=json"),
        "html 层请求不带 format=json——真因必须归到正确的层: {msg}"
    );
    // 故障归因不得被写成「源健康零结果」——那会让零结果/故障两态合并（熔断语义翻转）
    assert_ne!(
        SearxFail::HealthyEmpty.describe(),
        msg,
        "故障真因不得与源健康零结果同文"
    );
    // 回退分支的诊断行（run.message 同源）必须携真因
    let diag = circuit_diag(&msg);
    assert!(diag.contains("真因: "), "降级诊断行须带真因: {diag}");
    assert!(diag.contains("format=json"), "降级诊断行须带 SearXNG 真因: {diag}");
}

/// ④ 双层 200 空 → 源健康零结果（HealthyEmpty）：不是故障、不是降级；
/// run.status 按 recency 分 no_results / filtered_empty（两者都不等于 searxng_degraded）。
/// 防回归：健康空被归成 SourceError（误打熔断诊断行）/ 零结果被标 degraded（agent 误以为故障）。
#[tokio::test]
async fn state_04_zero_results_is_healthy_empty_not_fault_or_degraded() {
    harness();
    let out = run_batch(&["s4empty".to_string()], 10, None).await;
    match &out[0].1 {
        Err(SearxFail::HealthyEmpty) => {}
        other => panic!("双层 200 空应归 HealthyEmpty，得 {other:?}"),
    }
    assert_eq!(
        SearxFail::HealthyEmpty.describe(),
        "查询无结果（源健康）",
        "健康空归因文案"
    );
    assert_eq!(
        SearxFail::empty_status(None),
        (RunStatus::NoResults, NO_RESULTS_MSG),
        "无 recency 的健康空 → no_results"
    );
    assert_eq!(
        SearxFail::empty_status(Some(Recency::Day)),
        (RunStatus::FilteredEmpty, FILTERED_EMPTY_MSG),
        "带 recency 的健康空 → filtered_empty"
    );
    assert_ne!(
        SearxFail::empty_status(None).0,
        RunStatus::SearxngDegraded,
        "零结果不得被标成基础设施降级"
    );
}

/// ⑤ 部分引擎挂（unresponsive_engines）但其余引擎有结果 → 照常出结果，不误判整集失败。
/// 防回归：见到失败引擎就整集判失败（可用结果被丢光）。
#[tokio::test]
async fn state_05_partial_engine_failure_still_returns_available_results() {
    harness();
    let cfg = SearchConfig { query: "s5engine".into(), limit: 2, recency: None };
    let (urls, provider, _) = expect_results(try_searxng(&cfg).await);
    assert_eq!(provider, "searxng");
    assert_eq!(
        urls,
        vec!["http://s5/1", "http://s5/2"],
        "部分引擎挂不得丢光其余引擎的结果"
    );
}

// ---------- searxng_collect 内核：翻页 / 部分结果 / 健康归因 ----------

/// 翻页（多页拼接）：首页 2 条 + 第 2 页 2 条 + 第 3 页 2 条，limit=5 → 跨页按页序拼接后
/// 截断到 5 条，且第 3 页只取够数即停（不整页吐）。
/// 防回归：跨页顺序错乱（页内/页间混排）/ 截断丢失 / 凑满后继续翻页。
#[tokio::test]
async fn collect_concatenates_pages_in_order_and_truncates_to_limit() {
    harness();
    let out = run_batch(&["kpages".to_string()], 5, None).await;
    match &out[0].1 {
        Ok(SearchOutcome::Results { results, provider, .. }) => {
            assert_eq!(*provider, "searxng");
            assert_eq!(
                urls_of(results),
                vec![
                    "http://kp/1/a",
                    "http://kp/1/b",
                    "http://kp/2/a",
                    "http://kp/2/b",
                    "http://kp/3/a"
                ],
                "跨页须按页序拼接并截断到 limit"
            );
        }
        other => panic!("翻页应得结果集，得 {other:?}"),
    }
    assert_eq!(
        json_pagenos("kpages"),
        vec![1, 2, 3],
        "凑满 limit 后不得再翻页（页码序列即请求序）"
    );
}

/// 翻页（页上限）：每页 1 条、limit 永远凑不满 → 打满页上限自然收口：不死循环、不越界多取
/// （不得出现第 11 页请求），且上限内最后一页的结果必须吐出来。
/// 防回归：上限被改大（空转更多轮）/ 提前断（少取）/ 收口时丢掉末页结果。
#[tokio::test]
async fn collect_stops_at_page_cap_without_overfetching() {
    harness();
    let out = run_batch(&["kcap".to_string()], 100, None).await;
    match &out[0].1 {
        Ok(SearchOutcome::Results { results, .. }) => {
            let urls = urls_of(results);
            assert_eq!(urls.len(), 10, "10 页 × 1 条 = 打满页上限");
            assert_eq!(urls[0], "http://kc/1", "首页结果在列");
            assert_eq!(urls[9], "http://kc/10", "上限内最后一页的结果必须吐出");
        }
        other => panic!("应得结果集，得 {other:?}"),
    }
    let seen = json_pagenos("kcap");
    assert_eq!(seen, (1..=10).collect::<Vec<u32>>(), "页码须严格 1..=上限 递增");
    assert!(
        !seen.iter().any(|p| *p > 10),
        "不得越界多取（上限外页码）：{seen:?}"
    );
}

/// 部分结果（后续页失败）：首页 2 条命中、第 2 页双层故障 → 返回首页 2 条（不报错、不丢光）。
/// 防回归：后续页失败回归成 `return Err` → 已得结果丢光。
#[tokio::test]
async fn collect_keeps_partial_results_when_later_page_fails() {
    harness();
    let out = run_batch(&["kpart".to_string()], 10, None).await;
    match &out[0].1 {
        Ok(SearchOutcome::Results { results, provider, .. }) => {
            assert_eq!(*provider, "searxng");
            assert_eq!(
                urls_of(results),
                vec!["http://kpart/1", "http://kpart/2"],
                "后续页失败不得丢光已得结果"
            );
        }
        other => panic!("部分结果应得结果集，得 {other:?}"),
    }
}

/// 健康归因（失败引擎归到哪一层）：双层故障的 SourceError 真因链须逐层归位——
/// json 段带 `format=json`（那是 JSON 层请求），html 段不带（那是 HTML 层请求）；
/// 两层名不同 → 归因可分辨，改任一层都不会串到另一层去。
/// 防回归：真因链缺层/串层（运维按诊断行定位不到是哪层挂的）。
#[tokio::test]
async fn collect_attributes_failure_to_the_layer_that_failed() {
    harness();
    let out = run_batch(&["s3both".to_string()], 10, None).await;
    let msg = match &out[0].1 {
        Err(SearxFail::SourceError(m)) => m.clone(),
        other => panic!("双层故障应得 SourceError，得 {other:?}"),
    };
    let (json_part, html_part) = msg
        .split_once("；HTML 层: ")
        .unwrap_or_else(|| panic!("真因链缺 HTML 层: {msg}"));
    assert!(
        json_part.contains("SearXNG 返回错误状态") && json_part.contains("format=json"),
        "json 层真因（JSON 层请求）: {msg}"
    );
    assert!(
        html_part.starts_with("SearXNG HTML 返回错误状态") && !html_part.contains("format=json"),
        "html 层真因（HTML 层请求，不带 format=json）: {msg}"
    );
}
