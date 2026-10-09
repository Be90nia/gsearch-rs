VERDICT: PASS — 盲测四 3 个 P3 全部修复，双验收（单测锁 + 真实命令 e2e）通过，全量 clippy/test 绿。

# buq（verify 5xx 判 ok）

改动：src/verify.rs `exit_for_status` 404 特判 → `status >= 400`（4xx/5xx 一律 verdict=http_error，rc=2）；cmd_verify doc 注释同步；分类语义选型：**不单列 server_error**，5xx 并入 http_error——具体状态码已由 `status` 字段 + `error_detail`("HTTP 500") 携带，分类集合保持最小。

单测锁：`exit_for_status_classification_table`（替换原 `exit_for_status_maps_404_only`）——2xx(200/204)=0、3xx(301/302/307)=0（curl -L 跟随后终态，链在 redirect_chain）、4xx(400/403/404/429)=2、5xx(500/502/503)=2；DNS/SSL/超时/other 分类不变（`classify_curl_exit` 原测试仍绿）。

验证：
```
$ ./target/debug/gsearch.exe verify "https://httpbin.org/status/500" --json
{"status":500,...,"verdict":"http_error","error_detail":"HTTP 500"}   rc=2
```

# qbw（similar 不校验 URL 形态）

改动：src/search.rs 新增 `pub fn looks_like_url`（含空白 / 空 / scheme 空或 rest 空 → 拒；无 `://` 时要求含 `.` 或 `/`——向后兼容 `docs.rs/serde` 既有放行形态，`split_site_keys` 同源基准）；src/main.rs `cmd_similar` 发起搜索前闸：不匹配 → stderr 一行 `error: 输入应为 URL（如 https://example.com/page）：{url}` + rc=2，不发起 SearXNG 查询。

单测锁：`looks_like_url_gates_similar_input`——not-a-url=拒、https://tokio.rs=过、docs.rs/serde=过（既有行为）、含空白句=拒、空串=拒、`https://`=拒。

验证：
```
$ ./target/debug/gsearch.exe similar "not-a-url" --limit 2
error: 输入应为 URL（如 https://example.com/page）：not-a-url     rc=2（无 JSON 输出）
$ ./target/debug/gsearch.exe similar "https://tokio.rs" --limit 1 --json
{"meta":{...,"query":"site:tokio.rs",...},...}                    rc=0
```

# cm8（meta.browser_path/browser_kind 环境噪声）

改动：src/types.rs MetaOutput 删 `browser_kind`/`browser_path` 字段；组装点全量同步删（main.rs search 主路径/similar/batch/两个 emit fn——emit 签名收窄掉 browser 参数；general.rs browse）；main.rs `resolve_browser_meta` 因此零调用者 → 删除（clean cutover）；verify/doctor 未动（各自用 ProbeJson/DoctorOutput，本就不含这对键）。

单测锁：`meta_omits_browser_keys`——sample_meta 序列化后两键缺席断言。

验证：
```
$ ./target/debug/gsearch.exe search "tokio" --limit 2 --no-humanize --json
{"meta":{"tool":...,"profile":"default","proxy":null,...,"recency":null},"run":{"status":"ok"},"results":[...]}   rc=0
# meta 键序 tool/version/query/profile/proxy/... — browser_path/browser_kind 均缺席
```

# 全量闸

- `cargo clippy --all-targets -- -D warnings`：Finished，0 error 0 warning
- `cargo test --all-targets`：**96 passed + 57 passed = 153 passed / 0 failed / 2 ignored**（lib 21.04s + bin）

# README 同步

1. JSON 输出契约 meta 键清单：`browser_path` / `browser_kind` 已移除（环境噪声，浏览器信息走 `gsearch doctor`）
2. similar 段：输入 URL 形态校验语义（接受 docs.rs/serde 无 scheme 形态；非 URL 拒绝 rc=2）
3. verify 段：新增 verdict 分类语义段（ok=2xx/3xx、http_error=4xx+5xx、传输层四类；5xx 曾误判 ok 一句注记）
4. 退出码表：rc=2 行补 "verify 单 URL HTTP 4xx/5xx" 与 "similar 输入非 URL 形态"；同时修正既有失真——rc=1 行的 "verify HTTP 状态不符" 与代码不符（单 URL 4xx/5xx 实际走 rc=2），已移除

# Side-effects（三态）

- 预期内：browse（general.rs）的 meta 同步失去 browser 两键——与 search 同性质环境噪声，编译器强制同步；read 命令 meta 走 main.rs 同一组装点，一并移除。均为 token 经济正向收益。
- 预期内：`resolve_browser_meta` 函数删除（唯一用途是填 meta 浏览器字段）；两个 emit fn 签名收窄。lib 内部 API，无外部消费者。
- 需知晓：工作树中 AGENTS.md/CLAUDE.md/.claude/settings.json 删除、tk_*.json 未跟踪文件等为盲测四环境既有状态，本次未触碰。
