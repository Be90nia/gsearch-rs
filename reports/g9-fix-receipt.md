# G9 修复收据 (GitHub browse 三扣分)

**VERDICT: PASS (round 2 — fetch 路径 offset 透传补全)**

## 改动
|文件|行|摘要|
|---|---|---|
| src/skeleton.rs | +60 | 新增 `SEL_GITHUB_NEW` 优先级链（markdown-body/issue-body/article/.markdown-body）+ `SEL_GITHUB_NAV` nav 剥离集；重写 `github_comment_html` 走优先级链 + nav 剥离 helper `container_html_skip_nav`；新增 4 单测（react_dom_markdown_body / issue_body_testid / strips_nav_inside_container / falls_back_to_legacy_selectors）。 |
| src/postproc.rs | +50 / -20 | `cap_chars` / `cap_chars_json` 返回 `(String, bool, usize, usize)` 多一截断字节偏移；`cap_extract_source` / `read_full_text` 透传；`render_read` 新参 `truncated_at_offset: usize`，meta JSON 在 offset>0 时注入 `truncated_at_offset` 键；新增 1 单测 `cap_chars_records_truncation_offset`；既有 `cap_chars_cases` / `cap_chars_json_truncates_to_brace_boundary` / `cap_extract_source_prefers_github_containers` / `render_read_injects_meta_json_only` 升级为 4 元组 + offset 断言。 |
| src/types.rs | +7 | `MetaOutput` 新增 `truncated_at_offset: usize`（`is_zero_usize` 缺席=0=未截断），`sample_meta` 测试 fixture 同步。 |
| src/general.rs | +12 / -3 | browse markdown / full / 默认三路 destructure 改 4 元组；`MetaOutput` 构造补 `truncated_at_offset`；render_read 调用补 offset 参数。 |
| src/shell.rs | +9 / -3 | shell `cmd_read` destructure 改 4 元组 + 传 offset 给 render_read。 |
| src/fetch.rs | +28 / -8 | **round 2 补**：Fetched 新增 `truncated_at_offset: Option<usize>` 字段；`fetched_json` 在 Some 时注入 `meta.truncated_at_offset`；markdown / include / JSON 三路 destructure 用 `t.then_some(off)` 把 offset 装进 Fetched；process_html 4 元组 Fetched 构造补字段；既有 3 个 test fixture + 2 新单测（fetched_json_truncated_at_offset_emits_when_truncated / process_html_threads_truncated_at_offset）。 |
| src/main.rs | +6 / -0 | 4 处 MetaOutput 构造补 `truncated_at_offset: 0`（搜索路径无截断语义）。 |
| README.md | +1 / -1 | read/browse 输出契约节补 `truncated_at_offset` 字段说明。 |

## 范围
针对性修盲测九 K 三个扣分点（GitHub 新 React DOM 容器适配 / --markdown 命中 nav / cap 截断 offset 缺失），未触默认行为，非 GitHub URL 不受影响。

## 改动
|文件|行|摘要|
|---|---|---|
| src/skeleton.rs | +60 | 新增 `SEL_GITHUB_NEW` 优先级链（markdown-body/issue-body/article/.markdown-body）+ `SEL_GITHUB_NAV` nav 剥离集；重写 `github_comment_html` 走优先级链 + nav 剥离 helper `container_html_skip_nav`；新增 4 单测（react_dom_markdown_body / issue_body_testid / strips_nav_inside_container / falls_back_to_legacy_selectors）。 |
| src/postproc.rs | +50 / -20 | `cap_chars` / `cap_chars_json` 返回 `(String, bool, usize, usize)` 多一截断字节偏移；`cap_extract_source` / `read_full_text` 透传；`render_read` 新参 `truncated_at_offset: usize`，meta JSON 在 offset>0 时注入 `truncated_at_offset` 键；新增 1 单测 `cap_chars_records_truncation_offset`；既有 `cap_chars_cases` / `cap_chars_json_truncates_to_brace_boundary` / `cap_extract_source_prefers_github_containers` / `render_read_injects_meta_json_only` 升级为 4 元组 + offset 断言。 |
| src/types.rs | +7 | `MetaOutput` 新增 `truncated_at_offset: usize`（`is_zero_usize` 缺席=0=未截断），`sample_meta` 测试 fixture 同步。 |
| src/general.rs | +12 / -3 | browse markdown / full / 默认三路 destructure 改 4 元组；`MetaOutput` 构造补 `truncated_at_offset`；render_read 调用补 offset 参数。 |
| src/shell.rs | +9 / -3 | shell `cmd_read` destructure 改 4 元组 + 传 offset 给 render_read。 |
| src/fetch.rs | +3 / -3 | fetch 三处 destructure 改 4 元组（offset 用 `_off` 丢弃，fetch 不进 meta 也不进 OutputEnvelope）。 |
| src/main.rs | +6 / -0 | 4 处 MetaOutput 构造补 `truncated_at_offset: 0`（搜索路径无截断语义）。 |
| README.md | +1 / -1 | read/browse 输出契约节补 `truncated_at_offset` 字段说明。 |

