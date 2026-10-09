# FixG12 修复回执

日期：2026-10-09 · 基线：main f1c9607（FixG11）· 范围：盲测十一 M-2/N-2/O-2 的 4 项扣分

## 判定：PASS（4 项全修）

## 改动清单

### 1. docs.rs `<summary>` 折叠按钮文本剥离（-0.7）
- `src/fetch.rs` 新增纯函数 `strip_summary_elements`（字符串级扫描，非闭合 summary 不误伤、`<summaryx` 边界放行）；两个接线点：
  - `fetch_one_attempt` 漏斗单点（`is_html` 时先剥再进全部下游提取）；
  - `process_html` 内守门（独立调用路径 + 离线单测锚点，双剥幂等）。
- **e2e 实证发现的坑**：首跑 P0-1 FAIL——`apply_docsrs_host_route` / `apply_github_host_route` 从**原始 html** 重新提取正文并覆盖 `process_html` 输出，剥除只放在 process_html 内会被 host 路由整个绕过。修法即漏斗单点。markdown 模式不受影响（无 summary 注入路径；漏斗剥除对 md 转换同样无害）。非 HTML 源文保真不碰。

### 2. `auto_include_applied` 恒在 + `include_overridden_by_user`（-0.3）
- `Fetched` 新字段 `include_overridden_by_user: bool`；`host_route_applies` → `host_route_decision(Option<&str>, url) -> Option<(label, overridden)>`（host 判定恒保留，覆盖事实单列）。
- 接线：显式 `--include` 时 `auto_include_applied=Some(label)` + `include_overridden_by_user=true`；`--include` 整体重建 Fetched 处透传新字段；`fetched_json` 仅 `true` 出键（默认输出逐键不变）。
- e2e P0-2：`--markdown --json --include "pre"` → `auto_include_applied="docs.rs"` + `include_overridden_by_user=true` + `include_hit=true`（用户 selector 生效）。

### 3. `--help` 行话清理 + 守门测试升级（-0.4）
- main.rs 七处 help 可见 doc comment 重写为"是什么+怎么用"：`--include`（J-3）、`--timeout`（J-1/FixG11/盲测十 P0-3）、`--retry`（J-1/FixG11/盲测十 P0-3）、`--json-keys`（J-2）、`--read`（L-1）、`--browse`（L-1）、`--humanize`（盲测六）。
- 守门测试 `help_text_free_of_internal_issue_codes` 升级两层：① 原有 19 个内部短码；② 新模式 `FixG\d+` / `盲测+序号`（阿拉伯+中文数字）/ `[JKLM]-\d` / `P0-\d`，逐字节窗口扫描不引 regex 依赖；泄漏时报类别+具体行。
- e2e P0-3：`fetch --help` 六类模式全部零命中。

### 4. GitHub PR 标题栏补（-1.2，选 fetch 路径 title 前缀方案）
- `apply_github_host_route` 在容器改写后经 `with_title_prefix(title, body)` 补 `# {页面title}\n\n`（title 前 24 字符在正文前 300 字符判重，已含不重复；空 title 跳过）；前缀在 `cap_chars` 之前，截断统计含 title。
- 不选 skeleton.rs 标题栏 selector 方案的原因：`github_comment_html` 是 fetch/browse 共用，FixG9 已修 browse，动容器链有 browse 双标题风险；title 前缀只作用 fetch 路径的 `auto_include=github` 分支，改动面最小。
- e2e P0-4：`fetch https://github.com/tokio-rs/tokio/pull/7696` → text 头 = `# fs: support io_uring with \`tokio::fs::read\` by Daksh14 · Pull Request #7696 · tokio-rs/tokio · GitHub\n\nMotivation...`。

## 验证（真实命令输出）

```
cargo clippy --all-targets -- -D warnings   → Finished（0 error）
cargo test --all-targets                    → lib 121 passed / bin 99 passed（2 ignored）
                                              合计 220，基线 215，净增 5，0 failed
P0-1 fetch docs.rs fn.from_str.html（默认）  → text 头 300 字符无 "Expand description"，签名行直进描述
P0-2 同 URL --markdown --json --include pre  → auto_include_applied="docs.rs" + include_overridden_by_user=true
P0-3 fetch --help                           → FixG\d+/盲测序号/[JKLM]-\d/P0-\d/旧短码 全零命中
P0-4 fetch github PR #7696                  → text 头含 PR 标题（"fs: support io_uring"）
P0-5 基线回归                                → 220 全绿（215 基线 + 5 新增）
```

e2e 均以 `cargo build` 后的 `target/debug/gsearch.exe`（mtime 18:36 晚于全部源码改动）真实执行，exit=0。

## 新增测试（6 个新测试函数 + 守门升级）

- `fetch.rs::host_route_decision_flags_override_when_user_includes`（取代旧 host_route_applies 测试）
- `fetch.rs::fetched_json_include_overridden_by_user_emits_only_when_true`
- `fetch.rs::process_html_strips_summary_button_text`
- `fetch.rs::strip_summary_elements_edge_cases`（多 summary / 带属性 / `<summaryx` / 未闭合）
- `fetch.rs::process_html_non_html_keeps_summary_source_fidelity`
- `fetch.rs::apply_github_host_route_prefixes_missing_page_title`（含不重复前缀反例）
- `main.rs::help_text_free_of_internal_issue_codes`（升级：模式守门）

既有 fixture 升级：15 处 `Fetched` 字面量补 `include_overridden_by_user: false`；`--include` 重建处透传真实值。

## 文件

| 文件 | + | - |
|---|---|---|
| src/fetch.rs | 230 | 45 |
| src/main.rs | 69 | 24 |
| README.md | 8 | 6 |

注：main.rs 含工作树中既有未提交的 FixG11 改动（timeout/retry 默认值 +18/-8），非本次产生；本次叠加 doc comment 清理 + 守门升级。

## 范围合规

- 未动 SearXNG / Google / browse / shell / SSRF 门 / fetch 网络主体（漏斗剥除是提取前预处理，非 fetch 逻辑改动，为契约第 1 项的必要接线——e2e 实证 process_html 内剥除会被 host 路由绕过）。
- USER_GUIDE.md 无 fetch 功能段（全文仅 1 处顺带提及），不涉及。

## 残余风险

- `strip_summary_elements` 按小写标签名扫描，`<SUMMARY>`（大写）罕见形态不剥（html5ever 认，扫描器不认）；现实页面均为小写，e2e 通过。
- `include_overridden_by_user=true` 时若用户 selector 未命中容器（include_hit=false），label 仍标注——语义为"host 判定被覆盖"，符合契约设计。
- GitHub 标题前缀判重窗口是 title 前 24 字符 vs 正文前 300 字符，超长正文且标题极靠后才可能误判"缺标题"补前缀——GitHub 容器正文从描述起，实际不会命中。
