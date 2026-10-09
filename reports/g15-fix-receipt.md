# FixG15 修复回执

- 日期：2026-10-09
- 修复者：FixG15（子代理）
- 基线：main=545f67c（盲测十四 V 8.5/8.5, W 8.75/8.75, X 7.5/8.5 后的 4 项工具可修扣分）
- 契约：FixG15（4 项全修）

## 改动清单

| 项 | 文件:位置 | 改法 |
|---|---|---|
| 1. --include inner_html 伪影（X -1） | src/fetch.rs `extract_with_include` + 新增 `render_include_blocks`；两个调用点（fetch_one_attempt / apply_docsrs_host_route） | 根因：inner_html 层拼接的 `\n\n---\n\n` 经 `extract_text` 整体重解析后分隔符沦为裸文本节点，被空白规整压成 ` --- ` 胶连、块首缩进漏成前导空格。修法：收集块时每块 trim + CRLF 归 LF；渲染改为**每块独立提取正文再以 `\n\n---\n\n` 拼接**（分块提取是修法本体，仅改 inner_html 层拼接送不进最终 text） |
| 2. `[*]` 数组通配（V -0.75） | src/fetch.rs `Segment` 新变体 `Wildcard`；`parse_json_path` 认 `[*]`；新增 `eval_json_path`/`step_json_path`；`project_json_paths` 逐路径走 eval | `[*].tag_name` 对顶层数组每元素取该字段，输出数组形态 `{"tag_name":[...]}`（59B，原 11.4KB）；key 取通配后末段短名，纯 `[*]` → `"[*]"`；`[*]` 作用于非数组/双 `[*]` → 显式 Err；与 `0`/`[0]`/`.field` 同次调用共存；无通配路径行为逐字节不变 |
| 3. github_comments_hint 自相矛盾（W -0.5） | src/fetch.rs `github_thread_comment_gap` | "评论区由 JS 动态加载，未包含在本输出中" → "评论区仅部分包含在本输出正文中（可能缺作者归属且不完整，勿据本文判断完整讨论）；完整讨论：browse --markdown 或 GET api.github.com/.../comments"。meta 键名（github_comments_missing/hint）与输出结构不动 |
| 4. 数组裸路径报错不教语法（W -0.5） | src/fetch.rs `project_json_body` | 投影失败且根值为数组且路径未用 `[*]` 时，stderr 错误追加："响应为顶层数组：请用 `0.field` 索引语法（如 `0.user.login`）或 `[*]` 通配（如 `[*].tag_name`）"。已用 `[*]` 的失败（元素缺字段）不重复教学（再教 `[*]` 会误导） |

文档：README.md `--include`（渲染层拼接 + 归一说明）与 `--json-keys`（`[*]` 语法 + 数组教学）两条同步。--help（clap/main.rs）按契约禁区未动——本次修复后输出与既有 --help 宣称一致，无需改 help。

## 测试

- 新增 8 个单测（契约要求 ≥6）：
  1. `extract_with_include_normalizes_block_whitespace` — 块首尾 trim + CRLF 归 LF（项 1）
  2. `render_include_blocks_separator_exact_and_trimmed` — 分隔符精确 `\n\n---\n\n`、无前导空白、纯 LF、未选中容器不混入（项 1 渲染层）
  3. `parse_json_path_wildcard_segment` — `[*]` 解析与 Field/Index 并存（项 2）
  4. `project_json_paths_wildcard_projects_all_elements` — 通配投影数组形态 + 纯 `[*]`（项 2）
  5. `project_json_paths_wildcard_coexists_with_index_and_field` — 通配与索引段同次调用共存（项 2）
  6. `project_json_paths_wildcard_on_non_array_errors` — `[*]` 作用于非数组显式 Err（项 2 负面）
  7. `project_json_body_array_error_teaches_syntax` — 顶层数组报错教 `0.field`/`[*]`，已用 `[*]` 不重复教（项 4）
  8. `github_thread_hint_claims_partial_not_absent` — "仅部分"取代"未包含"、升级路径保留（项 3）
