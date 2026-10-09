//! 全链路统一搜索结果结构（M1 仅占位定义，M2 填 SERP 解析逻辑）

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// dd1：来源内部相关性分透传（SearXNG JSON results[].score）；无分来源（Google HTML、
    /// SearXNG HTML 降级）缺席该键，agent 按键存在性判断可否按分筛序。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// nw4：来源类型标注（URL host 启发式：docs/github/wikipedia/blog/forum/video/news/qa/other）。
    /// 零网络零猜测——未命中一律 other。装配处统一走 util::domain_class。
    pub domain_class: &'static str,
}

/// M14-1B：给 agent 用的 self-describing JSON 头部。`--json` 输出现在长这样：
/// ```json
/// { "meta": <MetaOutput>, "results": [...] }
/// ```
/// 字段保持扁平、与 schema spec 一一对应；新增字段请追加到末尾（serde 顺序即 JSON key 顺序）。
#[derive(Serialize, Clone, Debug)]
pub struct MetaOutput {
    #[serde(skip_serializing_if = "skip_compact_static")]
    pub tool: &'static str,
    #[serde(skip_serializing_if = "skip_compact_static")]
    pub version: &'static str,
    /// 仅 search 命令填查询串；browse / dl 留空串。
    pub query: String,
    #[serde(skip_serializing_if = "skip_compact_str")]
    /// `~/.gsearch/profiles/<name>/` 的末段名（未设 GSEARCH_PROFILE 时为 "default"）。
    pub profile: String,
    #[serde(skip_serializing_if = "skip_compact_absent_opt")]
    /// 代理 URL；未传/直连时键缺席（ago：缺席语义执行到底，README「未传时键缺席」为真）。
    pub proxy: Option<String>,
    #[serde(skip_serializing_if = "skip_compact_bool")]
    pub humanize: bool,
    #[serde(skip_serializing_if = "skip_compact_usize")]
    pub limit: usize,
    /// 从启动到产出结果的总耗时（毫秒）。
    pub elapsed_ms: u128,
    /// 是否被 `--limit` 截断；false = 正常态，键缺席（8lp 空值缺席=正常）。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// M16：本次搜索来源："searxng"（配了 SearXNG 且成功）/ "google"（直爬 / 回退）/
    /// "duckduckgo"（DDG html 直连）。h90：browse 等非搜索输出**无搜索来源**——置空串，
    /// 键整体缺席（8lp 缺席=正常）。曾硬编码 "google" 与实际渲染目标相悖、误导 agent 分流；
    /// 塞目标 host 又会与既定值域（三个 provider 名）冲突——缺席是最小惊讶解。
    /// 搜索路径恒非空，搜索输出的键存在性不变。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider: String,
    /// 时间过滤回显（--recency 的原始值）；未传时键缺席（ago：与 proxy 同缺席语义）。
    #[serde(skip_serializing_if = "skip_compact_absent_opt")]
    pub recency: Option<String>,
}

// 6dp：`--compact-meta` 压缩开关。skip 判定读进程级标志——serde 的 skip_serializing_if
// 拿不到 self，全局 AtomicBool 是最小改动（构造方零改动、保留字段 JSON 顺序不变）。
static COMPACT_META: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 6dp：设置 `--compact-meta` 生效值（flag && !debug，main 在派发前算好传入）。
/// debug 日志开启强制全量——元审计硬约束（compact 与 stderr 静默同开 = 排障现场双失）。
pub fn set_compact_meta(effective: bool) {
    COMPACT_META.store(effective, std::sync::atomic::Ordering::Relaxed);
}

fn compact_on() -> bool {
    COMPACT_META.load(std::sync::atomic::Ordering::Relaxed)
}
fn skip_compact_static(_: &'static str) -> bool {
    compact_on()
}
fn skip_compact_str(_: &String) -> bool {
    compact_on()
}
/// ago：proxy/recency 未传（None）时键缺席（README 缺席语义）；--compact-meta 下仍全跳。
fn skip_compact_absent_opt(v: &Option<String>) -> bool {
    compact_on() || v.is_none()
}
fn skip_compact_bool(_: &bool) -> bool {
    compact_on()
}
fn skip_compact_usize(_: &usize) -> bool {
    compact_on()
}

