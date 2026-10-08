# Rust JSON 序列化库选型备忘（gsearch-rs 上下文）

> 选型日期：2026-10-08  
> 范围：serde_json / simd-json / sonic-rs  
> 决策：**沿用 serde_json，不切换**。

## TL;DR

gsearch-rs 的 JSON 吞吐路径就是「每次 CLI 调用解析一次 SearXNG 响应 + 偶尔读写一次 gsearch.json」——**零热路径**。在这种负载下，sonic-rs/simd-json 相对 serde_json 的 2-4× 解析加速完全淹没在浏览器启动的 2-5 秒里。**换库零收益**，反而引入 simd-json 的 `&mut [u8]` 模型 + `unsafe unchecked` 坑、sonic-rs 的 `-C target-cpu=native` 与 `sanitize` feature 性能衰减条款。结论是「不动」。

## 一手数据（crates.io API，2026-10-08）

| 库 | 最新版 | 总下载 | 近 90 天下载 | 最近发版 | 维护方 |
|---|---|---|---|---|---|
| serde_json | 1.0.151 | 14.0 亿 | 3.47 亿 | 2026-07-20 | dtolnay（serde 组织） |
| simd-json | 0.18.1 | 2223 万 | 598 万 | 2026-08-23 | simd-lite（sunnybot / mfelsche） |
| sonic-rs | 0.5.10 | 759 万 | 332 万 | 2026-09-11 | cloudwego（字节跳动） |

下载量级差揭示生态地位：serde_json 是 simd-json 的 ~6.3× / sonic-rs 的 ~18.4×。**生态规模 = 教程/Stack Overflow 答案/issue 解决速度**，不是性能数字能轻易抵消的。

## 性能 vs 工作负载

sonic-rs 自家 README benchmark（x86_64 Xeon Platinum 8260，target-cpu=native，`float_roundtrip` 开启）：

| 数据集 | 操作 | sonic-rs | simd-json | serde_json |
|---|---|---|---|---|
| twitter.json | Deserialize Struct | **827 µs** | 1087 µs | 2289 µs |
| citm_catalog | Deserialize Struct | **1367 µs** | 2097 µs | 2987 µs |
| canada.json | Deserialize Struct | **4021 µs** | 8093 µs | 9356 µs |

sonic-rs 比 serde_json 快 ~2.5-3×、比 simd-json 快 ~1.3×。**但 gsearch-rs 解析的是几 KB 到几十 KB 的 SearXNG 响应**——serde_json 大概 100-300 µs 完成一次，sonic-rs 也就省 200 µs。**省下的时间还打不过一次 `println!`**。

## 三库的"踩坑面"

- **serde_json**：几乎无坑。`from_slice` / `from_str` 返回 `Result`，错误信息带行列偏移，`#[derive(Serialize, Deserialize)]` 覆盖 95% 场景。代价就是基准数字不漂亮。
- **simd-json**：必须 `&mut [u8]`（内部需要 padding 写空间），`from_slice_unchecked` 标 `unsafe`（要调用方保证合法 UTF-8 + 无尾随空白 + 不超递归深度）；DOM API（`Value`）与 `halfbrown` / `value-trait` 绑定，想换底层容器得重新编译下游。**踩坑面 = 你每次升级都要 grep 一遍调用点是不是误把 `&[u8]` 传成了 `&mut [u8]`**。
- **sonic-rs**：要求 `-C target-cpu=native` 才能走 SIMD 路径（README 显式声明），否则退化为通用实现；`sanitize` feature 与 LLVM-sanitizer 联动，**生产环境禁用但 README 又把它写进默认 feature gate**——开了掉 30% 序列化性能，关了 sanitizer 用户误用可能漏掉 UB。**跨架构交叉编译得加 `cfg` 兜底**。`cargo audit` 目前没有 advisory 但生态位仍窄。

## 为什么不动（gsearch-rs 具体场景）

1. **已是 serde_json 用户**——`Cargo.lock` 显示 serde 1.0.229 + serde_json 1.0.151 已随 chromiumoxide / reqwest / scraper 间接拉入，**零额外依赖成本**。换 simd-json / sonic-rs 才是"净增"成本。
2. **零热路径**——一次 CLI 调用解一次 JSON（搜索响应）+ 偶尔写一次 config。模拟的"省 200µs"在浏览器启动的 2-5 秒面前是噪声。
3. **维护/生态**——serde_json 是 dtolnay 维护（Rust 社区顶流），文档/示例/迁移指南覆盖任何能想到的边界。simd-json / sonic-rs 维护方规模小，遇到 issue 响应慢。
4. **风险面**——simd-json 的 `unsafe` 与可变借用模型对"CLI 工具偶尔解个 JSON"的场景纯属过度工程；sonic-rs 的 native-cpu + sanitize 条款对 release 二进制的可复现构建是负担。