- 既有测试：2 个因 `extract_with_include` 返回类型 String→Vec<String> 适配（`_hits_and_falls_back` / `_multi_selector_accumulates`，断言语义不变，分隔符断言移到渲染层）；FixG13/14 的 6 个边界单测零回归。
- FixG13 既有断言 `hint.contains("browse")` 与 api 端点——新文案两者保留，零回归。

## 验证（全部真实输出）

code 闸：
- `cargo clippy --all-targets -- -D warnings` → **clean**（0 error；中途修 1 处 `manual_contains`）
- `cargo test --all-targets` → **121 lib + 120 bin = 241 passed, 0 failed**（基线 233 + 新 8 = 241 ✓）

e2e（新构建 debug bin，mtime 晚于源码编辑；交付前自跑全过）：

| # | 命令 | 关键输出 | 判定 |
|---|---|---|---|
| P0-1 | `fetch https://docs.rs/serde_json/latest/serde_json/fn.from_str.html --include "pre" --json` | text 首字符 `'p'`（修复前 `' pub fn'`）；`lf_only=True`；分隔符实况 `"Deserialize<'a>,\n\n---\n\nuse serde::Dese"`；462B；stderr 空 | ✅ |
| P0-2 | `fetch "https://api.github.com/repos/tokio-rs/tokio/releases?per_page=3" --json-keys "[*].tag_name" --json` | text=`{"tag_name":["tokio-1.53.2","tokio-1.51.5","tokio-1.53.1"]}` 59B（<500，修复前 11.4KB 静默全量）；3 个 tag 值；`meta.truncated_by_json_keys=true` | ✅ |
| P0-3 | `fetch https://github.com/tokio-rs/tokio/issues/6741 --json` | hint=`"评论区仅部分包含在本输出正文中（可能缺作者归属且不完整，…）"`；含"仅部分"、不含"未包含"；missing=true 保留 | ✅ |
| P0-4 | `fetch "https://api.github.com/repos/tokio-rs/tokio/issues/6741/comments" --json-keys "user.login" --json` | stderr=`--json-keys 投影失败（json-keys 路径无此字段: "user.login"（在 "user" 处失败）\n响应为顶层数组：请用 \`0.field\` 索引语法…或 \`[*]\` 通配…）`；回退原文保留（16KB） | ✅ |
| P0-5 | 全量测试 | 241 passed / 0 failed（≥基线 233） | ✅ |

## 踩坑（上级分流）

拼接类文本提取管线：**结构级分隔符在中间表示层（inner_html）拼好，经下游"重解析+空白规整"必然被压扁**（`\n\n---\n\n` → ` --- ` 胶连）——多块拼接必须放在最终输出层（每块各自提取完再 join）。仅修中间层（trim/归一）治不了分隔符。另外本轮实证：盲测观察到的 CRLF 在本机 HTTP 路径未复现（html5ever 解析期已归一），归一逻辑照加作浏览器路径兜底。

## 范围纪律

- 未动：collapse_blank 本体 / skeleton / postproc / convert / main / searxng / Cargo.toml；既有投影输出形态（单路径末段短名、冲突全路径）逐字节不变
- `.beads/interactions.jsonl` 有非本任务改动（beads 守护进程自动交互日志），未触碰
- codebase-memory 图谱按项目记忆"子代理禁 reindex"未更新，建议 Main 合并后统一 `index_repository`

## 残余风险

- `[*]` 元素缺目标字段 → 整条路径 Err 回退全量（严格语义，与既有路径行为一致）；如需"缺字段跳过"语义另行立项
- CRLF 伪影在浏览器（CDP innerHTML）路径未实测（本机仅复现 HTTP 路径），归一逻辑理论上覆盖 `\r\n`/`\r`
- 提交/推送归上级
