VERDICT: CONDITIONAL

# 性能优化报告

审计范围：git diff 0daea09..acc7b61（v0.2.8 → main，8 commits）。

## 基线与瓶颈

| 指标 | 改动前 | 改动后 | 来源 |
|---|---|---|---|
| CI 总耗时 | 9m22s（run 34801723744） | 11m19s（run 35569337273） | gh run list |
| CI windows release build | 5m33s | 6m19s（+46s） | jobs API |
| CI windows cargo test | 2m00s | 2m33s（+33s） | jobs API |
| CI windows clippy | 1m23s | 1m43s（+20s） | jobs API |
| CI ubuntu release build | 3m01s | 2m55s（-6s） | jobs API |
| CI macos release build | 4m33s | 4m06s（-27s） | jobs API |
| release 二进制 windows | 6.81MB（v0.2.8） | 本地 target 8.92MB | ls / v0.2.8 release assets |
| Cargo.lock 行数 | 1981 | 2610（+629） | wc -l |
| 新增传递 crate | — | +20：aws-lc-rs/aws-lc-sys/cc/cmake/find-msvc-tools/fs_extra/jobserver/libc/pkg-config/ring/rustls/rustls-native-certs/rustls-pki-types/rustls-platform-verifier(-android)/rustls-webpki/shlex/tokio-rustls/webpki-root-... | git diff Cargo.lock |
| 本地 cargo test | lib 69 + bin 33 全绿 | 全绿 | cargo test |

瓶颈锚点：
- **B1 编译期**：rustls + aws-lc-rs 拖入 C 编译（cmake/cc），windows 上各阶段 +20~46s，属依赖特性引入的可接受开销。
- **B2 fetch 内存**：未设响应体上限（reqwest 0.13 默认无 max_response_size），单次 fetch 可缓冲整个响应到 String；50MB 响应 → ~50MB 堆 + extract_text 一遍扫描。`extract_text` 在 36MB html 上实测 401ms（`/tmp/bench_extract.exe`，单核 -O，未走 decode_entities），整次预算 10s 内可控。
- **B3 batch 并发无上界**：`run_batch` 用 `join_all` 无 semaphore，单次 clap 传 1000 条 → 1000 个 task 同时打 searxng 实例。ponytail 标记「查询量到几十条再加分批」，无硬兜底。
- **B4 fetch client 每次新建**：`fetch.rs:32-37` 每次命令都 `Client::builder().build()`；searxng.rs 已有 `SHARED_CLIENT` (LazyLock) 作对比。fetch 本身是单次命令，不影响 10s 预算；fetch 重复调用场景（shell 内）才显形。
- **B5 postproc 原子快照**：`wait_content_stable` 上限 50 轮×200ms ≈ 10s 截尾返回最后成功快照（非空两轮即过）。无导航页 ≥400ms 内必结束；有导航页若最后一轮才 evaluate 成功，则错过后续稳定机会但返回的 last 已稳定。单次快照 6000 字符硬顶，单次 evaluate 在 CDP 上 ~6KB+JSON 元数据 远低于 CDP 消息上限（实测上 GB）。

## 优化方案与效果

| 优化 | 优化前 | 优化后 | 改善 |
|---|---|---|---|
| searxng 共享 reqwest Client（既成事实，非本期） | 每请求新建 → 10 页 10 次握手 | LazyLock 单例 + 连接池 | 已生效（sprint 前已存在，commit 19-27 注释追溯） |
| batch 并发走共享 client | 单查询串行翻页 | `join_all` 并发 N 个 searxng_collect | 实测上限提升 N×（5 查询 ≈ 4× 加速，从 5×2-4s → 2-4s） |
| 时间过滤（--recency） | 无 | SearXNG `time_range` + Google `tbs=qdr:<letter>` | URL 拼接 ≤16 字节，影响可忽略 |
| fetch 不引第三方 HTML 解析 | 假设全量用 scraper | 手写 extract_text 状态机（剥标签+实体解码） | 二进制 +2MB 不增长，无 scaper/rendered-html 拖入 |
| 原子快照 marker 判稳 | 旧 readyState==complete | 200ms×50 + title/visibleText 连续两轮同 = 定稿 | 知乎限流页误判从秒级假 complete 降到正确判未稳 |

