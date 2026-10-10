# FixG19 交付回执：fetch --raw 逃生门 + read 子命令别名

基线 main=549d7ea（252 tests）→ 本改动后 256 tests 全绿。禁 commit/push 已遵守（diff 留工作树）。

## 改动清单

| 文件 | +/− | 内容 |
|---|---|---|
| src/fetch.rs | +98/−12 | ① `FetchOpts.raw: bool`（doc 注明跳过提取漏斗+互斥面）② `Fetched.raw: bool` 字段 ③ `fetch_one_attempt` 在 body 落地后早退分支调 `raw_fetched`（json_keys 投影/summary 剥除/正文提取/markdown/host 路由/include/锚点裁剪/JS 壳判定全跳过；PDF/二进制拒与 SSRF 门在共享 GET 路径天然保留）④ 新纯函数 `raw_fetched(url, body, limit, body_truncated)`：body 逐字符进 text，`--max-chars` cap + FETCH_BODY_LIMIT 字节截断均如实进 meta ⑤ `fetched_json` 写 `meta.format="raw"` ⑥ 既有 21 处 Fetched/FetchOpts 字面量补 `raw: false` ⑦ 新测试 ×2 |
| src/main.rs | +48/−2 | ① `Browse` variant 加 `#[command(visible_alias = "read")]`，doc comment 补语义区分句（本命令=完整正文 Chrome 渲染；`search --read N` 是 snippet，勿混淆）② `Fetch` variant 加 `--raw`（`conflicts_with_all = ["markdown", "include", "json_keys"]`，help 写明逃生门用途）③ dispatch 传递 `raw` ④ 新测试 ×2 |
| README.md | +2/0 | fetch 示例块 +1 行 `--raw`；契约 bullet +1 条（逃生门语义/互斥/截断如实） |

## 新增测试（4，契约要求 ≥3；raw ≥2 + alias ≥1 全覆盖）

1. `fetch::tests::raw_fetched_keeps_body_verbatim` — text 逐字符等于 body（标签/实体/NBSP 全保留）；`--max-chars` 照常截断（meta.truncated/omitted/truncated_at_offset 如实）；body 硬上限截断照常累计
2. `fetch::tests::fetched_json_marks_raw_format` — `meta.format="raw"`、content_untrusted 恒在；非 raw 路径 format 键缺席（默认输出结构不变）
3. `main::tests::fetch_raw_conflicts_with_extraction_flags` — `--raw --markdown` / `--raw --include` / `--raw --json-keys` 全拒；`--raw` 单独解析置位
4. `main::tests::read_alias_parses_equivalent_to_browse` — `read`/`browse` 同参解析 Debug 输出完全等价；顶层 help 含 read

## P0 验收（交付前自跑，真实输出）

**P0-1** `./target/debug/gsearch.exe fetch "https://docs.rs/serde_json/latest/serde_json/fn.from_str.html" --raw --json`
→ exit **0**；`meta: {"content_untrusted": true, "format": "raw", "omitted": 0, "truncated": false}`；
text 头 `<!DOCTYPE html><html lang="en"><head><meta charset="utf-8">...`（has_doctype=True，len=23128，标签间零空格=服务端原始压缩形态，零提取实锤）

**P0-2** 同 URL `--raw --markdown --json`
→ exit **2**；stderr `error: the argument '--raw' cannot be used with '--markdown'`；stdout 0 字节

**P0-3** `read --help` vs `browse --help`
→ 两输出**逐字节相等**（read_help == browse_help: True）；顶层 `--help` 命令列表 `browse ... [alias: read]`；语义区分句进 help 首行

**P0-4** `cargo test --all-targets` → **122 lib + 134 bin passed / 0 failed**（256 = 252 基线 + 4 新增）；
`cargo clippy --all-targets -- -D warnings` → **零 error 零 warning**

## 有意的契约外延（1 处，需知悉）

- 互斥面在契约点名的 `--markdown`/`--include` 之外**加了 `--json-keys`**：投影也是 text 变换，raw 语义（零变换）下若静默忽略会成 silent failure，clap 显式拒是唯一不违反任一条款的做法（fetch.rs FetchOpts doc、main.rs help、README 均已注明）。
- raw 模式跳过 JS 壳判定与 URL `#N-M` 锚点裁剪（提取漏斗一部分，逃生门语义下无意义；help/README 已注明）。

## 残余风险

- 非 UTF-8 响应体在 raw 下经 `from_utf8_lossy` 有损（非法字节→U+FFFD）——与既有 text 路径同语义，未新增处理
- raw + `--human` 时 title 恒空串（人读表头 `=== url |  ===`）——契约只规定 `--json` meta，未特判
- batch 多 URL + `--raw` 可用但未单独 e2e（batch 复用 `fetch_one` 同一路径）
- `.beads/interactions.jsonl` +75 行为 bd 记账系统自动写入，非本任务代码改动，未回滚
