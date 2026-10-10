//! M4 搜索结果后处理：`--open N` / `--read N` / `--dl N`（PLAN §3.4 / §3.5）。
//! 复用 cmd_search 已启动的同一 browser 实例（同 profile → 同 cookie），不另起 Chrome。
//!
//! M9：`--read N` 默认输出 AdaptiveRead（按文章结构自适应三段：目录/摘要/段落索引），
//! `--full` 拿纯 innerText 5000 字（兜底），`--json` 拿结构化 JSON，`--from K` 摘要偏移，
//! `--headings-only` 仅目录。

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow};
use chromiumoxide::browser::Browser;
use serde::Deserialize;

use crate::general::{LOGIN_POLL_SECS, browser_alive, login_poll_decision};
use gsearch::search::is_captcha;
use gsearch::skeleton::{extract_adaptive, format_adaptive, format_headings_only};
use gsearch::types::SearchResult;
use gsearch::util::filename_from_url;

const PAGE_TIMEOUT_SECS: u64 = 30;
/// M17 登录墙:等人工登录的总超时(顶层命令无人守窗,不能像 `login` 命令那样不限时)
const LOGIN_WALL_TIMEOUT_SECS: u64 = 180;
/// 登录墙 title 判定要求的「正文极短」阈值(innerText 字符数;登录页只有表单文案)
const LOGIN_WALL_SHORT_BODY: usize = 400;
/// read/browse 正文提取的字符硬上限（gsearch.json `read_max_chars` 可覆盖）。
/// 防超大页撑爆 agent 上下文；正文是注入面，超限一律截断并在 meta 标注。
const READ_BODY_MAX_CHARS: usize = 50_000;
/// uhp/j44：原子快照 visibleText 的字符硬顶（jev snapshot.js 同配方；只作 marker/登录墙判定，
/// 不作正文输出源，read_full 仍走独立 innerText 求值）。
const SNAPSHOT_MAX_TEXT_CHARS: usize = 6000;
/// uhp/j44：判稳轮询间隔（两轮快照间 200ms，给 setTimeout 晚跳转留触发窗口）。
const SNAPSHOT_POLL_MS: u64 = 200;

/// `--{flag} N` 下标校验：1-based；0 或超出结果数报错（含结果数为 0 的情况）
fn pick<'a>(results: &'a [SearchResult], n: usize, flag: &str) -> Result<&'a str> {
    if n == 0 || n > results.len() {
        return Err(anyhow!("--{flag} {n} 越界（结果数 {}）", results.len()));
    }
    Ok(&results[n - 1].url)
}

/// `--open N`：默认浏览器开窗。
/// 校验 scheme 仅放行 http(s)（file: / javascript: / 自定义 scheme 一律拒），且不调 cmd.exe 解析 URL，
/// 避免 Windows 上 `cmd /c start "" <url>` 的 cmd 元字符注入（实测 URL 含 `&` 会执行后半段）。
/// explorer.exe 接受单个 URL 参数作为命令行 token，不经 cmd 解析；它会把 http(s) URL
/// 交给默认浏览器。mac/linux 不走 cmd，保持 open / xdg-open 单参数。
pub fn open(results: &[SearchResult], n: usize) -> Result<()> {
    let url = pick(results, n, "open")?;
    if !is_http_url(url) {
        return Err(anyhow!("--open 仅支持 http/https URL（拒绝: {url}）"));
    }
    let spawned = if cfg!(target_os = "windows") {
        std::process::Command::new("explorer.exe").arg(url).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = spawned {
        tracing::warn!("打开默认浏览器失败（不影响输出）: {e}");
    }
    Ok(())
}

/// 校验 URL scheme 仅放行 http(s)（防 `cmd /c start` 注入与任意 scheme 兜底打开）。
/// 提取 scheme 段时大小写不敏感（HTTP 与 http 等价），其余段（路径/查询）不动。
fn is_http_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// uhp：单次 Runtime.evaluate 的原子快照——一次 CDP 往返拿 4 类值，压 -32000 时序暴露面。
/// JS 必须用 `async function` 声明形式（chromiumoxide 第 1 坑：async 箭头函数被当 Expression
/// 求值成 {}，into_value 报 invalid type）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageSnapshot {
    pub ready_state: String,
    pub title: String,
    /// TreeWalker 可见文本：跳 script/style/noscript/template 与 aria-hidden/inert 祖先、
    /// parent checkVisibility、视口内、空白归一、硬顶 {max} 字符。j44 语义 marker 的文本源。
    pub visible_text: String,
    // JS 同时返回 links（视口内 <a href> 采集，jev snapshot.js 同配方），留给 shell_snap
    // 后续复用；本结构暂不消费（serde 忽略未知字段），避免死字段。
}

/// uhp：原子快照 JS。`{max}` 由 page_snapshot 用 str::replace 注入（format! 会要求转义全部 JS 花括号）。
const SNAPSHOT_JS: &str = r#"async function () {
    const MAX = {max};
    const skip = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE']);
    const hiddenByAncestor = (el) => {
        for (let n = el; n; n = n.parentElement) {
            if (skip.has(n.tagName)) return true;
            if (n.getAttribute && (n.getAttribute('aria-hidden') === 'true' || n.hasAttribute('inert'))) return true;
        }
        return false;
    };
    const visible = (el) => {
        if (el.checkVisibility && !el.checkVisibility()) return false;
        const r = el.getBoundingClientRect();
        return r.width > 0 && r.height > 0 && r.bottom > 0 && r.right > 0
            && r.top < window.innerHeight && r.left < window.innerWidth;
    };
    const root = document.body || document.documentElement;
    let text = '';
    if (root) {
        const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
            acceptNode(t) {
                const p = t.parentElement;
                if (!p || hiddenByAncestor(p) || !visible(p)) return NodeFilter.FILTER_REJECT;
                return NodeFilter.FILTER_ACCEPT;
            }
        });
        for (let n = walker.nextNode(); n; n = walker.nextNode()) {
            const s = n.nodeValue.replace(/\s+/g, ' ').trim();
            if (!s) continue;
            text += (text ? ' ' : '') + s;
            if (text.length >= MAX) { text = text.slice(0, MAX); break; }
        }
    }
    const links = [];
    if (root) {
        for (const a of root.querySelectorAll('a[href]')) {
            if (links.length >= 50) break;
            if (hiddenByAncestor(a) || !visible(a)) continue;
            links.push({ href: a.href, text: (a.textContent || '').trim().slice(0, 100) });
        }
    }
    return { readyState: document.readyState, title: document.title || '', visibleText: text, links };
}"#;

/// uhp：执行原子快照。evaluate Err（导航中 / -32000 context 重建 / vag 超时）原样上抛，
/// 由调用方判稳逻辑消化（超时按「未就绪」重置 marker 继续轮询）。
pub(crate) async fn page_snapshot(page: &chromiumoxide::Page) -> Result<PageSnapshot> {
    let js = SNAPSHOT_JS.replace("{max}", &SNAPSHOT_MAX_TEXT_CHARS.to_string());
    cdp_timeout(page.evaluate(js), "快照 evaluate")
        .await?
        .into_value::<PageSnapshot>()
        .map_err(|e| anyhow!("快照反序列化失败: {e}"))
}

