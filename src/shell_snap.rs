//! M14 snap/click @eN：可交互元素抓取与重定位（从 shell.rs 拆出）。
//!
//! `snap` 抓当前页可见可交互元素列表（eN ref），`click @eN` 按 snap 时的 DOM 序号重定位后 JS click。

use anyhow::{Result, anyhow};
use chromiumoxide::Page;
use serde::Deserialize;

/// snap/click @eN 共用的可交互元素选择器；click 靠同一列表的 DOM 序号重定位元素。
const SNAP_SELECTOR: &str = "a,button,input,select,textarea,[onclick]";

/// M14 snap 抓到的单个可交互元素。`ref_id` 是可见元素列表的 eN 编号（click @eN 用），
/// `index` 是 querySelectorAll 的 DOM 序号（元素重定位用）；页面变动后两者漂移，重新 snap 即可。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SnapElem {
    pub(crate) ref_id: String,
    pub(crate) tag: String,
    pub(crate) text: String,
    /// a 标签的绝对 href（el.href 属性，浏览器已解析相对路径），click @eN 直接 goto。
    pub(crate) href: String,
    pub(crate) id: String,
    pub(crate) index: usize,
}

/// JS 侧原始返回（字段名对齐 JS 对象）；snap_elems_from_raw 补 eN 编号后转 SnapElem。
#[derive(Debug, Deserialize)]
struct RawSnapElem {
    i: usize,
    tag: String,
    text: String,
    href: String,
    id: String,
}

/// `click` 参数：`@eN` ref（来自 snap）或数字（last_results 序号，原行为）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ClickTarget {
    Ref(String),
    Idx(usize),
}

/// 页内 JS 遍历 SNAP_SELECTOR 元素，getBoundingClientRect 过滤宽/高为 0 的不可见项。
/// `async function ()` 声明形态（chromiumoxide 函数探测不认箭头函数）。
/// ponytail: 不滚动加载，懒加载页先等渲染完再 snap；需要时加滚动采集。
pub(crate) async fn snap_page(page: &Page) -> Result<Vec<SnapElem>> {
    let js = format!(
        "async function () {{
            const out = [];
            document.querySelectorAll({sel}).forEach(function (el, i) {{
                const r = el.getBoundingClientRect();
                if (r.width === 0 || r.height === 0) return;
                let text = (el.innerText || '').trim();
                if (!text && el.tagName === 'INPUT') {{
                    const p = el.getAttribute('placeholder') || '';
                    text = el.value || (p ? 'placeholder=\"' + p + '\"' : '');
                }}
                if (!text) text = el.getAttribute('aria-label') || '';
                out.push({{
                    i: i,
                    tag: el.tagName.toLowerCase(),
                    text: text.replace(/\\s+/g, ' ').slice(0, 30),
                    href: el.href || '',
                    id: el.id || ''
                }});
            }});
            return out;
        }}",
        sel = serde_json::to_string(SNAP_SELECTOR)?
    );
    let raw = page
        .evaluate(js)
        .await
        .map_err(|e| anyhow!("snap 失败: {e}"))?
        .into_value::<Vec<RawSnapElem>>()
        .map_err(|e| anyhow!("snap 返回结构异常: {e}"))?;
    Ok(snap_elems_from_raw(raw))
}

/// click @eN 元素路径：按 snap 时的 DOM 序号在同一 selector 列表里重定位，
/// tag 校验一致才 JS click（DOM 变动导致序号漂移时报"snap 已过期"，不静默点错元素）。
/// ponytail: JS .click() 不模拟鼠标移动，hover 展开类菜单不适用；需要时换 CDP Input 域。
pub(crate) async fn click_snap_elem(page: &Page, el: &SnapElem) -> Result<()> {
    let js = format!(
        "async function () {{
            const els = document.querySelectorAll({sel});
            const t = els[{i}];
            if (!t) throw new Error('snap 已过期: 序号 {i} 越界（当前 ' + els.length + ' 个）');
            const tag = t.tagName.toLowerCase();
            if (tag !== {want}) throw new Error('snap 已过期: 序号 {i} 是 <' + tag + '>，期望 <' + {want} + '>');
            t.click();
        }}",
        sel = serde_json::to_string(SNAP_SELECTOR)?,
        i = el.index,
        want = serde_json::to_string(&el.tag)?
    );
    page.evaluate(js).await.map_err(|e| anyhow!("click @{} 失败: {e}", el.ref_id))?;
    Ok(())
}

/// `click` 目标解析：`@eN` ref 或数字序号。
pub(crate) fn parse_click_target(s: &str) -> Result<ClickTarget> {
    if let Some(rest) = s.strip_prefix('@') {
        let ok = rest.starts_with('e') && rest.len() > 1 && rest[1..].bytes().all(|b| b.is_ascii_digit());
        if ok {
            Ok(ClickTarget::Ref(rest.to_string()))
        } else {
            Err(anyhow!("click 非法 ref: {s:?}（应为 @eN，如 @e3）"))
        }
    } else {
        s.parse::<usize>()
            .map(ClickTarget::Idx)
            .map_err(|_| anyhow!("click 参数非法: {s:?}（应为 N 或 @eN）"))
    }
}

