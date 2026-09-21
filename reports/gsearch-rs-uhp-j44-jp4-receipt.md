# 回执：postproc 链路三合一改造（uhp + j44 + jp4）

**VERDICT: PASS**（2026-09-21，TaskPostproc）

## 改动范围（本任务文件：config.rs / postproc.rs / general.rs / shell.rs）

- **uhp CDP 原子快照**：`postproc.rs` 新增 `PageSnapshot` + `SNAPSHOT_JS`（`async function () {...}` 声明形态），单次 Runtime.evaluate 返回 `{readyState, title, visibleText, links}`；`visibleText` 用 TreeWalker(SHOW_TEXT)：跳 script/style/noscript/template + aria-hidden/inert 祖先 + parent `checkVisibility()` + `getBoundingClientRect` 视口内 + 硬顶 6000 字符（`SNAPSHOT_MAX_TEXT_CHARS`，`{max}` 用 `str::replace` 注入避开 format! 花括号转义）。links 采集进 JS 返回（留给 shell_snap），Rust 侧暂不消费（serde 忽略未知字段，零死代码）。
- **j44 语义新鲜度**：`wait_content_stable(page, rounds)`（pub(crate) 共享）：marker = `{title, visibleText}` 拼接对等值比较（禁引哈希依赖），**连续两次相同才定稿**；`readyState != "complete"` 仅作必要条件地板（防慢加载页两个空 marker `("","")` 假定稿）；evaluate Err（-32000/导航中）视为未就绪并重置 marker。`goto_page` / `general::cmd_browse` 用 50 轮（≈10s，窗口量级同旧版）；`shell.rs` cmd_click 用 20 轮（≈4s）。旧 `wait_dom_complete` 降为私有，仅作 content_retry/eval_string_retry 的 -32000 恢复退避。
- **shell settle_after_click 缺口闭合**：函数删除，cmd_click 内联 `wait_content_stable(&ctx.page, 20)`；原 ponytail 注释所指「晚跳转漏判」缺口由 marker 判稳闭合（注释已在调用点更新说明）。
- **jp4 截断 + meta**：`READ_BODY_MAX_CHARS = 50_000` 缺省，`gsearch.json` 新键 `read_max_chars` 可覆盖（config.rs 归 TaskPostproc，Main 已确认）；`cap_chars` 按 chars 截断不劈 UTF-8；`render_read` 在 read/browse --json 输出对象末尾注入 `meta: {truncated, omitted, content_untrusted: true}`，文本模式截断走 eprintln（stdout 保持可解析）。--full 保持 M9 契约（innerText 5000 字）不变；登录墙判定改用快照 title/visibleText（快照全败退化为 URL 特征判定，与旧实现 evaluate 全败时行为一致）；CAPTCHA 轮询 / swap_to_headed / wait_login 语义未动。
- 顺带去重：`general::cmd_browse --full` 复用 `postproc::read_full_inner`（删重复 innerText 块与 TEXT_MAX_CHARS）；shell cmd_read 接同一 cap/render 契约。

## 验收证据

### code-level
```
cargo test --offline
  lib:  test result: ok. 63 passed; 0 failed
  bin:  test result: ok. 27 passed; 0 failed; 2 ignored   （live 测试保持 #[ignore]）
cargo check：0 warning（唯一 dead_code 警告已通过 readyState 地板消费修复）
cargo build --release：按 PM 指示不跑（PM 审计后统一构建）
```
新增单测：`cap_chars_cases` / `render_read_injects_meta_json_only` / config `parses_read_max_chars`。

### e2e（debug 构建，GSEARCH_PROFILE=uhp-verify 隔离）

1. `browse http://github.com` → EXIT=0，title「GitHub · Change is constant. …」= https 重定向后正文，无 -32000；stderr 含截断留痕「正文超上限已截断（省略 528751 字符）」（github HTML ~58 万字符被 5 万上限截断——jp4 真实生效）。
2. `search "武汉大学" --limit 2 --read 1 --json` → EXIT=0；read JSON 末行：`{"url":"https://…","meta":{"content_untrusted":true,"omitted":0,"truncated":false},"json_total_chars":5268}`——meta 三字段齐、总长 5268 ≤ 50000。
3. shell 交互（本地 onclick 跳转页）：`browse file://…/uhp_a.html` → `snap` 得 `e1 <button> "GO-B"` → `click @e1`（onclick location.href 导航）→ `read` 输出 `=== …uhp_b.html | UHP-B ===` + `B-LANDING-MARKER`——判稳骑过 context 销毁落在新页，无 -32000 无炸，晚跳转漏判缺口闭合。

## 边界与风险

- marker 两次采样间隔 200ms；>200ms 才发起的 setTimeout 晚跳转理论上仍可能在旧页两次采样后通过（窗口内 Err 重置只能闭合已触发的跳转）。与 jev fresh() 设计上限一致；action-keyed 节点身份属 shell_snap 领域（非目标）。
- 10s 窗口耗尽仍未稳定（持续动画改写视口文本等）→ 返回最后一次成功快照继续走，与旧版 wait_dom_complete 超时放行语义对齐。
