# gsearch-rs 深度内存+性能审计报告（PerfAudit）

- 日期：2026-10-10
- 对象：main 62edd30（v0.2.9），src 22 文件 13649 行
- 方法：静态读码（fetch.rs / postproc.rs / searxng.rs / duckduckgo.rs / shell.rs / shell_snap.rs / convert.rs / search.rs / browser.rs / general.rs / skeleton.rs / config.rs / util.rs 全读或关键路径全读）+ git 历史 / beads 负优化回查 + cargo 只读查询
- 只读审计：未改任何代码；未跑构建/测试（并行互踩约束）
- 工具：serena-cli 6 次（symbol-body/overview）+ grep/read 定位；codebase-memory 图与 graphify 图均不存在于本仓库（cbm-bridge 提示有图但 `graphify-out/graph.json` 缺失），按口径降级

VERDICT: CONDITIONAL

---

## 0. 负优化审查（硬性前置，已做）

- `git log --grep="perf|优化|cache|pool|revert|回退|慢" -i`：20 条命中全是盲测功能修复（FixG*），**无性能优化被 revert 的历史**。
- beads：无「性能/优化/缓存/内存」在册 issue。
- 结论：无历史回退信号需要纳入设计约束。历史上下文中与本次相关的唯一先例是 searxng.rs 的 Low① 客户端复用修复（`SHARED_CLIENT`，src/searxng.rs:22-29 注释）——**fetch 侧至今未跟进**，见 F-1。

---

## 1. 执行摘要（按「预期收益×改动风险」排序）

| # | Finding | 位置 | 收益 | 风险 |
|---|---------|------|------|------|
| F-1 | fetch 每次 attempt/每 URL 重建 reqwest Client，batch 无连接复用 | fetch.rs:371（build_client 定义 :211） | 高 | 低 |
| F-2 | `search --read N` 全页 content() 双取（captcha 检查 + 正文各一次） | postproc.rs:514 + :427 | 中高 | 低 |
| F-3 | profile fork 递归拷贝含 Cache/Code Cache，launch 关键路径 GB 级磁盘 I/O | browser.rs:346-402（copy_profile_contents / copy_dir_recursive） | 中高（条件触发） | 低 |
| F-4 | fetch `--markdown` 全页三次 html5ever 解析 + 两处无谓全量克隆 | convert.rs:73/80/99 + fetch.rs:1225 | 中 | 低-中 |
| F-5 | `--include`/host 路由路径全页二次解析 | fetch.rs:1225 + :1461 / skeleton.rs:214 | 中 | 中 |
| F-6 | wait_content_stable 每次 read/browse 固定 +200ms + 双次全 DOM TreeWalker | postproc.rs:33,151-190; 调用 general.rs:144, postproc.rs:552 | 中（协议拍板） | 中 |
| F-7 | strip_summary_elements 无条件全页克隆（无 `<summary>` 页也付 10MB 拷贝） | fetch.rs:1189-1213 | 低 | 低 |
| F-8 | collapse_blank 三段式分配链；非 HTML 大文本峰值 ≈3×body | fetch.rs:1228,1391-1399,1426-1449 | 低 | 低 |
| F-9 | 阻塞 DNS（std ToSocketAddrs）在 async 上下文与重定向每跳 | fetch.rs:66-79（resolve_host），:225-233（redirect policy 内调用） | 低 | 低 |
| F-10 | shell 用阻塞 stdin.read_line 占死一个 tokio worker（单核机会饿死 CDP handler） | shell.rs:80-95 | 低 | 低 |
| F-11 | tokio `features=["full"]` 可裁 signal/io-std（实际节省≈0，传递依赖已拉主体） | Cargo.toml:15 | 低 | 低 |
| F-12 | skeleton first_sentence 每段 Vec\<char\> collect | skeleton.rs:289-310 | 低 | 低 |

外部成本声明：SearXNG 往返 766-1359ms、Chrome 冷启动、Google 撞码等人解均为网络/外部主导，本报告不列为代码 finding；代码侧放大点（可并行未并行、重复建连、重复解析）单列如上。

---

## 2. 内存专项

