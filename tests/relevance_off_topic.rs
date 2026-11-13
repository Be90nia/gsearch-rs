//! bd gsearch-rs-d8s 回归：结果相关性自检（off_topic / weak / ok）与诚实标注。
//!
//! 数据（本地 fixture，零外网、零浏览器）：
//!   * `off_topic_searxng_20261113.json` = 实锤 off_topic 正样本（上游 searxng#6937：
//!     同一 query 连续两次返回完全无关的结果集）→ 必须 100% 判出。
//!   * `on_topic_control_20261113.json` = 61 条真实查询的对照组 → 零 off_topic 误杀。
//!
//! 判定与渲染都走生产同款函数（`search::apply_relevance` / `output::render_md` /
//! `types::OutputEnvelope`）——测试与真实 `gsearch search` 走同一条装配链。

use gsearch::output::{relevance_meta_fields, render_md, META_HEADER_MAX_CHARS};
use gsearch::search::{
    apply_relevance, judge_relevance, off_topic_diag, DEFAULT_RELEVANCE_THRESHOLD,
};
use gsearch::types::{
    MetaOutput, OutputEnvelope, Relevance, RunStatus, RunStatusInfo, SearchResult,
};

const FIX_OFF: &str = "tests/fixtures/off_topic_searxng_20261113.json";
const FIX_CTRL: &str = "tests/fixtures/on_topic_control_20261113.json";

/// 对照组里已人工核实的「引擎噪声行」→ **期望档位 + 语义依据**。
///
/// oracle 锁语义，不锁实现细节：不再断言「恰好第几行被判 off_topic」（那是「标题有没有
/// ASCII 字母」的字形族产物），而是逐行断言「被判 off_topic 的行必须与 query 零字面交集
/// ∧ 文本与 query 同族」，并对人工核实的行给出**期望档位**（改动语义时红得有意图）。
const KNOWN_UNRELATED_CONTROL_ROWS: &[(&str, usize, Relevance, &str)] = &[
    (
        "kubernetes pdb yaml",
        3,
        Relevance::OffTopic,
        "yandex 引擎返回的加密货币交易所首页（与 query 零关联；文本含拉丁字母 → 拉丁族可比 → 字面零交集即真阳性 off_topic）",
    ),
    (
        "kubernetes pdb yaml",
        1,
        Relevance::OffTopic,
        concat!(
            "yandex 引擎返回的俄语 СберБанк 首页（与 query 零关联；集合里有拉丁文本 → 拉丁族可比 → ",
            "与上一行同档 off_topic，不再因字形族不同被分到 weak——reviewer F6 的「字形族产物」已消除）"
        ),
    ),
];

/// 读 fixture → (query, 结果集)。domain_class 走生产同款启发式（util::domain_class）。
fn load_cases(path: &str) -> Vec<(String, Vec<SearchResult>)> {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读 fixture {path}: {e}"));
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("解析 {path}: {e}"));
    v["cases"]
        .as_array()
        .unwrap_or_else(|| panic!("{path} 缺 cases 数组"))
        .iter()
        .map(|c| {
            let query = c["query"].as_str().unwrap_or_default().to_string();
            let results = c["results"]
                .as_array()
                .unwrap_or_else(|| panic!("case {query} 缺 results 数组"))
                .iter()
                .map(|r| {
                    let url = r["url"].as_str().unwrap_or_default().to_string();
                    SearchResult {
                        title: r["title"].as_str().unwrap_or_default().to_string(),
                        url: url.clone(),
                        snippet: r["snippet"].as_str().unwrap_or_default().to_string(),
                        score: None,
                        domain_class: gsearch::util::domain_class(&url),
                        relevance: None,
                    }
                })
                .collect();
            (query, results)
        })
        .collect()
}

/// 构造一条结果（测试用；domain_class 固定 other——本组用例只关心相关性判定）。
fn mk_result(title: &str, snippet: &str) -> SearchResult {
    SearchResult {
        title: title.into(),
        url: "https://example.com/x".into(),
        snippet: snippet.into(),
        score: None,
        domain_class: "other",
        relevance: None,
    }
}

