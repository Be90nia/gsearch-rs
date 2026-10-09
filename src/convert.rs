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
use scraper::{ElementRef, Html, Node as HtmlNode, Selector};
use scraper::node::Text as HtmlText;

/// 与 fetch::extract_text 同语义：script/style/noscript/template 连内容跳过。
/// turndown 默认剥 script/style，noscript/template 显式补齐（html5ever 会把其内容当文本泄漏）。
pub(crate) fn html_to_markdown(html: &str) -> Result<String> {
    let converter = HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "noscript", "template"])
        .build();
    Ok(converter.convert(&sanitize_pre_blocks(html))?)
}

/// pre>code 保真化（盲测七 0bh）：rustdoc 类文档页的 code 块内含 span 高亮 / a 链接 /
/// div where 等结构，htmd 的 span 行尾修剪会折叠换行（#[derive] 与 struct 合行）、
/// a 转成 markdown 链接语法（代码块内注入链接）、div 块级边界注入伪影空行——三处
/// 静默变异，meta.truncated 标志不覆盖。此处把每个 pre 内 code 的子树重写为单
/// Text 节点（textContent + 块级元素前边界换行），保留 pre>code 容器与 class
/// （语言标注与 fenced 路径走 htmd 原生逻辑不变）。纯文本 code 块经此路径文本
/// 不变，输出逐字节等价。
fn sanitize_pre_blocks(html: &str) -> String {
    let mut doc = Html::parse_document(html);
    let sel = Selector::parse("pre code").expect("静态选择器必然合法");
    let rewrites: Vec<_> = doc
        .select(&sel)
        .map(|code| (code.id(), faithful_code_text(&code)))
        .collect();
    if rewrites.is_empty() {
        return html.to_string();
    }
    for (id, text) in rewrites {
        let child_ids: Vec<_> = doc
            .tree
            .get(id)
            .expect("node id 来自同一棵树")
            .children()
            .map(|c| c.id())
            .collect();
        for cid in child_ids {
            if let Some(mut n) = doc.tree.get_mut(cid) {
                n.detach();
            }
        }
        if let Some(mut code) = doc.tree.get_mut(id) {
            code.append(HtmlNode::Text(HtmlText { text: text.into() }));
        }
    }
    doc.html()
}

/// code 子树的保真文本：Text 原样（HTML 源码换行保留）；块级子元素（div.where 等）
/// 前补一个换行（末尾已有换行则不重复，防伪影空行）；br 视为换行；行内元素
///（span/a/em）仅透传内容。turndown 上游语义即 textContent（T2：code block 规则）。
fn faithful_code_text(code: &ElementRef) -> String {
    let mut out = String::new();
    // 显式栈 DFS 保持文档序（fetch::tree_text 同风格，ego_tree 类型全程推断）
    let mut stack: Vec<_> = code.children().rev().collect();
    while let Some(node) = stack.pop() {
        match node.value() {
            HtmlNode::Text(t) => out.push_str(t),
            HtmlNode::Element(el) => {
                let name = el.name();
                if name == "br" {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    continue;
                }
                if is_block_level(name) && !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                stack.extend(node.children().rev());
            }
            _ => {}
        }
    }
    out

}

/// fenced 内容保真场景下的块级元素集合（div.where 是 docs.rs 签名块的实测形态，
/// 其余为常见围栏内块级标签兜底；行内 span/a 不在列——加边界反而制造换行伪影）。
fn is_block_level(name: &str) -> bool {
    matches!(
        name,
        "div" | "p" | "li" | "tr" | "blockquote" | "section" | "article" | "details" | "summary"
    )
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

    /// 盲测七 0bh 回归一：fenced 块内换行保留——rustdoc span 高亮携带的行尾换行
    /// 不得被折叠（#[derive(...)] 与 struct 必须分行）。
    #[test]
    fn fenced_block_keeps_newlines() {
        let html = r#"<pre><code><span class="attr">#[derive(Deserialize, Debug)]
</span><span class="kw">struct </span>User {
    fingerprint: String,
}</code></pre>"#;
        let md = html_to_markdown(html).unwrap();
        assert!(
            md.contains("#[derive(Deserialize, Debug)]\nstruct User {"),
            "derive 与 struct 合行 = 换行折叠: {md:?}"
        );
    }

    /// 盲测七 0bh 回归二：fenced 块内 a 链接只留文本，不注入 markdown 链接语法；
    /// div.where 块级前边界换行、且 where 前无伪影空行。
    #[test]
    fn fenced_block_strips_link_syntax_and_where_gap() {
        let html = r#"<pre><code>pub fn f(s: &amp;<a class="primitive" href="https://doc.rust-lang.org/std/primitive.str.html">str</a>) -&gt; <a class="type" href="type.Result.html">Result</a>&lt;T&gt;<div class="where">where
    T: <a class="trait" href="de/trait.D.html">Deserialize</a>&lt;'a&gt;,</div></code></pre>"#;
        let md = html_to_markdown(html).unwrap();
        assert!(!md.contains("]("), "链接语法注入: {md:?}");
        assert!(md.contains("&str"), "链接文本丢失: {md:?}");
        assert!(md.contains("<T>\nwhere\n"), "where 前边界缺失: {md:?}");
        assert!(!md.contains("<T>\n\nwhere"), "伪影空行: {md:?}");
    }

    /// 盲测七 0bh 回归三：ASCII 撇号 U+0027 逐字保真，不偷换弯引号。
    #[test]
    fn fenced_block_keeps_ascii_apostrophe() {
        let html = "<pre><code>let s = 'it&#39;s fine';</code></pre>";
        let md = html_to_markdown(html).unwrap();
        assert!(md.contains("it's fine"), "撇号变异: {md:?}");
        assert!(!md.contains('\u{2019}'), "弯引号混入: {md:?}");
    }

    /// 防回退：纯文本 code 块（GitHub md 渲染形态）语言标注与内容不变——
    /// sanitize 路径对纯文本必须是透传等价。
    #[test]
    fn plain_code_block_language_annotation_unchanged() {
        let html = "<pre><code class=\"language-rust\">fn main() {}</code></pre>";
        let md = html_to_markdown(html).unwrap();
        assert!(md.contains("```rust"), "语言标注丢失: {md:?}");
        assert!(md.contains("fn main() {}"), "内容丢失: {md:?}");
    }
}
