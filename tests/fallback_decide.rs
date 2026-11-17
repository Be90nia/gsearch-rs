//! bd gsearch-rs-aws：回退链决策层纯函数 `decide_after_fail` 的五态决策表 +
//! bd gsearch-rs-1uc：代理感知的纯函数（代理 URL 归一 / 端点解析 / 空值归一）。
//!
//! 零外网、零浏览器、零 env 依赖：被测对象全是纯函数（网络 I/O 留在 `try_searxng` 调用方，
//! 由调用方把「DDG 命中/空/失败」与「Google 预检通/不通」测好后喂进决策层）。
//!
//! 决策表（每态至少一断言）：
//!   ① DDG 命中                 → provider="duckduckgo"（不熔断、不回退 Google）
//!   ② DDG 空 + 预检通 + 源健康 → FallbackGoogle(None)（源健康零结果，非降级）
//!   ③ DDG 空 + 预检通 + 源故障 → FallbackGoogle(Some(真因))（真因必须透传）
//!   ④ DDG 空 + 预检不通 + 源健康 → HealthyEmpty（不熔断）
//!   ⑤ DDG 空 + 预检不通 + 源故障 → CircuitBroken（真因 + 预检归因）
//!
//! 归因可区分（1uc）：代理不可达必须表达为「代理不可达」，不得混成「目标站超时/不可达」。

use gsearch::search::{
    circuit_diag, curl_proxy_url, decide_after_fail, proxy_endpoint, proxy_from_env_value,
    DdgOutcome, FallbackNet, Precheck, SearxFail, SearchOutcome, SearxngAttempt,
};
use gsearch::types::SearchResult;

fn hit_result(url: &str) -> SearchResult {
    SearchResult {
        title: "t".into(),
        url: url.into(),
        snippet: "s".into(),
        score: None,
        domain_class: "",
        relevance: None,
    }
}

/// 取 Results 态的 (urls, provider, captcha_solved)。
fn expect_results(outcome: SearxngAttempt) -> (Vec<String>, &'static str, bool) {
    match outcome {
        SearxngAttempt::Results(SearchOutcome::Results {
            results,
            captcha_solved,
            provider,
        }) => (
            results.iter().map(|r| r.url.clone()).collect(),
            provider,
            captcha_solved,
        ),
        other => panic!("应得结果态，得 {other:?}"),
    }
}

/// ① DDG 命中 → 直接返回 provider="duckduckgo"（预检状态无关：插层命中就不该再熔断）。
#[test]
fn state_1_ddg_hit_returns_duckduckgo_provider() {
    let (urls, provider, captcha) = expect_results(decide_after_fail(
        SearxFail::SourceError("searxng 挂了".into()),
        DdgOutcome::Hit(vec![hit_result("http://ddg/1"), hit_result("http://ddg/2")]),
        Precheck::Unreachable("代理不可达".into()),
    ));
    assert_eq!(provider, "duckduckgo", "DDG 插层命中的来源标注必须是 duckduckgo");
    assert!(!captcha, "DDG 路径不涉及人解验证码");
    assert_eq!(urls, vec!["http://ddg/1", "http://ddg/2"], "结果集须原样透传");
}

/// ② DDG 空 + 预检通 + SearXNG 源健康零结果 → FallbackGoogle(None)：
/// 「源健康零结果」不得被归成故障（否则回退也空时会被误标 degraded）。
#[test]
fn state_2_ddg_empty_precheck_ok_healthy_source_is_fallback_without_fault() {
    match decide_after_fail(
        SearxFail::HealthyEmpty,
        DdgOutcome::Empty,
        Precheck::Ok,
    ) {
        SearxngAttempt::FallbackGoogle(None) => {}
        other => panic!("源健康零结果 + 预检通 → FallbackGoogle(None)，得 {other:?}"),
    }
}

/// ③ DDG 空 + 预检通 + SearXNG 故障 → FallbackGoogle(Some(真因))：真因必须透传
/// （下游 run.message / stderr 诊断行同源用它）。
#[test]
fn state_3_ddg_empty_precheck_ok_fault_carries_cause() {
    match decide_after_fail(
        SearxFail::SourceError("SearXNG 返回错误状态 500".into()),
        DdgOutcome::Empty,
        Precheck::Ok,
    ) {
        SearxngAttempt::FallbackGoogle(Some(reason)) => {
            assert!(
                reason.contains("500"),
                "回退决策须携 SearXNG 真因: {reason}"
            );
        }
        other => panic!("故障 + 预检通 → FallbackGoogle(Some(真因))，得 {other:?}"),
    }
}

/// ④ DDG 空 + 预检不通 + 源健康 → HealthyEmpty：不是熔断（源健康 ≠ 基础设施故障）。
#[test]
fn state_4_ddg_empty_precheck_down_healthy_source_is_healthy_empty_not_circuit() {
    match decide_after_fail(
        SearxFail::HealthyEmpty,
        DdgOutcome::Empty,
        Precheck::Unreachable("直连 www.google.com:443 超时（1.5s 预算）".into()),
    ) {
        SearxngAttempt::HealthyEmpty => {}
        other => panic!("源健康 + 预检不通 → HealthyEmpty，得 {other:?}"),
    }
}