/// A1 code-level：实锤 off_topic 正样本 40 条必须 100% 判为 off_topic（零漏判）。
#[test]
fn off_topic_fixture_all_results_judged_off_topic() {
    let cases = load_cases(FIX_OFF);
    let mut total = 0usize;
    for (query, mut results) in cases {
        let out = apply_relevance(&query, &mut results, DEFAULT_RELEVANCE_THRESHOLD);
        assert!(out.suspicious, "整集应判可疑（off_topic 占比达阈值）: {query}");
        assert_eq!(out.off_topic, results.len(), "整集 off_topic 条数: {query}");
        for r in &results {
            total += 1;
            assert_eq!(
                r.relevance,
                Some(Relevance::OffTopic),
                "应判 off_topic: {query} | {}",
                r.title
            );
        }
    }
    assert_eq!(total, 40, "off_topic 判定 {total}/40");
    println!("off_topic 判定 {total}/40");
}

/// A3 code-level：对照组 61 条真实查询零 off_topic 误杀（**语义口径**，非实现细节口径）。
///
/// 三个口径同时锁：
///   * **输出口径**（消费方看到的）：无一例整集判可疑 → 零 off_topic 标签下发，误报率 0。
///   * **语义属性口径**：被判 off_topic 的行必须满足「与 query **零字面交集** ∧ 文本与 query
///     **同族**」——同族用测试自带的文字族检查，字面交集用测试自带的**独立**分词复核
///     （不拿生产实现自证；放宽可比性门只会让本断言按语义变红，不再是「恰好第几行」的字形产物）。
///   * **人工核实清单**：已人工核实的无关行给出**期望档位**（同族 → off_topic 真阳性；
///     跨文字族 → weak，判不了 ≠ 判无关），两者方向都锁；清单外的行一律不得被判 off_topic。
#[test]
fn on_topic_control_zero_off_topic_false_positives() {
    let cases = load_cases(FIX_CTRL);
    let mut flagged: Vec<(String, usize)> = Vec::new();
    for (query, mut results) in cases {
        // 输出口径：整集不可疑 → 零标签下发
        let out = apply_relevance(&query, &mut results, DEFAULT_RELEVANCE_THRESHOLD);
        assert!(!out.suspicious, "对照组整集不应判可疑: {query}");
        for r in &results {
            assert_eq!(r.relevance, None, "对照组不应下发任何标签: {query}");
        }
        // 内部判定口径：逐行判定 + 语义属性复核
        let judged = judge_relevance(&query, &results, DEFAULT_RELEVANCE_THRESHOLD);
        for (i, label) in judged.labels.iter().enumerate() {
            if *label == Some(Relevance::OffTopic) {
                let r = &results[i];
                assert!(
                    zero_literal_overlap(&query, r) && comparable_context(&query, r, &results),
                    "被判 off_topic 的行必须「与 query 零字面交集 ∧ 该行处在可比的判定语境」: {query} | {}",
                    r.title
                );
                flagged.push((query.clone(), i));
            }
        }
    }
    // 清单外的行不得被判 off_topic（多出即为误杀）
    for f in &flagged {
        assert!(
            KNOWN_UNRELATED_CONTROL_ROWS
                .iter()
                .any(|(q, i, _, _)| q == &f.0 && i == &f.1),
            "被判 off_topic 的行必须是已人工核实的无关行（多出即为误杀）: {f:?}"
        );
    }
    // 已核实的行：期望档位（真阳性必须判出；跨文字族的必须判 weak 不冤枉）
    for (query, idx, expect, why) in KNOWN_UNRELATED_CONTROL_ROWS {
        let (_, results) = load_cases(FIX_CTRL)
            .into_iter()
            .find(|(q, _)| q == query)
            .unwrap_or_else(|| panic!("对照组缺 query: {query}"));
        let judged = judge_relevance(query, &results, DEFAULT_RELEVANCE_THRESHOLD);
        assert_eq!(
            judged.labels[*idx],
            Some(*expect),
            "已核实的对照组行档位不符（{why}）"
        );
    }
}

