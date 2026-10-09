# FixG14 修复回执（盲测十三回归修复）

- 日期：2026-10-09
- 基线：main c84f7cb（FixG13 合并态）
- 结论：**PASS** — 3 项全修，5/5 P0 e2e 全绿

## 修复内容

| # | bug | 修法 | 位置 |
|---|-----|------|------|
| 1 | 行内 `<code>` 邻接空格被吞（"byT"/"ifTis"） | 行内 code 边界空格按相邻形态移进哨兵段：前置按 out 尾（块边界续 `\n`、空白续 `' '`、紧贴不加），后置按 peek 下一可见节点（空白文本续 `' '`、块级元素续 `\n`、紧贴正文不加） | src/fetch.rs tree_text |
| 2 | 签名 `Result<T>where`（pre 块边界换行丢） | ① pre 边界 `\n` 移进哨兵段（不被 prose 段 trim）② 哨兵段子树收集对内部块级子元素（div.where/br）注入 `\n` | src/fetch.rs tree_text |
| 3 | `--json-keys` 多路径末段同名静默 last-write-wins | 先求值全部路径 → O(n²) 冲突检测 → 冲突 key 改用全路径形态（`items.0.title`），单路径/无冲突末段短名零变化 | src/fetch.rs project_json_paths |

## 契约偏差声明（重要）

契约伪代码与真实页面字节结构不符，按 P0 验收硬闸修正实现：

1. **bug 2 根因与契约描述不同**。契约归因「pre 后置 sep 落后 prose 段被 trim」；实测 docs.rs 原始 HTML（curl 23132B）为 `Result&lt;T&gt;<div class="where">where\n    T: …` —— `Result<T>` 与 where div 之间**无文本换行**，粘连发生在哨兵段**内部**（子树收集对内部块级元素不注入分隔符）。契约伪代码的「边界 `\n` 移进哨兵段」修的是另一处真实缺陷（pre↔正文边界），但不修 `Result<T>where`。故在契约修法之上**增加**哨兵内块级子元素 `\n` 注入，P0-1 才得以通过。
2. **bug 1 契约伪代码条件反向**。契约「前置仅在 out 末尾**无**空白时注入」且空格压在 `\0` 之前（prose 段内）——按此 trace：`out="expected by "`（尾空格）→ 不注入 → prose 段尾空格仍被 trim → `byT` 不消失，P0-1 必挂。修正为**尾有空白时**把边界空格移进哨兵段（`\0` 之后），trace 全对。
3. 契约可选 meta 字段 `json_keys_collided_paths` **未实现**：需给 Fetched 结构加字段并波及全部构造点（含既有测试字面量），成本/收益不符；冲突语义已由全路径 key 本身自明 + README 标注。

## 验证证据

### 全量闸

- `cargo clippy --all-targets -- -D warnings`：clean（0 error / 0 warning）
- `cargo test --all-targets`：**121 lib + 112 bin = 233 passed, 0 failed, 2 ignored**（基线 226 + 新增 7）
- 既有 6 个指定单测全过零回归：extract_text_preserves_pre_code_whitespace / collapse_preserving_code_matches_collapse_blank_without_sentinel / collapse_preserving_code_keeps_code_segment_verbatim / parse_json_path_numeric_segment_becomes_index / project_json_paths_supports_top_level_array_index / project_json_paths_filters_to_selected_fields

### P0 e2e（debug exe 20:07:33 重链，晚于源码 20:05:56）

| # | 命令 | 结果 |
|---|------|------|
| P0-1 | `gsearch fetch https://docs.rs/serde_json/latest/serde_json/fn.from_str.html` | exit=0；`expected by T, for example` 在；byT/ifTis/typeTfrom/ofDeserializedecides 全不在；`Result<T>\nwhere` 在、粘连不在；签名区渲染 = 官方 3 行 |
| P0-2 | 同 URL `--markdown` | exit=0；签名行 `pub fn from_str<'a, T>(s: &'a str) -> Result<T>` + where 独立行 + 围栏代码块完整 |
| P0-3 | `gsearch fetch https://github.com/tokio-rs/tokio/releases` | exit=0；mpscPermitbefore / validateMAX_PERMITSin 零命中；且组件词全在（mpsc/MAX_PERMITS/validate/Permit present=True），`validate MAX_PERMITS in Semaphore::acquire` 正确分词——真修复非内容消失 |
| P0-4 | 单测 `project_json_paths_conflicting_keys_use_full_path` | items.0.title / items.1.title 两 key 全路径形态均保留（len=2）；单路径仍输出 `{"title": ...}` 短名；既有 `out["max_version"]` 断言不回归 |
| P0-5 | 基线对比 | 226 → 233（+7 新单测），0 failed |

## 改动清单

- `src/fetch.rs` +178/−9：tree_text pre/code 分支重写（边界进哨兵段 + 哨兵内块级注入 + 行内前后 peek）、project_json_paths 冲突全路径化、+7 单测
- `README.md` ±1：--json-keys 段补冲突语义
- `reports/g14-fix-receipt.md`：本回执

## 范围禁区遵守

collapse_blank 本体 0 行改动；通用行内元素（b/span/a）边界行为未动；skeleton/postproc/convert/main/searxng/Cargo.toml 未动；project_json_paths 单路径输出形态逐字节不变。

## 残余风险

1. 行内 code 后置 peek 仅向前扫描，遇纯空白文本节点连续接多个 script 类跳过元素再接正文时补 `' '` 的判定按穿透处理，极端嵌套页面理论上仍可能差一个空格（未构造出实页）。
2. 哨兵段内块级子元素仅开标签注入 `\n`（闭标签不注）：`<div>x</div>text` 形态在 pre 内会少一个换行（真实 rustdoc 结构未出现该形态）。
3. 文档以 pre/code 起始时输出不再有前导换行（lead 对空 out 抑制），文档以 pre 结尾时输出尾部多一个 `\n`（哨兵内，无害未 trim）。
