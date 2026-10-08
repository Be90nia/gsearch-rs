//! xih：html → markdown 本地转换封装（fetch --markdown / browse --markdown 共用）。
//! 转换在剥标签**前**的 HTML 上做——表格/标题/链接结构是 markdown 的价值所在，
//! 剥标签后只剩纯文本无从恢复。read（search --read N）属 search 输出路径（Wave1 冻结），
//! 暂不支持 --markdown（README 输出契约节已注明）。
//!
//! 选型：htmd（turndown.js 移植，passing 全部 turndown 测试用例；表格转 md 管道表格；
//! 传递依赖仅 html5ever 族，与 scraper 同源生态）。ATC 标题（# / ##）而非 Setext，
//! LLM 消费层级更直观。

use anyhow::Result;
use htmd::HtmlToMarkdown;

/// 与 fetch::extract_text 同语义：script/style/noscript/template 连内容跳过。
/// turndown 默认剥 script/style，noscript/template 显式补齐（html5ever 会把其内容当文本泄漏）。
pub(crate) fn html_to_markdown(html: &str) -> Result<String> {
    let converter = HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "noscript", "template"])
        .build();
    Ok(converter.convert(html)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 断言一：表格不拍平——md 管道表格保留行列结构。
    #[test]
    fn table_not_flattened() {
        let md = html_to_markdown(
            "<table><thead><tr><th>Lang</th><th>Year</th></tr></thead>\
             <tbody><tr><td>Rust</td><td>2010</td></tr>\
             <tr><td>Python</td><td>1991</td></tr></tbody></table>",
        )
        .unwrap();
        let lines: Vec<&str> = md.lines().collect();
        // 表头行 + 分隔行 + 两数据行 = 4 行管道结构（拍平则只有 1 行纯文本串）；
        // htmd 单元格带对齐空格（| Rust   |），断言只锚定管道 + 单元格文本。
        assert!(lines.len() >= 4, "表格应有 4 行管道结构: {md:?}");
        assert!(lines[0].contains("Lang") && lines[0].contains("Year"), "表头: {md:?}");
        assert!(lines[1].contains("---"), "分隔行: {md:?}");
        assert!(md.contains("| Rust") && md.contains("| 2010 |"), "数据行: {md:?}");
    }

    /// 断言二：标题层级保留——h1/h2/h3 → #/##/###（ATC 风格）。
    #[test]
    fn heading_levels_preserved() {
        let md = html_to_markdown(
            "<h1>一级</h1><h2>二级</h2><h3>三级</h3><p>正文</p>",
        )
        .unwrap();
        assert!(md.contains("# 一级"), "h1: {md:?}");
        assert!(md.contains("## 二级"), "h2: {md:?}");
        assert!(md.contains("### 三级"), "h3: {md:?}");
        assert!(!md.contains("=====") && !md.contains("-----"), "应为 ATC 非 Setext: {md:?}");
    }

    /// 断言三：链接保留——href 进 markdown 链接语法，citation 可追溯。
    #[test]
    fn links_preserved() {
        let md = html_to_markdown(
            r#"<p>见 <a href="https://doc.rust-lang.org/">Rust 文档</a> 与 <a href="/rel">相对链接</a>。</p>"#,
        )
        .unwrap();
        assert!(md.contains("[Rust 文档](https://doc.rust-lang.org/)"), "绝对链接: {md:?}");
        assert!(md.contains("[相对链接](/rel)"), "相对链接: {md:?}");
    }

    /// script/style/noscript 内容不泄漏进 markdown（与 fetch::extract_text 同语义）。
    #[test]
    fn skips_script_style_noscript() {
        let md = html_to_markdown(
            r#"<head><style>.x{color:red}</style><script>var a=1;</script></head>\
             <body><noscript>无 JS 提示</noscript><p>正文</p></body>"#,
        )
        .unwrap();
        assert!(md.contains("正文"));
        assert!(!md.contains("color:red"), "style 泄漏: {md:?}");
        assert!(!md.contains("var a"), "script 泄漏: {md:?}");
        assert!(!md.contains("无 JS 提示"), "noscript 泄漏: {md:?}");
    }
}
