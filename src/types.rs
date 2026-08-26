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
#[derive(Serialize, Clone, Debug)]
pub struct OutputEnvelope<T: Serialize> {
    pub meta: MetaOutput,
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

    /// M14-1B 验收：`--json` 信封顶层有 meta + results，且字段名与 schema spec 一一对应。
    #[test]
    fn envelope_serializes_meta_and_results() {
        let env = OutputEnvelope { meta: sample_meta(), results: sample_results() };
        let v: serde_json::Value = serde_json::to_value(&env).unwrap();
        assert!(v.get("meta").is_some(), "missing meta");
        assert!(v.get("results").is_some(), "missing results");
        let m = &v["meta"];
        assert_eq!(m["tool"], "gsearch");
        assert_eq!(m["version"], "0.2.0");
        assert_eq!(m["query"], "python asyncio");
        assert_eq!(m["profile"], "default");
        assert_eq!(m["browser_kind"], "Chrome");
        assert_eq!(m["proxy"], serde_json::Value::Null);
        assert_eq!(m["results_count"], 1);
        assert_eq!(m["truncated"], false);
        assert_eq!(v["results"][0]["url"], "https://example.com/");
    }

    /// M14-1B 验收：proxy = None 必须序列化成 JSON `null`（不是缺失字段，不是空字符串）。
    #[test]
    fn meta_proxy_none_serializes_as_null() {
        let env = OutputEnvelope { meta: sample_meta(), results: sample_results() };
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains("\"proxy\":null"), "proxy 应为 JSON null: {s}");
        assert!(!s.contains("\"proxy\":\"\""), "proxy 不应是空字符串");
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
