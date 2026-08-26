//! 文本 / JSON 输出（PLAN §3.4）

use anyhow::Result;

use crate::types::{OutputEnvelope, SearchResult};

/// snippet 截断长度（按字符不按字节，中文摘要不会截出乱码）
const SNIPPET_MAX_CHARS: usize = 160;

/// 默认输出：`N. 标题\n   url\n   snippet 前 160 字`
pub fn print_text(results: &[SearchResult]) {
    for (i, r) in results.iter().enumerate() {
        let snippet: String = r.snippet.chars().take(SNIPPET_MAX_CHARS).collect();
        println!("{}. {}\n   {}\n   {}\n", i + 1, r.title, r.url, snippet);
    }
}


/// M14-1B：`--json` 输出 `{meta, results}` 信封，agent 解析友好。
/// 泛型让 search 数组 / browse AdaptiveRead 共用同一序列化路径。
pub fn print_envelope_json<T: serde::Serialize>(envelope: &OutputEnvelope<T>) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(envelope)?);
    Ok(())
}