/// ⑤ DDG 空 + 预检不通 + SearXNG 故障 → CircuitBroken：真因链含 SearXNG 真因与预检归因，
/// 且诊断行（circuit_diag）把它透出来。
#[test]
fn state_5_ddg_empty_precheck_down_fault_is_circuit_broken_with_cause_chain() {
    let decision = decide_after_fail(
        SearxFail::SourceError("SearXNG 返回错误状态 500".into()),
        DdgOutcome::Empty,
        Precheck::Unreachable("代理不可达（socks5://127.0.0.1:1）".into()),
    );
    let reason = match decision {
        SearxngAttempt::CircuitBroken(r) => r,
        other => panic!("故障 + 预检不通 → CircuitBroken，得 {other:?}"),
    };
    assert!(reason.contains("500"), "熔断真因须含 SearXNG 真因: {reason}");
    assert!(
        reason.contains("代理不可达"),
        "熔断真因须含预检归因（代理不可达 ≠ 目标站不可达）: {reason}"
    );
    let diag = circuit_diag(&reason);
    assert!(diag.contains("真因: "), "诊断行须带真因: {diag}");
    assert!(diag.contains("代理不可达"), "诊断行须能区分代理不可达: {diag}");
}

/// ⑥ DDG 失败（不是零结果）与 DDG 零结果同语义：都进 Google 回退链；
/// 且 DDG 失败的归因不得被吞（进熔断真因链）。
#[test]
fn state_6_ddg_failed_behaves_like_empty_but_keeps_cause() {
    match decide_after_fail(
        SearxFail::HealthyEmpty,
        DdgOutcome::Failed("代理不可达：TCP 连接 127.0.0.1:1 失败".into()),
        Precheck::Ok,
    ) {
        SearxngAttempt::FallbackGoogle(None) => {}
        other => panic!("DDG 失败 + 预检通 + 源健康 → FallbackGoogle(None)，得 {other:?}"),
    }
    match decide_after_fail(
        SearxFail::SourceError("searxng 挂了".into()),
        DdgOutcome::Failed("代理不可达：TCP 连接 127.0.0.1:1 失败".into()),
        Precheck::Unreachable("代理不可达（socks5://127.0.0.1:1）".into()),
    ) {
        SearxngAttempt::CircuitBroken(reason) => {
            assert!(
                reason.contains("DDG 插层失败"),
                "DDG 失败归因不得被吞: {reason}"
            );
            assert!(reason.contains("代理不可达"), "代理不可达须在真因链里: {reason}");
            assert!(
                !reason.contains("目标站不可达"),
                "代理不可达不得混成目标站不可达: {reason}"
            );
        }
        other => panic!("DDG 失败 + 预检不通 + 故障 → CircuitBroken，得 {other:?}"),
    }
}

/// 代理 URL 归一（curl 侧）：socks5:// → socks5h://（域名交代理侧解析，与 Chrome SOCKS5 一致）；
/// http:// 原样（不误改）。
#[test]
fn curl_proxy_url_normalizes_socks5_only() {
    assert_eq!(
        curl_proxy_url("socks5://127.0.0.1:10808"),
        "socks5h://127.0.0.1:10808"
    );
    assert_eq!(
        curl_proxy_url("http://127.0.0.1:7890"),
        "http://127.0.0.1:7890"
    );
    assert_eq!(
        curl_proxy_url("socks5h://127.0.0.1:10808"),
        "socks5h://127.0.0.1:10808"
    );
}

/// 代理端点解析（TCP 探针用）：剥 userinfo、IPv6 认方括号、无端口补 curl 默认 1080、
/// 非法端口 → None（归因「代理地址无法解析」）。
#[test]
fn proxy_endpoint_parses_host_and_port() {
    assert_eq!(
        proxy_endpoint("socks5://127.0.0.1:10808"),
        Some(("127.0.0.1".to_string(), 10808))
    );
    assert_eq!(
        proxy_endpoint("http://user:pass@proxy.example:7890"),
        Some(("proxy.example".to_string(), 7890))
    );
    assert_eq!(
        proxy_endpoint("socks5://[::1]:1080"),
        Some(("::1".to_string(), 1080))
    );
    assert_eq!(
        proxy_endpoint("http://proxy.example"),
        Some(("proxy.example".to_string(), 1080)),
        "无端口按 curl 默认代理端口 1080"
    );
    assert_eq!(proxy_endpoint("http://proxy.example:notaport"), None);
}

/// 代理值归一：设了但空白 = 未设（对齐 config.rs 的 env 合并语义）。
#[test]
fn proxy_from_env_value_treats_blank_as_unset() {
    assert_eq!(proxy_from_env_value(Some("  ".into())), None);
    assert_eq!(proxy_from_env_value(Some("".into())), None);
    assert_eq!(proxy_from_env_value(None), None);
    assert_eq!(
        proxy_from_env_value(Some(" socks5://127.0.0.1:10808 ".into())),
        Some("socks5://127.0.0.1:10808".to_string()),
        "首尾空白应被裁掉"
    );
}

/// 直连上下文（无代理）：不带 --proxy、也无代理侧故障归因。
#[test]
fn fallback_net_direct_has_no_proxy_and_no_block() {
    let net = FallbackNet::direct();
    assert!(net.curl_proxy().is_none(), "直连语义不得带代理");
    assert!(net.blocked().is_none(), "直连语义无代理侧故障");
}