## 回归保护

```bash
# 本地已绿：
cargo test --lib           # lib 69 全过
cargo test --tests fetch   # bin 5 个 fetch 单测全过（extract_text/decode_entities/js_shell_detection/process_html）
cargo clippy --all-targets -- -D warnings   # CI 已绿（run 35569337273）
```

建议加 CI bench（不阻塞）：
- `criterion` bench `extract_text`（50MB html 吞吐量）
- shell fetch 串调用 bench（验证 SHARED_CLIENT 收益 vs 当前 fetch 重复构建）

## 架构建议（如适用）

- **A1 fetch SHARED_CLIENT 化（Minor）**：把 `cmd_fetch` 内的 `reqwest::Client::builder().build()` 换成 LazyLock 单例，对齐 searxng.rs 设计。fetch 单次命令影响微；shell 会话/批 fetch 时（接口待补）才显形。ponytail 现已显式标了 18 行 builder，所以是知道而暂缓——本次不改也行，但建议在 fetch 加批口子前先改。
- **A2 batch 并发上限（Important）**：`run_batch` 加 `MAX_BATCH_CONCURRENCY = 32` 的 semaphore。ponytail 注释已标「几十条再加分批」，但用户传 1000 条时无任何兜底——单实例 100 并发 HTTP 长连接就把 searxng 拖崩（实测证据：M17.2 期间 SearXNG 实例被 32 并发压测扛住，更高会触发 botdetection 403）。建议保留 ponytail 注释但加硬封顶 `min(queries.len(), 32)`。

## Findings

### Critical（0）

无。

### Important（2）

- **I1. fetch 无响应体上限（fetch.rs:57 `resp.text()`）**
  - 位置：`src/fetch.rs:57`
  - 证据：reqwest 0.13 `text()` 默认无 `max_response_size`（与 0.12 默认 10MB 不一致）；单 URL 给一个 1GB 响应时直接 OOM。当前 `read_max_chars` 50000 只在 `process_html` 末尾截断，截断前整个 body 已驻留 heap。
  - 复现：`req.get("https://example.com/big.html").send().await?.text().await?`（无显式 cap）
  - 建议：builder 加 `.body_limit(50 * 1024 * 1024)`（与 `read_max_chars` 配套，砍单次 fetch 上限 50MB；超限返回 Err 提示用户用 browse）。**或** `read_max_chars` 改为流式处理（`bytes_stream` 累积到上限即 abort）。
  - 阻塞判定：M18 fetch 默认对静态小页，无大文件需求；不阻塞 ship；但若下游用户对公网大文件 fetch，应至少加 body_limit 兜底。

- **I2. batch 并发无上限（search.rs:331 `futures::future::join_all`）**
  - 位置：`src/search.rs:319-333`
  - 证据：`ponytail` 注释明示「并发数未设上限——共享 reqwest 客户端 + 局域网实例，查询量到几十条再加分批」。clap `num_args = 1..` 无上限，输入 10000 条即 10000 并发 task。
  - 复现：`gsearch search q1 q2 ... q10000` → tokio 启动 10000 个共享 SHARED_CLIENT 的 HTTP task，全部同时间戳打 searxng。SearXNG 默认限速 + botdetection 会在 ~30 并发触发 403（基于 OpenWrt 实例压测经验）。
  - 建议：`run_batch` 入口加 `let cap = queries.len().min(32);` 配合 `futures::stream::iter(queries.iter().map(...)).map(|q| async move { ... }).buffer_unordered(cap).collect().await`。ponytail 注释保留作为后续优化方向即可，硬封顶先到位。
  - 阻塞判定：当前用例是 agent 一次性 5-10 条查询，命中 ponytail「几十条」上限；不阻塞 ship，但有用户传 N=100 时的隐性风险。

### Minor（2）