/// j44 语义新鲜度判稳：轮询原子快照，marker = {title, visibleText}（拼接对等值比较，禁引哈希依赖），
/// **连续两次相同才判正文定稿**，替代 readyState==complete（知乎限流页等风控页秒 complete 但
/// 内容是拦截体）。readyState!=complete 仅作必要条件地板（加载中正文未定，且防空 marker
/// ("","") 在慢加载页两次假定稿），不是定稿判据。evaluate Err 视为未就绪并重置 marker
/// （晚跳转销毁 context 后在新页重新积累）。无导航页面两轮快照（间隔 200ms）即过，无固定
/// sleep；窗口耗尽返回最后一次成功快照（一次都没成功 → None），调用方自行兜底。
/// 持续动态页（行情条 / 相对时间戳等 visibleText 持续变化的内容）会烧满判稳窗口
/// （read/browse +10s、click +4s）后返回 last 快照，结果无损纯延迟。
/// 共享入口：postproc::goto_page / general::cmd_browse / shell cmd_click。
pub(crate) async fn wait_content_stable(
    page: &chromiumoxide::Page,
    rounds: usize,
) -> Option<PageSnapshot> {
    let mut prev_marker: Option<(String, String)> = None;
    let mut last: Option<PageSnapshot> = None;
    for i in 0..rounds {
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(SNAPSHOT_POLL_MS)).await;
        }
        match page_snapshot(page).await {
            Ok(s) => {
                if s.ready_state != "complete" {
                    prev_marker = None;
                    continue;
                }
                let marker = (s.title.clone(), s.visible_text.clone());
                let settled = prev_marker.as_ref() == Some(&marker);
                last = Some(s);
                if settled {
                    return last;
                }
                prev_marker = Some(marker);
            }
            Err(_) => prev_marker = None,
        }
    }
    last
}

/// 正文截断上限（gsearch.json `read_max_chars` > 缺省 50000）。
pub(crate) fn read_max_chars() -> usize {
    gsearch::config::load().read_max_chars.unwrap_or(READ_BODY_MAX_CHARS)
}

/// ①打回轮1：GitHub issues/PR 页主评论容器在 DOM 尾部（head/nav/SVG sprite 占掉前几十万
/// 字符），先 cap 后抽会把正文全裁掉（实测 omitted=589331 / summary 空）。github.com 的
/// issues|pull URL 先抽容器再过同一字符预算；其余 URL 与容器未命中路径行为逐字节不变。
pub(crate) fn cap_extract_source(
    html_full: &str,
    limit: usize,
    url: &str,
) -> (String, bool, usize, usize) {
    let lower = url.to_ascii_lowercase();
    let is_github_thread =
        lower.contains("github.com/") && (lower.contains("/issues/") || lower.contains("/pull/"));
    if is_github_thread
        && let Some(container) = gsearch::skeleton::github_comment_html(html_full)
    {
        return cap_chars(&container, limit);
    }
    cap_chars(html_full, limit)
}

/// 2se：先截后提取 + head/nav 重站尾部补救。GitHub 外 URL 的 head/nav/SVG sprite 占满
/// 截断预算时首个正文节点被吃光（wikipedia WWII 实测首个 <p> 在 byte 164035）→
/// summary 全空、rc=0 只剩 meta.truncated。summary 空 + truncated + omitted 超阈值时，
/// 从源 HTML 尾部取同预算的第二片窗口重跑 extract_adaptive（内存有界，勿全量建树）；
/// 仍空维持第一片结果（hint 链照旧）。truncated/omitted 语义保持诚实：始终反映第一片
/// 预算截断，不因补救改写。原本输出非空的页面不进补救分支，stdout 逐字节不变。
pub(crate) fn extract_adaptive_capped(
    html_full: &str,
    limit: usize,
    url: &str,
    excerpt: Option<usize>,
) -> (gsearch::skeleton::AdaptiveRead, bool, usize, usize) {
    let (html, truncated, omitted, truncated_at_offset) = cap_extract_source(html_full, limit, url);
    let mut read = extract_adaptive(&html, excerpt);
    const TAIL_FALLBACK_MIN_OMITTED: usize = 10_000;
    if truncated && omitted > TAIL_FALLBACK_MIN_OMITTED && read.summary_paragraphs.is_empty() {
        // 第一片 = 源前 limit 字符（cap_chars 语义），总字符数 = limit + omitted；
        // 第二片窗口 = 源尾部同预算字符段（起点 = total-limit = omitted），
        // char→byte 换算后切片（不劈 UTF-8）
        let tail_start = char_to_byte_offset(html_full, omitted);
        let (tail_html, _, _, _) = cap_chars(&html_full[tail_start..], limit);
        let retry = extract_adaptive(&tail_html, excerpt);
        if !retry.summary_paragraphs.is_empty() {
            read = retry;
        }
    }
    (read, truncated, omitted, truncated_at_offset)
}

/// 字符级硬截断（按 chars 计，不劈 UTF-8）。返回 (截后文本, 是否截断, 省略字符数, 截断字节偏移)。
/// 9jx：第四个值是截断点在源里的 byte offset（meta.truncated_at_offset 用）；
/// 未截断时 offset=0 让缺席语义（0=缺席）自然生效。
pub(crate) fn cap_chars(s: &str, limit: usize) -> (String, bool, usize, usize) {
    let total = s.chars().count();
    if total <= limit {
        return (s.to_string(), false, 0, 0);
    }
    let byte_offset = char_to_byte_offset(s, limit);
    (s.chars().take(limit).collect(), true, total - limit, byte_offset)
}

/// JSON 感知 cap_chars：若文本以 `{` 或 `[` 开头且截断点不在字符串内，回退到
/// 最后一个完整的 `}`/`]` 边界——避免 json.loads 拿到半截 JSON 报
/// UnclosedBraceError（G 盲测八实锤：GitHub API list JSON 按字节截断后
/// json.loads 失败）。非 JSON 形态（不以 `{`/`[` 起首）走原 cap_chars，行为不变。
/// 边界找不到（截断发生在字符串字面量中）回退硬截断——避免返回合法但语义错乱。
/// 9jx：第四个值是截断点在源里的 byte offset（meta.truncated_at_offset 用）。
pub(crate) fn cap_chars_json(s: &str, limit: usize) -> (String, bool, usize, usize) {
    let trimmed = s.trim_start();
    let offset = s.len() - trimmed.len();
    let first = trimmed.chars().next();
    if !matches!(first, Some('{') | Some('[')) {
        return cap_chars(s, limit);
    }
    let total = s.chars().count();
    if total <= limit {
        return (s.to_string(), false, 0, 0);
    }
    // 取前 limit 字符，按 char 索引转 byte offset
    let byte_limit = char_to_byte_offset(s, limit);
    // 在 [0..byte_limit] 范围内找一个 brace 边界（`}` 或 `]`），不在字符串/转义内
    if let Some(end) = last_brace_boundary(trimmed, byte_limit - offset) {
        let truncated_str = format!("{}{}", &s[..offset], &trimmed[..end]);
        let truncated_chars = truncated_str.chars().count();
        let truncated_byte_offset = offset + end;
        return (truncated_str, true, total - truncated_chars, truncated_byte_offset);
    }
    // 兜底：硬截断（截断点在字符串字面量里也走硬截，不返回坏 JSON）
    cap_chars(s, limit)
}

/// 第 n 个 UTF-8 char 之后的 byte 偏移（n 字符对应 byte 长度）。
fn char_to_byte_offset(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map(|(b, _)| b).unwrap_or(s.len())
}

/// 在字符串前 byte_len 字节范围内找**最后一个** `}` 或 `]` 边界（不在字符串字面量
/// 或转义序列内）。返回 byte offset（相对 trimmed）；未找到返回 None。
///
/// 简化为「最后一个闭括号」语义（G 盲测八修复点名）：在 JSON list 场景下虽不能
/// 给出 fully-valid JSON（外层 `[` 仍未闭合），但至少比硬截到中段少"半截对象"的痛点
/// ——agent 拿到末尾 `}` 至少能识别到最后一个完整对象停在哪。fallback 兜底走 cap_chars。
fn last_brace_boundary(s: &str, byte_len: usize) -> Option<usize> {
    let scan_end = byte_len.min(s.len());
    let bytes = s.as_bytes();
    let mut in_string = false;
    let mut escape = false;
    let mut last_brace: Option<usize> = None;
    for (i, &b) in bytes[..scan_end].iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'}' | b']' => last_brace = Some(i + 1), // 闭括号位置 = i+1（slice 终点）
            _ => {}
        }
    }
    last_brace
}

