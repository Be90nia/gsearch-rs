# T4 漏网双补丁回执（dsg + k6o）

**VERDICT: PASS** —— 两补丁均落地，clippy 0 error + 51 tests 全绿 + gutenberg e2e 实测三场景通过。

## dsg：browse --full --json 补 meta.omitted

- 改动：`src/general.rs:116-125`（cmd_browse --full/--markdown --json 信封装配处）+4/-0：`omitted > 0` 时注入 `meta.omitted`，对齐 read 路径 `render_read`（postproc.rs:228-230）同语义：缺席=未截断（8lp/e19 空值缺席契约）。
- README 无需改：L64 既有承诺"截断发生时 meta 才出现 truncated / omitted 键"本就覆盖 browse——是代码违约而非文档过度承诺。

### 验证（真实命令输出）

clippy：`cargo clippy --all-targets -- -D warnings` → `Finished dev profile in 2.57s`（0 error）
test：`cargo test --all-targets` → `51 passed; 0 failed; 2 ignored`

e2e（`GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 ./target/debug/gsearch.exe browse … --json --full`，stdout 落文件后 python json.load 解析）：

| 场景 | 结果 |
|---|---|
| gutenberg 11-0.txt --full（144696 字符 > 50000 cap） | `truncated=True`，`omitted=94696`（= 144696 − 50000 ✅ 量级精确） |
| example.com --full（无截断） | meta 无 `omitted` 键、无 `truncated` 键（缺席语义与 read 一致 ✅） |
| gutenberg 11-0.txt --markdown | `meta.format=markdown`，`omitted=94695` 按 markdown 文本长度照常计 ✅ |

## k6o：JSON 消费分离 stderr 纪律

**选择：方案①（README 补纪律一行）。** 理由：方案②需把 `--verbose` 默认 "info" 降 "warn"，会连带静默 search CAPTCHA 人环心跳（15s/条 info 进度）、searxng 降级诊断等人环反馈，行为面波及全局且不可逆地改变存量用户观测——成本与风险远高于一行文档。

- 改动：`README.md:66` +1："- **JSON 消费分离 stderr**：stdout 是唯一 JSON 契约通道；stderr 会承载 Chrome 启动 INFO / 截断告警（"正文超上限已截断"），agent 消费 JSON 时禁止 `2>&1` 合并流"。落位在"read / browse 输出契约（供 agent 消费）"节 content_untrusted 行之后。

### 验证

e2e 实测 stderr 分离有效：gutenberg browse 全程 `stderr=115 bytes`（Chrome 启动 INFO 实锤复现）与 stdout JSON 物理分离，`json.load(stdout)` 解析成功即证明 stdout 纯净。

## 验收核对

- [x] code-level：clippy -D warnings 0 error；cargo test --all-targets 51 passed / 0 failed
- [x] e2e-a：browse gutenberg --json --full → meta.omitted=94696 > 0 且 = 总长 144696 − 50000
- [x] e2e-b：README 有改动 → diff 行见上（README.md:66 新增一行）
- [x] 回执落盘 reports/t4-miss-line-receipt.md

## side-effects

**未产生超出 scope 的副作用**：`git status` 仅 `M src/general.rs`、`M README.md` 两处；会话临时文件（browse_out.json 等 4 个）已清理；他人改动（AGENTS.md 删除等）未触碰。无未跟踪残留。

## 踩坑（沉淀）

`cargo clippy --all-targets` 不产出/更新 bin——第一轮 e2e `omitted=None` 用的是旧 exe 假象，显式 `cargo build` 后才验证到真行为。已沉淀至 `~/.omp/agent/rules/rust.md`（通用工具链坑，去项目化）。

## 未做 / 非目标确认

- 未动 search 输出（未触碰 search.rs / main.rs search 路径）
- 未 commit（工作树留给上级）
- 未跑图谱索引（任务明令禁动图谱）
