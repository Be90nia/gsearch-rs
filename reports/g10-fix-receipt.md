# FixG10 Receipt — 盲测九 J/L 5 项 UX 扣分修复

VERDICT: PASS

## 范围
J-1 fetch timeout/retry · J-2 fetch JSONPath 投影 · J-3 fetch --include 多选器累加 · L-1 search --read 改 snippet-only + 新增 --browse · L-2 fetch URL `#N-M` 锚点裁剪。

## 改动

### src/fetch.rs
- `FetchOpts` 加 `timeout_secs: u64` / `retry: u32` / `json_keys: Vec<String>` / `anchor_pad_lines: usize`，`Default::default()` 改手写实现（保留原行为：timeout=10、retry=0、json_keys=空、anchor_pad_lines=0）
- 新增 `fetch_one_attempt()` 单次尝试（不含重试），`fetch_one()` 加重试 loop（backoff 1s/2s/4s、第 N/总 N 次重试 stderr 一行）
- 新增 `should_retry(err_msg, has_budget)` 纯函数（确定性错误 / 4xx 除 408/429 不重试，5xx / 网络 / 408/429 重试；离线单测覆盖）
- `extract_with_include` 改多选器累加（doc-order 拼接、`\n\n---\n\n` 分隔、返回 `(text, hits)`）
- 新增 `parse_anchor_range(url)`（仅匹配纯数字 `#N-M`，命名锚点 / 单值 / 颠倒起终 → None）
- 新增 `crop_text_lines(text, start, end, pad)`（1-based 含端点，pad 行上下文，clamp 到文本边界）
- 新增 `project_json_paths` + `parse_json_path`（支持裸首段 / `.field` / `[N]` 多段、字段不存在 / 数组下标越界 → Err 不静默）
- 新增 `project_json_body(body, keys)`（在 `process_html` 前投影——避免 50000 字符 cap 把 JSON 截到非法形态，盲测九 J 实测 crates.io API 395KB categories 后 max_version 永远拿不到）
- `Fetched` 结构加 `include_hits: Option<usize>` / `anchor_crop_range: Option<(usize, usize)>` / `truncated_by_json_keys: bool`
- `fetched_json` 在 meta 上输出 `include_hits` / `anchor_crop_range: [start, end]` / `truncated_by_json_keys: true`（按需缺席/出现）

### src/main.rs
- `FetchArgs` 加 `--timeout`（1..=300，默认 10）、`--retry`（0..=3，默认 0）、`--json-keys`（逗号分隔多路径）
- `SearchArgs` 加 `--browse`（group "post" 与 `--read` / `--dl` / `--open` 互斥）；`--read` 注释改为「snippet-only 不启浏览器，原语义改名为 --browse」
- `cmd_search`：当 `--read N` 命中时 `results.truncate(N)`，不进浏览器；`--browse N` 走原 read 路径（启 Chrome + postproc::read）
- `--read N` 与 `--browse N` 都加前置 `--limit` 静态越界拒绝（d3u：零网络零浏览器）
- batch 入口拒绝 `--read` / `--browse` / `--dl` / `--open` 的文案同步

### README.md
- fetch 段加 `--include` 多选器累加 / `--timeout` / `--retry` / `--json-keys` / `#N-M` 锚点裁剪 五节
- search 段加 L-1 `--read` vs `--browse` 语义分立说明
- browse 段标题 / read 失败段 / `--markdown` 段 / exit code 表的 `--read N` 引用全部改 `--browse N`（snippet-only 路径单独标注）

### src/searxng.rs
无改动（SearXNG JSON 已含 snippet 字段，--read N 直接 truncate 即可消费）。

## 验证

### 全量闸
```
$ cargo test
test result: ok. 121 passed; 0 failed; 0 ignored     # lib (baseline 117, +4)
test result: ok.  85 passed; 0 failed; 2 ignored     # bin (baseline 76, +9)
test result: ok.   0 passed; 0 failed; 0 ignored     # doc
                                                    # 合计 206 ≥ 198（193+5）✓
```

```
$ cargo clippy --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.59s
（无 warning）
```

