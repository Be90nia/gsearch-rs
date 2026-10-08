# Rust 异步运行时选型备忘（gsearch-rs）

> 选型日期：2026-10-08  
> 范围：tokio / async-std / smol（顺带 glommio 作对照）  
> 决策：**沿用 tokio，不切换**。

## TL;DR

async-std 已在 2025-03-01 官方废弃（RustSec RUSTSEC-2025-0052），新项目不再考虑。smol 定位为「库作者友好、轻量」，CLI/网络代理的甜蜜点不匹配。glommio 仅 Linux + io_uring 场景。本项目已通过 `chromiumoxide` + `reqwest` 深度耦合 tokio，迁移零收益——**结论是「不动」**。

## 候选速览

| 运行时 | 状态 | 定位 | 何时用 |
|---|---|---|---|
| **tokio** | 活跃，企业支持 | 通用 / 网络服务默认 | 90% Rust 异步项目 |
| async-std | ⚠️ **2025-08 官方废弃** | 原「std 镜像」 | 仅存量 1754 crate 维护 |
| smol | 活跃（taiki-e） | <1500 行、不强加执行器 | 写库 / 嵌入式 / 极小二进制 |
| glommio | 活跃 | thread-per-core + io_uring（Linux only） | DB / 代理 / 消息总线 |

## 性能数据（lucaberton 2026-05-04 实测）

环境：128 核 AMD EPYC、10Gbps、50K 并发连接。

| 运行时 | req/s | P50 | P99 | 内存 |
|---|---|---|---|---|
| tokio | 1.2M | 0.8ms | 4.2ms | 85MB |
| smol | 1.0M | 0.9ms | 5.1ms | **45MB** |
| async-std | 900K | 1.1ms | 6.8ms | 92MB |
| glommio | **1.8M** | **0.4ms** | **1.2ms** | 120MB |

要点：tokio 是 smol 的 ~1.2× 吞吐，glommio 是 ~1.5× 吞吐但 Linux-only 且要 thread-per-core 模型。

## 维护 / 生态

- **tokio**：生态完全压倒——axum / hyper / tonic / tower / reqwest / tracing 全部围绕；tokio.rs 官方页直接背书。
- **async-std**：corrode.dev 2026-07-30 确认「as of March 1, 2025, async-std has officially been discontinued. The suggested replacement is smol」；wrenlearnsrust 2026-03-18 引 RUSTSEC-2025-0052 公告；docs.rs 顶部红字「async-std has been discontinued; use smol instead」。**出 ReFS 死刑。**
- **smol**：docs.rs 2.0.2（2026-09-05），模块化 = async-executor + async-io + async-task；可借 tokio reactor 兼容生态——是「与 tokio 共存」而非「替换 tokio」。

## 为什么不动

1. **已耦合** — `chromiumoxide` 0.9 强制 tokio runtime，`reqwest` 也基于 tokio。换运行时 = 改 CDP 客户端 + 加兼容层，无收益。
2. **场景不匹配** — smol 的甜蜜点是「不强制 runtime 的小二进制库」；gsearch-rs 是要 stdio 启动、有 profile 持久化、有 headless 浏览器、有登录态检测——恰好是 tokio 的主场（hyper/tower/select 工具齐全）。
3. **风险** — 主动迁到一个被官方 RUSTSEC 公告的依赖上 = 不可接受。

唯一可重评估的边界：若哪天抽出「不绑 runtime 的库」给上层用户选 runtime，再把 IO 边界 trait 化 + `#[cfg]` 编译门——但当前形态没这必要。

## 数据来源

1. corrode.dev "The State of Async Rust: Runtimes" — 2026-07-30，Matthias Endler
2. lucaberton.com "Rust Async Runtime Deep Dive: Tokio vs async-std vs smol" — 2026-05-04
3. wrenlearnsrust.com "The End of async-std: What Rust Developers Need to Know in 2026" — 2026-03-18
4. docs.rs/async-std 1.13.2 顶部「discontinued; use smol instead」
5. docs.rs/smol 2.0.2（2026-09-05）
6. tokio.rs 官方

---

## 工具使用附注（gsearch 真实感受）

**顺手的地方**
- `fetch` 对博客类纯 HTML 极快：corrode.dev / lucaberton / wrenlearnsrust 三页 0.5-1.7s 拉回可读正文，比 `browse`（要起 Chrome）省掉一整轮浏览器启动。
- 短查询 + `--no-humanize` 跳过 warmup，agent 反复调时体感几乎 0 延迟。
- SearXNG 命中后的 `search` 6s 内出 10 条带摘要 + URL，按时间倒序 + 摘要长度筛选友好。

**卡壳 / 疑惑过**
- 第一轮 query 写「tokio vs async-std vs smol Rust async runtime benchmark」——SearXNG 直接返回「查询无结果」且 HTML 降级也空，触发 Google 直爬回退，撞 `net::ERR_CONNECTION_TIMED_OUT`，**空耗 33s**。简化成「tokio async runtime Rust」就秒回。**教训：SearXNG 对长复合 query 容差低，agent 调用应默认拆短 + 用 `--limit` 兜多轮。**
- `fetch https://github.com/tokio-rs/tokio` 返回的是 GitHub 顶部 nav chrome，正文需要 `browse`（JS 渲染）才能拿到 README/About。**纯 fetch 对 SPA 站点无解**——我后来直接放弃 GitHub repo，迁去 docs.rs / 博客拿信息。
- `fetch https://crates.io/crates/tokio` 拿到 404，应该是路径结尾斜杠或 anti-bot；没去 debug，绕开。
- 看 `audit-quality.md` 提到 `fetch` 撞 JS 壳会退 1 + stderr 提示；这次没踩到，但记一笔：拿到非 200 退出码时第一反应应该看 stderr 而不是重试。

**直接扔掉的输出**
- 3-4 个搜索结果是同一篇 corrode.dev 文章被 blog 镜像站转贴，标题党但内容是同一份，**全部扔**。
- 第一轮那条 0 结果的搜索 + 33s 浏览器回退 trace，**没看**，只取最终 query 命中的结果。
- GitHub repo 页面的 nav HTML，**整段丢**。

**想改它的地方**
- `fetch` 加 `--include <css selector>` 跳过 nav/footer，对 GitHub / Medium / Substack 这类壳重正文轻的站点一键出正文。
- `search` 默认 query 分词 + OR 兜底，复合 query 失败时自动降级到「任一关键词」，避免上面那种 33s 黑洞。
- `search` 返回的 URL 表里**直接标注是否 docs.rs / GitHub / 个人博客**——同类内容聚合时一眼能挑权威源，不用肉眼筛。
- 给 `crates.io` 路径加 `redirect: follow` 默认 + 在 404 时建议补 `/api/v1/crates/<name>`，免去手工猜路径。

整体：gsearch + 自托管 SearXNG 这套，对「挑 2-3 个权威源 + 拉正文」的小调研**完全够用**；唯一会被 0 结果 + 浏览器回退拖死的是「长 query 撞不上搜索引擎」这种 query 设计问题，不是工具本身。
