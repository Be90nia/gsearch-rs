# P1 verify 线回执 — VERDICT: PASS

结论：三连全过。--timeout 可调（默认 5 不变、超时语义不变）；HEAD 403/405 自动 GET 回退带 `probe: get-fallback` 标注；批量 URL（多位置参数 / --urls-file）+ 对比表，退出码全 OK 0 / 否则 1；单 URL 行为与输出结构与修复前一致。code-level：cargo check 0 error + `cargo test verify::` 13 passed / 0 failed。端到端：全部真实跑通（下述输出为本机 target/debug 实测，fetch 线落定后的合并工作树）。

## 1. gsearch-rs-7qx：verify 加 --timeout

改动：src/main.rs Verify flag 行（`#[arg(long, default_value_t = gsearch::verify::VERIFY_TIMEOUT_SECS)] timeout: u64`）；src/verify.rs `curl_args` 改收 timeout 参数，`VERIFY_TIMEOUT_SECS` pub 化、值仍 5。

【验证命令+关键输出】
```
$ ./target/debug/gsearch.exe verify https://api.github.com/zen --timeout 10 --json
{ "status": 200, "final_url": "https://api.github.com/zen", "redirect_chain": [], "ssl_valid": true, "latency_ms": 784 }
EXIT=0
```
超时语义对齐：阈值只改预算不改分类，curl exit 28 → 退出码 5 路径不变（单测 `timeout_returns_exit_code_5` 仍绿）。

## 2. gsearch-rs-02z：HEAD 被拒(403/405)自动回退 GET

改动：src/verify.rs 新增 `should_get_fallback`（HEAD 成功且终态 403|405 → GET 重测一次）；GET 探测带 `Range: bytes=0-0`（-o NUL 兜底防大文件）；输出标注：JSON 增 `"probe":"get-fallback"`、文本增 `probe:       get-fallback` 行，无回退时不出现（旧结构零扰动）。

【验证命令+关键输出】
```
$ ./target/debug/gsearch.exe verify https://crates.io --json
{ "status": 403, "final_url": "https://crates.io/", "redirect_chain": [], "ssl_valid": true, "latency_ms": 1034, "probe": "get-fallback" }
EXIT=0
```
不再判 FAIL/状态不符（exit 0）；GET 结果为最终判定。crates.io 实测其反爬连 curl 的 GET 也拒（仍 403）——机制按 issue 落地，403 是目标站对 curl UA 的真实行为，probe 标注让 agent 可区分。

## 3. gsearch-rs-e58：批量 URL + 对比表

改动：src/main.rs `url: Vec<String>`（`num_args = 1..` + `required_unless_present = "urls_file"`，零参数仍被 clap 拒）+ `--urls-file`（`conflicts_with = "url"`）；src/verify.rs 单/批分派（len==1 走原路径）+ `print_batch`（--json = 数组，人读 = url/status/ssl/latency_ms 对齐表）+ 退出码全 OK 0 / 部分·全部失败 1。

【验证命令+关键输出】
```
$ ./target/debug/gsearch.exe verify https://api.github.com https://docs.rs --json
[ { "status": 200, ..., "latency_ms": 326 }, { "status": 200, ..., "latency_ms": 319 } ]   EXIT=0

$ ./target/debug/gsearch.exe verify https://api.github.com https://docs.rs https://crates.io
url                     status    ssl  latency_ms
https://api.github.com     200   true         341
https://docs.rs            200   true         228
https://crates.io          403   true         567
EXIT=0

$ ./target/debug/gsearch.exe verify --urls-file %TEMP%\gsearch_verify_urls.txt   （含 1 空行，已跳过，3 条全探）EXIT=0

$ ./target/debug/gsearch.exe verify https://api.github.com https://nonexistent-gsearch-e58-test.invalid
verify https://nonexistent-gsearch-e58-test.invalid: DNS 失败 (curl exit 6)   ← stderr 逐条
（表中该行 status 0 / ssl false）EXIT=1
```
批量 JSON 元素复用 VerifyReport 结构（+可选 probe 键）。

## 回归闸（向后兼容）

```
$ ./target/debug/gsearch.exe verify https://api.github.com/zen    （不带新 flag）
status:      200
final_url:   https://api.github.com/zen
redirects:   0
ssl_valid:   true
latency_ms:  465
EXIT=0
```
五行旧文本结构不变；单 URL --json 无回退时直接序列化 VerifyReport，与旧输出逐字节一致（无 probe 键）；`gsearch verify` 零参数 → clap "required arguments were not provided" exit 2（与旧 required String 行为一致）。

## side-effects 三态

1. 预期内：src/verify.rs（重写 cmd_verify + 新增 probe/批量/超时，+6 单测）、src/main.rs（Verify 定义行、分发分支）、README.md 退出码表 3 行（0/1 行加 verify 批量语义、5 行加 --timeout 提示）。
2. 预期外但必要：src/main.rs:813-823 既有测试 `verify_subcommand_parses_with_json_flag` 因 `url` 变 `Vec<String>` 编译必红，已最小适配（`..` + vec 断言）——该测试是 flag 定义行的直接耦合面，属同一边界；超出请 PM 复核。
3. 无：未触碰 fetch.rs / general.rs / search.rs / searxng.rs / types.rs；未 commit；临时 fixture 在 %TEMP% 未入仓。

## 备注 / 残余

- 任务文本称批量退出码"与 batch search 语义对齐（全失败也 1）"，但 search batch 实际是全失败→2（main.rs cmd_search_batch + README 表）；按任务明文 0/1/1 执行，未引入新码。PM 如需真对齐 search 需另行定夺。
- 按并行纪律未跑全量 `cargo test --all-targets` / `clippy --all-targets`（归 PM 合并态统一闸）；codebase-memory 图谱更新按项目纪律留给 PM（子代理禁重索引）。
- 单元测试 13/13：header 解析 4 + 分类 3 + curl_args 超时/Range 2 + 回退判定 1 + probe 标注 2 + 回环超时 1。