/// FixG20 OO：summary_paragraphs 逐字重复检测（trim 后完全相同归一组）。
/// 返回重复组，每组 ≥2 个 0-based 下标、按首现顺序；无重复返回空Vec。
fn dup_paragraph_groups(paragraphs: &[String]) -> Vec<Vec<usize>> {
    use std::collections::HashMap;
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut first_seen: HashMap<&str, usize> = HashMap::new();
    for (i, p) in paragraphs.iter().enumerate() {
        let key = p.trim();
        match first_seen.get(key) {
            Some(&g) => groups[g].push(i),
            None => {
                first_seen.insert(key, groups.len());
                groups.push(vec![i]);
            }
        }
    }
    groups.into_iter().filter(|g| g.len() > 1).collect()
}

/// AdaptiveRead → 输出串。--json 在序列化对象末尾注入 meta（网页正文进 agent 上下文
/// = 注入面，正文永远是数据非指令，content_untrusted 恒在）；文本模式截断时 eprintln 提醒
/// （stdout 保持可解析，stderr 承载告警）。
/// 8lp/e19：缺席=正常——truncated=false / omitted=0 不占键；headings 截断时 meta 附标记。
/// 9jx：truncated_at_offset > 0 时 meta 附 `truncated_at_offset` 键（截断字节位置）；
/// 未截断/未注入偏移时键缺席（offset=0=正常态）。
// 参数按调用方语义分组传递（截断三联：truncated/omitted/truncated_at_offset），无 helper 简化空间。
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_read(
    read: &gsearch::skeleton::AdaptiveRead,
    json: bool,
    headings_only: bool,
    from: usize,
    truncated: bool,
    omitted: usize,
    truncated_at_offset: usize,
    headings_truncated: bool,
) -> String {
    // ①打回轮1通用保底：截断发生了却一无所获（GitHub 类页正文在 DOM 尾部被 cap 吃光）→
    // stderr 一行出口指引（stderr 不污染 stdout JSON 契约）。headings-only 摘要本就为空，不适用。
    if !headings_only && read.summary_paragraphs.is_empty() && omitted > 0 {
        eprintln!("[hint] 正文提取为空（omitted {omitted} 字符），试 --markdown 或 --full");
    }
    if json {
        // b95：headings-only 的 JSON 只带标题数组——段落索引/char_count/摘要不进输出
        // （「只要标题」反而更贵的根因：旧 json 分支从未看 headings_only，全量序列化 AdaptiveRead）。
        let mut v = if headings_only {
            serde_json::json!({
                "url": read.url,
                "title": read.title,
                "headings": read.headings,
            })
        } else {
            match serde_json::to_value(read) {
                Ok(v) => v,
                Err(e) => return format!("{{\"error\": \"{e}\"}}"),
            }
        };
        if let Some(obj) = v.as_object_mut() {
            let mut meta = serde_json::Map::new();
            if truncated {
                meta.insert("truncated".into(), serde_json::Value::Bool(true));
            }
            if omitted > 0 {
                meta.insert("omitted".into(), serde_json::json!(omitted));
            }
            if truncated_at_offset > 0 {
                meta.insert(
                    "truncated_at_offset".into(),
                    serde_json::json!(truncated_at_offset),
                );
            }
            meta.insert("content_untrusted".into(), serde_json::Value::Bool(true));
            if headings_truncated {
                meta.insert("headings_truncated".into(), serde_json::Value::Bool(true));
            }
            if !headings_only {
                // FixG20 OO：引用块展平产生的逐字重复段标注（段落文本不动，保逐字引用能力）。
                let dup = dup_paragraph_groups(&read.summary_paragraphs);
                if !dup.is_empty() {
                    meta.insert("dup_paragraphs".into(), serde_json::json!(dup));
                }
            }
            obj.insert("meta".into(), serde_json::Value::Object(meta));
        }
        v.to_string()
    } else {
        if truncated {
            eprintln!("注意：正文超上限已截断（省略 {omitted} 字符；--json 输出在 meta 字段标注）");
        }
        if headings_only {
            format_headings_only(read)
        } else {
            format_adaptive(read, from)
        }
    }
}

/// `--read N` 选项集。M9：默认 AdaptiveRead，`--full/--json/--headings-only/--from K` 互斥选择。
#[derive(Debug, Clone, Default)]
pub struct ReadOpts {
    pub full: bool,
    pub json: bool,
    pub headings_only: bool,
    pub from: usize,
    /// e1i：Some(N) = paragraph_index 每项附前 N 字符 excerpt（--json 生效）；None 行为不变。
    pub excerpt: Option<usize>,
    /// 4bq M3：结果集 URL 来自搜索/抓取结果 = 不可信——open_page 前过 ensure_browsable_url
    /// 私网门（与 browse 初始门对齐）；--allow-private（search 子命令）透传放行。
    pub allow_private: bool,
}

/// read --json 的 headings 载荷上限（超过截断，meta.headings_truncated 标记）。
const HEADING_JSON_LIMIT: usize = 30;

/// paragraph_index 默认剔除已进摘要的段落——摘要段全文已在 summary_paragraphs，
/// pi 再列 first_sentence 是同载荷重复（~944B/page）。摘要 = 文档序前 N 个非空段
/// （skeleton 自适应规则）；pi 中 char_count==0 的空段无重复载荷，保留对齐 --from K 段号。
fn drop_summarized_pi(read: &mut gsearch::skeleton::AdaptiveRead) {
    let summarized = read.summary_paragraphs.len();
    let mut nonempty_seen = 0usize;
    read.paragraph_index.retain(|p| {
        if p.char_count == 0 {
            return true;
        }
        nonempty_seen += 1;
        nonempty_seen > summarized
    });
}

/// `--read N`：M9 默认走 AdaptiveRead（按文章结构自适应）。opts 见 ReadOpts。
/// html 先过 read_max_chars 硬截断；--json 在 meta 字段标注 truncated/omitted/content_untrusted。
pub async fn read(
    browser: &mut Browser,
    h_slot: &mut Option<tokio::task::JoinHandle<()>>,
    results: &[SearchResult],
    n: usize,
    opts: &ReadOpts,
) -> Result<String> {
    let url = pick(results, n, "read")?;
    // c5f：正文复用 open_page 的 captcha 检查取，不再二次 content_retry。
    let (page, snap, html_full) = open_page(browser, h_slot, url, opts.allow_private).await?;
    let title = match &snap {
        Some(s) => s.title.clone(),
        None => eval_string_retry(&page, "document.title").await,
    };
    let (mut read, truncated, omitted, truncated_at_offset) =
        extract_adaptive_capped(&html_full, read_max_chars(), url, opts.excerpt);
    read.url = url.to_string();
    read.title = title;

    // e19：JSON 默认 pi 只列未摘要段（--excerpt 场景恢复全量，保 e1i 契约）；
    // headings >30 截断并给 meta 标记。headings-only 分支不序列化 pi，不在此列。
    let mut headings_truncated = false;
    if opts.json && !opts.headings_only {
        if opts.excerpt.is_none() {
            drop_summarized_pi(&mut read);
        }
        if read.headings.len() > HEADING_JSON_LIMIT {
            read.headings.truncate(HEADING_JSON_LIMIT);
            headings_truncated = true;
        }
    }

    let out = render_read(
        &read,
        opts.json,
        opts.headings_only,
        opts.from,
        truncated,
        omitted,
        truncated_at_offset,
        headings_truncated,
    );
    // 0mf：--json 时不再直接打印（envelope 先打 + read JSON 追加 = 两段拼接破坏 json.loads），
    // 串由 cmd_search 装配进单一 JSON 文档后输出；文本模式照旧。
    if !opts.json {
        println!("{out}");
    }
    Ok(out)
}

/// `--read N --full` 兜底：纯 innerText。--json（0mf）时静默返回正文串，由 cmd_search
/// 装配进单一 JSON 文档；文本模式照旧打印 `=== url ===` 头。
pub async fn read_full(
    browser: &mut Browser,
    h_slot: &mut Option<tokio::task::JoinHandle<()>>,
    results: &[SearchResult],
    n: usize,
    opts: &ReadOpts,
) -> Result<String> {
    let url = pick(results, n, "read")?;
    let (page, _, _) = open_page(browser, h_slot, url, opts.allow_private).await?;
    let (txt, _, _, _) = read_full_text(&page, read_max_chars()).await?;
    if !opts.json {
        println!("=== {url} ===\n{txt}");
    }
    Ok(txt)
}

