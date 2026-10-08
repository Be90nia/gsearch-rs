# T1 Flip-Line 回执 — 3gw 主案 AI-first 翻转全套 11 项

**结论：11/11 项全部落地，code-level（cargo check lib+bin 0 error；我模块过滤测试 lib 45 + bin 46 全绿）与 end-to-end（真实 SearXNG 实测 5 组）双验收通过。**

VERDICT: PASS — 默认输出已翻转为紧凑 JSON（单行、snippet≤160、score 透传、空值缺席），--human 保留旧人读格式，--json 兼容 noop 逐字节一致；实测单 query（limit 10）默认输出 3324B vs 翻转前行为 4586B（pretty+不截断），**省 1262B = 27.5%**（3gw 预期 25-30% 区间内）。

环境：GSEARCH_SEARXNG_URL=http://192.168.89.249:8888，Windows Git Bash，target/debug/gsearch.exe（cargo 指纹确认与源码同步）。

## 逐项验证

### 1. `--json` 变默认 + `--human` + 兼容 noop（3gw）
- 命令：`gsearch search "tokio" --limit 3 --no-humanize`（默认）vs `... --human` vs `... --json`
- 输出：默认 exit 0 单行紧凑 JSON（1060B）；`--human` 输出旧人读格式（`1. Tokio [wikipedia]\n   https://en.wikipedia.org/wiki/Tokio\n   ...`）；`--json` 与默认 diff 仅 elapsed_ms 与 SearXNG 实时分值抖动（3.0→4.0），契约字段零差异 → 存量脚本零破坏
- 覆盖命令：search / browse / verify / fetch / doctor 五个 JSON-capable 命令全部翻转（enum 层 `human: bool` + `json: bool` hidden noop，dispatch 传 `!human`，cmd_* 函数签名不变）；login/dl/shell 无 JSON 模式不涉及

### 2. compact 序列化默认（6nj）
- 改动：output.rs 三函数 to_string_pretty → to_string（search/read/batch 三路径全走此层）
- 验证：验收 a 输出单行（`'\n' not in raw` = True）；verify/doctor/fetch 三路径归 T2Correct（其侧 e2e 全过，见其回执）
- 实测：同数据 pretty 3929B vs compact 3324B = **省 16.0%（605B/query）**（6nj 报告口径 12.3%，吻合）

### 3. snippet cap 160 默认 + `--snippet-len`（cw8）
- 命令：`search "tokio" --limit 10 --no-humanize` vs `... --snippet-len 100000`
- 输出：默认 3 条 snippet 长度 35/160/158（≤160）；cap160 3324B vs 不截断 3981B = **省 19.9%（657B/query）**（cw8 报告 11.3%，本查询 snippet 偏长故更高）
- 人读不变：print_text 恒 160 cap（装配层先 cap 后打印，`--human` 输出与旧版逐字节一致）

### 4. 空值缺席（8lp①④）
- 验收 a 输出：`run` 仅剩 `{"status":"ok"}`（captcha_solved=false/message="" 缺席）；`meta.results_count` 键不存在；`truncated` 仅 true 时出现
- batch：ok 元素 message 键缺席（types.rs 单测 batch_entry_serializes_contract_keys / batch_envelope_v2_contract_keys 锁死）；n_total/n_ok/n_fail/fail_count/warn_count 全保留（退出码语义）
- 单测：`null_value_absence_and_score_passthrough`（types.rs:349）

### 5. README 定位改写（3gw）
- 开头改 "AI-first"，新增输出契约节（紧凑 JSON 默认 / 缺席语义 / score / snippet cap / 参数护栏 / read_error / searxng_degraded 统一），全部示例默认 JSON、人读标 `--human`，退出码表补 read 失败 exit 1 行

