# gsearch-rs 静默失败/错误传播专项审计（2026-10-10）

VERDICT: ISSUES_FOUND — 1×P1（shell read 吞 -32000）+ 2×P2（read 终态空无 hint / CDP evaluate 无超时上界）+ 2×P3（swap 失败态不一致 / run_shell `?` 跳过 graceful_close）。无发布阻塞项；SearXNG→DDG→Google 回退链、dl 三路径、fetch batch、DDG curl 分类（khn）、SSR/风控空语义（j44/uhp/o1p）均已良好闭环，不再列为 finding。

范围：src/ 全量模式扫描（unwrap_or_default / .ok() / let _ = / if let Ok / eprintln warn / timeout 覆盖），逐条人工核实调用链。只读审计，未改码。

---

## F1 [P1] shell `read` 用裸 content().unwrap_or_default()，j44 的 -32000 修复未覆盖 shell 路径

- 位置：src/shell.rs:310（title 兜底同型：src/shell.rs:331-336；shell `browse` 也无稳定性等待：src/shell.rs:427-439）
- 对照：顶层路径已建全套防护——postproc::open_page → wait_content_stable（50×200ms 语义判稳）→ content_retry（3 次重试）；postproc.rs:495-500 注释明确记载「-32000 被 unwrap_or_default 吞成空串 → 正文为空」是已修的生产事故（j44，知乎实锤）。
- 失败场景：shell 会话中 `goto <url>` 或 `click @eN` 命中有服务端重定向链的页面（goto 只覆盖首个响应，跳转链继续销毁旧 JS context）→ 紧接着 `read` → content() 撞 -32000 → 吞成 "" → `is_captcha("")` = false 放行 → title evaluate 此时在新 context 上成功（title 有值）→ 输出 `=== url ===` + 空 AdaptiveRead 结构，rc=0。
- 用户/agent 看到的假象：「该页无正文」，与真空页面不可区分；agent 据此放弃该来源。
- 最小修复：src/shell.rs:310 改 `postproc::content_retry(&ctx.page).await`（已是 pub(crate)，general::cmd_browse 同款用法）；空结果时复用 general.rs:236 的 `[hint] 正文提取为空` stderr 行。shell browse 加 wait_content_stable 与顶层对齐。
- 可利用性：本地/远程（时序竞态，重定向链页面必现窗口）。

## F2 [P2] postproc `read`/`read_full` 终态空正文无 hint——general browse 加了保底，read 路径漏掉

- 位置：src/postproc.rs:427-430（content_retry 终态返回 ""：src/postproc.rs:639-649）；read_full_text 同型：src/postproc.rs:483-491；对照 general.rs:236-239（needs_empty_body_hint 覆盖「截断吃光 / content 拿空」两形态）与 postproc.rs:321-323（render_read 仅 omitted>0 时 hint）。
- 失败场景：tab 崩溃 / context 永久丢失（renderer OOM、目标页把 tab 换成 about:blank）→ content_retry 3 次打尽静默返回 "" → omitted=0 不触发 render_read 的截断 hint → `--read N` 输出 title 正常但 summary_paragraphs 空、meta 全正常、rc=0。
- 假象：与「页面本来就短」不可区分；browse 同场景有 `[hint] 正文提取为空，试 --markdown 或 --full` 自救出口，read 没有。
- 最小修复：postproc::read / read_full 在 extract 后加 general.rs 同款判定：`html_full.trim().is_empty() && !headings_only` → eprintln hint（meta 可选加 `extraction_empty: true`）。
- 可利用性：本地/远程。

## F3 [P2] CDP evaluate 链无超时上界——renderer 被楔死时 CLI 永久挂起

