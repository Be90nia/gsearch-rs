//! 全链路统一搜索结果结构（M1 仅占位定义，M2 填 SERP 解析逻辑）

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// M14-1B：给 agent 用的 self-describing JSON 头部。`--json` 输出现在长这样：
/// ```json
/// { "meta": <MetaOutput>, "results": [...] }
/// ```
/// 字段保持扁平、与 schema spec 一一对应；新增字段请追加到末尾（serde 顺序即 JSON key 顺序）。
#[derive(Serialize, Clone, Debug)]
pub struct MetaOutput {
    pub tool: &'static str,
    pub version: &'static str,
    /// 仅 search 命令填查询串；browse / dl 留空串。
    pub query: String,
    /// `~/.gsearch/profiles/<name>/` 的末段名（未设 GSEARCH_PROFILE 时为 "default"）。
    pub profile: String,
    /// 浏览器大类："Chrome" 或 "Edge"。
    pub browser_kind: String,
    /// 浏览器可执行文件绝对路径。
    pub browser_path: String,
    /// 代理 URL；直连时为 None → JSON null。
    pub proxy: Option<String>,
    pub humanize: bool,
    pub limit: usize,
    /// 从启动到产出结果的总耗时（毫秒）。
    pub elapsed_ms: u128,
    pub results_count: usize,
    /// 是否被 `--limit` 截断。search 命令：达到 limit 且最后一页满则 true。
    pub truncated: bool,
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
    #[default]
    Error,
}

/// M15 扩展：人类可读的状态文本，Agent 可直接喂回 LLM。
#[derive(Serialize, Clone, Debug, Default)]
pub struct RunStatusInfo {
    pub status: RunStatus,
    /// 本次是否经过人工 CAPTCHA 验证（仅 Ok 时有信息量）。
    pub captcha_solved: bool,
    /// 人类可读提示。CaptchaRequired 时含“弹窗请用户验证 + 已等 N 秒/120 秒”。
    pub message: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct OutputEnvelope<T: Serialize> {
    pub meta: MetaOutput,
    /// M15：放在 results 前面让 Agent 优先看到状态字段（按 JSON key 顺序）。
    pub run: RunStatusInfo,
    pub results: T,
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
        }]
    }

    fn sample_meta() -> MetaOutput {
        MetaOutput {
            tool: "gsearch",
            version: "0.2.0",
            query: "python asyncio".into(),
            profile: "default".into(),
            browser_kind: "Chrome".into(),
            browser_path: r"C:\Program Files\Google\Chrome\Application\chrome.exe".into(),
            proxy: None,
            humanize: false,
            limit: 10,
            elapsed_ms: 1234,
            results_count: 1,
            truncated: false,
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
    /// M14-1B 验收：proxy = None 必须序列化成 JSON `null`（不是缺失字段，不是空字符串）。
    #[test]
    fn meta_proxy_none_serializes_as_null() {
        let env = OutputEnvelope {
            meta: sample_meta(),
            run: RunStatusInfo { status: RunStatus::Ok, captcha_solved: false, message: String::new() },
            results: sample_results(),
        };
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains("\"proxy\":null"), "proxy 应为 JSON null: {s}");
        assert!(!s.contains("\"proxy\":\""), "proxy 不应是空字符串");
    }

    /// M15：四种状态都正确序列化小写 snake_case。
    #[test]
    fn run_status_all_variants_serialize_snake_case() {
        for (status, expected) in [
            (RunStatus::Ok, "\"status\":\"ok\""),
            (RunStatus::CaptchaRequired, "\"status\":\"captcha_required\""),
            (RunStatus::CaptchaTimeout, "\"status\":\"captcha_timeout\""),
            (RunStatus::Error, "\"status\":\"error\""),
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
}
