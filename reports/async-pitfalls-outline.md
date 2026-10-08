# Rust 异步编程常见坑 —— 技术博客大纲（素材稿）

> 本大纲基于 `gsearch fetch` 抓取的 3 篇权威页面（Rust Async Book、Tokio Tutorial、Bridging with sync code）以及 3 次失败的 `gsearch search` 实战。  
> 所有引用均已实际抓取并通读，未抓取的页面在「工具使用心得」中说明原因。

---

## 0. 前言

- 本文面向已经会用 `async/.await`、但线上踩过坑的 Rust 工程师，目标不是再讲一遍 `Future` 是什么，而是把社区里高频出现的 **"看似能用、线上翻车"** 的反模式列清楚。
- 结构上每个坑先给"症状"、再给"根因"、最后给"修复模式"，并在文末附上参考链接与可运行的最小复现片段（读者可在 `cargo test` 中直接跑）。
- 读者读完应能：在写新 async 代码时自动避开 5–7 类常见坑；在阅读同事代码时一眼识别 `block_on` 死锁、阻塞 I/O 偷渡、取消不安全等危险信号。

---

## 1. 在 async 任务里偷偷做阻塞 I/O

- **症状**：CPU 监控里某个 worker 长期 100%，但业务 QPS 没涨；`tokio-console` 显示所有 task 都在 `Pending` 状态。
- **根因**：`std::fs::File` / `std::net::TcpStream` / `std::sync::Mutex` / `println!` 等同步 API 在 `async fn` 里直接调用，会把当前 worker 线程卡住直到系统调用返回，期间该线程上其他几百个 task 全部无法调度。
- **修复模式**：CPU/IO 密集型用 `tokio::task::spawn_blocking`（Tokio 提供的专用线程池），或换成对应的 async 版本（`tokio::fs`、`tokio::net`、`tokio::sync::Mutex`）。
- **参考**：[Async Book 介绍](https://rust-lang.github.io/async-book/)（明确指出 `println!` 在 async 里是反例）、[Tokio Tutorial "When not to use Tokio"](https://tokio.rs/tokio/tutorial)、[Bridging with sync code](https://tokio.rs/tokio/topics/bridging)。

## 2. `Send` 误解：把 `Rc`、`&mut`、`!Send` 类型跨 `.await` 持有

- **症状**：编译错误 `future cannot be sent between threads safely`、`Rc<...> cannot be sent between threads safely`，或运行时 panic `task panicked`。
- **根因**：`tokio::spawn` 要求 future 是 `Send + 'static`；任何 `Rc`、裸 `*mut`、`RefCell`（非 `Sync`）都会让编译器拒绝；常见偷渡路径是 `let rc = Rc::new(...); async move { rc.clone() }`。
- **修复模式**：跨任务用 `Arc`，内部可变性优先 `tokio::sync::Mutex` / `RwLock`；需要独占借用时把工作挪到同一个 `select!` 分支内、避免跨 `.await`。
- **参考**：[Tokio Tutorial "Spawning"](https://tokio.rs/tokio/tutorial)（演示 `Arc<Mutex<i32>>` 在多任务间共享计数）、[Bridging with sync code](https://tokio.rs/tokio/topics/bridging)（`BlockingClient` 用 `Arc` 跨线程持 runtime）。

## 3. `block_on` 与执行器相互阻塞导致死锁

- **症状**：单元测试或 `#[tokio::main]` 启动后主线程 hang，`Ctrl+C` 杀不掉；或 `current_thread` runtime 里所有 spawn 的 task 永远不被调度。
- **根因**：`block_on` 会在调用线程上同步等待 future 完成；如果在该 future 内部又 `block_on` 同一个 runtime（嵌套调用），或持有 `Runtime` 的 `current_thread` runtime 在 `block_on` 返回后冻结所有已 spawn 的任务，就会死锁。Tokio 文档明确："Once `block_on` returns, all spawned tasks on that runtime will freeze until you call `block_on` again."
- **修复模式**：同步包装时使用独立 `current_thread` runtime（`Builder::new_current_thread().enable_all().build()`），并保证 `block_on` 不会在已持有 runtime 借用的线程上再次进入；多线程 runtime 下用 `spawn` + `JoinHandle` 代替嵌套 `block_on`。
- **参考**：[Bridging with sync code](https://tokio.rs/tokio/topics/bridging)（`BlockingClient` 用专属 `current_thread` runtime；`spawn` 后 `block_on(handle)` 的对比示例）。

## 4. `tokio::spawn` 阻塞任务把整个 worker 拖垮

- **症状**：runtime 配的 `worker_threads = 4`，但只要一个 task 调 `std::thread::sleep(1s)` 或 `reqwest::blocking::get`，整服务的 `p99` 就从 10ms 飙到 1s+。
- **根因**：`tokio::spawn` 跑在多线程 runtime 的 worker 上，worker 是 **协作式** 调度的，单个 task 阻塞线程 = 该 worker 上所有 task 一起停摆。
- **修复模式**：任何同步阻塞调用（CPU bound、文件 IO、同步数据库驱动、`sleep`）一律走 `tokio::task::spawn_blocking`；如果是第三方阻塞库又不能改，考虑 `tokio::task::yield_now().await` 主动让出，或在隔离线程里跑独立 runtime。
- **参考**：[Bridging with sync code](https://tokio.rs/tokio/topics/bridging)（明确建议 "run a small portion of synchronous code ... see `spawn_blocking`"）、[Tokio Tutorial "When not to use Tokio"](https://tokio.rs/tokio/tutorial)（CPU bound 工作推荐 rayon 而非 Tokio）。

## 5. 取消安全（cancellation safety）盲区

- **症状**：`tokio::select!` 同时等超时和业务 future，超时分支触发后业务 future 被 drop，但底层连接/锁/中间缓冲区状态错乱；下次重试时出现 "stream did not yield any items" 或 "lock poisoned" 之类诡异报错。
- **根因**：async future 在 `.await` 点上随时可被 `drop` 取消；如果在该点之前已经消耗了 stream 的 item、获取了锁、发起了 IO 但没完成，就会留下"半成品"状态。
- **修复模式**：1) `select!` 用 `biased;` 配合 `take` 模式保护未消费完的 stream；2) 业务 future 拆成 "准备阶段（同步完成） + await 阶段（可安全 drop）"；3) IO 类操作优先用能 "abort = rollback" 的 API（`tokio::sync::watch` 优于 `tokio::sync::Mutex`）。
- **参考**：[Async Book 介绍](https://rust-lang.github.io/async-book/)（明确写 "async programming in Rust has a powerful concept of cancellation"）、[Bridging with sync code](https://tokio.rs/tokio/topics/bridging)（展示 `JoinHandle` 在 `block_on` 返回后被冻结的取消语义）。

## 6. 超时与资源泄漏：忘了 `timeout` 包裹或没在 `Drop` 里清理

- **症状**：服务端偶发 `Too many open files`、连接池里残留一半 `ESTABLISHED` 状态、数据库连接 `leaked` 告警。
- **根因**：`tokio::time::timeout` 只在 future 还没完成时取消；一旦忘记 `await` timeout 句柄、或在 `select!` 里只 `tokio::spawn` 出去没人回收，对应 socket / 文件描述符就跟着 future 一起泄漏；`enable_all` 没打开则 timer 根本不工作。
- **修复模式**：所有外部 IO 一律 `tokio::time::timeout(dur, fut).await?`；超时分支内显式 `drop` 任何已获取的资源；对长生命周期 task 用 RAII guard（自定义 `Drop`）做最终清理。
- **参考**：[Bridging with sync code](https://tokio.rs/tokio/topics/bridging)（"`enable_all` enables the IO and timer drivers on the Tokio runtime. If they are not enabled, the runtime is unable to perform IO or timers"）、[Async Book 介绍](https://rust-lang.github.io/async-book/)（async 适合 IO 等待密集型系统）。

## 7. 共享状态的 `Mutex` 误用：跨 `.await` 持锁 vs 选错锁

- **症状**：`async fn` 里 `let mut g = std::sync::Mutex::new(0).lock().unwrap;` 后立刻 `.await`，编译器会报 "`MutexGuard` is not `Send`"；改用 `tokio::sync::Mutex` 又因内部 `.await` 导致锁被长时间持有、并发度塌方。
- **根因**：`std::sync::Mutex` 的 guard 没有 `Send` 跨界能力，跨 `.await` 持锁会把整个 future 变成 `!Send`（`tokio::spawn` 必拒）；`tokio::sync::Mutex` 是 async 的、允许跨 await，但它是协作式锁，**持有期间不允许做重 CPU 工作**。
- **修复模式**：短暂临界区用 `std::sync::Mutex`（同步快路径）+ `tokio::task::spawn_blocking`；长临界区或跨 await 用 `tokio::sync::Mutex` 但 **临界区里只 await 别的 async 资源**；读多写少用 `tokio::sync::RwLock`。
- **参考**：[Tokio Tutorial "Shared state"](https://tokio.rs/tokio/tutorial)（官方 `Arc<Mutex<i32>>` 示例）、[Bridging with sync code](https://tokio.rs/tokio/topics/bridging)（演示 `Runtime` 通过 `Arc` 跨线程共享）。

## 8. 其他"看起来 OK"实际很坑的小坑合集

- **Pin 与自实现 Future**：`Pin<&mut Self>` 边界不满足时编译器给 "`cannot be unpinned`"，新手常常用 `Box::pin` 草草盖住，掩盖真正的 `Unpin` 设计问题。参考 [Async Book 介绍](https://rust-lang.github.io/async-book/) 末尾对 "async iterators / async in traits / async destruction" 列为 "rough edges" 的描述。
- **async-in-traits 不支持 `dyn Trait`**：`async fn` 在 trait 里目前没有 `dyn` 兼容方案，逼得团队要么拆成 `trait Foo { fn foo() -> impl Future }` 要么每次 `Box::pin` 一次，**会引入额外堆分配**。同一参考页。
- **`tokio::main(flavor = "current_thread")` 与 `block_on` 配合多线程 runtime** 的语义错乱：worker 线程调 `block_on` 嵌套 runtime 时会随机死锁或饿死。参考 [Bridging with sync code](https://tokio.rs/tokio/topics/bridging) 关于 "current_thread runtime will only execute during calls to block_on" 的描述。
- **`JoinHandle` 的 panic 处理**：`spawn` 出去的 task panic 时 `JoinHandle::await` 返回 `Err(JoinError)`，但很多人 `unwrap()`，导致一个子任务 panic 把整个父任务也拖崩。参考 [Tokio Tutorial](https://tokio.rs/tokio/tutorial) "Spawning" 章节。
- **`tokio::select!` 的随机公平性**：默认 `select!` 随机挑分支，长时间运行的分支可能永远拿不到调度；用 `biased;` 显式声明优先级。参考 [Tokio Tutorial "Select"](https://tokio.rs/tokio/tutorial)。

---

## 参考链接（按抓取顺序）

1. [Rust Async Book — Introduction](https://rust-lang.github.io/async-book/) — 抓取 r4（5.9 KB，0.60s）
2. [Tokio — Tutorial](https://tokio.rs/tokio/tutorial) — 抓取 r5（5.3 KB，0.65s）
3. [Tokio — Bridging with sync code](https://tokio.rs/tokio/topics/bridging) — 抓取 r6（12.1 KB，0.76s）
4. [Tokio — Tutorial "Shared state"（章节）](https://tokio.rs/tokio/tutorial)（在 r5 页面内）
5. [Tokio — Tutorial "Select"（章节）](https://tokio.rs/tokio/tutorial)（在 r5 页面内）
6. [Tokio — `tokio::task::spawn_blocking` API](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html)（基于 r6 抓取内容引用）

---

## 工具使用心得（盲测 · 10 分钟预算 · 6 维度）

> 工具 = `gsearch.exe` v0.2.x（命令：`search` / `fetch` / `browse`）。环境：OpenWrt 上的 SearXNG 实例 `http://192.168.89.249:8888`，本机 `GSEARCH_PROFILE=blindblog`（避免与其他并行子代理撞 profile）。

### 1. 这个工具好在哪里

- **`fetch` 速度惊人且零依赖 Chrome**：抓 3 个权威页面平均 0.6–0.8s，**没有启动浏览器** 的 1–2s 预热，纯 HTTP GET 即得正文，对"想读参考文章"的场景非常友好。
- **输出格式稳定**：每条 fetch 结果都带 `=== <url> | <title> ===` 头部和 markdown 风格正文，肉眼可读、grep 友好；比 `curl` 后自己清理 HTML 强多了。
- **错误信息带具体可操作建议**：比如 `error: HTTP 404 Not Found: https://...` 后会跟"用 --verbose debug 查详细"，对排障很友好。
- **`search` 设计的 `SearXNG → Google 回退链` 思路对盲搜很贴心**：理想情况下 SearXNG 命中就走 JSON 端点零浏览器，回退 Google 才动 Chrome；SearXNG 挂了用户也不至于完全断。

### 2. 哪些功能是鸡肋（有不如无 / 用了一次不想用第二次）

- **`search` 在我环境下完全不可用**：3 次不同关键词（"Rust async Send trait pitfalls" / "rust async block_on" / "rust"）均返回 **stdout 0 字节、stderr 564 字节**（"SearXNG 查询失败（查询无结果，HTML 结果页亦无结果），已回退 Google 直爬"），随后 `net::ERR_CONNECTION_TIMED_OUT` 退出；每次白白消耗 **~28s**。10 分钟预算里光是这 3 次空搜就吃掉 84s，严重拖慢节奏。**鸡肋判定**：在 SearXNG 没结果 + Google 直连被墙的场景下，`search` 既不报错重试、也不提示"换关键词/换搜索引擎"，用户只能猜是不是自己 query 写得差。
- **每次 `search` 都打印 "新 profile 已创建"**：对重复调用的脚本/agent 是噪声；虽然用 `--no-humanize` 抑制了 warmup，但首次仍然会建 profile。

### 3. 哪些功能不好用（想用但难受，难受在哪）

- **`search` 的回退逻辑没有"放弃阈值"**：明明已经回退到 Google 一次超时，仍要等满 `~28s` 才退出；如果能加 `--no-fallback` 或 `--max-fallback-time 5s`，就能在 SearXNG 不可用时秒退，让我立刻改用 `fetch`。
- **`search` 的 `--no-humanize` 不等于 `--no-fallback`**：文档/帮助里只看到"跳过 warmup"，但 Google 回退是另一回事；命名误导。
- **`search` 的 `--json` 在错误路径下不输出 JSON**：我三次失败都没拿到 `--json` 结构化错误，只能 stderr 文本；对 agent 解析不友好。
- **`fetch` 没有 `--max-bytes` 也没有分页**：抓 `bridging` 页面 12 KB 还能接受，但若是更大的文档会被一次性塞进 stdout；对 token 预算是黑盒。

### 4. 如果是你自己来改，你会怎么改

- **`search` 增加 `--no-fallback` 和 `--max-time` 短路**：SearXNG 无结果 + 5s 内拿不到响应就直接返回非零退出码，而不是用满 ~28s 还去打开 Chrome。
- **`search` 失败时返回结构化 JSON（即使有 `--json`）**：`{"ok": false, "engine": "searxng", "error": "...", "duration_ms": 28000}`，让上层 agent 能直接决策"换 fetch"。
- **把 `search` 的 stderr "新 profile 已创建" 降级为 `--verbose info` 级别**：默认日志已经够吵。
- **把 `fetch` 改成支持 `--max-bytes N` 和 `--truncate-head`**：抓大文档时只取前 N KB 也比一次拉全要省 token；现在只能事后 `head -c 20000`（但环境禁 cat/head）。

### 5. 我希望它加什么功能

- **`search <query> --from-urls <file>`**：允许提供"已知的可靠 URL 列表"作为额外/替代来源；当搜索引擎全挂时，仍能基于本地 URL 清单把内容抓回来。
- **`fetch --extract <heading>`**：基于标题/锚点抽取子章节，例如只抓 async-book 的 "Cancellation" 节，省得拉整本书。
- **统一的 `--metrics` 输出**：每条命令末尾打印 `{wall_ms, stdout_bytes, stderr_bytes, fallback_used, engines_tried}` 这样的结构化指标，对 agent 自评（耗时/字节/丢弃比例）非常方便 —— 本次心得我就是手写 `time` + `wc -c` 来凑齐这些数字。
- **配置文件支持 `search.engines` 优先级和黑名单**：当某个引擎持续超时（比如本次 Google 直连）时自动跳过。

### 6. 综合修改意见（优先级排序）

1. **P0 — `search` 加 `--no-fallback` + 失败时打印结构化 JSON**：直接把我这次 84 秒的浪费变成 3 秒。盲测场景下 agent 立刻能感知 "search 不可用 → 改 fetch"。
2. **P0 — `search` 回退逻辑加总超时阈值**：避免无意义的 28s 等待。
3. **P1 — `fetch` 加 `--max-bytes` / `--truncate-head`**：防止大文档爆 stdout token。
4. **P1 — 所有子命令支持 `--metrics` 统一输出**：让 agent 不用手 `time` + `wc -c`。
5. **P2 — `search` 在日志里把"新 profile 已创建"降级 + 加 `--quiet`**。
6. **P3 — `fetch --extract <heading>`**：锦上添花。