/// CJK 判定（测试自带，独立于生产实现的私有函数）。
fn is_cjk_char(ch: char) -> bool {
    let o = ch as u32;
    (0x4E00..=0x9FFF).contains(&o) || (0x3400..=0x4DBF).contains(&o) || (0xF900..=0xFAFF).contains(&o)
}

/// query 的字面 token（测试自带、**独立于**生产实现：拉丁词 ≥3 字符 + 中文不重叠 2-gram，
/// 不带停用词表）——只用来复核「被判 off_topic 的行与 query 是否真的零字面交集」。
fn query_tokens(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut cjk: Vec<char> = Vec::new();
    fn flush(word: &mut String, cjk: &mut Vec<char>, out: &mut Vec<String>) {
        if word.chars().count() >= 3 {
            out.push(std::mem::take(word));
        }
        word.clear();
        for g in cjk.chunks(2) {
            if g.len() == 2 {
                out.push(g.iter().collect());
            }
        }
        cjk.clear();
    }
    for ch in query.chars() {
        if is_cjk_char(ch) {
            if !word.is_empty() {
                flush(&mut word, &mut cjk, &mut out);
            }
            cjk.push(ch);
        } else if ch.is_alphanumeric() {
            if !cjk.is_empty() {
                flush(&mut word, &mut cjk, &mut out);
            }
            word.push(ch.to_ascii_lowercase());
        } else {
            flush(&mut word, &mut cjk, &mut out);
        }
    }
    flush(&mut word, &mut cjk, &mut out);
    out
}

/// 该行与 query 是否**零字面交集**（测试自带分词口径）。
fn zero_literal_overlap(query: &str, r: &SearchResult) -> bool {
    let hay = format!("{} {}", r.title, r.snippet).to_lowercase();
    !query_tokens(query).iter().any(|t| hay.contains(t.as_str()))
}

/// 该行是否处在「可比」的判定语境：文本有字母可比，且结果集里存在 query 所属文字族的文本
/// （否则该族词元在此集永不可能命中 → 零交集是必然 → 判不了 ≠ 判无关，不该指控）。
fn comparable_context(query: &str, r: &SearchResult, set: &[SearchResult]) -> bool {
    let row = format!("{} {}", r.title, r.snippet);
    if !row.chars().any(|c| c.is_alphabetic()) {
        return false;
    }
    let q_has_latin = query.chars().any(|c| c.is_ascii_alphabetic());
    let q_has_cjk = query.chars().any(is_cjk_char);
    let set_has_latin = set.iter().any(|x| {
        format!("{} {}", x.title, x.snippet)
            .chars()
            .any(|c| c.is_ascii_alphabetic())
    });
    let set_has_cjk = set
        .iter()
        .any(|x| format!("{} {}", x.title, x.snippet).chars().any(is_cjk_char));
    (q_has_latin && set_has_latin) || (q_has_cjk && set_has_cjk)
}

/// A3 code-level：正常查询（整集不判可疑）不得带任何 relevance 标签——缺席语义。
#[test]
fn normal_control_results_carry_no_relevance_label() {
    for (query, mut results) in load_cases(FIX_CTRL) {
        let out = apply_relevance(&query, &mut results, DEFAULT_RELEVANCE_THRESHOLD);
        assert!(!out.suspicious, "对照组整集不应判可疑: {query}");
        for r in &results {
            assert_eq!(r.relevance, None, "正常态不得带 relevance 标签: {query}");
        }
    }
}

