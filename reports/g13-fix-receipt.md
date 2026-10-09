# FixG13 修复回执（2026-10-09）

基线 main=29862a4（FixG12 合并）。修盲测十二 3 扣分（R -1 / P -0.5 / Q -0.5）。

## 修 1：markdown/text 转换器剥代码缩进（R -1 Token）

**根因（实测三处同根）**：
- `where\n    T:` 缩进剥掉：`tree_text` 输出经 `collapse_blank` 全文空白折叠，pre/code 内缩进被当噪音
- `from_ str` 游离空格：docs.rs h1 `Function <span class="fn">from_<wbr>str</span>`——`<wbr>`（零宽断行点）被当 inline 元素 push(' ') 撕词
- `println! (` 游离空格：`<span class="macro">println!</span>(` 元素闭边界注入空格

**修法**：
- `tree_text`（fetch.rs:1125-1167）：主 DFS 跳过 `wbr`；遇 `pre`/`code` 时子树 Text 原样输出（不递归主栈、不注入元素边界分隔符），前后加 `\u{0}` 哨兵
- 新 `collapse_preserving_code`（fetch.rs:1170-1182）：`\0` 哨兵奇数段（代码）逐字节保留，偶数段（prose）仍走 `collapse_blank`（函数本体未动）
- 调用点 `extract_text` / `process_html` 换用新组合函数；非 HTML 分支不动
- markdown 路径（convert.rs `sanitize_pre_blocks`，盲测七 0bh 已有）零改动——P0-1 实测本就通过

**契约符合性**：`collapse_blank` 行为不变（未改一行）；普通 HTML 文本仍走 collapse_blank；无哨兵时 `collapse_preserving_code` 与 `collapse_blank` 逐字节一致（单测 `collapse_preserving_code_matches_collapse_blank_without_sentinel` 钉死）。

## 修 2：用户 --include 覆盖路径无 nav 剥离（P -0.5 UX）

**根因**：fetch.rs FixG12 段 overridden=true 只标注字段不跑 host 路由；`--include` 未命中回退全文时 nav（"Skip to content"/"Navigation Menu"）混入。

**修法**：新 `apply_host_route_on_include_fallback`（fetch.rs:948-974）——include_hit≠Some(true) 时按 label 调既有 `apply_github_host_route` / `apply_docsrs_host_route`（主体未重写）；applied 后 `include_hit` 复位 Some(false)、`include_hits=None`（命中标志只对应用户 selector）。调用点在 `--include` 段之后（fetch.rs:504-515）；命中用户 selector 时 no-op（用户明确要什么就是什么）。`include_overridden_by_user=true` / `auto_include_applied` 沿用 FixG12 标注不变。

## 修 3：--json-keys 不支持数组索引路径（Q -0.5 Token）

**根因**：`parse_json_path` 把裸 `0` 当 Field，对象查 `"0"` 失败回退原始 4.4KB。

**修法**：fetch.rs:872-878——字段名纯数字 parse 成 usize → `Segment::Index`；非纯数字（`-1`、`0abc`）维持 Field；`[0]` 显式语法与既有路径形态不回归。只补「裸数字段 = Index」这一种形态。

## 测试

- 新单测 6（fetch.rs:2107-2291）：
  1. `extract_text_preserves_pre_code_whitespace`——docs.rs 实测 fixture：where 缩进 / wbr 撕词 / println 边界 / prose 折叠不回归
  2. `collapse_preserving_code_matches_collapse_blank_without_sentinel`——无哨兵等价性（零回归保证）
  3. `collapse_preserving_code_keeps_code_segment_verbatim`——哨兵段逐字节 + 首/中/尾哨兵
  4. `parse_json_path_numeric_segment_becomes_index`——裸 0 / 单段 / `[0]` / 连续下标 / 非纯数字 / 负数 / 空路径
  5. `project_json_paths_supports_top_level_array_index`——GitHub comments 形态投影 + 越界仍报错
  6. `include_miss_fallback_applies_host_route_stripping`——回退剥 nav + include_hit 复位 + 命中 selector no-op
- 全量：lib 121 passed + bin 105 passed = 226（基线 220 + 6 新），0 failed；既有 collapse_blank / tree_text / parse_json_path / project_json_paths 单测零回归
- `cargo clippy --all-targets -- -D warnings`：0 error 0 warning（修掉一处 collapsible_match）
- `Segment` 加 `#[derive(Debug, PartialEq)]`（私有 enum，测试比较用，公共 API 未动）

## E2E（交付前自跑，全部真实命令）

| # | 命令 | 结果 |
|---|---|---|
| P0-1 | `gsearch.exe fetch https://docs.rs/serde_json/latest/serde_json/fn.from_str.html --markdown --json` | text 含 `where\n    T:` ✓；无 `from_ str` ✓；无 `println! (` ✓（1159 字符） |
| P0-2 | `gsearch.exe fetch https://github.com/tokio-rs/tokio/releases --include "user-selector" --json` | 回退头 1KB nav 词零命中 ✓；include_hit=false ✓；include_overridden_by_user=true ✓；auto_include_applied=github ✓ |
| P0-3 | `gsearch.exe fetch https://api.github.com/repos/tokio-rs/tokio/issues/7170/comments --json-keys "0.user.login,0.body" --json` | text 264 字节（原 4.4KB）✓；truncated_by_json_keys=true ✓；stderr 0 字节 ✓ |
| P0-4 | `cargo test --all-targets` + `project_json_paths_filters_to_selected_fields` | 226 passed ✓；点名测试 1 passed ✓ |
| P0-5 | `gsearch.exe fetch https://docs.rs/serde_json/latest/serde_json/fn.from_str.html --json` | 签名行 `pub fn from_str<'a, T>(s: &'a str) -> Result<T>` 无游离空格 ✓；`where\n    T:` 缩进 ✓；h1 `Function from_str` 无撕词 ✓（1022 字符，修前 966） |

## 改动清单

- `src/fetch.rs`：+282/-3（实现 ~90 行 + 新单测 ~190 行 + 注释）
- `README.md`：3 行同步（--include 回退语义 / host 路由行 / --json-keys 裸数字段语法）

## 未动（范围禁区确认）

skeleton.rs / postproc.rs / searxng / search / browse / shell / SSRF / Content-Type / redirect / convert.rs（markdown 路径 P0-1 本就通过，零改动）/ FixG11/12 host 路由主体（只补 overridden 回退分支）

## 残余风险

- `\u{0}` 哨兵依赖页面文本不含真实 NUL（html5ever 把 `&#0;` 解码为 U+FFFD，理论不可达；若上游解析器行为变化需换哨兵方案）
- 纯数字对象键（如 `{"2024": ...}` 路径 `2024.title`）会被解释为 `Index(2024)` 越界报错——契约拍板「裸数字段 = Index」单一形态，对象数字键场景罕见
- 覆盖回退路径上 GitHub 容器链未命中时（releases 页无 markdown-body 类容器）text 仍为未剥 nav 全文——skeleton 主体行为，超出本任务范围