/// last_snap 里按 ref 查元素；未命中给可行动提示。
pub(crate) fn find_snap_elem<'a>(snap: &'a [SnapElem], ref_id: &str) -> Result<&'a SnapElem> {
    snap.iter()
        .find(|e| e.ref_id == ref_id)
        .ok_or_else(|| anyhow!("snap 无 {ref_id}（共 {} 个元素；页面变动后先重新 snap）", snap.len()))
}

/// JS 原始返回 → SnapElem：eN 编号按可见元素顺序 1 起，index 保留 DOM 序号（两者不混淆）。
fn snap_elems_from_raw(raw: Vec<RawSnapElem>) -> Vec<SnapElem> {
    raw.into_iter()
        .enumerate()
        .map(|(n, r)| SnapElem {
            ref_id: format!("e{}", n + 1),
            tag: r.tag,
            text: r.text,
            href: r.href,
            id: r.id,
            index: r.i,
        })
        .collect()
}

/// snap 单行：`e1  <a> "Sign in" → https://x/login`；href 优先，无 href 用 #id，都无则省略尾部。
/// 文本自身带引号（如 input 的 `placeholder="..."`）不再包外层引号。
pub(crate) fn format_snap_line(e: &SnapElem) -> String {
    let text = if e.text.contains('"') {
        e.text.clone()
    } else {
        format!("\"{}\"", e.text)
    };
    let target = if !e.href.is_empty() {
        e.href.clone()
    } else if !e.id.is_empty() {
        format!("#{}", e.id)
    } else {
        return format!("{}  <{}> {}", e.ref_id, e.tag, text);
    };
    format!("{}  <{}> {} → {}", e.ref_id, e.tag, text, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_click_target_ref_and_idx() {
        assert_eq!(parse_click_target("@e3").unwrap(), ClickTarget::Ref("e3".into()));
        assert_eq!(parse_click_target("3").unwrap(), ClickTarget::Idx(3));
    }

    #[test]
    fn parse_click_target_rejects_invalid() {
        for bad in ["", "e3", "@x3", "@e", "@3", "@E3", "abc", "@"] {
            assert!(parse_click_target(bad).is_err(), "应报错: {bad:?}");
        }
    }

    #[test]
    fn find_snap_elem_hit_and_miss() {
        let snap = vec![
            SnapElem { ref_id: "e1".into(), tag: "a".into(), text: "Sign in".into(), href: "https://x/login".into(), id: String::new(), index: 0 },
            SnapElem { ref_id: "e2".into(), tag: "button".into(), text: "Submit".into(), href: String::new(), id: "submit-btn".into(), index: 4 },
            SnapElem { ref_id: "e3".into(), tag: "input".into(), text: "placeholder=\"Search\"".into(), href: String::new(), id: "search-q".into(), index: 7 },
        ];
        let hit = find_snap_elem(&snap, "e3").unwrap();
        assert_eq!((hit.tag.as_str(), hit.index), ("input", 7));
        assert!(find_snap_elem(&snap, "e99").is_err());
    }

    #[test]
    fn format_snap_line_cases() {
        let a = SnapElem { ref_id: "e1".into(), tag: "a".into(), text: "Sign in".into(), href: "https://x/login".into(), id: String::new(), index: 0 };
        let btn = SnapElem { ref_id: "e2".into(), tag: "button".into(), text: "Submit".into(), href: String::new(), id: "submit-btn".into(), index: 4 };
        let ipt = SnapElem { ref_id: "e3".into(), tag: "input".into(), text: "placeholder=\"Search\"".into(), href: String::new(), id: "search-q".into(), index: 7 };
        let bare = SnapElem { ref_id: "e4".into(), tag: "div".into(), text: "菜单".into(), href: String::new(), id: String::new(), index: 9 };
        assert_eq!(format_snap_line(&a), "e1  <a> \"Sign in\" → https://x/login");
        assert_eq!(format_snap_line(&btn), "e2  <button> \"Submit\" → #submit-btn");
        assert_eq!(format_snap_line(&ipt), "e3  <input> placeholder=\"Search\" → #search-q");
        assert_eq!(format_snap_line(&bare), "e4  <div> \"菜单\"");
    }

    #[test]
    fn snap_elems_from_raw_assigns_ref_ids() {
        let raw = vec![
            RawSnapElem { i: 2, tag: "a".into(), text: "t1".into(), href: "/x".into(), id: String::new() },
            RawSnapElem { i: 5, tag: "button".into(), text: "t2".into(), href: String::new(), id: "b".into() },
        ];
        let snap = snap_elems_from_raw(raw);
        assert_eq!(snap[0].ref_id, "e1");
        assert_eq!(snap[0].index, 2); // DOM 序号原样保留，不与 eN 编号混淆
        assert_eq!(snap[1].ref_id, "e2");
        assert_eq!(snap[1].index, 5);
    }
}
