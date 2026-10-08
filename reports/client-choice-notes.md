# Rust HTTP 客户端选型备忘录（reqwest / ureq / hyper，附 rquest / isahc 对照）

**结论先行**：业务项目默认 reqwest；无 async 栈、追求最小依赖与可审计性选 ureq；只有"客户端本身就是产品"才直用 hyper。rquest 排除，isahc 仅特殊需求考虑。

## 一手数据（2026-10-08，crates.io API + 官方仓库 README）

| 库 | 总下载 / 近90天 | 最新 stable | 最近更新 | 模型与定位 |
|---|---|---|---|---|
| hyper | 985M / 225M | 1.12.0 | 2026-10-06 | async 低层构建块，HTTP/1+2，官方自称性能标杆；reqwest/axum/warp 的底座 |
| reqwest | 779M / 207M | 0.13.5 | 2026-09-08 | async+blocking 高层客户端，rustls 默认，cookie/代理/JSON/multipart/WASM |
| ureq | 222M / 66M | 3.4.2 | 2026-09-13 | 纯 blocking，纯 Rust（forbid unsafe），依赖最少，HTTP/1.1，rustls/native-tls 可选 |
| isahc | 17.7M / 1.2M | 2.0.1 | 2026-07-05 | libcurl 后端（curl-sys），HTTP/2 经 libnghttp2，默认静态链接 bundled libcurl |
| rquest | 0.31M / 31K | **无 stable** | 2025-07-11 | TLS 指纹伪装客户端；已停更 15 个月，仓库主分支已转型为 netty（低层协议库） |

## 分析

1. **reqwest**：社区事实标准，下载量与生态最强，API 覆盖全（含 blocking 客户端）。代价是依赖树大、与 tokio 生态亲和。不写 async 业务代码也能用它的 blocking 模式，但依赖树不会因此变小。
2. **ureq**：CLI 工具/脚本的甜点区——同步 API 上手即用、依赖树最小、`forbid(unsafe)` 利于安全审计；局限是不支持 async、仅 HTTP/1.1、3.x 的 TLS provider 配置走 unversioned API 有 semver 风险。
3. **hyper**：性能与协议覆盖（HTTP/1+2，HTTP/3 在生态内）都是底座级，但它是构建块不是便利层——README 自己都说"要方便的客户端请看 reqwest"。业务项目直用会写大量样板。
4. **rquest**：唯一差异化卖点是 TLS/JA3 指纹伪装（过反爬）。但 crates.io 无 stable 版、停更 15 个月、作者已把仓库转做 netty——供应链风险不可接受，排除。
5. **isahc**：价值在 libcurl 生态独有特性（SPNEGO/GSS-API 协商认证、静态链接分发），代价是引入 C 工具链与交叉编译负担。一般业务项目没有选它的理由。

## 局限声明

第三方性能横评本次未采到（当日检索层故障，详见文末工具心得），性能结论仅基于官方一手声明（hyper："Leading in performance"；ureq："low-overhead"）。落地前建议用目标真实负载做一轮压测对拍 reqwest/ureq 两条候选路径。

---

## 工具使用心得（gsearch 黑盒试用，2026-10-08）

**1. 好在哪里**
- `fetch` 子命令纯 HTTP 不起浏览器，抓 raw.githubusercontent/docs.rs 这类轻源 0.5-1.2s 出结果，是本次唯一好使的路径；
- `search` 支持 batch 多查询并发（单条失败不阻塞），设计对批量调研场景很对味（虽然今天没跑通）；
- `--no-humanize` 明确给 agent 留了快路径，帮助文本里连"人用保留默认"都写了，作者懂用户分层；
- 报错信息带原因链（`error: 请求失败 → 原因: operation timed out`），404 和超时分得清，不用猜。

**2. 鸡肋**
- `search` 的 warmup + 指纹补丁默认开启，对 agent 是纯开销（好在有开关，但默认不该让机器等人做拟人化）；
- GitHub 仓库页 fetch 回来的"正文"60% 是导航栏和营销区块（"AI CODE CREATION"之类），信息密度极低。

**3. 不好用**
- `fetch` 没有正文提取/去噪，HTML 壳全量吐出，得自己 sed 筛——对 token 消耗不友好；
- 输出只进 stdout，没有 `--to-file` 或 `--json`（status/耗时/字节数元数据）；
- search 挂掉时只报"查询无结果"，不提示下一步可以跑 `doctor` 自诊断，用户得自己翻 help。

**4. 如果我自己改**
- `fetch` 加 readability 式正文提取 + `--json` 元数据输出，这是最高频痛点；
- `search` 失败时自动附带一行 doctor 结论（SearXNG 端点通不通、Google 回退是否被墙）；
- GitHub 站点做适配器：仓库页只留 README 区块。

**5. 希望加的功能**
- 批量 `fetch` 多 URL 并发（对齐 search 的 batch 语义）；
- 同一 URL 结果缓存（本次 reqwest 仓库页抓了 3 次超时，前两次白等）。

**6. 综合修改意见（优先级）**
1. fetch 正文提取 + 元数据（每次调用都受益）
2. search 故障自诊断提示（今天这种 SearXNG 零结果 + Google 回退连接超时的场景，一行诊断能省用户十分钟）
3. 批量 fetch 并发
4. GitHub 页面噪音过滤

**本次事故记录（照实）**：2026-10-08 09:51Z，SearXNG 对所有查询返回零结果（端点活着但无结果），Google 直爬回退报 `net::ERR_CONNECTION_TIMED_OUT`，`search` 全链路不可用；`fetch` 走另一条网络路径基本正常（仅 github.com 个别仓库页超时）。改用 fetch 抓 crates.io API 与 raw README 完成调研。
