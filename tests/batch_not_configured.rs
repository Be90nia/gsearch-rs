//! P0-1 NotConfigured 门 + P1-4 未配置快速路径回归。
//! 独立测试二进制（独立进程）：用空显式配置（`"{}"`）确定性制造「未配置」态——
//! 机器上任何 gsearch.json / env 都不参与，`run_batch`/`try_searxng` 在触网前返回。

use std::sync::LazyLock;

use gsearch::config;
use gsearch::search::{run_batch, try_searxng, SearchConfig, SearxFail, SearxngAttempt};

/// 进程级一次性装配：移除机器 env 覆盖（env > file）→ 空显式配置 → CONFIG = 默认。
static EMPTY_CFG: LazyLock<()> = LazyLock::new(|| {
    unsafe { std::env::remove_var("GSEARCH_SEARXNG_URL") };
    let p = std::env::temp_dir().join(format!("gsearch-b39-empty-{}.json", std::process::id()));
    std::fs::write(&p, "{}").expect("写临时空配置");
    config::set_explicit_and_load(p).expect("显式空配置加载应成功");
});

fn init_empty_config() {
    LazyLock::force(&EMPTY_CFG);
}

const UNCONFIGURED_MSG: &str =
    "SearXNG 未配置（gsearch.json 缺 searxng_url）；batch 模式禁浏览器回退，请改用单查询";

/// P1-4：未配置时 batch 不走网络、不 panic，逐条得 SourceError 未配置文案，
/// 且输出与输入逐位对齐（批 JSON 数组顺序契约的最基础形态）。
#[tokio::test]
async fn batch_unconfigured_yields_source_error_per_entry_in_input_order() {
    init_empty_config();
    let queries = vec!["q1".to_string(), "q2".to_string(), "q3".to_string()];
    let out = run_batch(&queries, 10, None).await;
    assert_eq!(out.len(), 3);
    for (i, (q, r)) in out.iter().enumerate() {
        assert_eq!(q, &queries[i], "输出顺序必须与输入一致");
        match r {
            Err(SearxFail::SourceError(s)) => assert_eq!(s, UNCONFIGURED_MSG, "第 {i} 条文案漂移"),
            other => panic!("未配置应逐条 SourceError，第 {i} 条得 {other:?}"),
        }
    }
}

/// P0-1：NotConfigured 门——未配置直接返回该态，不预检、不回退、不起浏览器。
#[tokio::test]
async fn try_searxng_unconfigured_returns_not_configured() {
    init_empty_config();
    let cfg = SearchConfig { query: "x".into(), limit: 10, recency: None };
    assert!(
        matches!(try_searxng(&cfg).await, SearxngAttempt::NotConfigured),
        "未配置必须走 NotConfigured 门"
    );
}