/// innerText 截断 + 截断标注（fve：cap 与 AdaptiveRead 同一 READ_BODY_MAX_CHARS 上限；
/// browse --full --json 在 meta.truncated 照标；n76：上限由调用方注入——browse 走 --max-chars，
/// read_full 保持 read_max_chars() 配置语义）。read_full / general::cmd_browse 共用。
/// 9jx：第四个返回值为截断字节偏移（meta.truncated_at_offset 用）。
pub(crate) async fn read_full_text(
    page: &chromiumoxide::Page,
    limit: usize,
) -> Result<(String, bool, usize, usize)> {
    let txt = eval_string_retry(page, "document.body.innerText").await;
    Ok(cap_chars(&txt, limit))
}

/// M4 健壮性修复：打开结果页并等语义定稿——read / read_full 共用的 goto 前置。
/// 知乎等站 goto resolve 后仍会内部跳转（登录墙/风控重定向），旧 JS context 被销毁，
/// 紧跟的 content()/evaluate 会撞 CDP -32000 "Cannot find context"（此前被 unwrap_or_default
/// 吞成空串 → 正文为空）。j44：等稳定不再看 readyState==complete（风控限流页秒 complete 但
/// 内容是拦截体），改轮询原子快照语义 marker 连续两次相同（wait_content_stable，约 10s 上限），
/// evaluate 类错误（含 -32000）视为未就绪继续轮询。
///
/// M17 登录墙（Part 2）：检测到登录墙 → eprintln 提示 → swap_to_headed 弹有头窗
/// 等人工登录（复用 general::login_poll_decision 决策，180s 超时）→ 登录成功重抓正文。
/// cookie 随 profile 落盘，下次无感。人关窗 = browser 已死 → 重起 headless 再试一次；
/// 重抓仍撞墙/CAPTCHA 报错退出（限一次登录机会，防循环弹窗）。
/// 返回 (page, 定稿快照, 正文 HTML)——正文是 captcha 检查时取的那份（c5f：调用方 read
/// 直接复用，不再二次 content_retry，省一次 content 往返）；快照 None（窗口内 evaluate 全败）
/// 时 title/正文特征退化为空，登录墙判定只剩 URL 路径特征（与旧实现 evaluate 全败时的
/// 可观测行为一致）。
async fn open_page(
    browser: &mut Browser,
    h_slot: &mut Option<tokio::task::JoinHandle<()>>,
    url: &str,
    allow_private: bool,
) -> Result<(chromiumoxide::Page, Option<PageSnapshot>, String)> {
    // 4bq M3：URL 来自搜索/抓取结果集 = 不可信——与 browse 初始门对齐（scheme 白名单
    // + 私网门），防结果集里的 SEO 恶意页把内网页面渲染后全文打进 agent 上下文。
    let url: &str = &crate::general::ensure_browsable_url(url, allow_private)?;
    let (page, snap) = goto_page(browser, url).await?;
    let html_full = content_retry(&page).await;
    if is_captcha(&html_full) {
        return Err(anyhow!("{url} 遇 CAPTCHA，M4 后处理不支持人解，请重试或手动浏览器打开"));
    }
    if !login_wall_hit_page(&page, &snap).await {
        return Ok((page, snap, html_full));
    }

    eprintln!(
        "检测到登录墙（{url}），弹出有头窗口请登录，登录后自动继续（最长 {LOGIN_WALL_TIMEOUT_SECS}s；cookie 落 profile，下次无感）"
    );
    gsearch::browser::swap_to_headed(browser, h_slot).await?;
    wait_login(browser, url).await?;
    // 登录完成（URL 变化）或用户关窗。关窗路径 browser 已死：重起 headless 重抓。
    if !browser_alive(browser).await {
        let (b, handler) = gsearch::browser::launch(true).await?;
        *h_slot = Some(gsearch::browser::spawn_handler(handler));
        *browser = b;
    }
    let (page, snap) = goto_page(browser, url).await?;
    let html_full = content_retry(&page).await;
    if is_captcha(&html_full) {
        return Err(anyhow!("{url} 遇 CAPTCHA，请重试或手动浏览器打开"));
    }
    if login_wall_hit_page(&page, &snap).await {
        return Err(anyhow!("登录后重抓 {url} 仍遇登录墙（登录未生效？）；可先 `gsearch login <url>` 手动完成登录"));
    }
    Ok((page, snap, html_full))
}

/// new_page（browser::open_page，含 focus emulation）+ goto + 等语义定稿（登录墙重抓路径复用）。
async fn goto_page(
    browser: &Browser,
    url: &str,
) -> Result<(chromiumoxide::Page, Option<PageSnapshot>)> {
    let page = gsearch::browser::open_page(browser).await?;
    tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
        .await
        .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
        .map_err(|e| anyhow!("goto {url} 失败（浏览器被手关？）: {e}"))?;
    let snap = wait_content_stable(&page, 50).await; // 50×200ms ≈ 10s
    Ok((page, snap))
}

/// 登录墙页内判定：最终 URL + 定稿快照的 title/可见正文特征（见 login_wall_hit）。
async fn login_wall_hit_page(page: &chromiumoxide::Page, snap: &Option<PageSnapshot>) -> bool {
    let url = page.url().await.ok().flatten().unwrap_or_default();
    let (title, body, body_chars) = match snap {
        Some(s) => (s.title.as_str(), s.visible_text.as_str(), s.visible_text.chars().count()),
        // 快照全败 → 只剩 URL 路径特征（与旧实现 evaluate 全败时行为一致）
        None => ("", "", 0),
    };
    login_wall_hit(&url, title, body, body_chars)
}

/// M17 登录墙判定（契约特征，纯函数可单测）：
/// 1. 最终 URL 路径段精确命中 signin/signup/login 等登录跳转（知乎 /signin、GitHub /login）；
/// 2. 或 title/DOM 命中「登录/sign in/log in」且正文极短——SPA 型墙 URL 不变的兜底，
///    也覆盖知乎 IP 风控页（正文是含「登录后…反馈」的短 JSON 错误体；带登录态访问即解，
///    所以弹窗登录是正确动作而非误伤）。
///   * ponytail: 路径段精确匹配而非全文 contains——防「文章标题/路径含 login」误伤正文页；
///   * 正文长度门槛 <400 字挡住「正文提到登录」的正常文章。不做站点特征库（Non-goal）。
fn login_wall_hit(url: &str, title: &str, body: &str, body_chars: usize) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let path_hit = path
        .split('/')
        .any(|seg| matches!(seg, "login" | "signin" | "signup" | "log-in" | "sign-in" | "accounts"));
    if path_hit {
        return true;
    }
    let t = title.to_lowercase();
    let b = body.to_lowercase();
    let text_hit = ["登录", "sign in", "log in"].iter().any(|p| t.contains(p) || b.contains(p));
    text_hit && body_chars < LOGIN_WALL_SHORT_BODY
}

/// 决策函数与 general::cmd_login 同源（URL 变化 = 登录完成跳转；evaluate 死 + page 消失 =
/// 用户关窗 = 完成），语义对齐其 login_poll_decision_* 单测。差异仅两点：
/// 180s deadline（顶层命令无人守窗）+ 完成后不退出进程而是返回重抓。
async fn wait_login(browser: &Browser, url: &str) -> Result<()> {
    let (page, _) = goto_page(browser, url).await?;
    let initial_url = page
        .url()
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| url.to_string());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(LOGIN_WALL_TIMEOUT_SECS);
    loop {
        tokio::time::sleep(Duration::from_secs(LOGIN_POLL_SECS)).await;
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "登录等待超时（{LOGIN_WALL_TIMEOUT_SECS}s）: {url}；可先 `gsearch login <url>` 完成登录后重试"
            ));
        }
        let evaluate_ok = page.evaluate("1").await.is_ok();
        let current_url = page.url().await.ok().flatten().unwrap_or_default();
        let attached = browser_alive(browser).await
            && browser
                .pages()
                .await
                .map(|ps| ps.iter().any(|p| p.target_id() == page.target_id()))
                .unwrap_or(false);
        if login_poll_decision(evaluate_ok, &initial_url, &current_url, attached) {
            return Ok(());
        }
    }
}

