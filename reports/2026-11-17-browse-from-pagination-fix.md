# bd gsearch-rs-p8r：browse 顶层 `--from` 静默失效修复 + AdaptiveRead 滑动窗口 + 文档同步

**结论**：三项（browse --from 接线 / 窗口语义修正 / 三方漂移同步）全部完成，双验收全过。`VERDICT: PASS`

日期：2026-11-17 ｜ 执行：BrowseFromFix（实现子代理）｜ commit 归 PM

---

## 1. 改动清单

### src/skeleton.rs（窗口语义 + 数据面）
| 位置 | 改动 |
|---|---|
| `AdaptiveRead`（:39-42） | 新增 `#[serde(skip)] pub all_paragraphs: Vec<String>`——全量段落（含空段，段号与 paragraph_index 1:1），滑动窗口的基底；serde skip 保 v0.3.0 JSON 逐字节契约 |
| `summary_take_n()`（:84-93） | 新增 helper：`<SHORT_MAX 全给 / ≤LONG_THRESHOLD 给 10 / 否则 5`——阈值规则单一来源 |
| `extract_adaptive`（:128, :170） | 改用 helper；struct literal 填充 `all_paragraphs` |
| `format_adaptive`（:295-340） | 窗口语义修正：`from=0` → 原摘要逐字节不变；`from=K>0` → `all_paragraphs.skip(K).take(take_n).filter(非空)`（跳过全文前 K 段、从第 K+1 段起取最多 take_n 段，窗口不随 K 缩水）；越界消息改「`(--from K 越界，全文共 N 段)`」 |
| tests（:577-630） | 删旧 skip 语义测试 `format_adaptive_from_offset_skips_summary_lead`（它锁的就是本 bug 行为）；新增 `format_adaptive_from_window_slides`（--from 3 → 第 4..8 段、窗口恒 = LONG_TAKE）与 `format_adaptive_from_out_of_range_marks_boundary`（--from 99999 → 越界标注不回退全文；--from 99 → 只剩第 100 段） |

### src/postproc.rs（契约测试）
| 位置 | 改动 |
|---|---|
| tests（:1154-1164） | 新增 `md_page_section_from_skips_blocks`：from=0 输出与旧 `println!("# t\n\n{body}")` 逐字节一致 / from=K 跳过前 K 个空行分隔块 / from 越过块数 → 正文空（不 panic 不回退）——browse 接线所依赖的契约 |

### src/general.rs（browse 接线 + hint + 漂移修复）
| 位置 | 改动 |
|---|---|
| 模块注释（:4） | 漂移修复：「M9 默认 AdaptiveRead」→「v0.4.0 默认 md 全文；--json 回 AdaptiveRead JSON；--from K 默认 md 形态跳过前 K 块」 |
| `BrowseOpts.from`（:36） | 补字段 doc（消费面与不消费面） |
| `cmd_browse` doc（:114） | 漂移修复：「默认 AdaptiveRead（M9）」→「默认 md 全文（--from K 跳过正文前 K 块）」 |
| URL 门后（:132-137） | 新增 stderr hint：`from>0 && (json || full || headings_only)` → 一行「--from K 仅默认 md 形态生效…」（禁止静默失效） |
| 输出点（:286-287） | **核心接线**：`println!("# {heading}\n\n{body}")` → `print!("{}", postproc::md_page_section(&heading, &body, opts.from))`（`print!` 因 section 自带结尾换行——from=0 时与旧输出逐字节一致） |

### src/main.rs（CLI 文档）
| 位置 | 改动 |
|---|---|
| browse `--from`（:117-119） | 补 flag 描述（此前无任何 doc） |
| search `--from`（:323） | doc 同步为滑动窗口语义（该路径行为随 format_adaptive 改变：「摘要段从第 K 段开始（1-based）」这句对旧实现本来就是错的） |

### CHANGELOG.md
- `[0.4.1] > Fixed` 顶部新增一条（browse --from 分页生效 + 窗口语义修正 + JSON 契约不变）

---

## 2. 验证证据（真实命令 + 关键输出）

