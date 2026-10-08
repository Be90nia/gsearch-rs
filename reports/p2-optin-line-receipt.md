# P2 opt-in 双 issue 收尾回执（gsearch-rs-e1i + gsearch-rs-nx4）

结论：两 issue 均按元审计锁定形态落地——默认行为零变更，opt-in 新档全通过；clippy -D warnings 0 error、全量测试 80+44 全绿、双 e2e verify 实跑通过、新旧 exe 逐键回归闸通过。 VERDICT: PASS

## 1. gsearch-rs-e1i：`--read N --excerpt <chars>` 摘录档

改动（净 +60/-12）：
- src/skeleton.rs：`Paragraph` 加 `excerpt: Option<String>`（`skip_serializing_if = Option::is_none`，默认档不出键）；`extract_adaptive(html, excerpt_chars: Option<usize>)` 构造时 `p.chars().take(n)` 填充
- src/postproc.rs：`ReadOpts.excerpt` 字段；`read()` 透传 `opts.excerpt`；html 先过 `cap_chars(read_max_chars)` 再 extract → excerpt 总量天然受总 cap 约束
- src/main.rs：`SearchArgs.excerpt: Option<usize>`，`conflicts_with_all = ["full", "headings_only"]`（clap 互斥）；ReadOpts 构造透传
- 调用方补 `None`：src/general.rs:126（browse）、src/shell.rs:346（shell read）、skeleton/postproc 测试 12 处

【验证命令 + 关键输出】
```
$ export GSEARCH_SEARXNG_URL=http://192.168.89.249:8888
$ ./target/debug/gsearch.exe search "tokio tutorial" --read 1 --excerpt 300 --json --no-humanize
exit=0 | meta.provider=searxng | run.status=ok
paragraph_index items: 17 | items with excerpt key: 17
  idx 1 char_count 5   excerpt_len 5   head: 'Tokio'
  idx 2 char_count 270 excerpt_len 270 head: 'Tokio is an asynchronous runtime for the Rust prog'
  idx 3 char_count 55  excerpt_len 55  head: 'At a high level, Tokio provides a few major compon'
→ 每项含 excerpt，len = min(char_count, 300) ✓
$ ./target/debug/gsearch.exe search "q" --read 1 --excerpt 300 --full      → error: the argument '--excerpt <EXCERPT>' cannot be used with '--full'
$ ./target/debug/gsearch.exe search "q" --read 1 --excerpt 300 --headings-only → error: ... cannot be used with '--headings-only'   （clap 拒绝 ✓）
$ ./target/debug/gsearch.exe search "tokio tutorial" --read 1 --excerpt 300 --no-humanize（无 --json）
→ text 模式无 'excerpt' 字样、[目录]/[段落索引] 头照旧（text 行为不变 ✓）
```

## 2. gsearch-rs-nx4：`--envelope v2` batch 信封

改动（净 +101/-7）：
- src/types.rs：`BatchEnvelopeV2{meta, results}` / `BatchMetaV2{n_total,n_ok,n_fail,elapsed_ms}` / `BatchEntryV2{query,status,message,results}`（每条 14 字段 meta 移除，elapsed_ms 并入顶层）
- src/main.rs：`EnvelopeArg`（ValueEnum, V2）+ `SearchArgs.envelope`；`cmd_search_batch` json 分支分叉（n_ok 统计 + 装配）；单查询路径不读此 flag
- src/output.rs：`print_batch_envelope_v2`

【验证命令 + 关键输出】
```
$ ./target/debug/gsearch.exe search "rust tokio" "rust async-std" --json --envelope v2 --no-humanize
exit=0 | top keys: ['meta', 'results']
meta: {"n_total": 2, "n_ok": 2, "n_fail": 0, "elapsed_ms": 3026}
results len: 2，元素 keys: ['query', 'status', 'message', 'results']
  | rust tokio      | ok | 10 results | message ''
  | rust async-std  | ok | 10 results | message ''            ✓ 顶层 meta 一次 + 元素无 meta
$ ./target/debug/gsearch.exe search "rust tokio" --json --envelope v2 --no-humanize（单查询）
→ top keys: ['meta', 'run', 'results']（老单查询信封，flag 被忽略 ✓）
```

## code-level（全量闸）
```
$ cargo clippy --all-targets -- -D warnings   → Finished dev profile, 0 error/warning
$ cargo test --all-targets
  lib: 80 passed; 0 failed（含新增 skeleton::excerpt_opt_in_fills_and_skips、types::batch_envelope_v2_contract_keys）
  bin: 44 passed; 2 ignored（既有解析用例全绿）
```

## 回归闸（无新 flag 老输出逐键一致）

方法：`git worktree add ../gsearch-baseline HEAD` 构建改动前基线 exe，新旧 exe 同命令对比。

| 场景 | 键集合（递归路径） | excerpt 泄漏 | 值级 |
|---|---|---|---|
| batch 裸数组（新 vs 旧 exe） | **一致**（新增/消失键均无） | 无 | 有差异 → 见归因 |
| read 无 flag（新 vs 旧 exe） | **一致** | 无 | read 正文部分零差异 |

值差异归因（对照实验）：**同一新 exe 连跑两次同命令，值级同样非确定**（SearXNG SERP 实时内容漂移：title/url/snippet/domain_class 顺序波动 + elapsed_ms）。差异全部集中在 SERP 内容字段，无任何 schema 键变化 → 值漂移为外部源波动，非代码行为。

## side-effects（三态）
- **有意**：上述 7 源文件改动（+161/-19）；图谱已重索引（index_repository，skipped 0）
- **附带**：`Cargo.lock` gsearch version 0.2.8→0.2.9——首次 `cargo build` 时 cargo 自动对齐 Cargo.toml 既有版本号的 churn，非依赖变更；随上级 commit 带上即可
- **无意/未触碰**：工作树内既有他人改动（AGENTS.md、CLAUDE.md、.claude/settings.json 等删除，.beads/interactions.jsonl 修改）原样保留；e2e 脚手架（e2e_tmp/、../gsearch-baseline worktree）已清理

## 未做（非目标确认）
- 未 commit / 未动其他已关 issue 行为（回归闸锁定）；未按 bd e1i 原描述实施"默认给前 2-3 段 text"（元审计 comment 锁定为 opt-in，本回执即重设计形态）

已沉淀: 无新增经验（/tmp 路径错位已由 rules common.md「MSYS 与原生程序 /tmp 错位」及 rust-ffi-cross-target.md 同族条目覆盖；clap conflicts_with 字段名引用无踩坑成本不记）