/// M14-1B：`--json` 输出的统一信封，`results` 是真正的载荷（Vec 或 AdaptiveRead）。
/// ponytail: 用泛型让 search / browse / dl 共用同一序列化路径；不引新依赖。
/// M15 扩展：每次响应顶层带状态，便于 Agent 识别四种结局而不必解析 stderr / 文案。
/// 协议约定：
///   * `Ok`               → 正常出结果，captcha_solved 记录本次是否经过人工验证
///   * `CaptchaRequired`  → 当前页撞验证，已起有头窗等人解；results 留空但 Agent 拿到事件
///   * `CaptchaTimeout`   → 等人解超时，results 留空
#[derive(Serialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ok,
    CaptchaRequired,
    CaptchaTimeout,
    /// zc6：SearXNG json+html 双空且 Google:443 预检不通——熔断快速失败。
    /// 元审计硬约束：必须是这个值而非 error（基础设施降级 ≠ 查询无资料 ≠ 出错），
    /// provider 保持 searxng，防 agent 把熔断误读为「该话题无资料」。
    SearxngDegraded,
    /// o1p：SearXNG 健康（HTTP 200）但 recency 过滤后零结果——「没新鲜结果」≠ 基础设施降级，
    /// agent 的正确动作是去掉 --recency / 换时间窗，而非 doctor 排障。
    FilteredEmpty,
    /// o1p：查询无果且 SearXNG 源健康——既非熔断也非过滤空，换词重试即可。
    NoResults,
    #[default]
    Error,
}

/// M15 扩展：人类可读的状态文本，Agent 可直接喂回 LLM。
/// 8lp：happy-path 空值缺席——captcha_solved=false、message="" 是正常态，不占键。
#[derive(Serialize, Clone, Debug, Default)]
pub struct RunStatusInfo {
    pub status: RunStatus,
    /// 本次是否经过人工 CAPTCHA 验证（仅 Ok 时有信息量）。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub captcha_solved: bool,
    /// 人类可读提示。CaptchaRequired 时含“弹窗请用户验证 + 已等 N 秒/120 秒”。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct OutputEnvelope<T: Serialize> {
    pub meta: MetaOutput,
    /// M15：放在 results 前面让 Agent 优先看到状态字段（按 JSON key 顺序）。
    pub run: RunStatusInfo,
    pub results: T,
}

/// af9-e7c：`similar` 输出条目 = SearchResult + similarity 启发标注（serde flatten 并列展开，
/// search 全套字段与键缺席语义不变）。
#[derive(Serialize, Clone, Debug)]
pub struct SimilarHit {
    #[serde(flatten)]
    pub hit: SearchResult,
    /// 启发来源标注：`title=<词>` / `site=<host>` 分号连接；两者皆无时为 none 说明串。
    pub similarity: String,
}

/// batch 多查询（`search q1 q2 --json`，issue gsearch-rs-doh）输出的裸数组元素：
/// 每条自带 meta，单条失败（status=error、results 空、message 给原因）不阻塞其他条目。
#[derive(Serialize, Clone, Debug)]
pub struct BatchEntry {
    pub query: String,
    pub status: RunStatus,
    /// status=error 时的人类可读原因；ok 时空串 → 键缺席（8lp）。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
    pub meta: MetaOutput,
    pub results: Vec<SearchResult>,
}

/// nx4：`--envelope v2` 的 batch 信封——批统计 meta 只出一层，元素不带 14 字段 meta。
/// 默认仍是裸数组（BatchEntry）；v2 为 opt-in，存量 agent 零破坏。
#[derive(Serialize, Clone, Debug)]
pub struct BatchEnvelopeV2 {
    pub meta: BatchMetaV2,
    pub results: Vec<BatchEntryV2>,
}