/// A1 end-to-end：fixture → 装配 → 渲染（与真实 search 同一函数）→ JSON 与 md 都如实标注。
#[test]
fn suspicious_fixture_renders_off_topic_in_json_and_md() {
    let (query, mut results) = load_cases(FIX_OFF)
        .into_iter()
        .next()
        .expect("off_topic fixture 至少一个 case");
    let out = apply_relevance(&query, &mut results, DEFAULT_RELEVANCE_THRESHOLD);
    assert!(out.suspicious);

    // --- JSON 路径：与真实 search 同款信封装配 ---
    let meta = MetaOutput {
        tool: "gsearch",
        version: "0.4.0",
        query: query.clone(),
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
        status: RunStatus::OffTopicResults,
        captcha_solved: false,
        message: off_topic_diag(out.off_topic, out.total),
    };
    let env = OutputEnvelope {
        meta,
        run,
        results: &results,
    };
    let json = serde_json::to_string(&env).expect("信封序列化");
    let v: serde_json::Value = serde_json::from_str(&json).expect("JSON 合法");
    assert_eq!(v["run"]["status"], "off_topic_results", "run.status 新态: {json}");
    assert!(
        v["run"]["message"]
            .as_str()
            .expect("message")
            .contains("零词元交集"),
        "run.message 需人可读说明: {json}"
    );
    let arr = v["results"].as_array().expect("results 数组");
    assert_eq!(arr.len(), 10);
    for r in arr {
        assert_eq!(r["relevance"], "off_topic", "每条结果须带标签: {json}");
    }

    // --- md 路径：meta 头带计数（≤120 字符）+ 结果行可见标记 ---
    let mut fields = vec![
        ("provider", "searxng".to_string()),
        ("results", results.len().to_string()),
        ("limit", "10".to_string()),
    ];
    fields.extend(relevance_meta_fields(out.off_topic, out.total));
    fields.push(("elapsed", gsearch::output::elapsed_s(4)));
    let md = render_md(&results, &fields);
    let header = md.lines().next().unwrap_or_default();
    assert!(
        header.chars().count() <= META_HEADER_MAX_CHARS,
        "meta 头 ≤120 字符: {} ({})",
        header.chars().count(),
        header
    );
    assert!(header.contains("off_topic: 10/10"), "meta 头带计数: {header}");
    assert_eq!(md.matches("[off_topic]").count(), 10, "结果行须可见标记: {md}");
}

/// A2：run.status 新态与既有三态互不混用（序列化名各自独立）。
#[test]
fn off_topic_status_is_distinct_from_zero_result_states() {
    let env = OutputEnvelope::<Vec<SearchResult>> {
        meta: MetaOutput {
            tool: "gsearch",
            version: "0.4.0",
            query: "q".into(),
            profile: "default".into(),
            proxy: None,
            humanize: false,
            limit: 10,
            elapsed_ms: 1,
            truncated: false,
            truncated_at_offset: 0,
            provider: "searxng".into(),
            recency: None,
            site_warn: None,
            truncated_detail: None,
            off_topic: None,
        },
        run: RunStatusInfo {
            status: RunStatus::OffTopicResults,
            captcha_solved: false,
            message: "x".into(),
        },
        results: vec![],
    };
    let s = serde_json::to_string(&env).expect("序列化");
    assert!(s.contains("\"status\":\"off_topic_results\""), "{s}");
    for other in [
        "\"status\":\"searxng_degraded\"",
        "\"status\":\"filtered_empty\"",
        "\"status\":\"no_results\"",
    ] {
        assert!(!s.contains(other), "新态不得与三态混用: {s}");
    }
}

