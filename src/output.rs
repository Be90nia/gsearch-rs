//! 文本 / JSON 输出（PLAN §3.4）

use anyhow::Result;

use crate::types::SearchResult;

/// snippet 截断长度（按字符不按字节，中文摘要不会截出乱码）
const SNIPPET_MAX_CHARS: usize = 160;

/// 默认输出：`N. 标题\n   url\n   snippet 前 160 字`
pub fn print_text(results: &[SearchResult]) {
    for (i, r) in results.iter().enumerate() {
        let snippet: String = r.snippet.chars().take(SNIPPET_MAX_CHARS).collect();
        println!("{}. {}\n   {}\n   {}\n", i + 1, r.title, r.url, snippet);
    }
}

/// `--json` 输出：serde_json 全量
pub fn print_json(results: &[SearchResult]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(results)?);
    Ok(())
}
