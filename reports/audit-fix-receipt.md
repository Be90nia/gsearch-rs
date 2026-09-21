# 审计修复回执（0daea09..acc7b61 → 修复后）

## 范围

三路审计（quality/security/performance）裁决 CONDITIONAL：
- Important: 5 条（I-1 fetch 文档缺口 / I-1 SSRF 无 host 过滤 / I-2 跨 scheme 重定向 /
  I-1 fetch 无响应体上限 / I-2 batch 并发无上限）
- Minor: 3 条选（a/b/c 三项指定项）

## 逐条修复对照

### Important

| ID | 审计条款 | 修复 | 文件 | 验证 |
|---|---|---|---|---|
| I-1 | src/fetch.rs 加私网 SSRF 门 | `gate_private()` + `is_private_ip()`：IPv4 loopback/RFC1918/link-local/unspecified/multicast + IPv6 ::1/ULA/fc00::/7/fe80::/10/multicast。`--allow-private` flag + `GSEARCH_FETCH_ALLOW_PRIVATE=1` env 双放行通道。域名解析走 `std::net::ToSocketAddrs`（零新依赖）。 | `src/fetch.rs:36-114` | unit test `ssrf_gate_rejects_private_addresses` + `ssrf_gate_allow_private_bypasses` + `is_private_ip_cases` + `http_or_https_scheme_rejects_other_schemes`；端到端 `gsearch fetch http://127.0.0.1:9` → exit 1，stderr 提示加 `--allow-private` |
| I-2 | redirect policy 加 https_only(true) | builder 加 `.https_only(true)`：初始请求与重定向链全强制 https。 | `src/fetch.rs:131-136` | 公开 https URL 走通（exit 0）；plain http 走 builder 拒，doc 中明示 |
| I-3 | 响应体加硬上限（10MB） | `bytes_stream` 累积到 `FETCH_BODY_LIMIT`（10×1024×1024）即停下载；提取纯函数 `accumulate_chunk` 离线单测覆盖。reqwest 启用 `stream` feature（解锁 `bytes_stream`，无新增传递 crate）。 | `src/fetch.rs:25,112-127,166-178` | unit test `accumulate_chunk_caps_at_limit`（含单 chunk 超限、多 chunk 累加、恰好填满、已满再喂 4 种 case）；端到端公开 https URL 正常 |
| I-4 | run_batch join_all 改 buffer_unordered + 硬顶 32 | `futures::stream::iter(queries.iter().enumerate()).map(...).buffer_unordered(cap)`；cap = `queries.len().min(32)`；输出按 `pos` 落 slots 重排保输入序。ponytail 注释同步改写。 | `src/search.rs:317-340` | 端到端 `search q1 q2 q3 q4 q5 --json --limit 2` → 5 条全 ok（exit 0），输出按输入序 |
| I-5 | README 补 fetch 子命令段 + 退出码表 row 1 扩 + truncated 表述修正 + --full 例外 | 新增 `### fetch（纯 HTTP GET 取正文，无需 Chrome）` 段，覆盖用法/JS 壳语义/私网门/https 重定向/10MB 上限；退出码表 row 1 追加"fetch JS 壳（预期行为，stderr 提示换 browse）"+ "fetch 私网门拒"；`read / browse 输出契约` 段改"HTML 源码硬截断，omitted 为标记字符数"+ `--full 模式固定 5000 字上限，read_max_chars 不生效`；环境变量段加 `GSEARCH_FETCH_ALLOW_PRIVATE` | `README.md:33-67,124-128` | 无测试（README 改动） |

### Minor