/// 阈值可配：占比不足时（阈值 1.0）不判可疑，阈值调低即判可疑——默认值来自常量。
#[test]
fn threshold_is_configurable() {
    let mk = |title: &str, snippet: &str| SearchResult {
        title: title.into(),
        url: "https://example.com/x".into(),
        snippet: snippet.into(),
        score: None,
        domain_class: "other",
        relevance: None,
    };
    let mut results = vec![
        mk("Bloomberg Anywhere", "Access the Bloomberg Terminal"),
        mk("Bloomberg Excel Add-In", "Microsoft Excel running with Bloomberg"),
        mk("PodDisruptionBudget YAML", "kubernetes pdb yaml guide"),
    ];
    let out = apply_relevance("kubernetes pdb yaml", &mut results, 1.0);
    assert!(!out.suspicious, "2/3 占比 < 阈值 1.0 → 不判可疑");
    assert_eq!(out.off_topic, 2);
    assert!(results.iter().all(|r| r.relevance.is_none()), "不判可疑则不落标签");

    let mut results = vec![
        mk("Bloomberg Anywhere", "Access the Bloomberg Terminal"),
        mk("Bloomberg Excel Add-In", "Microsoft Excel running with Bloomberg"),
        mk("PodDisruptionBudget YAML", "kubernetes pdb yaml guide"),
    ];
    let out = apply_relevance("kubernetes pdb yaml", &mut results, DEFAULT_RELEVANCE_THRESHOLD);
    assert!(out.suspicious, "2/3 占比 ≥ 默认阈值 0.5 → 判可疑");
    assert_eq!(out.off_topic, 2);
    assert_eq!(out.total, 3);
}

/// query 词元 < 2 不判定（短 query 不冤枉）——零交集不判 off_topic。
#[test]
fn short_query_is_not_judged() {
    let results = vec![SearchResult {
        title: "Bloomberg Anywhere".into(),
        url: "https://example.com/x".into(),
        snippet: "Access the Bloomberg Terminal".into(),
        score: None,
        domain_class: "other",
        relevance: None,
    }];
    let mut results = results;
    let out = apply_relevance("rust", &mut results, DEFAULT_RELEVANCE_THRESHOLD);
    assert!(!out.suspicious);
    assert_eq!(out.off_topic, 0);
    assert!(results.iter().all(|r| r.relevance.is_none()));
}

/// B1（**正例**，防回归）：中文 query（含拉丁词元）× **全英文且不含该拉丁词元**的结果集
/// → **必须**判整集可疑 + 逐条标 `off_topic`。
///
/// 这就是 2026-11-13 上游 bing 事故的形态（中文 query → 全英文无关结果集）：判定只依据
/// **可比**的拉丁族（CJK 词元在外语文本里永不可能命中 → 跳过，不制造假零交集），
/// 拉丁词元零交集 → 指控。任何后续「优化」不得把它改成不指控（漏指控 = 污染静默通过）。
#[test]
fn mixed_query_with_all_foreign_results_is_flagged() {
    let query = "如何用 Python 读取 CSV";
    let mut results = vec![
        mk_result(
            "Ear Wax Removal Singapore",
            "Professional ear cleaning services in Singapore.",
        ),
        mk_result(
            "Crypto Exchange Lsto.me",
            "How crypto exchanges are turning digital assets into spending power.",
        ),
        mk_result(
            "Currency Converter XE",
            "Live exchange rates for major world currencies and precious metals.",
        ),
    ];
    let out = apply_relevance(query, &mut results, DEFAULT_RELEVANCE_THRESHOLD);
    assert!(
        out.suspicious,
        "中文 query × 全英文无关结果集必须判可疑（bing 事故形态）"
    );
    assert_eq!(out.off_topic, 3, "整集与可比族零交集");
    assert!(
        results.iter().all(|r| r.relevance == Some(Relevance::OffTopic)),
        "可疑集逐条标 off_topic"
    );
}

/// B1（**反例**，防误伤）：中文 query（含拉丁词元）× **含该拉丁词元**的英文结果集
/// → **不得**触发 off_topic_results（拉丁族有交集 → 不指控）。
#[test]
fn mixed_query_with_matching_foreign_results_is_not_flagged() {
    let query = "如何用 Python 读取 CSV";
    let mut results = vec![
        mk_result(
            "Read CSV files with Python",
            "A practical guide to reading CSV data in Python.",
        ),
        mk_result(
            "Python CSV reader guide",
            "Parsing CSV files in Python with the csv module.",
        ),
        mk_result(
            "Python csv module docs",
            "Official documentation for the Python csv module.",
        ),
    ];
    let out = apply_relevance(query, &mut results, DEFAULT_RELEVANCE_THRESHOLD);
    assert!(!out.suspicious, "拉丁词元有交集 → 不得指控");
    assert_eq!(out.off_topic, 0, "无零交集行");
    assert!(
        results.iter().all(|r| r.relevance.is_none()),
        "不判可疑则不下发标签"
    );
}