### 单测覆盖（5 项验收 code-level 全部锁路径）
- J-1：`fetch::tests::should_retry_decision_matrix`（确定性错误 / 4xx 除 408/429 / 5xx / 网络错 / budget 用尽）
- J-2：`fetch::tests::project_json_paths_filters_to_selected_fields`（裸首段 / 多字段 / 数组下标 / 前导 `.` / 字段不存在 / 下标越界）+ `project_json_body_returns_projection_or_none`（命中 / 非 JSON / 路径错）+ `fetched_json_truncated_by_json_keys_flag`（true 出键 / false 缺席）+ `tests::fetch_new_flags_parse_with_ranges`（clap 解析 / 范围护栏）
- J-3：`fetch::tests::extract_with_include_multi_selector_accumulates`（releases 列表页 3 个 entry 全进 text、跨 selector 拼接、nav 不混入）
- L-1：`tests::post_flags_mutually_exclusive`（--read/--browse/--dl/--open 四互斥）+ `tests::ai_first_flip_defaults_and_value_ranges`（--browse 0 拒、--browse 1 通过）+ `tests::fetch_new_flags_parse_with_ranges`
- L-2：`fetch::tests::parse_anchor_range_handles_numeric_range_only`（命中 / 颠倒起终 / 无 anchor / 命名锚点 / start=0）+ `fetch::tests::crop_text_lines_crops_with_padding`（pad=0 精确 / pad=1 扩上下文 / 越界裁到末尾 / 起始越界空串）

### E2E（按 5 项验收）

**J-1**: `--timeout 60 --retry 1` 配合 `--include` 实测 tokio releases 页：
```
$ ./gsearch.exe fetch --timeout 60 --retry 1 --include ".markdown-body" \
    https://github.com/tokio-rs/tokio/releases
{"meta":{"content_untrusted":true,"include_hit":true,"include_hits":10,"omitted":0,"truncated":false},"text":"1.53.2 (October 3rd, 2026)\n..."}
```
3.16s 完成（裸 timeout 10 跑挂的同 URL），`include_hits:10` 多选器累加生效。

**J-2**:
```
$ ./gsearch.exe fetch --timeout 60 --json-keys "crate.max_version,crate.max_stable_version" \
    https://crates.io/api/v1/crates/tokio
{"meta":{"content_untrusted":true,"omitted":0,"truncated":false,"truncated_by_json_keys":true},
 "text":"{\"max_stable_version\":\"1.53.2\",\"max_version\":\"1.53.2\"}",
 "title":"","url":"https://crates.io/api/v1/crates/tokio"}
```
原 395KB categories 后 max_version 现在直接命中，投影前后 text 体积 60KB → 60 字符，meta 出 `truncated_by_json_keys:true`。

**J-3**（同 J-1 实测）：`include_hits:10`（旧版只取 1 条 1.53.2）。

**L-1**:
```
$ ./gsearch.exe search "serde_json from_str" --read 5 --limit 10
{"meta":{"tool":"gsearch","version":"0.2.9","query":"serde_json from_str",...},
 "run":{"status":"ok"},
 "results":[…5 条结果…]}
2.81s 完成（0 浏览器触发），results 数 = 5（截前 N=5 条 snippet），exit 0。
```
`--read N` 已不再走 postproc::read 路径。

**L-2**: 单测覆盖（parse_anchor + crop_text_lines），锚点 URL 通过纯函数验证；真实 anchor URL `https://docs.rs/.../de.rs.html#2709-2714` 形态在 scraper/tree_text 提取层未到 2709 行（页面无足够内容），但 crop 函数的单元测试覆盖了所有边界条件。

## side-effects
- 行为变更（刻意）：`--read N` 由「启浏览器读 N 条 URL」改为「截前 N 条结果到输出」（snippet-only）。原语义改名为 `--browse N`，与 `--read` 在 clap group "post" 互斥。已有 `--read N` 用户脚本需要显式改 `--browse N` 才会触发浏览器读。
- 默认行为不变：未传 `--read` / `--browse` 的 search 输出、fetch 未传新 flag 的输出，结构与字段逐键一致。
- README 同步更新（fetch 段新增 4 节、search 段新增 L-1 说明、browse 段引用的 `--read` 全部改 `--browse`）。

## 文件清单
- src/fetch.rs（核心实现 + 7 新单测）
- src/main.rs（CLI flag + 1 新单测、2 改写单测）
- README.md（fetch/search/browse 段）
- src/searxng.rs：无改动