## 验证
### 全量闸
- `cargo clippy --all-targets -- -D warnings`：**0 error** (Finished dev profile)
- `cargo test --all-targets`：**204 passed / 0 failed / 2 ignored**（lib 121 + bin 83；基线 193 + 新增 11）

### Code-level 测试新增
- `skeleton::tests::github_comment_html_matches_react_dom_markdown_body` (扣分 1)
- `skeleton::tests::github_comment_html_matches_issue_body_testid` (扣分 1)
- `skeleton::tests::github_comment_html_strips_nav_inside_container` (扣分 2)
- `skeleton::tests::github_comment_html_falls_back_to_legacy_selectors` (扣分 1)
- `postproc::tests::cap_chars_records_truncation_offset` (扣分 3)
- `fetch::tests::fetched_json_truncated_at_offset_emits_when_truncated` (扣分 3 — round 2)
- `fetch::tests::process_html_threads_truncated_at_offset` (扣分 3 — round 2)

### 验证命令+关键输出摘录

```
$ cargo clippy --all-targets -- -D warnings
    Checking gsearch v0.2.9 (D:\Project\gsearch-rs)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.56s

$ cargo test --all-targets
    Running unittests src\lib.rs (target\debug\deps\gsearch-67d4107c44eb78cd.exe)
test result: ok. 121 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
    Running unittests src\main.rs (target\debug\deps\gsearch-ec778cfa712209bd.exe)
test result: ok. 87 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out
```

## 关键设计取舍
- **优先级链而非合并**：新选择器命中即用（旧选择器仍存在）；旧 SSR 页（`.js-comment-body`/`.comment-body`）走兜底——盲测九 SSR 时代的兼容路径不破。
- **nav 剥离仅作用于广义容器**：命中 issue-body（专容器）通常无 nav，但函数仍统一调用 nav 剥离（no-op 当无 nav），代码更简单。
- **cap_chars 4 元组 vs 新函数**：选了 4 元组——唯一最小代价 = 11 处 destructure 机械升级；fetch 不进 MetaOutput 所以丢 offset 即可。
- **`render_read` 加 allow(too_many_arguments)**：参数语义独立无 helper 化空间，pub(crate) 内部接口。

## Side-effects (三态必含一项)
- **无外部副作用**：未 commit、未 push、未改默认行为；非 GitHub URL 完全走原 cap_chars 路径，缺席语义保持；非搜索路径（dl / login / search / batch）的 MetaOutput.truncated_at_offset 恒 0 = 缺席，键不出。

## End-to-end (待主代理验证，未跑)
| 验收项 | 待跑命令 | 期望 |
|---|---|---|
| 扣分 1 | `gsearch browse https://github.com/tokio-rs/tokio/issues/8065` | summary_paragraphs 非空 + headings 非空（不再是 298B 空壳） |
| 扣分 2 | `gsearch browse --markdown --max-chars 30000 https://github.com/tokio-rs/tokio/issues/8065` | text 前 1KB 不出现 Platform/Solutions/Resources |
| 扣分 3 | 拉一超长 issue 触发截断 → meta.truncated_at_offset | 非零 offset 值 |

## 残余风险 / 未做
- 无 commit/push（上级负责）
- nav 剥离只针对广义容器命中场景——若 GitHub 未来把 nav 直接放进 issue-body，剥离会无效；当前 DOM 结构无此迹象
- 截断字节偏移对 brace 边界 fallback 路径同样有效，但回退到 brace 时的 offset = 该 brace 的源内 byte offset（已实测 4 元组已涵盖）

## Round 2 补 fetch 路径说明
PM round 1 验证发现 fetch 路径未透传 offset（round 1 在 fetch 用 `_off` 丢弃）。Round 2 修复：
- `Fetched` 新增 `truncated_at_offset: Option<usize>` 字段
- `fetched_json` 在 `Some(off)` 时注入 `meta.truncated_at_offset`（与 read/browse 路径同契约：Some 出键、None 缺席）
- markdown / include / JSON 三路 destructure 改用 `t.then_some(off)` 把 offset 装入 Fetched
- 既有 3 个 test fixture 同步加字段；2 个新单测覆盖 (a) 序列化契约 (b) 端到端 process_html 真实流转
- 全量闸：clippy 0 error；cargo test 208 passed（lib 121 + bin 87，round 1 的 204 + 新增 4）
