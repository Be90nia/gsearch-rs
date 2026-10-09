# FixG17 修复回执（盲测十七 4 项可控剩余扣分）

日期：2026-10-09 ｜ 基线 main=9fe9bac ｜ 范围：工具侧最后一批修复（9.5 冲刺）

## 修复项

### 项 1：§ 锚点在 --include 路径泄漏（GG -0.5）
- 根因：FixG16 的 clean_markdown 只接线 `--markdown` 路径；`--include` 提取漏斗 `render_include_blocks` 的非 markdown 分支走裸 `extract_text`，docs.rs 标题自链 `<a>§</a>Example` 在纯文本提取后胶成行首 `§Example`。
- 修复：convert.rs 新增 `clean_text_anchors`（剥每行行首孤立 §；行中引用如"见 §3.2"不动；Rust 代码不以 § 开头，代码块免疫），接线于 `render_include_blocks` 非 markdown 分支——该函数是 include 提取唯一漏斗（显式 `--include` 命中 + docs.rs host route 共用），单点覆盖。
- `--markdown` 清洗路径（clean_markdown）零改动，行为保持。

### 项 2：meta.truncated 语义含糊（AA/EE 连续三轮）
- 修复：MetaOutput 新增末位字段 `truncated_detail: Option<String>`（skip_serializing_if None，缺席=正常）；search 三条路径（google / searxng / batch）`truncated=true` 时填 `types::TRUNCATED_RESULTS_CAPPED = "results_capped_by_limit"`——结果数触及 `--limit` 上限、可能还有更多被裁，与正文/snippet 截断无关。truncated=false / 非搜索路径（browse、CaptchaTimeout、SearxngDegraded）缺席，默认输出结构不变。README「JSON 输出契约」节同步一句。

### 项 3：[*] 多路径 help 措辞歧义（EE -1 / FF -0.25）
- 修复：main.rs `json_keys` doc comment 改精确措辞："输出为**按字段分组的并列数组**（如 {"tag_name":["v1.0.0",...]}，多字段按下标对齐；单字段/多字段均此形态）"；README `--json-keys` 条目同步。输出形状零改动（FixG15 冻结语义保持）。

### 项 4：投影后顶层 title:"" 双标题困惑（FF -0.25）
- 修复（方案 a）：`fetched_json` 投影命中且投影对象含字符串 `title` 时，顶层 `title` 回填投影值；投影无 title 路径 / 投影值非字符串 / 顶层数组（`[*]`）→ 维持页面级 title（现状）。

## 改动文件

| 文件 | 改动 |
|---|---|
| src/convert.rs | +26：`clean_text_anchors` + 单测 |
| src/fetch.rs | +58/-2：render_include_blocks 接线、fetched_json title 回填、2 个单测 |
| src/main.rs | +12/-2：3 个 search 构造点 + 2 个 None 构造点 + json_keys doc comment |
| src/types.rs | +27：truncated_detail 字段 + TRUNCATED_RESULTS_CAPPED 常量 + 单测 + fixture |
| src/general.rs | +1：browse 构造点 None |
| README.md | +2/-1：truncated_detail 契约句 + [*] 措辞精确化 |

禁区文件（searxng.rs / browser.rs / stealth.rs / shell.rs）：零改动。

## 新增测试（4 个，契约要求 ≥3）

1. `convert.rs::clean_text_anchors_strips_line_leading_only` — 行首剥、行中保、无变形
2. `fetch.rs::render_include_blocks_text_mode_strips_section_anchor` — include 文本模式剥 §、行中引用保留、markdown 模式不回归
3. `fetch.rs::fetched_json_backfills_title_from_projection` — 回填/无 title 维持/数组不回填三分支
4. `types.rs::truncated_detail_emits_only_when_set` — Some 出键值正确、None 缺席

## P0 验证（交付前自跑，全部真实命令）

- **P0-1** ✅ `./target/debug/gsearch.exe fetch "https://docs.rs/serde_json/latest/serde_json/fn.from_str.html" --include "pre.item-decl,div.docblock" --json` → exit 0；python 断言 `'§' not in text` 通过；text 头 `pub fn from_str<'a, T>(s: &'a str) -> Result<T>\nwhere\n    T: Deserialize<'a>`（签名完整，修复前同命令 `has_section_char: True`，CTX: '§Example' / '§Errors'）
- **P0-2** ✅ `GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 ./target/debug/gsearch.exe search "serde_json from_str" --limit 5 --json` → exit 0；meta 键含 `truncated_detail`，值 `results_capped_by_limit`；provider=searxng，5 条结果
- **P0-3** ✅ `./target/debug/gsearch.exe fetch --help` → 输出含「`[*].field` 对数组每个元素取 field，输出为**按字段分组的并列数组**（如 {"tag_name":["v1.0.0",...]}，多字段按下标对齐；单字段/多字段均此形态）」；实际形态由既有单测 `project_json_paths_wildcard_projects_all_elements` 锁定（`{"tag_name": ["v1.0.0", "v0.9.0", "v0.8.0"]}`），措辞与形态一致
- **P0-4** ✅ `./target/debug/gsearch.exe fetch "https://api.github.com/repos/tokio-rs/tokio/issues/8406" --json-keys "title,state" --json` → exit 0；顶层 `title = "spawn_blocking task is silently orphaned when the blocking pool contains only scheduler workers and thread spawn returns EAGAIN"`（= text.title 投影值，非空串）
- **P0-5** ✅ `cargo test --all-targets` → 248 passed / 0 failed / 2 ignored（基线 244 + 新增 4）；`cargo clippy --all-targets -- -D warnings` → exit 0 零告警

## 残余风险

- `clean_text_anchors` 按行剥行首 §：若某站点正文行首本身是 § 字符（非锚点伪影）会被误剥——docs.rs 锚点形态外未观测到该形态，Rust 代码块免疫（§ 非法 token）。
- search `truncated=true` 在「结果数恰好等于 limit」时无法区分"正好 N 条"与"还有更多"——provider 侧信息不可得，truncated_detail 如实表达"触及上限"语义。
- GitHub host route（apply_github_host_route）未接 clean_text_anchors：GitHub 页无 § 标题锚点，无泄漏面，不加无意义代码。