### code-level：`cargo test`（全量非 ignored）
```
TEST_RC=0
test result: ok. 130 passed; 0 failed; ...   # lib（含 2 个新滑窗测试）
test result: ok. 148 passed; 0 failed; ...   # bin（含 md_page_section_from_skips_blocks）
test skeleton::tests::format_adaptive_from_window_slides ... ok
test skeleton::tests::format_adaptive_from_out_of_range_marks_boundary ... ok
test postproc::tests::md_page_section_from_skips_blocks ... ok
test result: ok. 6 passed; 0 failed; 1 ignored   # cli_golden_bytes，含：
test normal_state_json_stdout_is_byte_identical_to_pre_feature_golden ... ok
test normal_state_md_stdout_is_byte_identical_to_pre_feature_golden ... ok
```
（其余各集成套件 0 failed；4+1 个 #[ignore] live 测试未跑，惯例）

### code-level：`cargo clippy --all-targets`
```
Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.45s
CLIPPY_RC=0
```
（无新增 warning）

### end-to-end（101 段本地页面 + python http.server 8931 + 修复前 release 二进制基线 A/B）

基线先行：改动前 `cargo build --release`（2m02s）→ 复制 exe 为 `gsearch-prefix.exe` → 同页面采基线。**bug 实证**：修复前 `browse --from 5` 与无 flag 的 stdout 归一后 `cmp` 相等（静默失效实锤）。

| 验收 | 命令 | 结果 |
|---|---|---|
| A1 默认 md 接线 | `gsearch browse http://127.0.0.1:8931/long.html --allow-private --from 5` | **PASS**：正文首块 = `PARA-006 ...`（line 4），`grep PARA-00[1-9]` 仅命中 006-009，001..005 不在输出 |
| A2 长文尾部可达 | 同页面 `--from 100` | **PASS**：`PARA-101 sample paragraph 101 ...` 在输出（唯一 marker） |
| A3 默认零破坏 | 无 flag 跑新旧两个二进制，stdout 归一（`elapsed: X.Xs→0.0s`）后 `cmp` | **PASS**：逐字节一致 |
| A4 JSON 契约 | `--json` vs `--json --from 5`，stdout 归一（`elapsed_ms`）后 `cmp` | **PASS**：逐字节一致；stderr 恰多一行 `[hint] --from K 仅默认 md 形态生效…`（115→236 字节，差额=该 hint 行） |
| A5 越界 | `--from 99999` | **PASS**：rc=0，stdout 95 字节 = meta 头 + `# 标题`，正文空；不 panic、不回退全文 |
| 附带验证 | default/from5/from100/from99999 各自 stderr | 均 115 字节（仅 Chrome INFO 启动日志）——md 路径不误发 hint ✓ |

`cargo build --release`：rc=0。

---

## 3. Side-effects 三态声明

- **预期内（bd 修复面）**：`search --browse N --from K`（文本形态）行为随窗口语义修正改变——旧 `--from 3` 在 5 段摘要上砍出 2 段，新出「第 4..8 段」5 段窗口；`main.rs:323` doc 已同步。无 `--from` 的 search 输出不变（cli_golden_bytes 两条逐字节 golden 锁 + 实跑绿）。
- **无副作用面**：JSON stdout 契约（A4 双跑逐字节一致 + golden 绿）；无 `--from` 的 browse 输出（A3 新旧二进制逐字节一致）；fetch/verify/doctor/batch 路径零接触；`--max-chars` cap 语义未动。
- **观察项（范围外，未动，留 PM 裁决）**：`src/postproc.rs:582` `read_full`（`search --read N --full` 的 md 路径）硬编码 `md_page_section(&title, &txt, 0)`——search 路径的 `--full + --from` 同样不消费（innerText 无空行分块，性质与本次 browse `--full` 相同；本次对 browse 选择了 stderr hint 方案）。属非目标保护的 search 路径，未顺手改。

其他上报：
- CLAUDE.md 缺失（按契约上报，不补建）。
- README 无 `browse --from` 提及（仅有 search `--from` 上下文），无需同步——已核。
- e2e 注意点：`html_to_markdown` 会把 `<title>` 文本作为正文首块输出（实测），含 `<title>` 的页面正文块数 = 段落数 + 1，`--from K` 块计数随之前移；验收 fixture 因此去掉了 `<title>`（101 段 = 101 块）。这是转换器既有行为，非本次引入。

## 4. 临时产物清理
- 测试页面/基线二进制/输出文件均在 `%TEMP%/p8r/`，验证后整目录删除；http.server 后台任务已停。repo 根无遗留。
