# FixG18 修复回执

日期：2026-10-10 ｜ 基线：main=a291e04（248 tests）｜ 范围：盲测十八 4 项可控长尾

## 改动清单

| 项 | 文件 | 内容 |
|---|---|---|
| 1 (JJ -0.5) | src/convert.rs | 新增私有 `strip_docsrs_controls`（剥 `…\xa0Copy item path` / `" Copy item path"`，单点共用）；`clean_markdown` 尾部接它 + 全局 `\u{a0}`→空格归一；`clean_text_anchors` 同接（NBSP 已由 collapse_blank 折叠，无需归一）。既有 §/\_ 清洗与 extract_text 原语行为零改动 |
| 2 (HH -0.3) | src/main.rs:695 | `limit: args.limit` → `limit: read_n.unwrap_or(args.limit)`——`--read N` truncate 后 meta.limit 如实回写 N（N ≤ limit 已前置校验）；README:40 契约句同步 |
| 3 (II -1) | src/main.rs:233 | fetch `--json-keys` doc 补键命名规则句（FixG14 语义：末段短名/冲突全路径化，fetch.rs:829 实证）。输出行为零改动 |
| 4 (HH -0.2) | src/main.rs:211 | fetch `--max-chars` doc 补作用顺序句；顺序经真实 fetch 实测确认（见下） |

## 新增测试（4，基线 248 → 252 全绿）

1. `convert.rs::clean_markdown_strips_docsrs_button_and_nbsp` — 标题行按钮+NBSP 剥除，§/\_ 不回归
2. `convert.rs::clean_text_anchors_strips_docsrs_button` — 文本路径同剥
3. `fetch.rs::docsrs_route_strips_copy_button_and_nbsp_both_modes` — host route markdown/text 双路径产物无按钮/无 NBSP、签名逐字符保真
4. `main.rs::fetch_help_documents_jsonkeys_naming_and_maxchars_order` — 断言两个 arg 的 help 原文（get_help()，不经 render_help 防 CJK 换行拆断）

## P0 验证（全部实跑，产物在 reports/g18_p0_*.{json,err,txt}）

- **P0-1** `gsearch.exe fetch "https://docs.rs/serde_json/latest/serde_json/fn.from_str.html" --markdown --json`（exit=0）→ python 断言：`Copy item path` 无、`\xa0` 无、`pub fn from_str<'a, T>(s: &'a str) -> Result<T>` 在、标题行 `# Function from_str` 干净
- **P0-2** `GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 gsearch.exe search "serde_json" --read 5 --json`（exit=0）→ `meta.limit=5 == len(results)=5`，provider=searxng，run.status=ok
- **P0-3** `gsearch.exe fetch --help` → 四片段全命中：`嵌套路径取末段` / `冲突时自动全路径化` / `作用顺序：--json-keys 投影先替换 text` / `截断按投影后长度算`
- **顺序实测** `fetch "https://crates.io/api/v1/crates/serde" --json-keys "crate.id" --max-chars 12` → text=`{"id":"serde`、omitted=2——投影先替换 text、max-chars 对投影产物计预算，与 help 句一致
- **P0-4** `cargo test --all-targets`：122+130 passed / 0 failed（基线 248 + 新增 4 = 252）；`cargo clippy --all-targets -- -D warnings` 零警告；行为验证前已显式 `cargo build`（产物 mtime 07:39 晚于源码改动）

## 副作用与残余风险

- 非目标未触碰：searxng.rs/browser.rs/stealth.rs/shell.rs 零改动；`[*]` 投影形状/键命名行为/search 排序/browse/shell 未动
- strip_docsrs_controls 是通用清洗：任意页面若正文含 `" Copy item path"` 短语（非按钮语境）会被剥——契约明示该短语 docs.rs 特有、通用清洗处定点 sanctioned
- clean_markdown 的 NBSP 归一是全局（含 fenced 内）：契约引 JJ 实测代码块无 NBSP，签名保真测试已锁
- 批量路径 main.rs:908 等 meta 不受影响（batch 拒 --read；similar 无 --read）
- .beads/interactions.jsonl 工作树变更为非本任务产物，未动
- codebase-memory 图谱未重建（项目记忆：子代理禁 reindex）——归上级合并后跑 index_repository

已沉淀: rule://rust-pitfalls ← clap 4 单段 doc comment 只进 short help（get_long_help 恒 None），help 文案测试用 get_help + find_subcommand，勿对 render_help 做 CJK contains 断言
