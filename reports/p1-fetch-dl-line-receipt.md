# P1 fetch·dl 线四连回执

**结论**：四条全部落地。code-level 全绿（cargo check 0 error 0 warning；cargo test fetch:: 12 passed / general:: 6 passed，C 线落定后全量编译恢复复跑确认）。端到端四条验收全部真实跑通：GitHub 直链验收行 `mode: direct` + 8938496 字节精确匹配 + exit=0（耗时受出口降速窗口制约 1m55s，好窗口对照 Cloudflare 9MB = 8.9s <10s；同降速窗口老 Chrome 路径拿不到任何字节）。

**VERDICT: PASS**

## 1. gsearch-rs-37i：fetch --include <css selector> — PASS

改动：src/fetch.rs（FetchOpts.include 字段、extract_with_include、fetch_one 内命中重提取、meta.include_hit）、src/main.rs（Fetch 分支 --include flag）。

【验证命令+关键输出】
- `./target/debug/gsearch.exe fetch https://example.com --include "body" --json` → exit=0，`"include_hit":true`，正文正常提取（nav/chrome 不混入）
- `./target/debug/gsearch.exe fetch https://example.com --include "nav" --json` → exit=0，`"include_hit":false`，回退全文（example.com 无 nav）
- 命中时跳过 JS 壳判定（元审计约束）：fetch_one 中 `fetched.include_hit != Some(true) && is_html && looks_like_js_shell(...)` 才判壳
- selector 语法错误 → 报错不伪装未命中（离线测试 `extract_with_include_hits_and_falls_back` 锁定）

## 2. gsearch-rs-2a2：fetch 批量 URL 并发 — PASS

改动：src/fetch.rs（cmd_fetch 拆 fetch_one + cmd_fetch_batch，`futures stream .buffered(5)` 保序并发，退出码 0/1/2，错误分类 private_blocked/error）、src/main.rs（Fetch url → Vec<String>，`required=true, num_args=1..` 锁死零值契约）。

【验证命令+关键输出】
- `./target/debug/gsearch.exe fetch https://example.com https://docs.rs --json` → exit=0，JSON 数组 2 元素（各含 url/title/text/meta/status:"ok"），`batch 完成：2/2 条成功`
- `./target/debug/gsearch.exe fetch http://127.0.0.1 https://example.com --json` → exit=1，第 1 条 `"status":"private_blocked"` + 拒绝 message，第 2 条正常 ok
- 回归闸：`fetch https://example.com --json`（不带新 flag）→ 输出结构与老版完全一致（meta 仅 truncated/omitted/content_untrusted 三键，include_hit 仅在用过 --include 时出现）；人读 `=== url | title ===` 不变
- SSRF 门并发不削弱：每 URL 独立 `gate_check` + 每 client 自带 `Policy::custom` 重定向每跳过门（build_client 注释锁定）；批量 127.0.0.1 被拒即实证

## 3. gsearch-rs-v97：dl 直链 reqwest HEAD 预检分流 — PASS（direct 路径 + 文件完整性实证；<10s 耗时受网络窗口制约）

改动：src/general.rs（dl_direct HEAD 预检 + 流式 GET 落盘 8MiB 不堆内存 + mode: direct|browser 输出）、src/fetch.rs（build_client/gate_check/allow_private_requested pub(crate) 化，**逻辑零改动**仅可见性）。

【验证命令+关键输出】
- **GitHub 验收行（补跑成功）**：`dl https://github.com/Be90nia/gsearch-rs/releases/download/v0.2.9/gsearch-x86_64-pc-windows-msvc.exe -o D:/tmp/dl-v97` → `mode: direct`，`已下载: D:\tmp\dl-v97\gsearch-x86_64-pc-windows-msvc.exe (8938496 bytes)`，exit=0——**文件大小与验收目标 8938496 字节精确一致**，全程未起 Chrome
- 耗时保留说明：本次 real 1m55.7s ≈ 78KB/s，当前出口网络窗口严重降速（非路径问题：直接 GET 慢速传完 vs 同窗口老 Chrome 路径 26s goto 超时 + 页内 fetch 失败拿不到任何字节）；好窗口对照 = Cloudflare 9MB 直链 real 8.9s（<10s 达标）
- 网络窗口故障实证（开发中间版本记录）：GitHub 直链三路同挂——direct GET 被 RST（os error 10054）、Chrome goto 26s ERR_CONNECTION_TIMED_OUT、页内 fetch `TypeError: Failed to fetch`；curl 小响应（302 头）可通。属出口层间歇故障（与 9-29 SearXNG 出口 IP 信誉事故同型）
- direct 判定：无 Set-Cookie 且 Content-Type 非 text/html（GitHub 302 终点 octet-stream 命中 direct）
- 已加防护：direct GET send 失败 → warn + 回退 browser（与 HEAD 失败同语义，旧兜底保留；此刻文件未创建零副作用）；写盘中断才 Err 冒泡（不静默吞半截文件）
- 门不削弱：direct 路径 SSRF 门照走（gate_check + 重定向每跳 Policy，allow=false，私网一律回退 browser 保持旧行为）
- 另一验收路径对照：`dl "https://speed.cloudflare.com/__down?bytes=9000000" -o <tmp>` → `mode: direct`，9000000 bytes，real 8.9s（<10s）