- 位置：src/postproc.rs:639-649（content_retry）、651-663（eval_string_retry）、133-140（page_snapshot）、741-744（fetch_in_page 的页内 fetch evaluate）。
- 失败场景：agent 经 search/browse 消费不可信 URL → 目标页跑同步死循环 JS 楔死 renderer（或 fetch_in_page 对端接受连接后不响应）→ `page.evaluate` future 永久 pending。goto 有 30s 预算（postproc.rs:549、search.rs:665），evaluate 无任何包装；wait_content_stable 的轮询循环卡在第一次 evaluate 上，无输出挂死。dl 的 fetch_in_page 挂起同样无界（浏览器默认 fetch 无超时）。
- 假象：进程假死，无错误无日志；上层 agent 调用方只能靠外层超时兜底。
- 最小修复：这 4 处 helper 内部对 `page.evaluate(...)` 统一包 `tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), ...)`，超时按「未就绪」/「下载失败」语义归入现有错误链。
- 可利用性：远程（恶意/劣质页面即可触发），影响交互与 agent 自动化两条路径。

## F4 [P3] swap_to_headed/swap_to_headless 失败路径状态不一致：旧 handler 未 abort、browser slot 悬挂

- 位置：src/browser.rs:525-530（swap_to_headless 541-556 对称同型）。
- 失败场景：`graceful_close` 已杀掉当前 browser → `launch(false/true)` 失败（profile 锁/无显示环境）→ Err 上抛，但此时旧 handler task 从未 `abort()`（abort 在 launch 成功之后才执行）、`*browser` 已是死实例而 slot 仍指旧 handler。shell 会话中报错本身响亮（不静默），但 ctx 处于不一致态，用户重试 `login` 前的任何命令都以混乱的 "receiver is gone" 类错误失败；旧 handler 仅靠 WS 连接关闭自灭，非确定性。
- 最小修复：把 `handler_slot.take()` + `abort()` 移到 `graceful_close` 之前（swap 的语义起点就是弃旧实例），失败路径同样收敛。
- 可利用性：本地（需 headed 重启失败的环境：锁竞争/无显示）。

## F5 [P3] run_shell REPL 中 `?` 早退跳过 graceful_close → 残留 chrome.exe 持 profile 锁

- 位置：src/shell.rs:87-93（`stdout.flush()?`、`reader.read_line(&mut buf)?`）。
- 失败场景：stdin/stdout IO 错误（管道对端关闭、重定向句柄失效）→ `?` 直接返回 Err → 循环末尾的 graceful_close（shell.rs:119）不执行 → Windows 上 chromiumoxide kill_on_drop 不生效（graceful_close 文档自证，browser.rs:933-941）→ chrome.exe 残留持锁，下次 launch 走 35s 退避甚至 terminal 报错。
- 最小修复：REPL 循环体收进内层 async 块取 Result，外层任何路径都先 `graceful_close(&mut ctx.browser)` 再返回（与 main.rs:592-628 的 H2 收尾模式同构）。
- 可利用性：本地，低频。

---

## 已核实为良性/已闭环的高危点（不报）

- SearXNG→DDG→Google 回退链：每跳 eprintln 留痕 + o1p 三态（HealthyEmpty/SourceError/CircuitBroken）+ khn 的 DDG challenge 页显式 bail（duckduckgo.rs:69-84）——无静默降级。
- `dl` 三路径 `let _ = goto_for_download(...)`：I4 刻意设计，warn 带 URL+耗时，fetch_in_page 失败传播、空字节显式报错（postproc.rs:681）。
- `--json-keys` 投影 miss：step_json_path 逐段报「在 X 处失败」（fetch.rs:962-970），投影失败 stderr 一行 + 原 body 兜底（fetch.rs:439-443），契约内。
- 配置解析失败：eprintln WARN + PARSE_FAILURE 登记 + doctor 显式 FAIL（config.rs:105-114，vw2）。
- wait_new_file：.crdownload/.tmp 跳过 + 双读同尺寸才算完成 + 超时显式报错（general.rs:575-600）。
- fork profile copy 失败：tracing::warn 留痕后回落 default，terminal 错误带持锁 PID 清单（browser.rs:754-771）。
- verify.rs / update.rs / searxng.rs（5s timeout + error_for_status + context 全链）：无吞点。

## 方法说明

serena-cli overview/symbol-body + 定向 read 全量扫 src/（22 文件）；模式：`unwrap_or_default|.ok()|let _ =|if let Ok` 全命中逐条定性（良性缺省 vs 错误吞点）；对照 memory 第 6 坑（-32000）与 j44/uhp/o1p/khn/I4/I5 已修面防重复报告。