### 2.1 单命令峰值（大 HTML 处理路径）

峰值构成（fetch 单 URL，body 上限 FETCH_BODY_LIMIT=10MB，fetch.rs:28）：

```
buf: Vec<u8>（流式累积，上限截断）
→ String::from_utf8_lossy(&buf).into_owned()   ≈1×body（fetch.rs:432 附近）
→ strip_summary_elements 全量 out 克隆          ≈+1×body（F-7）
→ Html::parse_document（ego-tree）              ≈3-5×body 瞬态
```

即理论峰值 ≈ 5-7×body（~50-70MB @10MB body），**有界、不失控**。F-7/F-8 修掉后可到 ≈4-5×body。非 HTML 路径的 `collapse_blank(html.to_string())`（fetch.rs:1228）额外放大约 3×（克隆 + out + trim().to_string()），10MB JSON 源场景 ≈30MB 瞬态（F-8）。

### 2.2 shell 会话长驻增长面：**未发现只增不减结构**

- `last_results` / `last_snap` / `current_url`：每条命令**整替**而非累积（shell.rs:51-58 定义，cmd_search:170 `ctx.last_results = results`、cmd_snap:216 `ctx.last_snap = elems`）。
- REPL 读入 `buf.clear()` 每轮复用（shell.rs:90）。
- Chrome 子进程生命周期：`graceful_close` = close + 5s 超时 wait + kill 兜底（browser.rs:934-953），无泄漏路径；`fetch_in_page` base64 通道已有 32MB 拒绝阈值（postproc.rs:660-661，I5 已收口）。
- 唯一长驻增长体是 Chrome 自身（渲染进程/堆），属外部成本。

### 2.3 clone 链检查（深拷贝三问）

- `fetched_json` 的 `f.text.clone()`（fetch.rs:610 附近）≤50KB（max_chars 上限后），**可接受**。
- `launch_with_retry` 每轮 `config.clone()`（browser.rs:822-838）仅失败重试路径，**冷路径**。
- `run_batch` 的 `q.clone()` / slots 重排：查询串级别，**可忽略**。
- 结论：无热路径大对象无谓 clone 需要动（F-7/F-8 属管线中间产物克隆，已单列）。

---

## 3. CPU / 启动专项

### 3.1 启动到首字节

- `#[tokio::main(flavor = "multi_thread")]`（main.rs:355）：spawn num_cpus 个 worker，~1-3ms；tracing-subscriber env-filter init（main.rs:341-352）与 clap parse 均 <1ms；config OnceLock 单次读盘（config.rs:47-50）。**代码侧启动开销合计 <10ms，无 finding**。SearXNG 快路径耗时为网络主导（已有实测 766-1359ms），符合口径不报。
- release profile（lto=fat/codegen-units=1/strip/panic=abort）已到位，不重报。

### 3.2 热路径重复解析（核心 CPU finding，见 F-4/F-5）

全库 `Html::parse_document` 站点（grep 实证）：convert.rs:73、duckduckgo.rs:94、fetch.rs:1225/1413/1461、parse.rs:20、searxng.rs:147、skeleton.rs:84/214。

放大链最重的是 **fetch `--markdown`**：

1. `process_html` 全页解析（fetch.rs:1225）——其正文产物被 markdown 覆盖，仅 title 被消费；
2. `sanitize_pre_blocks` 全页再解析（convert.rs:73）+ `doc.html()` 全页序列化回字符串（convert.rs:99）；
3. htmd `converter.convert()` 内部第三次解析。

docs.rs 类 2MB 页 ≈ 3 次全量 html5ever 解析 + 1 次全页序列化（html5ever parse ≈ 50-150ms/MB 量级，静态估算）。无 pre>code 的页面还白付 `html.to_string()` 全页克隆（convert.rs:80，F-4 子项）。

`--include` / host 路由（F-5）：`process_html` 全页解析 → `extract_with_include` 全页再解析（fetch.rs:1461）→ 每命中块 `extract_text` 块级再解析（fetch.rs:1413，块小可接受）；GitHub host 路由走 `github_comment_html` 全页再解析（skeleton.rs:214）。修法是解析一次把 `&Html` 传下去（extract_with_include 本质只需要 doc.select + inner_html，不依赖重新 parse）。