/// B1 第三形态：同 query × **中英混合**结果集 → 正常判定路径仍工作（该判的判、该标的标）。
#[test]
fn mixed_query_with_mixed_results_still_judged() {
    let query = "如何用 Python 读取 CSV";
    let mut results = vec![
        mk_result(
            "货币转换器计算器 | XE 实时汇率",
            "利用此货币计算器在全球所有主要货币之间换算。",
        ),
        mk_result(
            "Ear Wax Removal Singapore",
            "Professional ear cleaning services in Singapore.",
        ),
        mk_result(
            "Crypto Exchange Lsto.me",
            "How crypto exchanges are turning digital assets into spending power.",
        ),
    ];
    let out = apply_relevance(query, &mut results, DEFAULT_RELEVANCE_THRESHOLD);
    assert!(
        out.suspicious,
        "集合含拉丁文本 → 拉丁族可比 → 正常判定路径仍工作"
    );
    assert_eq!(out.off_topic, 3, "整集与可比族零交集 → 三条都判 off_topic");
    assert!(
        results.iter().all(|r| r.relevance == Some(Relevance::OffTopic)),
        "可疑集逐条下发标签"
    );
}

/// S1：三档各自有断言——JSON 的 `"off_topic"`/`"weak"`/`"ok"` 值与 md 的 `[off_topic]`/`[weak]`/`[ok]`。
#[test]
fn three_relevance_tiers_are_rendered_in_json_and_md() {
    let query = "kubernetes pdb yaml";
    let mut results = vec![
        mk_result(
            "Kubernetes pdb yaml guide",
            "How to configure a PodDisruptionBudget in kubernetes.",
        ),
        mk_result(
            "Kubernetes networking basics",
            "Services and ingress for kubernetes clusters.",
        ),
        mk_result(
            "Bloomberg Anywhere",
            "Access the Bloomberg Terminal wherever you are.",
        ),
        mk_result(
            "Earwax removal Singapore",
            "Professional ear cleaning services in Singapore.",
        ),
    ];
    let out = apply_relevance(query, &mut results, DEFAULT_RELEVANCE_THRESHOLD);
    assert!(out.suspicious, "2/4 off_topic → 占比 0.5 ≥ 阈值 → 判可疑");
    let labels: Vec<Option<Relevance>> = results.iter().map(|r| r.relevance).collect();
    assert_eq!(
        labels,
        vec![
            Some(Relevance::Ok),
            Some(Relevance::Weak),
            Some(Relevance::OffTopic),
            Some(Relevance::OffTopic),
        ],
        "三档分档：全覆盖=ok / 部分交集=weak / 零交集=off_topic"
    );
    let json = serde_json::to_string(&results).expect("序列化结果");
    assert!(json.contains("\"relevance\":\"ok\""), "JSON 需出 ok 档: {json}");
    assert!(
        json.contains("\"relevance\":\"weak\""),
        "JSON 需出 weak 档: {json}"
    );
    assert!(
        json.contains("\"relevance\":\"off_topic\""),
        "JSON 需出 off_topic 档: {json}"
    );
    let md = render_md(&results, &[]);
    assert!(md.contains("[ok]"), "md 需出 [ok] 标记: {md}");
    assert!(md.contains("[weak]"), "md 需出 [weak] 标记: {md}");
    assert!(
        md.contains("[off_topic]"),
        "md 需出 [off_topic] 标记: {md}"
    );
}