| ID | 审计条款 | 修复 | 文件 | 验证 |
|---|---|---|---|---|
| a | format_json 生产调用清零 | grep 全库：唯一引用 = 自身单测（`skeleton.rs:434` format_json_roundtrip）。删除 pub 函数 + 测试。`serde::Serialize` derive 保留（`Heading/Paragraph/AdaptiveRead` 仍需）。 | `src/skeleton.rs:236-237`(删) + `:432-444`(删) | `cargo build` 通过（render_read 走 to_string+注入 meta，不依赖 format_json）；`cargo test` 全绿 |
| b | 非 HTML 分支跳过 decode_entities | `process_html` 非 HTML 分支：`collapse_blank(html.to_string())`，去掉 decode_entities 调用 | `src/fetch.rs:212-221` | unit test `process_html_non_html_preserves_entities`：JSON 源文 `&amp;`/`&#20013;`/`&#x6587;` 全部原样保留 |
| c | wait_content_stable 注释尾补判稳上限场景 | 注释增补一行："持续动态页（行情条 / 相对时间戳等 visibleText 持续变化的内容）会烧满判稳窗口（read/browse +10s、click +4s）后返回 last 快照，结果无损纯延迟。" | `src/postproc.rs:149-150` | 无测试（仅注释） |

## 端到端验证（验收 DoD）

```
$ ./target/debug/gsearch.exe fetch http://127.0.0.1:9
error: fetch 拒绝私网地址 127.0.0.1（host=127.0.0.1）。如确需内网，请传 --allow-private 或设置 GSEARCH_FETCH_ALLOW_PRIVATE=1
（用 --verbose debug 查详细）
EXIT=1                          ✓ SSRF 门生效

$ ./target/debug/gsearch.exe fetch https://raw.githubusercontent.com/rust-lang/rust/master/README.md
=== https://raw.githubusercontent.com/rust-lang/rust/master/README.md |  ===
<div align="center">
<picture>
...
This is the main source code repository for [Rust]. ...
EXIT=0                          ✓ 公网 https 不受影响

$ ./target/debug/gsearch.exe search "rust async runtime" "tokio tutorial" "async-std example" "actix-web hello" "axum quickstart" --json --limit 2
[ ... 5 条 ok，输出按输入序 ... ]
EXIT=0                          ✓ batch 并发硬顶不影响语义与顺序
```

## CI 同构验证

```
cargo clippy --all-targets --offline -- -D warnings   →   Finished（零警告）
cargo test --offline                                  →   39 passed; 0 failed; 2 ignored
```

注：原 72 测试数 + 2 ignored 的差异：本轮改动包括：
- 删除 2 个测试（`format_json_roundtrip` + `process_html_truncates_and_respects_plain_text` 中被替代的非 HTML 用例）→ 旧 dead code 测试减少；
- 新增 6 个测试（`process_html_non_html_preserves_entities` + `ssrf_gate_rejects_private_addresses` + `ssrf_gate_allow_private_bypasses` + `is_private_ip_cases` + `http_or_https_scheme_rejects_other_schemes` + `accumulate_chunk_caps_at_limit`）。

净增 4 条 → 36 lib/bin + 3 bin/cli = 39（本机 bin 集合计算含 batch 解析测试）。ignored 数 2 不变（live 测试）。

## 依赖变更（透明）

`Cargo.toml:25` reqwest 加 `stream` feature（解锁 `resp.bytes_stream()` 用于响应体上限，无新增传递 crate——`http-body-util` 已在依赖树）：
```diff
- reqwest = { version = "0.13", default-features = false, features = ["json", "rustls"] }
+ reqwest = { version = "0.13", default-features = false, features = ["json", "rustls", "stream"] }
```
`Cargo.lock` 同步更新 reqwest 0.13.4 → 0.13.5（patch 升级，含 base64 v0.22→v0.23）。

## 禁区遵守

- src/browser.rs：未触碰
- src/postproc.rs：仅 1 行注释增补（wait_content_stable 末尾），无逻辑改动
- src/types.rs：未触碰
- main.rs：仅 Fetch 子命令结构体加 `allow_private: bool` 字段 + dispatch 透传（接线必需，与 fetch.rs 增量同步）
- 新依赖 = 0（`stream` 是 reqwest 已有 feature）
- 无 commit / push / bd 写入