正则/选择器重复编译：`Selector::parse` 均为每次调用一处、无循环内重复编译；全库**零 regex 依赖**。无 finding。

### 3.3 轮询与忙等（见 F-6/F-10）

- `wait_content_stable`（postproc.rs:151-190）：静态页最少 2 次快照 + 200ms 固定 sleep；每次快照是一次 CDP evaluate + 全 DOM TreeWalker（SNAPSHOT_MAX_TEXT_CHARS=6000 硬顶，postproc.rs:30）。browse（general.rs:144）与 search --read（postproc.rs:552）每命令固定付这 ≥200ms。这是 j44 判稳契约的设计成本（注释已自认"约 10s 上限/无固定 sleep"指窗口语义），**任何放宽需 PM 拍板**，故列 CONDITIONAL 项而非确定 finding。
- `poll_until_solved`（search.rs:690-737）：1s tick 的 `page.content()` 全量序列化——但撞码页本身是小时级 HTML，代价低，**不报**。
- shell `cmd_read --full`（shell.rs:322-341）：先 `page.content()` 做 captcha 检查再 innerText——`--full` 时 content 白取一次（F-2 同族，shell 版无 retry 语义差异）。

---

## 4. I/O 与并发专项

### F-1【高收益/低风险】fetch 每次 attempt 重建 reqwest Client

- **证据**：fetch.rs:371 `let client = build_client(...)` 位于 `fetch_one_attempt` 内；该函数被 fetch_one 重试 loop（fetch.rs:337-360）逐次调用，batch 路径 `cmd_fetch_batch`（fetch.rs:656-667）对每 URL 调 fetch_one。即 **每 URL×每 attempt 一个新 Client**（新连接池、新 TLS session cache、新 DNS cache）。
- **触发场景**：`gsearch fetch url1..urlN` batch（并发 5，FETCH_CONCURRENCY，fetch.rs:30）对同 host 多 URL：N 次完整 TCP+TLS 握手（同 host 本可复用 keep-alive 连接；HTTPS 握手 ~100-300ms/次公网）；单 URL 重试场景也丢连接复用。
- **对照**：searxng.rs:22-29 已有 `SHARED_CLIENT: LazyLock` 先例（Low① 修复注释自述"原来每次请求都 builder().build()"）——同一问题在 fetch 侧未跟进。
- **修法**：在 `cmd_fetch`/`cmd_fetch_batch` 入口按 (proxy, allow, timeout) 构建一次 `reqwest::Client`（单次命令内三参数恒定；SSRF 重定向门是 builder 级配置，逐 URL 传入的只有 URL 本身），`fetch_one_attempt` 改收 `&Client`。general.rs:493/502（cmd_dl 直连路径一次命令建至多 2 个 client）与 update.rs:40 同族，一并复用。
- **预期收益**（静态估算）：batch 同 host 5 URL 场景省 4 次握手（数百 ms）；风险：无——client 参数在单命令内不变。

### F-2【中高收益/低风险】search --read N 全页 content 双取

- **证据**：postproc.rs:514 `if is_captcha(&content_retry(&page).await)`（open_page，全页 DOM 序列化第 1 次）→ postproc.rs:427 `let html_full = content_retry(&page).await;`（read，第 2 次）。browse 路径无此问题（general.rs:146 的 html_probe 同时喂 captcha 检查与提取）。
- **触发场景**：`search --read N` 打开 GitHub issues 类 1-2MB 页：page.content() 是全 DOM outerHTML 序列化 + WebSocket JSON 传输，大页单次 ~50-300ms + html 大小级内存；双取 = 延迟与峰值内存 ×2。
- **修法**：`open_page` 返回 `(page, snap, Option<String>)`（captcha 检查用的 content 直接带回，read 复用；read_full 路径可返回 None 或同样复用做 captcha 判定）。改动面：open_page/read/read_full 三个函数签名。
- **预期收益**：大页 read 少一次全页序列化+传输；顺带削峰 1×html。

### F-3【中高收益/低风险（条件触发）】profile fork 全量拷贝含 Cache

