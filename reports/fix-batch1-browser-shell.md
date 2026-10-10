# Fix Batch 1 — browser.rs / postproc.rs / shell.rs（FixBatchA）

结论：6 条 finding（bnh / c5f / 7l0 / vag / 3t9 / n9j）全部修复；`cargo check` 零错误零警告；定向单测 browser 15 / postproc 14 / shell 3 全绿（含 2 条新增）。未 commit。

---

## 1. bnh — fork profile 不再拷缓存目录

**改动**（src/browser.rs）
- 新增 `FORK_SKIP_DIRS` 常量表（:348-357）：Cache、Code Cache、GPUCache、Service Worker、Crashpad、GrShaderCache；注释标明**只影响 fork 拷贝，default profile 本体不动**。
- `copy_profile_contents`（:375-379）：顶层循环按目录名跳过（覆盖根级 Crashpad/GrShaderCache）。
- `copy_dir_recursive`（:410-413）：子树内按目录名整棵跳过（覆盖 Default/ 下的 Cache 系）。

**验证**
```
cargo test --lib browser::tests
test browser::tests::copy_profile_contents_skips_cache_dirs ... ok   ← 新增
test browser::tests::copy_profile_contents_recursive_and_skips_locks ... ok
test result: ok. 15 passed; 0 failed
```
新增测试断言：fork 无 Crashpad/GrShaderCache/Default/Cache/Default/Code Cache；default 本体 Cache/entry 仍在；Cookies/History 照拷。

## 2. c5f — open_page 正文带回复用，read 不再二次 content_retry

**改动**（src/postproc.rs）
- `open_page`（:510-544）签名 → `Result<(Page, Option<PageSnapshot>, String)>`；两处 captcha 检查改为先绑定 `html_full = content_retry(&page)`，登录墙重抓路径返回重抓后的那份。私有函数，调用方仅 read/read_full。
- `read`（:421-427）：解构三元组，删除原 `content_retry` 二次取（:427 旧行）。
- `read_full`（:475）：适配三元组（丢弃正文）。
- general.rs html_probe 路径未动（非本文件所有）。

**验证**：cargo check 零错误；`cargo test --bin gsearch postproc::tests` → 14 passed / 0 failed（含 render_read/cap_chars 等既有契约测试，read 输出管线无回归）。

## 3. 7l0 — shell read/browse 对齐顶层语义

**改动**（src/shell.rs）
- `cmd_read`（:326-339）：裸 `ctx.page.content().await.unwrap_or_default()` → `content_retry(&ctx.page)`（-32000 context 重建不再静默吞成空串）；空正文补 stderr hint `[hint] 正文提取为空，试 --full`（对齐 general.rs:239 措辞；shell 无 --markdown，指到实际存在的 --full）。
- `cmd_browse`（:462-464）：goto 后补 `wait_content_stable(&ctx.page, 50)`（50×200ms ≈ 10s，与顶层 general::cmd_browse 同参数）。

**验证**：cargo check 零错误；`cargo test --bin gsearch shell::tests` → 3 passed / 0 failed。stdout 契约未动（仅新增 stderr hint）。

## 4. vag — 四处 CDP 往返包 PAGE_TIMEOUT_SECS 超时

**改动**（src/postproc.rs）
- 新增 `cdp_timeout<F,T,E>`（:643-657）：统一 `tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS))`（30s）；超时/失败折叠为 Err，交由既有语义消化。泛型 future 与 `goto_for_download` 同款，可注入 pending() 单测。
- `content_retry`（:661-669）超时按「未就绪」走既有 wait_dom_complete 重试链；`eval_string_retry`（:671-684）同。
- `page_snapshot`（:135-141）超时上抛 Err → wait_content_stable 按未就绪重置 marker 继续轮询。
- `fetch_in_page`（:762-767）超时折叠进既有「页内 fetch 失败（{url}）…」下载失败链（外层 map_err 文案保持原文）。

**验证**
```
cargo test --bin gsearch postproc::tests
test postproc::tests::cdp_timeout_pending_future_times_out ... ok   ← 新增（start_paused 注入 pending()）
test postproc::tests::goto_for_download_timeout_carries_url ... ok
test result: ok. 14 passed; 0 failed
```

## 5. 3t9 — shell browse/goto 入口接 URL 门（顶层同款）

**改动**（src/shell.rs）
- `cmd_browse`（:452-461）：入口接 `general::ensure_browsable_url(url, false)`；shell 无 --allow-private flag，错误文案追加说明「退出 shell 后用顶层 `gsearch browse <url> --allow-private`」。
- `cmd_login`（:476-478）/ `cmd_dl`（:423-424）：仅接 `general::browsable_scheme_ok`（顶层同款：login/dl 私网不设门）。
- 说明：shell 的 `goto` helper 为 browse/login 共用，门放命令入口而非 goto 内部，否则 login 会被私网门误伤（违背顶层 login 语义）。general.rs 只调用未修改。

**验证**：cargo check 零错误；`cargo test --bin gsearch shell::tests` → 3 passed。门函数本身的行为回归由 general.rs 既有测试覆盖（browsable_scheme_ok / browsable_private_gate_table）。

## 6. n9j — swap 收割 handler 先行 + shell REPL 全路径收尾

**改动**
- src/browser.rs `swap_to_headed`（:541-558）/ `swap_to_headless`（:563-576）：`handler_slot.take()+abort()` 移到 `graceful_close` 之前。依据：graceful_close 的 `close()` 无超时保护（源码仅 wait() 包 5s timeout），handler 卡死时 close().await 永不返回；abort 后 close 立即失败走 warn → wait 5s 超时 → kill 兜底，swap 全程有界，launch 失败路径旧 handler 也不再悬挂。
- src/shell.rs `run_shell`（:65-137）：REPL 收进内层 async 块，browser 停在外层 `Option<Browser>` slot；内层唯二的 `?` 早退点改为 slot 兜底覆盖（page 创建失败）+ break 处理（stdout flush 失败 / read_line Err 按 EOF），外层任何路径先 `graceful_close` 再返回（对齐 main.rs H2 收尾）。

**验证**：cargo check 零错误；`cargo test --lib browser::tests` → 15 passed / 0 failed；`cargo test --bin gsearch shell::tests` → 3 passed。（真实浏览器 swap/REPL 行为属 live 路径，归 PM 合并态 e2e。）

---

## 新增测试清单
| 测试 | 位置 | 覆盖 |
|---|---|---|
| `copy_profile_contents_skips_cache_dirs` | browser.rs tests | 跳过表判定（顶层+子树+本体不动+登录数据照拷） |
| `cdp_timeout_pending_future_times_out` | postproc.rs tests | 超时分支（pending future 必超时）+ 成功值透传 |

## 边界说明
- 按硬纪律只跑了 `cargo check` + 定向单测，未跑全量 build/test/clippy、未 commit。
- live_tests（真 Chrome）未跑，需 `--test-threads=1` 串行，归合并态统一执行。
- c5f 行为差：read 现在用 captcha 检查时那份正文（同页无导航，内容等价，省一次 CDP 往返）。