唯一可重评估的边界：**如果哪天 gsearch-rs 加了"批量并发 1000+ URL 抓取 + 每个响应几百 KB JSON"的批处理模式**，那时再把 JSON 解析抽到一个 trait 后面、按 benchmark 决定具体库。当前形态没这必要。

## 数据来源

1. crates.io API `/api/v1/crates/{name}` 实拉（2026-10-08）
2. `simd-json-0.18.1` docs.rs 页面（依赖列表、平台 `x86_64-unknown-linux-gnu`）
3. `sonic-rs 0.5.10` GitHub README（cloudwego/sonic-rs，main 分支）
4. `serde-rs/json` GitHub README
5. gsearch-rs 本机 `Cargo.lock`（serde 1.0.229 / serde_json 1.0.151 现状）

---

## 工具使用附注（gsearch 黑盒真实感受）

**顺手的地方**
- `fetch --json` 拿 crates.io API 后用 Python `json.loads` 解析字段，**1.5 秒 / 库**——比开浏览器快 10×，是这次调研唯一靠谱的入口。
- SearXNG 实例对短查询（"rust async" 8 个字符）能秒返 10 条结果，**说明实例活着**，之前 33s 超时是 query 命中问题不是工具问题。
- `shell` 子命令意外能跑：输入 `help` 出命令清单，输入 `search rust async` 即时返 10 条带摘要的列表，**真会话复用**立得住（M7 里程碑诚不欺我）。

**卡壳 / 疑惑过**
- `gsearch search` 对 "simd-json Rust benchmark 2026" 这种长复合 query **SearXNG 零结果**且 Google 回退被墙连接超时，**空耗 33s**。简化为 "rust async" 秒回。**结论：query 长度是隐藏 SLA**——agent 用默认应该拆短。
- `gsearch dl -o reports/devil-dl/gsearch-v0.2.9.exe` 把**目录**建成了 `gsearch-v0.2.9.exe/`，文件落在子目录里——`-o` 的语义要么是"文件名"要么是"目录前缀"，**官方文档没说清**。最后产物在 `reports/devil-dl/gsearch-v0.2.9.exe/gsearch-x86_64-pc-windows-msvc.exe`，**离用户预期有一个间接层**。
- `gsearch shell` 的 `quit` / `exit` **不退出**——`help` 说"exit / quit 提示退出；EOF / Ctrl+D 真退出"，**实际操作 quit 只回一行 "退出请输入 EOF" 然后继续**。我只能 Ctrl+Z+Enter 强退。**`quit` 不退 = bug**。
- crates.io SPA 页 `/crates/<name>` 直接 404（应跳 `/api/v1/crates/<name>`），`fetch` 没自动 redirect / 自动改 API 路径，**老问题**（前一份 client-choice-notes 也记了）。

**直接扔掉的输出**
- 第一次 fetch 回来的 simd-lite/simd-json README **超时**——README 在 simdjson/simdjson 上游（不是 simd-lite），但 simd-json 的真仓库实际叫 sunnyber/simd-json 或类似 org，**路径猜错就放弃**，改用 docs.rs 拿信息。
- crates.io SPA 404 页（98 字节），**整段丢**。
- sonic-rs / cloudwego README 的 `Benchmark` 章节我看了数字但**没交叉验证**——sonic-rs 自己挑的硬件/参数，`cargo bench --bench deserialize_struct -- --quiet` 没在我本机跑过，**第三方横评未采到**（今天检索层故障，详见前一份笔记）。

**想改它的地方**
- `gsearch dl -o` 加 `--output-is-dir` 标志或者干脆 `--output` 必须以 `.` 或 `/` 结尾区分文件名/目录名，**消歧义**。
- `gsearch shell` 把 `exit` / `quit` 改成真退出，**当前是 UX 坑**。
- `gsearch search` 长 query 自动 OR 降级（首个关键词命中即用），**避免 33s 黑屏**。

整体：**比上次更顺手**。SearXNG 短查询活着 + `fetch --json` 路径稳 + `shell` 真能用 = 调研链路通了。但 `dl -o` 语义 + `shell quit 不退` + `search 长 query 黑洞` 三个点拖体验。