- **证据**：browser.rs:346-402 `copy_profile_contents` 遍历顶层后对 `Default/` 走 `copy_dir_recursive`（:386-402），只跳过锁文件与 symlink——**Cache / Code Cache / GPUCache / Service Worker 全部递归拷贝**。注释自述目标是"copy cookie/历史/GAEX"（browser.rs:262-263），与实现不符。
- **触发场景**：多 agent 并发同一 default profile（fork 机制存在的场景，browser.rs:266-306 + launch 第二层防御 :842-858）→ fork 在 launch 关键路径同步做 std::fs 递归拷贝；养熟的 profile 带 Cache 可达数百 MB-GB 级 → 命令启动卡顿数十秒 + 磁盘空间翻倍。且 fork 目录从不清理（fork-<ts>-<pid>-<rand> 每次新建），磁盘持续增长。
- **修法**：copy_dir_recursive 增加目录名跳过表（`Cache`、`Code Cache`、`GPUCache`、`Service Worker`、`Crashpad`、`GrShaderCache` 等）；可选：fork 目录加 aged 清理（非本次必须）。
- **预期收益**：fork 路径 I/O 量降 1-2 个数量级。

### F-9【低】阻塞 DNS 在 async 上下文

fetch.rs:66-79 `resolve_host` 用 `std::net::ToSocketAddrs`（阻塞）；调用点：fetch_one → gate_check（async 内，fetch.rs:331）+ build_client 的 redirect Policy::custom 闭包内每跳调用（fetch.rs:225-233）。OS 缓存命中时 ms 级，但慢解析器场景会占死 tokio worker。修法：async 入口用 `tokio::net::lookup_host`；redirect 闭包是 sync，可预解析首跳后对同 host 走小缓存（或接受现状，量级小）。**低优先**。

### 并发语义检查（无 finding 项）

- **reqwest 复用**：searxng ✓（SHARED_CLIENT）；ddg 走 curl 子进程（5s --max-time，duckduckgo.rs:36-46）✓。
- **超时/重试语义**：fetch 30s×(1+retry≤3) 带 1/2/4s backoff、确定性错误不重试（fetch.rs:336-360）✓；SearXNG 5s（searxng.rs:24）✓；浏览器路径 30s goto 超时兜底（search.rs:785-794 / postproc.rs:549-553）✓。
- **锁**：全库零 Mutex/RwLock，无跨 await 持锁问题 ✓。
- **batch 并发**：fetch buffered(5) 保序 ✓；search batch buffer_unordered(min(n,32))，SearXNG ~30 并发 403 风险注释已认账（search.rs:447-451），PM 已知不重报。

---

## 5. 依赖拖挂

| 依赖 | 实际使用面 | 结论 |
|------|-----------|------|
| tokio `full` | rt-multi-thread/macros（main.rs:355, tokio::test）、time（sleep/timeout 遍布）、net（TcpStream×3 处）、process（ddg curl）、io-util（AsyncRead/WriteExt，general.rs:487、main.rs:1436）、fs（general.rs:527 tokio::fs::File）；signal/io-std 无使用 | 可去 signal+io-std，**但 chromiumoxide/reqwest 传递依赖已拉 rt/sync/time/net/io-util，净节省≈0**；纯编译期收益，低优先（F-11） |
| futures | 仅 StreamExt + iter（fetch batch / search batch）；已是 chromiumoxide 传递依赖 | 零边际成本 ✓ |
| htmd | 仅 convert.rs 消费 | 按需 ✓ |
| reqwest | json/rustls/stream 均有消费者；native-tls 无消费者（**PM 已知，不重报**） | ✓ |
| scraper/tracing-subscriber/clap/anyhow/serde | 核心消费 | ✓ |

---

## 6. 优化清单（按 预期收益×改动风险 最终排序）