### 6. e19：pi 默认只列未摘要段 + --excerpt 全量 + headings>30 截断
- 实现：postproc.rs read() 装配层过滤（drop_summarized_pi：非空段序数 > summary_paragraphs.len() 才保留，空段保留占位对齐 --from K 段号）；opts.excerpt=Some 时跳过过滤（e1i 契约）；read.headings>30 截断 + `meta.headings_truncated:true`
- 单测：`e19_pi_lists_unsummarized_only`（12 段中等文摘 10 段 → pi 剩段号 11/12 + 空段）；`render_read_injects_meta_json_only` 扩展缺席断言 + 截断标记注入
- skeleton.rs 未触碰（过滤在 postproc 装配层，extract_adaptive 行为零变更）

### 7. dd1：score 透传
- 验收 a：`score: 8.0 / 3.0 / 1.67` 随 results 输出；键序 title,url,snippet,score,domain_class（nw4 末键契约不破，单测锁定）
- Google HTML / SearXNG HTML 降级源 score:None → 键缺席（serde skip_serializing_if）

### 8. w9y：read 失败 read_error + exit 1
- 命令：`gsearch search "tokio" --read 99 --json --no-humanize`
- 输出：**exit 1**；顶层键 `['meta','read_error','results','run']`，`read_error: "--read 99 越界（结果数 10）"`；不再静默 exit 0
- `--human` 模式同 exit 1（stderr 提示）

### 9. yq6：SearXNG 连接失败路径统一 degraded
- 实现：cmd_search 记 `searxng_fell_back`（FallbackGoogle），回退后 results 仍空 → `run.status=searxng_degraded` + SEARXNG_CIRCUIT_MSG message（与熔断分支同值）；Google 回退成功则 status=ok（降级自愈）
- 验证方式：逻辑分支 + 既有 CircuitBroken→degraded 信封路径未动；**未 e2e 复现**（需 staging「SearXNG 死端口 + Google 直爬也零结果」场景，不可靠构造）——标注为推断级完成

### 10. l6o：`--read` 拒 0
- 命令：`search "tokio" --read 0` → **exit 2**（clap 拒绝）；`--read 1` 正常解析
- 实现：`value_parser = clap::builder::RangedI64ValueParser::<usize>::from(1..)`（clap 4.6 已移除 value_range 属性，此为现行 API）

### 11. cxa：`--limit` 1..=100
- 命令：`--limit 200` → **exit 2**（clap 拒绝）；`--limit 100` 正常（exit 0 实搜验证）
- 实现：同上 API `from(1..=100)`

## code-level 汇总
- `cargo check --lib` / `cargo check --bin gsearch`：0 error 0 新增 warning
- `cargo test --lib -- types:: parse:: searxng:: search:: output:: postproc:: skeleton::`：45 passed / 0 failed
- `cargo test --bin gsearch -- tests::`：46 passed / 0 failed（含新增 ai_first_flip_defaults_and_value_ranges、doctor/verify 翻转契约更新用例）
- 全量 clippy / 全量 test / release build：按并行契约归 PM 合并态，未跑

## side-effects（三态）
1. **禁区触碰：无**。build.rs/verify.rs/config.rs/fetch.rs 未动；main.rs 的 cmd_doctor 函数体与 Doctor 结构未动。main.rs 属我区域的必要接线：T2Correct 请求的 version_line 一行（Cli derive `version = gsearch::build::version_line()`，T2 已自修双名 bug 并 e2e 通过）。
2. **非我所有文件的机械适配（编译依赖波及，非逻辑改动）**：general.rs 删 results_count 构造行 + render_read 补第 7 参 `false`；shell.rs render_read 补第 7 参 `false`（shell 人读路径不加 heading cap，JSON 过滤仅在顶层 read() 装配）。
3. **行为变更面（均获拍板）**：doctor/verify/fetch/browse 默认输出人读→JSON（存量人读脚本需加 `--human`）；meta.results_count 字段移除（breaking）；truncated/message/captcha_solved 默认态缺席（消费者按缺席=正常解析）；search --read 失败 exit 0→1；--limit/--read 越界值 exit 2。

## 未做 / 残余
- yq6 degraded 分支未 e2e 复现（staging 场景不可靠构造，见上）
- markdown 输出（Wave2）、commit/push、全量 clippy/test：非本任务范围
- 8lp②③（fetch message 去 URL / doctor message 值类化）归 T2Correct，已完成于其侧

—— T1Flip · 2026-10-08
