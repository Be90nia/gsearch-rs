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

/// 默认输出：`N. 标题\n   url\n   snippet 前 160 字`
/// 剥 title/url/snippet 里的 ANSI ESC 序列——日志/输出被彩色化（trace / 服务端标记
/// 注入 [31m / 自定义 ANSI）时打印到 stdout 会污染 agent 解析；strip 走纯函数好单测。
pub fn print_text(results: &[SearchResult]) {
    for (i, r) in results.iter().enumerate() {
        let title = strip_ansi(&r.title);
        let url = strip_ansi(&r.url);
        let snippet = truncate_snippet(&strip_ansi(&r.snippet), SNIPPET_MAX_CHARS);
        // nw4：标题行尾 [class] 来源标注（与 --json domain_class 同源），AI/人一眼挑权威源
        println!("{}. {} [{}]\n   {}\n   {}\n", i + 1, title, r.domain_class, url, snippet);
    }
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