| 序 | 项 | 收益 | 风险 | 改动面 | 验证方式 |
|----|----|------|------|--------|----------|
| 1 | F-1 fetch Client 进程级复用（含 dl/update 同族） | 高 | 低 | fetch.rs/general.rs/update.rs 签名传参 | batch 5×同 host fetch 前后计时对比（握手次数从 5→1） |
| 2 | F-2 open_page 带回 content 免双取 | 中高 | 低 | postproc.rs 3 函数签名 | 大页 `search --read N` 耗时对比（少一次 content 往返） |
| 3 | F-3 fork 拷贝跳过 Cache 类目录 | 中高 | 低 | browser.rs copy_dir_recursive 加跳过表 | 造锁场景触发 fork，对比拷贝耗时与 fork 目录体积 |
| 4 | F-4 --markdown 解析漏斗收敛（title 单独小解析 + Cow 免克隆） | 中 | 低-中 | fetch.rs/convert.rs | docs.rs 大页 `fetch --markdown` 前后计时；输出逐字节 diff |
| 5 | F-5 解析一次传 &Html（include/host 路由） | 中 | 中 | fetch.rs/skeleton.rs 函数签名 | `fetch --include` / GitHub URL 输出逐字节 diff + 计时 |
| 6 | F-6 判稳协议放宽（静态页单快照早退） | 中 | 中（**需 PM 拍板 j44 契约**） | postproc.rs wait_content_stable | read/browse 延迟分布 p50 对比；风控页回归 |
| 7 | F-7 strip_summary 返回 Cow | 低 | 低 | fetch.rs 1 函数 | 大 HTML 计时 + 既有单测 |
| 8 | F-8 collapse_blank 零拷贝重写 | 低 | 低 | fetch.rs 2 函数 | 既有 collapse 系单测逐字节 |
| 9 | F-9 DNS 异步化 | 低 | 低 | fetch.rs | 功能不回归即可 |
| 10 | F-10 shell stdin 移出 worker（spawn_blocking+channel） | 低 | 低 | shell.rs | 单核机 shell 手测 |
| 11 | F-11 tokio 裁 feature | 低 | 低 | Cargo.toml | cargo check（预期净收益≈0，可选） |
| 12 | F-12 first_sentence 去 Vec\<char\> | 低 | 低 | skeleton.rs 1 函数 | 既有单测 |

P0-3 候选（1-3 项）合计预期：batch fetch 与大页 read 的代码侧延迟削减数百 ms 级、fork 场景削秒级-十秒级 I/O；均不触碰输出契约（stdout 逐字节不变），回归风险低。

### 回归保护建议（供主代理/PM 落地时采纳）

- 现有单测已锁输出契约（fetch process_html/collapse/strip_summary/include 投影等逐字节断言），1/2/4/7/8/9/12 落地后跑 `cargo test` 即可覆盖输出不变性。
- 计时基线：`gsearch fetch` 对同一 5×同 host URL 集 batch 3 次取 p50；`search --read N` 对 GitHub issue 大页 3 次 p50——与本次报告的静态估算对照。

---

## 7. 架构建议（locality / deletion test）

- F-1/F-2 的修法都收敛在各自 module 内部（fetch.rs 内部传参、postproc.rs 内部签名），不向调用方泄漏新接口，deletion test 通过。
- F-4/F-5 若做「解析一次传 &Html」，注意 fetch.rs 的 `process_html(html:&str)` 与 `extract_with_include(html:&str)` 是两个独立入口（后者被 host 路由复用），建议新增 `&Html` 变体而非改签名语义，避免第二套约定。
- 不建议为本轮收益引入 HTTP 缓存层/连接池抽象——searxng 的 `LazyLock<Client>` 单例模式已是本仓库既有惯例，照抄即可（最小 diff）。

## 8. 审计覆盖声明

- 全读：fetch.rs（代码段 1-1560，尾部为测试）、postproc.rs、searxng.rs、duckduckgo.rs、shell.rs、shell_snap.rs、convert.rs、search.rs、browser.rs、config.rs、util.rs；抽读：main.rs（入口/搜索 wiring/tracing）、general.rs（browse/dl 路径）、skeleton.rs（提取内核）、parse.rs（选择器站）、types/output/verify/stealth/update（低热，按信号扫过）。
- 未验证项：所有收益均为静态估算（只读约束，未跑 benchmark 对比）；F-6 的收益上限取决于真实页面 DOM 规模分布。