/// 轮询 document.readyState 到 complete；evaluate 报错（context 重建中的 -32000 等）视为未就绪。
/// 仅作 content_retry / eval_string_retry 的 -32000 恢复退避；正文定稿判定走 wait_content_stable
/// （j44：readyState==complete 对风控限流页不可信）。
async fn wait_dom_complete(page: &chromiumoxide::Page, rounds: usize) {
    for _ in 0..rounds {
        let ok = page
            .evaluate("document.readyState")
            .await
            .ok()
            .and_then(|v| v.into_value::<String>().ok())
            .is_some_and(|s| s == "complete");
        if ok {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// vag：CDP 往返（content/evaluate）统一包 PAGE_TIMEOUT_SECS 预算——chromiumoxide 的
/// evaluate/content future 在 context 销毁/渲染器挂死时可能永不 resolve，拖死整条链。
/// 超时折叠进既有错误语义：retry 路径按「未就绪」重试，fetch_in_page 按「下载失败」上抛。
/// 泛型 future 以便单测注入 pending() 打超时分支，无需真浏览器（同 goto_for_download 理由）。
pub(crate) async fn cdp_timeout<F, T, E>(fut: F, what: &str) -> Result<T>
where
    F: Future<Output = std::result::Result<T, E>>,
    E: std::fmt::Display,
{
    match tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), fut).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(anyhow!("{what} 失败: {e}")),
        Err(_) => Err(anyhow!("{what} 超时（{PAGE_TIMEOUT_SECS}s 未返回）")),
    }
}

/// page.content() 带 -32000 容错：context 重建中等 DOM 稳定后重取，至多 3 次。
/// pub(crate)：general::cmd_browse 复用，避免直接 content() 撞 -32000 取空。
pub(crate) async fn content_retry(page: &chromiumoxide::Page) -> String {
    for _ in 0..3 {
        match cdp_timeout(page.content(), "page.content").await {
            Ok(c) => return c,
            Err(_) => wait_dom_complete(page, 25).await, // ≈5s 内等 context 重建
        }
    }
    String::new()
}

/// page.evaluate(js) → String，带 -32000 容错：等 DOM 稳定后重试，至多 3 次。
/// pub(crate)：postproc title 兜底 / read_full_inner / general::cmd_browse title 兜底共用。
pub(crate) async fn eval_string_retry(page: &chromiumoxide::Page, js: &str) -> String {
    for _ in 0..3 {
        match cdp_timeout(page.evaluate(js), "evaluate").await {
            Ok(v) => match v.into_value::<String>() {
                Ok(s) => return s,
                Err(_) => wait_dom_complete(page, 25).await,
            },
            Err(_) => wait_dom_complete(page, 25).await,
        }
    }
    String::new()
}

/// `--dl N [-o DIR]`：走浏览器 cookie 的简化下载（M13 加 `-o`）。
/// `-o DIR` 把文件落到 DIR 下（按 URL 末段命名）；缺省落 CWD（M4 历史行为）。
/// ponytail: `general::cmd_dl` 已对（同名 `output: Option<&Path>` + `dir.join(filename_from_url(url))`），
/// 此处只把 `postproc::dl` 改一致，不动 general。
pub async fn dl(
    browser: &Browser,
    results: &[SearchResult],
    n: usize,
    output: Option<&Path>,
) -> Result<()> {
    let url = pick(results, n, "dl")?;
    let page = gsearch::browser::open_page(browser).await?;
    // I4：goto 失败/超时 warn 留痕后继续——下载靠后续 fetch 直链兜底（goto 慢的站 fetch 可能可达）。
    let _ = goto_for_download(page.goto(url), url).await;
    let bytes = fetch_in_page(&page, url).await?;
    if bytes.is_empty() {
        return Err(anyhow!("下载内容为空（{url}）"));
    }

    let dir = std::path::absolute(output.unwrap_or_else(|| Path::new(".")))?;
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("创建下载目录 {} 失败: {e}", dir.display()))?;
    let path = dir.join(filename_from_url(url));
    std::fs::write(&path, &bytes).map_err(|e| anyhow!("写文件 {} 失败: {e}", path.display()))?;
    println!("已下载: {} ({} bytes)", path.display(), bytes.len());
    crate::general::pdf_hint(&path);
    Ok(())
}

/// 页内 goto 预算封装（I4，deep-audit §1.2）：dl 链路 goto 失败/超时不再静默——
/// warn 留痕（URL + 耗时）后返回 Err。调用方保持兜底语义：goto 未完成仍尝试页内 fetch 直链。
/// 泛型 future 以便单测注入 pending() 打超时分支，无需真浏览器。
pub(crate) async fn goto_for_download<F, T, E>(fut: F, url: &str) -> Result<()>
where
    F: Future<Output = std::result::Result<T, E>>,
    E: std::fmt::Display,
{
    let started = std::time::Instant::now();
    match tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), fut).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => {
            tracing::warn!(
                "dl 导航失败（{url}，{}ms）: {e} —— 继续尝试页内 fetch 直链",
                started.elapsed().as_millis()
            );
            Err(anyhow!("dl 导航失败（{url}）: {e}"))
        }
        Err(_) => {
            tracing::warn!(
                "dl 导航超时（{url}，{:.1}s > {PAGE_TIMEOUT_SECS}s 预算）—— 继续尝试页内 fetch 直链",
                started.elapsed().as_secs_f64()
            );
            Err(anyhow!("dl 导航超时（{url}）: 超过 {PAGE_TIMEOUT_SECS}s"))
        }
    }
}

/// 页内 fetch 直链 → base64 回传 → Rust 解码。三条 dl 路径共用同一实现
/// （general::cmd_dl 兜底 / postproc::dl / shell::dl_in_page，I5）。
/// I5 通道选择：保留 base64（1.33x 文本膨胀）而非 JSON 字节数组——数组对二进制是
/// ~3.9x 文本（"255," 4 字符/字节）+ V8 number 数组 8B/元素，50MB 文件反而放大峰值；
/// 改为 JS 侧 32MB 拒绝阈值封顶内存（超限先拒，不白编码再传），报错引导走原生下载路径。
pub(crate) async fn fetch_in_page(page: &chromiumoxide::Page, url: &str) -> Result<Vec<u8>> {
    let js = format!(
        "async function () {{
            const r = await fetch({}, {{credentials: 'include'}});
            if (!r.ok) throw new Error('HTTP ' + r.status);
            const buf = await r.arrayBuffer();
            if (buf.byteLength > {FETCH_IN_PAGE_MAX_BYTES}) throw new Error('文件超过页内 fetch 上限 {FETCH_IN_PAGE_MAX_BYTES} bytes（实际 ' + buf.byteLength + '），改用可触发原生下载的直链');
            const u8 = new Uint8Array(buf);
            let s = '';
            for (const x of u8) s += String.fromCharCode(x);
            return btoa(s);
        }}",
        serde_json::to_string(url)?
    );
    let raw = cdp_timeout(page.evaluate(js), "页内 fetch")
        .await
        .map_err(|e| anyhow!("页内 fetch 失败（{url}）: {e}（同源 fetch 受 CORS 限制，未放行的站会在此报错）"))?;
    let b64 = raw
        .into_value::<String>()
        .map_err(|e| anyhow!("fetch 返回值非字符串（{url}）: {e}"))?;
    // 4bq M4：32MB 上限原只在 JS 侧——同上下文页面 JS 可 patch btoa/返回值绕过，
    // Rust 侧收货复核 base64 长度（明文 N 字节 ≈ 4/3·N + padding），超限拒绝解码。
    if b64.len() > FETCH_IN_PAGE_MAX_BYTES / 3 * 4 + 4 {
        anyhow::bail!(
            "页内 fetch 返回数据超限（base64 {} bytes > 上限 {}，约 {}MB 明文）；页面 JS 可能篡改了返回值，已拒绝（{url}）",
            b64.len(),
            FETCH_IN_PAGE_MAX_BYTES / 3 * 4 + 4,
            FETCH_IN_PAGE_MAX_BYTES / 1024 / 1024
        );
    }
    gsearch::util::b64_decode(&b64).map_err(|e| anyhow!("页内 fetch base64 解码失败（{url}）: {e}"))
}

