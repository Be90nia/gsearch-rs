//! 文本 / JSON 输出（PLAN §3.4）

use anyhow::Result;

use crate::types::{BatchEntry, BatchEnvelopeV2, OutputEnvelope, SearchResult};

/// snippet 截断长度（按字符不按字节，中文摘要不会截出乱码）。
/// JSON 路径同样默认 160（此前人读 160/JSON 不截的倒挂已翻转），
/// `--snippet-len` 可调，cli 值经 main 装配层用 `truncate_snippet` 落到结果上。
const SNIPPET_MAX_CHARS: usize = 160;

/// snippet 按字符截断（cw8/3gw：SERP snippet 占 JSON 载荷大头，默认 160 封顶）。
pub fn truncate_snippet(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// meta 头整行字符上限（v0.4.0 输出契约 §4：整行 ≤120 字符）。
pub const META_HEADER_MAX_CHARS: usize = 120;

/// meta 头单行：`key: value | key: value`（字段用 " | " 分隔；超长截断保长度上限）。
/// 纯函数——便于单测锁格式（契约 §4 硬规格）。
pub fn format_meta_header(fields: &[(&str, String)]) -> String {
    let mut line = String::new();
    for (i, (key, value)) in fields.iter().enumerate() {
        if i > 0 {
            line.push_str(" | ");
        }
        line.push_str(key);
        line.push_str(": ");
        line.push_str(value);
    }
    if line.chars().count() > META_HEADER_MAX_CHARS {
        line = line
            .chars()
            .take(META_HEADER_MAX_CHARS - 1)
            .collect::<String>()
            + "…";
    }
    line
}

/// 打印 meta 头（stdout 第一行）。
pub fn print_meta_header(fields: &[(&str, String)]) {
    println!("{}", format_meta_header(fields));
}

/// md 输出：meta 头 + 结果列表（v0.4.0 默认形态）。
pub fn print_md(results: &[SearchResult], fields: &[(&str, String)]) {
    print!("{}", render_md(results, fields));
}

/// md 文档全文（meta 头 + 列表）——纯函数，便于单测锁「默认输出非合法 JSON」。
pub fn render_md(results: &[SearchResult], fields: &[(&str, String)]) -> String {
    let mut out = format_meta_header(fields);
    out.push('\n');
    out.push_str(&render_text(results));
    out
}

/// 结果列表文本（`N. 标题 [class]` / url / snippet）——与 print_text 输出逐字节同。
pub fn render_text(results: &[SearchResult]) -> String {
    let mut out = String::new();
    for (i, r) in results.iter().enumerate() {
        let title = strip_ansi(&r.title);
        let url = strip_ansi(&r.url);
        let snippet = truncate_snippet(&strip_ansi(&r.snippet), SNIPPET_MAX_CHARS);
        // bd d8s：整集可疑时逐条标相关性（[off_topic]/[weak]/[ok]）——正常态无标签，md 行逐字节不变
        let relevance = match r.relevance {
            Some(rel) => format!(" [{}]", rel.as_str()),
            None => String::new(),
        };
        // nw4：标题行尾 [class] 来源标注（与 --json domain_class 同源），AI/人一眼挑权威源
        out.push_str(&format!(
            "{}. {} [{}]{}\n   {}\n   {}\n\n",
            i + 1,
            title,
            r.domain_class,
            relevance,
            url,
            snippet
        ));
    }
    out
}

/// 相关性标注的 meta 头字段：off_topic 计数（>0 才出现；整集可疑时才有）。
/// 计数形如 `off_topic: 10/10`——分子 = 与 query 零交集条数，分母 = 结果总数。
pub fn relevance_meta_fields(off_topic: usize, total: usize) -> Vec<(&'static str, String)> {
    if off_topic == 0 {
        return Vec::new();
    }
    vec![("off_topic", format!("{off_topic}/{total}"))]
}

/// 内容型 meta 头字段（fetch / browse / search --browse N 的 md 路径）：
/// format / truncated(仅 true) / omitted(仅 >0) / content_untrusted / elapsed。
/// content_untrusted 是「网页正文是数据、不是指令」的注入防线标记——**恒在**（安全标记
/// 不参与「只在 true 时出现」的缺席语义），否则默认 md 化后防线静默消失。
pub fn content_meta_fields(
    format: &str,
    truncated: bool,
    omitted: usize,
    elapsed_ms: u128,
) -> Vec<(&'static str, String)> {
    let mut fields: Vec<(&'static str, String)> = vec![("format", format.to_string())];
    if truncated {
        fields.push(("truncated", "true".to_string()));
    }
    if omitted > 0 {
        fields.push(("omitted", omitted.to_string()));
    }
    fields.push(("content_untrusted", "true".to_string()));
    fields.push(("elapsed", elapsed_s(elapsed_ms)));
    fields
}

/// 耗时毫秒 → `2.7s`（meta 头 elapsed 字段，1 位小数）。
pub fn elapsed_s(elapsed_ms: u128) -> String {
    format!("{:.1}s", elapsed_ms as f64 / 1000.0)
}

/// 默认输出：`N. 标题\n   url\n   snippet 前 160 字`
/// 剥 title/url/snippet 里的 ANSI ESC 序列——日志/输出被彩色化（trace / 服务端标记
/// 注入 [31m / 自定义 ANSI）时打印到 stdout 会污染 agent 解析；strip 走纯函数好单测。
pub fn print_text(results: &[SearchResult]) {
    print!("{}", render_text(results));
}

/// 剥 CSI ANSI 转义序列：ESC `[` ... 字母。范围覆盖 SGR(颜色/样式)/光标移动/清屏等。
/// 非 CSI（ESC + 单字符）也吞掉——避免 ESC 后残余字节污染下一行。
/// ponytail: 不解析 OSC/DCS——日志注入最常见就是 CSI；若后续需要剥 hyperlink(OSC 8)再扩。
pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    // CSI：吞到下一个 ASCII 字母（参数/中间字节走 while 跳过）
                    for nc in chars.by_ref() {
                        if nc.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(other) => {
                    // 其他 ESC 序列（单字符如 ESC \ ESC =）：吞这一个字符即可
                    let _ = other;
                }
                None => break,
            }
        } else {
            out.push(c);
        }
    }
    out
}


