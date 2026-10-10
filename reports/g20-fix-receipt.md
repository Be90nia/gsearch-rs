# FixG20 修复回执（盲测二十剩余 3 条 -0.5 长尾）

基线：main=e9e5e60（256 tests）。三项全部落地，P0-1~P0-4 交付前自跑全过。

## 范围

| 盲测扣分 | 项 | 修法 |
|---|---|---|
| NN -0.5 | fetch 单/多 URL schema 分叉（单=扁平无 status，批=数组元素含 status） | `fetched_json` 顶层统一注入 `status:"ok"`（单/批同源，只增不删）；batch 冗余覆盖行删除 |
| OO -0.5 | AdaptiveRead 引用展平重复段落无标注（"接近误导"） | `render_read` meta 构建处加 `dup_paragraph_groups`（trim 后逐字相同归组），meta 注入 `dup_paragraphs`（summary_paragraphs 数组 0-based 下标组，如 `[[3,4]]`）；段落文本不动 |
| PP -0.5 | truncated 词义过载（search=结果封顶 vs fetch/read/browse=正文截断） | 纯文档消歧：README 契约节写明按子命令两义 + search `--limit` help 一句话点明；字段不改名、truncated_detail 行为不动 |

## 改动

|文件|+/-|摘要|
|---|---|---|
| src/fetch.rs | +14/-3 | `fetched_json` json! 顶层补 `"status": "ok"`（FixG20 NN 注释）；`cmd_fetch_batch` 删冗余 `v["status"]` 覆盖（同值单源）；新增 1 测试 `fetched_json_includes_status_ok` |
| src/postproc.rs | +65/-0 | 新增私有 helper `dup_paragraph_groups`（HashMap 归组，首现序，组内 ≥2 才返回）；`render_read` 非 headings-only 分支注入 `meta.dup_paragraphs`（无重复键缺席，默认输出结构不变）；新增 2 测试 `render_read_annotates_dup_paragraphs` / `dup_paragraph_groups_triples_trim_and_order` |
| src/main.rs | +3/-2 | Fetch 子命令 help 补两形态 status 说明；search `--limit` help 补 truncated=结果集封顶消歧句 |
| README.md | +3/-1 | L39 truncated 按子命令两义显式消歧；L85 browse 契约补 `meta.dup_paragraphs` 字段说明（0-based 下标组）；L120 fetch 节补单 URL 扁平形态说明 |

禁区核对：searxng.rs / stealth.rs / shell.rs 未动；公共 API 签名未动（`fetched_json`/`render_read` 签名不变，新 fn 均私有）；Cargo.toml/lock 未动；既有测试断言零改动；`--raw/--markdown/--include/--json-keys` 路径未触碰（既有 fetch 测试全绿佐证）。

## 验证（P0，交付前自跑）

### P0-1 fetch 两形态共有 status（python 断言）

```
$ ./target/debug/gsearch.exe fetch "https://api.github.com/repos/tokio-rs/tokio" > p01_single.json   # exit=0
$ ./target/debug/gsearch.exe fetch "https://api.github.com/repos/tokio-rs/tokio" "https://api.github.com/repos/rust-lang/rust" > p01_batch.json  # exit=0
P0-1 单URL: 扁平对象, status == 'ok' , 键集: ['meta', 'status', 'text', 'title', 'url']
P0-1 批量: 数组元素 status = ['ok', 'ok']
两形态共有 status 键: OK
```

### P0-2 真实 GitHub issue 8470 dup_paragraphs

```
$ export GSEARCH_SEARXNG_URL=http://192.168.89.249:8888
$ ./target/debug/gsearch.exe read "https://github.com/tokio-rs/tokio/issues/8470" > p02_read.json   # exit=0, 2257 bytes
meta.dup_paragraphs = [[2, 3]]
组 [2, 3]: len=148 chars, 首段前60字='Some users have reported contention on the lock that protect'
P0-2 OK: dup_paragraphs 存在且组内逐字相同（trim 后）
```

（OO 报的 paragraph 3==4 即 0-based [2,3]，实锤复现同组引用展平重复。）

### P0-3 README truncated 双语义（grep）

```
README.md:39 - `meta.truncated` **按子命令两义，消费前先看子命令**（FixG20 PP 消歧）：`search` = **结果集封顶**……必附 truncated_detail……；`fetch` / `read` / `browse` = **正文/响应体截断**……
```

### P0-4 全量闸

```
$ cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.52s   ← 0 error
$ cargo test --all-targets
    lib: 122 passed; 0 failed
    bin: 137 passed; 0 failed; 2 ignored
    = 259 passed（基线 256 + 新增 3）
```

TDD：两项核心行为测试先落盘跑红（`fetched_json_includes_status_ok` / `render_read_annotates_dup_paragraphs` 均 FAILED），实现后转绿；第三个测试随实现同批加入。

## 关键设计取舍

- **status 注入点在 `fetched_json` 而非单 URL 分支**：单/批同源单点，batch 原覆盖行成同值冗余随之删除（clean cutover）；`fetched_json` 只被成功路径调用，恒 "ok" 语义精确。serde_json Map 键序输出不变（BTreeMap 字母序，加键不挪既有键）。
- **dup 检测放 `render_read` 而非 `extract_adaptive`**：read / browse / shell read 三条路径的 JSON 全部汇入 render_read 的 meta 构建处，单点覆盖全部出口；AdaptiveRead 序列化本体不动（段落文本逐字保留）。
- **0-based 数组下标而非 1-based 段号**：summary_paragraphs 剔除空段后与 paragraph_index 段号可能错位，数组下标是消费方可直接执行（`sp[3]`）的唯一无歧义参照系；README 已注明。

## 残余风险

- GitHub issue 8470 内容随上游变化：复跑 P0-2 时若引用被编辑，dup 组可能变化/消失（检测逻辑有单测兜底，不依赖该页）。
- `dup_paragraphs` 是新键，旧消费方不感知（缺席语义 = 无重复，零破坏）。
- truncated 双语义靠文档消歧，同名键事实未变（契约禁改名）；老消费方仍需按子命令区分。

## 未做（非目标，明确不做）

- 单 URL 改数组形态（破坏性）/ 段落去重删除 / truncated 改名 / FixG17 truncated_detail 行为改动 / commit·push / 图谱 reindex（均归上级）。

已沉淀: 无新增经验（唯一候选坑「MSYS /tmp 对原生 python 不可见」经查 rules/common-windows.md L260 已有且含 -c 内嵌变体细化，不重复记）。