/// 页内 fetch 单文件上限（I5）：32MB × ~3x 通道峰值 ≈ 100MB 内存封顶。
const FETCH_IN_PAGE_MAX_BYTES: usize = 32 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_bounds() {
        let r = [SearchResult {
            title: "t".into(),
            url: "u".into(),
            snippet: "s".into(),
            score: None,
            domain_class: "other",
        }];
        assert!(pick(&r, 0, "read").is_err());
        assert!(pick(&r, 2, "read").is_err());
        assert_eq!(pick(&r, 1, "read").unwrap(), "u");
        assert!(pick(&[], 1, "read").is_err());
    }

    /// PM 契约：postproc.rs tests 加 skeleton_extract_cases（h1/h2/h3 各提取 + 段数 + first_chars）。
    /// M9 升级为：直接验证 AdaptiveRead 三个核心行为（短文 / 中等 / 长文自适应）。
    /// 测试 HTML 通过 in-memory 字符串驱动，不启 Chrome → CI 安全。
    #[test]
    fn skeleton_extract_cases() {
        use gsearch::skeleton::extract_adaptive;
        let html = r#"<!doctype html><html><head><title>T</title></head><body>
<h1>One</h1>
<p>p1 alpha.</p>
<h2>Two</h2>
<p>p2 bravo charlie delta echo.</p>
<h3>Three</h3>
<p>p3.</p>
</body></html>"#;
        let r = extract_adaptive(html, None);
        assert_eq!(r.headings.len(), 3);
        assert_eq!(r.headings[0].level, 1);
        assert_eq!(r.headings[0].text, "One");
        assert_eq!(r.headings[1].level, 2);
        assert_eq!(r.headings[2].level, 3);
        assert_eq!(r.paragraph_index.len(), 3);
        assert!(r.paragraph_index[0].char_count > 0);
    }

    /// M17 登录墙判定:URL 路径段特征 / title 或 DOM+短正文特征 / 正文页不误伤。
    #[test]
    fn login_wall_hit_cases() {
        // URL 路径段:知乎 /signin、GitHub /login、注册墙
        assert!(login_wall_hit("https://www.zhihu.com/signin?next=%2Fp%2F1", "登录 - 知乎", "", 300));
        assert!(login_wall_hit("https://github.com/login", "Sign in to GitHub", "", 500));
        assert!(login_wall_hit("https://example.com/signup", "注册", "", 100));
        // title 特征 + 正文极短(SPA 型墙,URL 不变)
        assert!(login_wall_hit("https://example.com/article", "请登录后继续", "", 50));
        assert!(login_wall_hit("https://example.com/x", "Sign in required", "", 399));
        // DOM 特征 + 正文极短:知乎 IP 风控页(title 空,body 是含「登录」的短 JSON 错误体)
        assert!(login_wall_hit(
            "https://zhuanlan.zhihu.com/p/405555378",
            "",
            r#"{"error":{"message":"您当前请求存在异常，暂时限制本次访问。……登录后私信知乎小管家反馈。","code":40362}}"#,
            100
        ));
        // 正文页不误伤:title/DOM 撞词但正文长;URL 含 login 但非登录段
        assert!(!login_wall_hit("https://example.com/article", "如何登录的完全指南", "登录教程正文……", 3000));
    }
    /// jp4：cap_chars 字符级硬截断（按 chars 计不劈 UTF-8；未超限零拷贝语义）。
    #[test]
    fn cap_chars_cases() {
        let (s, trunc, omitted, offset) = cap_chars("hello", 10);
        assert!(!trunc && omitted == 0 && s == "hello" && offset == 0);
        let (s, trunc, omitted, offset) = cap_chars("你好世界", 2);
        assert!(trunc && omitted == 2 && s == "你好" && offset == "你好".len());
        let long = "x".repeat(READ_BODY_MAX_CHARS + 1);
        let (s, trunc, omitted, offset) = cap_chars(&long, READ_BODY_MAX_CHARS);
        assert!(trunc && omitted == 1 && s.chars().count() == READ_BODY_MAX_CHARS);
        assert_eq!(offset, READ_BODY_MAX_CHARS);
    }

    /// 9jx：cap_chars 截断字节偏移——ASCII 输入 offset 恰好等于 limit（每字符 1 字节）；
    /// UTF-8 中文 offset 等于已截字符的 byte 总长；未截断 offset=0（缺席语义）。
    #[test]
    fn cap_chars_records_truncation_offset() {
        // ASCII：offset = limit
        let (_s, _t, _o, offset) = cap_chars("abcdefghij", 5);
        assert_eq!(offset, 5);
        // UTF-8：「你好世界」每字 3 字节，截 2 字 = 6 字节
        let (_s, _t, _o, offset) = cap_chars("你好世界", 2);
        assert_eq!(offset, 6, "中文字节偏移按 char→byte 换算");
        // 未截断：offset=0（缺席语义）
        let (_s, _t, _o, offset) = cap_chars("short", 100);
        assert_eq!(offset, 0);
    }

    /// P1 fetch JSON 截断：JSON 形态文本截断时回退到最后一个完整 `}`/`]` 边界。
    /// G 盲测八实锤：GitHub API list JSON 按字节截断后 json.loads UnclosedBraceError。
    /// 简化为「最后一个不在字符串内的闭括号」语义：JSON list 场景下虽不能给出 fully-valid
    /// JSON（外层 `[` 仍未闭合），但比硬截到中段少"半截对象"的痛点——agent 拿到末尾 `}` 至少
    /// 能识别到最后一个完整对象停在哪；后续可换 --max-chars 加预算重取或按对象边界解析。
    #[test]
    fn cap_chars_json_truncates_to_brace_boundary() {
        // 简单对象：截到 8 字符 → `{"a":1,"` 内无右花括号 → 兜底硬截
        let (s, trunc, omitted, _offset) = cap_chars_json(r#"{"a":1,"b":2}"#, 8);
        assert!(trunc, "应截断");
        assert_eq!(s, r#"{"a":1,""#, "8 字符内无右花括号 → 硬截到 limit");
        assert!(omitted > 0);

        // 简单对象：limit=9 拿全（输入 13 字符）
        // 实际 13 字符需要 limit >= 13 才不截
        let (s, trunc, _o, _offset) = cap_chars_json(r#"{"a":1,"b":2}"#, 13);
        assert!(!trunc && s == r#"{"a":1,"b":2}"#, "未超限不截");

        // limit=11 截到 `{"a":1,"b"`；前 11 字符内无右花括号 → 兜底硬截
        let (s, trunc, _o, _offset) = cap_chars_json(r#"{"a":1,"b":2}"#, 11);
        assert!(trunc && s.chars().count() == 11, "无闭括号 → 硬截: {s}");

        // 嵌套对象：最后一个右花括号应被找到
        let json = r#"{"a":{"x":1},"b":2}"#; // len=19；位置 11 是内层 `}`
        // limit=10 截到 `{"a":{"x":` 内无闭括号 → 硬截
        let (s2, t2, _o2, _offset) = cap_chars_json(json, 10);
        assert!(t2 && s2 == r#"{"a":{"x":"#, "无闭括号 → 硬截: {s2}");
        // limit=12 含内层 `}`（位置 11）→ 应回退到 12，结果含闭合 `}`
        let (s4, t4, _o4, offset4) = cap_chars_json(json, 12);
        assert!(t4 && s4.ends_with('}'), "应回退到右花括号: {s4}");
        assert_eq!(s4, r#"{"a":{"x":1}"#, "应得内层完整对象: {s4}");
        assert_eq!(offset4, 12, "brace 边界回退的字节偏移 = 截后文本字节长度");

        // 字符串内的右花括号不算边界（heuristic）
        let json = r#"{"msg":"hi}there","x":1}"#; // len=24
        let (s2, t2, _o2, _offset) = cap_chars_json(json, 5);
        assert!(t2 && s2.chars().count() <= 5, "字符串内无闭括号 → 兜底硬截: {s2}");

        // 数组形态：截到中段无右方括号 → 兜底硬截
        let (s2, t2, _o2, _offset) = cap_chars_json("[1,2,3]", 5);
        assert!(t2 && s2 == "[1,2," && s2.chars().count() == 5, "兜底硬截: {s2}");
        let (s3, t3, _o3, _offset) = cap_chars_json("[1]", 2);
        assert!(t3 && s3 == "[1" && s3.chars().count() == 2, "兜底硬截: {s3}");

        // 未超限：不截断
        let (s, trunc, omitted, offset) = cap_chars_json(r#"{"a":1}"#, 100);
        assert!(!trunc && omitted == 0 && s == r#"{"a":1}"# && offset == 0);

        // 非 JSON 形态（不以 `{`/`[` 起首）走原 cap_chars，行为不变
        let (s, trunc, omitted, _offset) = cap_chars_json("plain text", 5);
        assert!(trunc && s == "plain" && s.chars().count() == 5 && omitted == 5, "非 JSON 走 cap_chars: {s}");

        // G 盲测八实证场景：GitHub API list JSON 按字符截断 → 至少停在最后一个完整对象后。
        let gh = r#"[{"tag":"v1","assets":10},{"tag":"v2","assets":20}]"#;
        // len=52；limit=30 截到第 2 个对象中段；前 30 字符内最后一个 `}` 是位置 24（`assets":10}`）
        let (s4, t4, _o4, _offset) = cap_chars_json(gh, 30);
        assert!(t4 && s4.ends_with('}'), "应停在最后一个完整对象: {s4}");
        assert_eq!(s4, r#"[{"tag":"v1","assets":10}"#, "应得第 1 个完整对象: {s4}");
    }

    /// ①打回轮1：GitHub issues|pull URL 先抽主评论容器再 cap——head 巨页（正文在 DOM 尾部）
    /// 不再被 cap 全裁；非 GitHub URL / 非线程页行为与 cap_chars 逐字节一致。
    #[test]
    fn cap_extract_source_prefers_github_containers() {
        let junk = "x".repeat(5_000);
        let html = format!(
            "<html><head>{junk}</head><body><div class='js-comment-body'><p>real issue body</p></div></body></html>"
        );
        // GitHub 线程 URL：容器命中（head 噪声被剔除，预算内拿到正文）
        let (html2, trunc, _omitted, _off) = cap_extract_source(&html, 200, "https://github.com/tokio-rs/tokio/issues/2782");
        assert!(!html2.contains(&junk), "容器抽取绕过 head 噪声");
        assert!(html2.contains("real issue body"));
        assert!(!trunc || html2.len() <= 200);
        // 同 html 非 GitHub URL：走原 cap_chars（head 噪声在预算内，正文被裁——保底 hint 由 render_read 出）
        let (html3, _, _, _) = cap_extract_source(&html, 200, "https://example.com/page");
        assert!(html3.contains("xxxx"), "原路径仍从 head 开始 cap（行为不变）");
        assert!(!html3.contains("real issue body"), "原路径正文仍被 cap 裁掉（对比组）");
        // GitHub 非 issues|pull 页：不启用容器抽取
        let (html4, _, _, _) = cap_extract_source(&html, 200, "https://github.com/tokio-rs/tokio");
        assert!(!html4.contains("real issue body"), "仓库主页不走容器抽取");
    }

    /// 2se：head/nav 占满截断预算（head>50KB）时正文全空 → 从源尾部第二片窗口补救，
    /// truncated/omitted 语义保持第一片的诚实值。
    #[test]
    fn extract_adaptive_capped_recovers_body_from_tail_window() {
        // head 60KB junk（> 50K 默认预算）+ 正文在文档尾部（wikipedia WWII 同构）
        let html = format!(
            "<html><head><style>{}</style></head><body><article><h1>Tail Body</h1><p>real body text after huge head</p></article></body></html>",
            "x".repeat(60_000)
        );
        let limit = 50_000;
        let (read, truncated, omitted, _off) =
            extract_adaptive_capped(&html, limit, "https://example.com/heavy", None);
        assert!(truncated, "60KB head 超预算必须截断");
        assert!(omitted > 10_000, "omitted 应超补救阈值: {omitted}");
        assert!(!read.summary_paragraphs.is_empty(), "尾部窗口应补救出正文");
        let joined = read.summary_paragraphs.join("\n");
        assert!(joined.contains("real body text"), "正文应含尾部 marker: {joined:?}");
        // meta 语义诚实：补救不改写截断事实
        assert_eq!(read.url, "", "url/title 由调用方补（helper 不越权）");
    }

    /// 2se 对照组：正文在预算内（不截断/正文非空）→ 不进补救分支，行为与直连 cap+extract 一致。
    #[test]
    fn extract_adaptive_capped_matches_plain_path_when_body_present() {
        let html = "<html><body><article><h1>Early</h1><p>body early in page</p></article></body></html>";
        let (read, truncated, omitted, off) =
            extract_adaptive_capped(html, 50_000, "https://example.com/", None);
        assert!(!truncated);
        assert_eq!(omitted, 0);
        assert_eq!(off, 0);
        assert!(!read.summary_paragraphs.is_empty());
    }

    /// jp4：render_read 仅 --json 注入 meta（追加不覆盖既有字段）；文本模式不加任何 JSON 键，
    /// 截断走 eprintln。8lp/e19：truncated=false / omitted=0 缺席；headings_truncated 标记可注入。
    /// 9jx：truncated_at_offset > 0 时 meta 附该键；offset=0 缺席。
    #[test]
    fn render_read_injects_meta_json_only() {
        let html = r#"<html><head><title>T</title></head><body><h1>One</h1><p>p1 alpha.</p></body></html>"#;
        // extract_adaptive 不取 title（url/title 由调用方补，与 skeleton 既有测试同款）
        let mut read = extract_adaptive(html, None);
        read.url = "u".into();
        read.title = "T".into();
        let v: serde_json::Value =
            serde_json::from_str(&render_read(&read, true, false, 0, true, 7, 100, false)).unwrap();
        assert_eq!(v["meta"]["truncated"], true);
        assert_eq!(v["meta"]["omitted"], 7);
        assert_eq!(v["meta"]["truncated_at_offset"], 100, "offset>0 注入键");
        assert_eq!(v["meta"]["content_untrusted"], true);
        assert_eq!(v["title"], "T");
        // 空值缺席：正常态 meta 只剩 content_untrusted 一键
        let v: serde_json::Value =
            serde_json::from_str(&render_read(&read, true, false, 0, false, 0, 0, false)).unwrap();
        assert!(v["meta"].get("truncated").is_none(), "truncated=false 应缺席: {v}");
        assert!(v["meta"].get("omitted").is_none(), "omitted=0 应缺席: {v}");
        assert!(v["meta"].get("truncated_at_offset").is_none(), "offset=0 应缺席: {v}");
        assert_eq!(v["meta"]["content_untrusted"], true);
        let v: serde_json::Value =
            serde_json::from_str(&render_read(&read, true, false, 0, false, 0, 0, true)).unwrap();
        assert_eq!(v["meta"]["headings_truncated"], true, "截断标记可注入: {v}");
        let text = render_read(&read, false, false, 0, false, 0, 0, false);
        assert!(!text.contains("content_untrusted"), "文本模式不应出现 meta: {text}");
    }

    /// FixG20 OO：summary_paragraphs 内逐字重复段（GitHub 引用块展平为与被引评论相同的段）
    /// meta.dup_paragraphs 标注重复组（summary_paragraphs 数组的 0-based 下标组）；无重复键缺席。
    #[test]
    fn render_read_annotates_dup_paragraphs() {
        let html = r#"<html><head><title>T</title></head><body><h1>One</h1>
<p>first comment text.</p>
<p>first comment text.</p>
<p>unique paragraph.</p>
</body></html>"#;
        let mut read = extract_adaptive(html, None);
        read.url = "u".into();
        read.title = "T".into();
        let v: serde_json::Value =
            serde_json::from_str(&render_read(&read, true, false, 0, false, 0, 0, false)).unwrap();
        assert_eq!(v["meta"]["dup_paragraphs"], serde_json::json!([[0, 1]]), "逐字重复段应标注重复组: {v}");
        // 无重复页：键缺席（默认输出结构不变）
        let plain_html = r#"<html><head><title>T</title></head><body><p>alpha.</p><p>beta.</p></body></html>"#;
        let mut plain = extract_adaptive(plain_html, None);
        plain.url = "u".into();
        plain.title = "T".into();
        let v2: serde_json::Value =
            serde_json::from_str(&render_read(&plain, true, false, 0, false, 0, 0, false)).unwrap();
        assert!(v2["meta"].get("dup_paragraphs").is_none(), "无重复不得出键: {v2}");
    }

    /// FixG20 OO：dup_paragraph_groups 单元——三连重复归同组、trim 后相同算重复、
    /// 组间按首现顺序、无重复返回空。
    #[test]
    fn dup_paragraph_groups_triples_trim_and_order() {
        let paras: Vec<String> = ["a", "b", " a ", "b", "a"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            dup_paragraph_groups(&paras),
            vec![vec![0, 2, 4], vec![1, 3]],
            "三连重复归同组，组间按首现顺序"
        );
        let no_dup: Vec<String> = vec!["x".into(), "y".into()];
        assert!(dup_paragraph_groups(&no_dup).is_empty(), "无重复返回空");
    }

    /// e19：pi 默认只列未摘要段（12 段中等文摘 10 段 → pi 剩 2 段，段号 11/12）；
    /// 空段保留占位对齐段号；--excerpt 场景由 read() 调用方跳过过滤（e1i 契约）。
    #[test]
    fn e19_pi_lists_unsummarized_only() {
        let mut html = String::from("<html><body>");
        for i in 1..=12 {
            html.push_str(&format!("<p>para{text}</p>", text = i));
        }
        html.push_str("<p></p></body></html>"); // 空段：无重复载荷，应保留
        let mut read = extract_adaptive(&html, None);
        assert_eq!(read.summary_paragraphs.len(), 10, "10..=50 段摘前 10 段");
        drop_summarized_pi(&mut read);
        assert_eq!(read.paragraph_index.len(), 3, "剩 2 非空未摘要段 + 1 空段");
        assert_eq!(read.paragraph_index[0].index, 11);
        assert_eq!(read.paragraph_index[1].index, 12);
        assert_eq!(read.paragraph_index[2].char_count, 0);
    }

    /// C1：open() 拒绝非 http(s) scheme（防 cmd 注入 + file:/javascript: 兜底打开）。
    #[test]
    fn open_rejects_non_http_scheme() {
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "ftp://example.com/x",
            "data:text/html,hi",
            "  javascript:alert(1)",
        ] {
            assert!(!is_http_url(bad), "应拒绝: {bad}");
        }
        for ok in [
            "http://example.com",
            "https://example.com/path?q=1",
            "HTTPS://EXAMPLE.COM",
            "  https://x.com/a",
        ] {
            assert!(is_http_url(ok), "应放行: {ok}");
        }
    }

    /// I4 回归：goto_for_download 超时分支返回 Err 且文本带 URL（start_paused 下时间自动推进，
    /// pending future 永不完成 → 必超时，无需真浏览器）。
    #[tokio::test(start_paused = true)]
    async fn goto_for_download_timeout_carries_url() {
        let err = goto_for_download(std::future::pending::<std::result::Result<(), std::convert::Infallible>>(), "https://example.com/slow")
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("https://example.com/slow"), "Err 缺 URL: {msg}");
        assert!(msg.contains("超时"), "Err 未标超时: {msg}");
    }

    /// vag 回归：cdp_timeout 超时分支——pending future 在 start_paused 下时间自动推进必超时，
    /// Err 标超时；正常完成值透传。无需真浏览器。
    #[tokio::test(start_paused = true)]
    async fn cdp_timeout_pending_future_times_out() {
        let err = cdp_timeout(
            std::future::pending::<std::result::Result<(), std::convert::Infallible>>(),
            "evaluate",
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("超时"), "Err 未标超时: {msg}");
        assert!(msg.contains("evaluate"), "Err 缺操作名: {msg}");

        let v = cdp_timeout(async { Ok::<_, std::convert::Infallible>(7usize) }, "evaluate")
            .await
            .unwrap();
        assert_eq!(v, 7, "成功值应透传");
    }

}

#[cfg(test)]
mod live_tests {
    //! 免 Google 集成测试。
    //! 起真 Chrome 且独占 profile：并行会因 profile 锁竞态挂（ExitStatus(21)），
    //! 必须串行跑 `cargo test -- --test-threads=1`（CI 已 skip live，不受影响）。

    use super::*;

    fn fixture() -> Vec<SearchResult> {
        vec![SearchResult {
            title: "t".into(),
            url: "https://example.com/".into(),
            snippet: "s".into(),
            score: None,
            domain_class: "other",
        }]
    }

    /// 单一 async 测试独占 profile 锁，避免并行启两个 Chrome 抢锁。
    /// #[ignore]：真启 Chrome + 访问外网，属手工验证测试（cargo test --ignored 跑）。
    /// 之前混进默认 cargo test：真 Chrome 不 close 造成进程残留。
    #[ignore]
    #[tokio::test]
    async fn postproc_live() {
        let (mut browser, handler) = gsearch::browser::launch(true).await.unwrap();
        let mut h_slot = Some(gsearch::browser::spawn_handler(handler));
        let results = fixture();

        let err = read(&mut browser, &mut h_slot, &[], 1, &ReadOpts::default()).await.unwrap_err();
        assert!(err.to_string().contains("越界"), "got: {err}");

        let txt = read(&mut browser, &mut h_slot, &results, 1, &ReadOpts::default()).await.unwrap();
        assert!(txt.contains("[目录]"), "default read missing [目录]: {txt:?}");
        assert!(txt.contains("[摘要"), "default read missing [摘要]: {txt:?}");
        assert!(txt.contains("Example Domain"), "default read got: {txt:?}");

        let txt = read_full(&mut browser, &mut h_slot, &results, 1, &ReadOpts::default()).await.unwrap();
        assert!(txt.contains("Example Domain"), "read_full got: {txt:?}");

        dl(&browser, &results, 1, None).await.unwrap();
        let bytes = std::fs::read("download.bin").unwrap();
        assert!(!bytes.is_empty());
        assert!(
            bytes.windows(14).any(|w| w == b"Example Domain"),
            "dl content head: {}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
        );
        std::fs::remove_file("download.bin").unwrap();
    }

    /// 直接断言 explorer.exe / open / xdg-open 路径——Windows 走 explorer.exe 单参数，
    /// macOS/Linux 走 open / xdg-open 单参数，都不再经 cmd 解析（防元字符注入）。
    /// #[ignore]：真开默认浏览器窗口（每次 cargo test --ignored 弹 example.com 就是它），
    /// 属手工验证测试。CI matrix 编译覆盖了 cfg 分支。
    #[ignore]
    #[cfg(windows)]
    #[test]
    fn open_mechanism_explorer() {
        // 不实际 explorer.exe spawn——explorer.exe 是 GUI 进程，不会同步结束。
        // 直接断言 open() 的 scheme 校验 + 平台分支构造（explorer.exe / open / xdg-open）。
        assert!(open(&fixture(), 1).is_ok(), "https URL 应被放行并交给 explorer.exe");
        // 越界仍按既有逻辑报错
        assert!(open(&[], 1).is_err(), "越界应报错");
    }
}