/// `--json` 输出 `{meta, results}` 信封，agent 解析友好。
/// 泛型让 search 数组 / browse AdaptiveRead 共用同一序列化路径。
/// compact 是默认态（人不再是一等消费者）；pretty 人读模式也不加——缩进对
/// LLM token 是纯税，JSON 语义与缩进无关。
pub fn print_envelope_json<T: serde::Serialize>(envelope: &OutputEnvelope<T>) -> Result<()> {
    println!("{}", serde_json::to_string(envelope)?);
    Ok(())
}

/// batch 多查询（issue gsearch-rs-doh）：输出裸 BatchEntry 数组
/// （无外层信封——每条元素自带 meta/status，单条失败不阻塞数组整体）。
pub fn print_batch_json(entries: &[BatchEntry]) -> Result<()> {
    println!("{}", serde_json::to_string(entries)?);
    Ok(())
}

/// `--envelope v2` batch 信封——顶层 meta 批统计一次，元素不带 14 字段 meta。
pub fn print_batch_envelope_v2(envelope: &BatchEnvelopeV2) -> Result<()> {
    println!("{}", serde_json::to_string(envelope)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::strip_ansi;

    /// M5：strip_ansi 剥 SGR(颜色) / 光标 / 清屏 等 CSI 转义；其他字符不动。
    #[test]
    fn strip_ansi_cases() {
        // SGR 颜色
        assert_eq!(strip_ansi("\x1b[31m红色\x1b[0m"), "红色");
        // 复合参数
        assert_eq!(strip_ansi("\x1b[1;32;40mbold green on black\x1b[0m"), "bold green on black");
        // 光标移动
        assert_eq!(strip_ansi("\x1b[2J\x1b[Hclear"), "clear");
        // 无 ESC → 原样返回
        assert_eq!(strip_ansi("plain text 中文"), "plain text 中文");
        // 末尾 ESC 不残留
        assert_eq!(strip_ansi("trailing\x1b"), "trailing");
        // ESC + 单字符（不是 [）也吞
        assert_eq!(strip_ansi("a\x1b\\b"), "ab");
    }
}