/// nx4：批处理级统计（一次）；原每条 meta 的 elapsed_ms 并入此处。
#[derive(Serialize, Clone, Debug)]
pub struct BatchMetaV2 {
    pub n_total: usize,
    pub n_ok: usize,
    pub n_fail: usize,
    pub elapsed_ms: u128,
}

/// nx4：v2 数组元素——只留 query/status/message/results（14 字段 meta 移除）。
#[derive(Serialize, Clone, Debug)]
pub struct BatchEntryV2 {
    pub query: String,
    pub status: RunStatus,
    /// status=error 时的人类可读原因；ok 时空串 → 键缺席（与 BatchEntry 同语义，8lp）。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
    pub results: Vec<SearchResult>,
}

/// M14-1A `verify <url>` 的报告。status=0 表示未拿到任何 HTTP 响应（仅 SSL 失败路径）。
#[derive(Serialize, Clone, Debug, Default)]
pub struct VerifyReport {
    pub status: u16,
    /// redirect 跟随后的最终 URL。
    pub final_url: String,
    /// 已跟随的每一跳 Location 值（原样，可能相对路径）；无重定向为空。
    pub redirect_chain: Vec<String>,
    /// https：握手+证书验证通过为 true，握手错误 false；http 无握手恒 true。
    pub ssl_valid: bool,
    pub latency_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_results() -> Vec<SearchResult> {
        vec![SearchResult {
            title: "T".into(),
            url: "https://example.com/".into(),
            snippet: "S".into(),
            score: None,
            domain_class: "other",
        }]
    }

    fn sample_meta() -> MetaOutput {
        MetaOutput {
            tool: "gsearch",
            version: "0.2.0",
            query: "python asyncio".into(),
            profile: "default".into(),
            proxy: None,
            humanize: false,
            limit: 10,
            elapsed_ms: 1234,
            truncated: false,
            provider: "google".into(),
            recency: None,
        }
    }