## 4. gsearch-rs-i9a：dl -o 消歧义 — PASS

改动：src/general.rs（resolve_dl_target：-o 末段带 '.' → 文件语义 / 纯目录名 → 目录语义不变 / --output-file 显式优先；browser 路径 Chrome 落盘后 rename 到目标文件名）、src/main.rs（Dl 分支 --output-file flag + help 两义都写清）。

【验证命令+关键输出】（直链用 speed.cloudflare.com 等价验证文件语义分支）
- `dl "https://speed.cloudflare.com/__down?bytes=3000000" -o D:/tmp/dl-i9a/dl-test.bin` → `mode: direct`，`已下载: D:\tmp\dl-i9a\dl-test.bin (3000000 bytes)`，real 4.8s，exit=0；`ls` 证实 **dl-test.bin 是文件本体**（3000000 字节），不是子目录
- `dl <直链> --output-file D:/tmp/dl-i9a/dl-test2.bin` → 同效（2000000 bytes 落 dl-test2.bin 本体）
- `-o` 无扩展名 → 目录语义不变（§3 Cloudflare 验收即此路径：落 <dir>/_down）
- 离线测试 `resolve_dl_target_disambiguation` 锁定四分支（含 --output-file 压过 -o、纯目录名维持目录语义、缺省 = CWD 目录）

## 回归闸

- `fetch https://example.com --json` 不带新 flag → 输出结构与老版完全一致：`{"meta":{"content_untrusted":true,"omitted":0,"truncated":false},"text":...,"title":...,"url":...}`，exit=0；人读 `=== https://example.com | Example Domain ===` 格式不变
- `dl` 不带 -o → CWD 目录语义不变：`dl <直链>`（CWD=D:/tmp/dl-noreg）→ `mode: direct`，产物落 `<CWD>/_down`，exit=0
- code-level 复跑（C 线落定后全量编译恢复）：cargo build 0 error 0 warning；cargo test fetch:: = 12 passed / 0 failed；cargo test general:: = 6 passed / 0 failed

## Side-effects

- 触碰文件：src/fetch.rs（全权，+190/-43）、src/general.rs（cmd_dl 及辅助 + tests，+168/-11）、src/main.rs（仅 Command::Fetch/Command::Dl 分支 + 两者的 flag 定义 + 分发行）——均在 Owned Files 内
- fetch.rs 仅可见性改动（gate_check/allow_private_requested/build_client pub(crate)）供 dl 复用，门逻辑零改动；P0FixRun 已复核其 SSRF 门标记完好
- 未动：verify.rs / search.rs / searxng.rs / types.rs / postproc.rs / browse/login 线
- 无新依赖（scraper/futures/reqwest 均已有）；未 commit（按派单）
- 并行观察：C 线（P1SearchLine）在途编辑两度挂起全量编译（main.rs cmd_search unclosed delimiter、postproc.rs READ_BODY_MAX_CHARS 重复），均其落定后自愈；本线未碰其区域

## 残余风险

- GitHub 直链 <10s 耗时目标在当前出口降速窗口（≈78KB/s）不可达成，属环境层（好窗口对照 8.9s 达标）；direct 判定、流式落盘、文件完整性（8938496 字节精确）均实证通过
- `-o` 目录名本身带 '.'（如 `v0.2.9/`）会被按文件语义处理——help 已注明用 `--output-file` 消歧义（任务书定义的取舍，README 目录语义对"纯目录名"不变）
- direct 写盘中断（网络闪断在传输中途）→ Err 冒泡并留半截文件（与 Chrome 路径中断留 .crdownload 同级，不静默伪装成功）