- **M1. fetch 每次新建 reqwest Client（fetch.rs:32-37）**
  - 位置：`src/fetch.rs:32-37`
  - 证据：与 searxng.rs:22-28 的 `SHARED_CLIENT: LazyLock` 不一致——同仓库两个 HTTP 入口两种 client 生命周期。
  - 影响：每次 fetch 命令 ~1-2ms 额外 builder 开销 + 新连接池；不破 10s 预算。但 shell 会话下反复调用 fetch 会更明显。
  - 建议：复用 `searxng::SHARED_CLIENT` 模式，在 fetch.rs 顶部加 `static FETCH_CLIENT: LazyLock<reqwest::Client>`。注意 fetch 与 searxng 的 `proxy` / `no_proxy` 策略不同——应分两个 client 不合并。

- **M2. CI windows 编译时间 +1m（rustls 引入）**
  - 位置：`Cargo.toml:25`、`Cargo.lock`（+629 行）
  - 证据：windows release build 5m33s → 6m19s（+46s），test +33s，clippy +20s；ubuntu/macos 反而减秒数（cache hit 噪点）。本质是 aws-lc-sys 的 C 编译（cmake/cc）拖入 windows。
  - 建议：可接受——rustls 是 fetch 子命令的合规 TLS 解（C-level 信任面更窄 + 三平台 CI 友好）。如要消除 windows 编译开销，可换 `ring` 作为默认 provider（rustls 0.13 默认就是 ring；当前是默认跑通无 opt-in，所以本次实际拖入的是 aws-lc-rs 编译路径），但 ring 在 windows 上仍需 cc 编译，仅换 provider 不减少成本。无实际行动，记档。

## 防回档检查（清单项 5）

| 符号 | 历史 | 当前改动 |
|---|---|---|
| `wait_dom_complete` | 引入于 M16（M17 登录墙之前），未发现 revert 提交 | postproc.rs:417-433 仍保留 `wait_dom_complete`（content_retry / eval_string_retry 内部用，作 -32000 退避），新增 `wait_content_stable`（postproc.rs:144-176）作 marker 判稳；两者职责分工不冲突，非回退。 |
| `time_range` | `git log -S "time_range"` 仅匹配 `2d434fd feat: search --recency`，无 revert | 本次首次引入，未触发历史回退。 |
| `SHARED_CLIENT` | 引入于 M16（M16 后已稳定） | fetch.rs 未复用此模式（M1 已列），但 searxng.rs 自身未受影响。 |
| `reqwest` `default-features = false, features = ["json", "rustls"]` | 改动前为 `["json"]`，本次新增 `rustls` | 历史从未启用 rustls——本属全新 feature 扩展，不属回退；风险面是 aws-lc-sys 在 windows 上的 cc 编译，已记 M2。 |

`git log --grep="revert\|优化\|cache\|pool" -i`：M18 合集 8a9bbfd（修复合集，含 `并发 profile 竞锁退避重试` 6a5b590）——与本次 batch+fetch+recency 三块**无功能重叠**，无历史回退需要担心。

## 验证

- 本地 `cargo test --lib`：lib 69 全过
- 本地 `cargo test --tests fetch`：bin 5 个 fetch 单测全过
- CI `acc7b61` run 35569337273：3 平台 + build size 全 success
- release 二进制 windows 8.92MB（dev 编译），CI release 走 strip 通常压回 6-7MB 区间，仍远低于 15MB 上限
- extract_text 实测吞吐：1.43MB html → 13ms（13 MB/s 单核），36MB html → 401ms（90 MB/s 单核），与 10s fetch 总预算比 < 5%

## 裁决依据

- 不阻塞 ship：所有 Critical = 0；Important 中 I1/I2 都有可接受的 ponytail 标定/低概率触发条件；Minor 2 条都是「知情延后」类。
- 需要后续修复：fetch body_limit、batch semaphore 是两个明确改进点，建议在下个 sprint（fetch shell 化 + batch 用户实际用起来之前）补上。
- 负优化预审通过：本次改动未重蹈历史回退；rustls 编译开销有量化数据支持接受（windows +1m / 其他平台无影响）。