    /// M14-1B 验收：`--json` 信封顶层有 meta + run + results，且 run.status 序列化小写 snake_case。
    #[test]
    fn envelope_serializes_meta_run_results() {
        let env = OutputEnvelope {
            meta: sample_meta(),
            run: RunStatusInfo {
                status: RunStatus::Ok,
                captcha_solved: false,
                message: String::new(),
            },
            results: sample_results(),
        };
        let v: serde_json::Value = serde_json::to_value(&env).unwrap();
        assert!(v.get("meta").is_some(), "missing meta");
        assert!(v.get("run").is_some(), "missing run");
        assert!(v.get("results").is_some(), "missing results");
        // run.status 序列化为字符串（snake_case），不是 enum tag
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains("\"status\":\"ok\""), "run.status 应小写 snake_case: {s}");
        assert!(!s.contains("\"Ok\""), "Ok 不应作为字符串原样输出: {s}");
        let m = &v["meta"];
        assert_eq!(m["tool"], "gsearch");
        assert_eq!(m["version"], "0.2.0");
        assert_eq!(m["query"], "python asyncio");
        assert_eq!(m["profile"], "default");
    }
    /// ago：缺席语义执行到底——proxy/recency 未传（None）时键真缺席
    ///（README 契约「未传时键缺席」；原 M14-1B 的 null 常驻语义已被拍板废弃）。
    #[test]
    fn meta_proxy_recency_none_keys_absent() {
        let env = OutputEnvelope {
            meta: sample_meta(),
            run: RunStatusInfo { status: RunStatus::Ok, captcha_solved: false, message: String::new() },
            results: sample_results(),
        };
        let s = serde_json::to_string(&env).unwrap();
        assert!(!s.contains("\"proxy\""), "proxy 未传应键缺席: {s}");
        assert!(!s.contains("\"recency\""), "recency 未传应键缺席: {s}");
        // 传了值则键照常出现（缺席 ≠ 永远缺席）
        let mut m = sample_meta();
        m.proxy = Some("http://127.0.0.1:10808".into());
        m.recency = Some("week".into());
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"proxy\":\"http://127.0.0.1:10808\""), "proxy 传值应出现: {s}");
        assert!(s.contains("\"recency\":\"week\""), "recency 传值应出现: {s}");
    }

    /// h90：browse 等非搜索输出 provider 置空串 → 键整体缺席；搜索输出（非空）键照常。
    #[test]
    fn provider_empty_key_absent_search_keeps_key() {
        // browse 形态：provider 空 → 键缺席（不再伪装成 "google" 误导分流）
        let mut m = sample_meta();
        m.query = String::new();
        m.provider = String::new();
        let s = serde_json::to_string(&m).unwrap();
        assert!(!s.contains("\"provider\""), "空 provider 应键缺席: {s}");
        // 搜索形态：非空 → 键照常出现
        let mut m = sample_meta();
        m.provider = "searxng".into();
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"provider\":\"searxng\""), "{s}");
    }

    /// M15：四种状态都正确序列化小写 snake_case。
    #[test]
    fn run_status_all_variants_serialize_snake_case() {
        for (status, expected) in [
            (RunStatus::Ok, "\"status\":\"ok\""),
            (RunStatus::CaptchaRequired, "\"status\":\"captcha_required\""),
            (RunStatus::CaptchaTimeout, "\"status\":\"captcha_timeout\""),
            (RunStatus::Error, "\"status\":\"error\""),
            // o1p：三态零结果语义（filtered_empty / no_results 与 searxng_degraded 分立）
            (RunStatus::FilteredEmpty, "\"status\":\"filtered_empty\""),
            (RunStatus::NoResults, "\"status\":\"no_results\""),
        ] {
            let env = OutputEnvelope::<Vec<SearchResult>> {
                meta: sample_meta(),
                run: RunStatusInfo { status, captcha_solved: false, message: "x".into() },
                results: vec![],
            };
            let s = serde_json::to_string(&env).unwrap();
            assert!(s.contains(expected), "{expected} not in {s}");
        }
    }

    /// M14-1B 验收：browse / dl 命令 query 留空串（schema spec 要求）。
    #[test]
    fn meta_query_empty_for_non_search() {
        let mut m = sample_meta();
        m.query = String::new();
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"query\":\"\""), "browse/dl query 应为空串: {s}");
    }

    /// zc6：熔断状态序列化 snake_case（元审计硬约束：searxng_degraded 而非 error）。
    #[test]
    fn searxng_degraded_serializes_snake_case() {
        let env = OutputEnvelope::<Vec<SearchResult>> {
            meta: sample_meta(),
            run: RunStatusInfo {
                status: RunStatus::SearxngDegraded,
                captcha_solved: false,
                message: "x".into(),
            },
            results: vec![],
        };
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains("\"status\":\"searxng_degraded\""), "{s}");
    }

    /// nw4：domain_class 追加在 SearchResult 末尾（serde 顺序 = key 顺序契约）。
    #[test]
    fn search_result_appends_domain_class_last() {
        let s = serde_json::to_string(&sample_results()[0]).unwrap();
        assert!(
            s.ends_with(r#""domain_class":"other"}"#),
            "domain_class 应为末键: {s}"
        );
    }

    /// batch 契约（issue gsearch-rs-doh）：元素含 query/status/meta/results 键，
    /// status 序列化小写 snake_case；error 条目 results 为空数组且 message 带原因。
    /// 8lp：ok 条目 message 空串 → 键缺席（缺席=正常）。
    #[test]
    fn batch_entry_serializes_contract_keys() {
        let mut m = sample_meta();
        m.query = "rust async".into();
        m.provider = "searxng".into();
        let ok = BatchEntry {
            query: "rust async".into(),
            status: RunStatus::Ok,
            message: String::new(),
            meta: m,
            results: sample_results(),
        };
        let v: serde_json::Value = serde_json::to_value(&ok).unwrap();
        for k in ["query", "status", "meta", "results"] {
            assert!(v.get(k).is_some(), "缺少键 {k}");
        }
        assert!(v.get("message").is_none(), "ok 条目 message 空串应缺席: {v}");
        assert_eq!(v["status"], "ok");
        assert_eq!(v["meta"]["provider"], "searxng");
        assert_eq!(v["meta"]["query"], "rust async");

        let err = BatchEntry {
            query: "dead query".into(),
            status: RunStatus::Error,
            message: "SearXNG 查询失败".into(),
            meta: sample_meta(),
            results: vec![],
        };
        let v: serde_json::Value = serde_json::to_value(&err).unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["message"], "SearXNG 查询失败");
        assert_eq!(v["results"].as_array().unwrap().len(), 0);
    }

    /// 8lp：空值缺席=正常——run.captcha_solved=false / run.message="" / meta.truncated=false
    /// 不占键；dd1：score=Some 时透传且 domain_class 仍是末键（nw4 契约不破）。
    #[test]
    fn null_value_absence_and_score_passthrough() {
        let env = OutputEnvelope::<Vec<SearchResult>> {
            meta: sample_meta(),
            run: RunStatusInfo::default(),
            results: vec![],
        };
        let s = serde_json::to_string(&env).unwrap();
        assert!(!s.contains("captcha_solved"), "captcha_solved=false 应缺席: {s}");
        assert!(!s.contains("\"message\""), "message=\"\" 应缺席: {s}");
        assert!(!s.contains("\"truncated\""), "truncated=false 应缺席: {s}");
        assert!(!s.contains("\"results_count\""), "results_count 已从 meta 移除: {s}");

        let mut r = sample_results().remove(0);
        r.score = Some(1.5);
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"score\":1.5"), "score 应透传: {s}");
        assert!(s.ends_with(r#""domain_class":"other"}"#), "domain_class 应仍是末键: {s}");
    }

    /// nx4 契约：v2 信封顶层 meta{n_total,n_ok,n_fail,elapsed_ms} 只出一层；
    /// 元素四键 query/status/message/results，无 meta。
    #[test]
    fn batch_envelope_v2_contract_keys() {
        let env = BatchEnvelopeV2 {
            meta: BatchMetaV2 { n_total: 2, n_ok: 1, n_fail: 1, elapsed_ms: 88 },
            results: vec![
                BatchEntryV2 {
                    query: "q1".into(),
                    status: RunStatus::Ok,
                    message: String::new(),
                    results: sample_results(),
                },
                BatchEntryV2 {
                    query: "q2".into(),
                    status: RunStatus::Error,
                    message: "boom".into(),
                    results: vec![],
                },
            ],
        };
        let v: serde_json::Value = serde_json::to_value(&env).unwrap();
        let m = &v["meta"];
        assert_eq!(m["n_total"], 2);
        assert_eq!(m["n_ok"], 1);
        assert_eq!(m["n_fail"], 1);
        assert_eq!(m["elapsed_ms"], 88);
        let arr = v["results"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // 8lp：ok 条目 message 空串缺席，error 条目才有 message 键
        for (i, k) in [(0usize, "query"), (0, "status"), (0, "results"), (1, "message")] {
            assert!(arr[i].get(k).is_some(), "元素 {i} 缺少键 {k}");
        }
        assert!(arr[0].get("message").is_none(), "v2 ok 条目 message 空串应缺席: {}", arr[0]);
        assert!(arr[0].get("meta").is_none(), "v2 元素不应带 meta: {}", arr[0]);
        assert_eq!(arr[0]["status"], "ok");
        assert_eq!(arr[1]["status"], "error");
    }

    /// cm8：meta 不再携带 browser_path/browser_kind 环境噪声（77B×每命令的 token 税）；
    /// 浏览器路径信息仍可经 `gsearch doctor` 获取。
    #[test]
    fn meta_omits_browser_keys() {
        let meta = sample_meta();
        let v: serde_json::Value = serde_json::to_value(&meta).unwrap();
        assert!(v.get("browser_path").is_none(), "browser_path 应已移除: {}", v);
        assert!(v.get("browser_kind").is_none(), "browser_kind 应已移除: {}", v);
    }
}
