# 竞品调研：2026 年 AI/agent 搜索栈 + gsearch-rs 的 SearXNG 实效诊断

> 日期：2026-11-13　方法：GitHub API 扫描 + 官方文档核查 + 3 路 librarian 并行（托管 API / 开源栈 / 反方）+ 本机与路由器实测
> 关联：`reports/cross-project-gsearch-feedback/report.md`（跨项目使用反馈）

## 0. 结论速览

1. **你的体感是真的，但根因不是"我们架构落后"，而是"出口 IP 是阿里云机房段"**——SearXNG 侧一半引擎被判定为机房流量打死（实测 brave=0 / google=0 / DDG 23 次 CAPTCHA/30min），剩下的引擎质量差异很大。
2. **"竞品比我们强"的感觉需要打折**：独立基准榜头差距在统计噪声内（n=100、p≈0.8 时 95% CI ≈ ±8pp，而榜头 Exa deep 83.0 vs Exa auto 81.7 差 1.3pp）；同一 endpoint 在两个榜上一个第 9 一个垫底。
3. **真差距在别处**（arXiv 2608.01913 实测）：同一固定 retriever 下 gold recall 52.0%→78.7%，**26pp 差异只能来自 query 表述**——检索器侧的差距远小于提问侧的差距。
4. **最该抄的三件**（按实证收益排序，均不需要付费 key / GPU）：① snippet 门控与去重（snippet 占上下文 66–85%）② 近重复 query 标记（唯一稳健的失败预测量，ρ=-0.83）③ 输出层格式归一（白捡 17.7–19.9% 的 agent 错误）。

---

## 1. SearXNG 实效诊断（实测，非推测）

### 1.1 完整链路

```
SearXNG 容器 172.18.0.3
  → 路由器 nft table inet passwall（透明代理 mark 0x50535731，redirect/tproxy 生效）
  → xray → 阿里云节点（所有 passwall 节点 remarks 均 yzsw-sg-*，address 指向
     company.begonia92.top / sg.yzswgroup.top —— 同一后端的不同协议）
  → 出口 IP 8.219.85.68（阿里云机房段）
  → 搜索引擎按"机房/共享 IP"判定 → CAPTCHA / 403 / 429
```

**关键排除项**：
- SearXNG **没有配任何代理**（`settings.yml` 的 `outgoing` 段只有 `request_timeout: 3.0` / `max_request_timeout: 4.0` / `enable_http2: true`）——不是配置 bug。
- 容器出口 IP 与路由器本机出口 IP **相同**（均 8.219.85.68）→ 流量确实被透明代理劫持。
- 节点池**没有第二个出口 IP**：全部指向同两个域名 → **换协议/换节点不改变出口 IP**（与 2026-09-29 故障同源）。

### 1.2 引擎存活实测（逐引擎 `!engine` 探测，英文查询）

| 引擎 | results | 引擎 | results |
|---|---|---|---|
| brave | **0（挂）** | github | 30 |
| google | **0（挂）** | mojeek | 28 |
| duckduckgo | 10（活，但近 30 分钟 **23 次 CAPTCHA**） | yahoo | 25 |
| bing | 10（活，**中文查询错位**） | startpage | 25 |
| yandex | 10 | qwant | 25 |
| wikipedia | 27 | wikidata | 26 |

容器近 30 分钟错误计数：`SearxEngineCaptchaException` ×26（duckduckgo 23）/ `TooManyRequests` ×9 / `AccessDenied` ×3 / `HTTP 403` ×3 / google ×10 / brave ×3。

### 1.3 结果质量实测（gsearch 真跑）

| 查询 | 结果 | 判 |
|---|---|---|
| `rust async` | 3 条全 docs/blog | ✓ |
| `如何用Python读取CSV` | CSDN×2 + 菜鸟教程 | ✓ |
| `tokio vs async-std 性能对比` | CSDN + async-std + RayByte | ✓ 可接受 |
| `支付宝` | 3 条全是「登录 - 支付宝」 | ✗ 全是登录页，无实质内容 |

→ **问题集中在特定查询类型**（品牌/商业词），不是全盘。

### 1.4 本机代理侧（附带发现）

本机 Windows 上 10808 / 10809 / 7890 / 1080 **全部 closed**；`GSEARCH_PROXY=127.0.0.1:10808` 已失效（os error 10061）。直连 Google 10s 超时。路由器 `127.0.0.1:1070`（xray socks）**可用**（git push 走它成功）——但它是**显式 socks**，不是透明代理。

---

## 2. 竞品机制清单（开源/自托管线，11 个项目，38 条机制）

> 完整条目见 librarian 原始产出（OpenSourceStack）；此处按"对 gsearch-rs 的可借鉴性"重排。

### 2.1 最贴定位的直接竞品：wigolo（5.5K★，TS，本地优先，$0/query）

https://github.com/KnockOutEZ/wigolo —— 明确对标 Exa/Firecrawl/Tavily，主打 **no keys, no cloud**。

**提高搜索的机制**：
- **多引擎直连适配器（18 个）→ RRF 排名融合 → 本地 ONNX reranker（`bge-reranker-v2-m3`）→ 相关性阈值丢弃低分**
- 内嵌 `BAAI/bge-small-en-v1.5` embedding → 语义缓存索引 + `find_similar`
- **honest failure labels**：被反爬挡住的页返回 `blocked_by_challenge` 标签，**不把挑战壳当内容返回**
- 每条结果带 **evidence score 分解**（semantic / lexical / engine_consensus）+ **逐引擎 telemetry** + `source_span`（字节级出处锚点）
- **质量信号驱动的回退**（hybrid backend：core 先跑，仅在质量信号触发时才合并聚合器结果，并显式给 `fallback_signal`）

**约束**：~1.5GB 磁盘；CPU ONNX 推理（**无需 GPU**）；AGPL-3.0（抄机制可以，抄代码要看许可）。

### 2.2 检索流水线拆解最清晰：argo（260★，Python）

https://github.com/taxueseek/argo —— **路由 → 召回 → 融合 → 证据打分** 四段流水线。

- 意图澄清 + **查询改写**（路由仍看原意图）
- 语言检测 + **按领域规则/TF-IDF 路由**到垂直源（code/学术/财经/影视），带预算与热路径缓存
- 多引擎并行召回 + **熔断器 + 负缓存**
- **空结果分阶段恢复**：放宽 → 同族 → 跨语言，并防污染
- **evidence skim** 复合分（authority 0.40 / absorption 0.35 / freshness 0.15 / consensus 0.10），并把 SERP 壳页排名压到极低

### 2.3 Find+Read 分离契约最完整：RustXNG（964★，Rust）

https://github.com/sahenjp/RustXNG —— **provider profile 分层**（standard 34 免密源 / clean 只查本地索引+Wikipedia dump / full 加 HTML 抓取源）。

- `read_page` **只返回与 query 最相关的至多 3 个 passage**（非整页），默认 4000 字符 / 上限 20000，支持 offset 续读
- **按 `cl100k_base`/`o200k_base` 精确计 token 预算**
- 每条结果报 `raw_content_status`（ok / too_large / robots_blocked / extract_failed / render_failed）
- SSRF 防护 + 遵从 robots.txt (RFC 9309) + 明确"页面文本为不可信数据"

### 2.4 其余可借鉴点（成本标注见 librarian 原表）

| 项目 | 机制 | 对我们的价值 |
|---|---|---|
| **crawl4ai**（87K★，Apache-2.0） | Playwright + LLM-free 抽取，输出干净 markdown | 正文抽取质量基线，可横向对标 AdaptiveRead |
| **open-webSearch**（1903★） | 引擎池含 **sogou/juejin/csdn/baidu**；fetch 按站点类型走**专用提取器** | **中文垂直引擎**——正好补我们 bing 中文错位的坑 |
| **trafilatura**（7.1K★） | **自研规则法抽取 + jusText + readability-lxml 双 fallback** | AdaptiveRead 可做"多算法接力"分层 |
| **fatsearch**（Rust，166★） | **outline 化阅读**（带编号章节，agent 只读 `u3#2`）；provider ladder（最便宜优先 + hedge + 成本记账）；**loss guard**（价格/日期/数字必须保留） | 省 token 的机制化落地；数字保留检查可作 read 契约断言 |
| **pagesift**（Rust，267★） | **仅当 JS 会增加内容时才渲染浏览器**；输出带 `warnings` 标记不完整结果 | 按需渲染策略可抄 |
| **openserp**（Go，1576★） | 结果带 rank/type/来源引擎标注；`extract=1` 把"搜到"与"读到"拆开 | meta 加来源引擎标注 |
| **ddgs**（3K★） | `region`/`safesearch`/`timelimit` 参数化；多后端 auto | 时效/地域直通 SearXNG `time_range` |
| **SearXNG 上游 2026** | **Anubis 挑战求解器**（2026-09/11 上游）+ `hostnames` 插件 + `oa_doi_rewrite` | 被 Anubis 挡的引擎可救；DOI/站点限定改写 |
| **FlashRank / fastembed-rs**（1K★/1K★） | ONNX+CPU 的 rerank/embedding，**不要 torch/transformers**，模型 ~4MB 起 | **无 GPU 提升排序质量的最短路径**（fastembed-rs 直接是 Rust） |

---

## 3. 反方：哪些"提升"是幻觉（Munger 反转）

### 3.1 榜单名次 ≠ 检索器强弱

- **同一 endpoint 在两个榜上一个第 9 一个垫底**：Brave LLM Context 在 Artificial Analysis Search Index（2026-11-09）排第 9（75 分），在 OpenBenchmarks search-only 排第 11（43.0±2.0）。
- **榜头差距在噪声内**：n=100、p≈0.8 时 95% CI ≈ ±8pp；榜头 Exa deep 83.0 vs Exa auto 81.7 差 **1.3pp**。
- **±SD 是 run-to-run 方差，不是抽样误差**（Parallel basic ±0.0 = 三轮完全一致）。
- 榜单自曝偏差：OpenBenchmarks 自己声明「后测的厂商分数更高，与质量无关……factual-lookup 排名不能用于比较厂商」。
- Parallel 自曝：「厂商榜只告诉你固定测试在固定某天的分数」；100 query 时 50% 准确率 CI≈±10pp，**相差 3pp 是平手**。

### 3.2 真差距在 query 侧，不在 retriever 侧（arXiv 2608.01913，6 agent × BrowseComp-Plus）

- 同一固定 retriever（Qwen3-Embedding-8B top-5）下，gold recall **52.0% → 78.7%**，26pp 差异**只能来自 query 表述**。
- accuracy 与**累计召回**相关 r=0.99，与最终上下文长度仅 r=0.16，与搜索量无关。
- **早停证据**：77.5–93.6% 的 episode **不带新证据**（仅 6–23% 有效）；首次命中 gold 后 57.9–74.0% 后续 episode 零产出；K=5 时已有 28.8–51.9% 轨迹拿到 gold，K=20 截断只损 ≤14% 准确率。**「早没搜到的，后面基本也搜不到」**；同题内正确方平均**少发 14 次 search** 且 gold recall 高 54pp。
- **上下文经济学**：搜索 snippet 占上下文 **66–85%**，正文仅 2–16%；强 agent 的错误里 **17.7–19.9% 纯属 markdown/大小写格式问题**。
- **多一个能抓 Google 的后端 ≠ 提升**：raw Google SERP 在 multi-hop 任务上 F1 **0.4%**、precision 0.7%。

### 3.3 自托管的真实成本（实操栈 2026-10-31）

- 2026-10-07 四条线全速 → DDG/Qwant 弹 CAPTCHA、Brave/Google 封 180s；现限速 **1 query/3s（≈1200/h）**——「**爆发量才是被封原因**」。
- 该栈自曝：「我们没测过它跟付费 API 的召回，别把返回当来源」。

---

## 4. 按证据排序的可借鉴优先级（对 gsearch-rs）

| 优先级 | 项 | 依据 | 成本 | 是否需要付费/GPU |
|---|---|---|---|---|
| **P0** | **修出口 IP**（加不同出口节点 / 或让容器绕开透明代理直连家宽） | 我们 12 引擎实测 2 个全挂 + DDG CAPTCHA 23 次/30min；根因单一且可修 | 低-中（改路由器） | 否 |
| **P0** | **query 改写层**（意图澄清 + 语言检测 + 领域路由） | arXiv：retrieval gap 26pp 来自 query 侧 | 中 | 否 |
| **P1** | **snippet 门控 + 去重 + 格式归一** | snippet 占上下文 66–85%；格式问题白捡 17.7–19.9% 错误 | 低 | 否 |
| **P1** | **近重复 query 标记**（唯一稳健失败预测量 ρ=-0.83） | arXiv | 低 | 否 |
| **P1** | **空结果分阶段恢复**（放宽→同族→跨语言） | argo；正对我们"全挂就硬回退 Google" | 中 | 否 |
| **P2** | **RRF 融合**（SearXNG 结果 + Google 真浏览器结果按排名倒数合并） | wigolo/argo | 中 | 否 |
| **P2** | **本地 ONNX rerank**（fastembed-rs / FlashRank，`bge-reranker-v2-m3`） | wigolo 核心质量手段 | 中（+模型体积，分发策略要先定） | 否 |
| **P2** | **中文垂直引擎**（sogou/juejin/csdn）+ bing 中文错位规避 | open-webSearch；我们实测"支付宝"全是登录页 | 低 | 否 |
| **P3** | **抽取状态码契约**（`blocked_by_challenge` / `robots_blocked` / `extract_failed`） | wigolo/RustXNG 的 honest output | 低 | 否 |
| **P3** | **read 只返 query 相关段落 + 精确 token 预算** | RustXNG；契合我们省 token 硬指标 | 中 | 否 |
| ❌ | **接 Exa/Perplexity 等托管 API** | 期望提升 ≤9 分且不可控 | 高 | 需 key/付费/数据出境 |
| ❌ | **多查询 fan-out** | 77.5–93.6% episode 零产出，直接乘 token 与 ban 风险 | — | — |
| ❌ | **多引擎扩容** | 每多一个引擎 = 每查询多一份被封面；24 白名单已边际递减 | — | — |

## 5. 与竞品的关键定位差异（我们的护城河）

- **wigolo 对标 Exa/Firecrawl，用 Node + ONNX + ~1.5GB 磁盘**；我们是 Rust 单 exe、零 key、零模型依赖、<15MB。
- 基准里**没有"本地零依赖"这一档**——wigolo 的 $0 靠本地模型，代价是 1.5GB + ONNX 运行时；我们的 $0 靠 SearXNG + 真浏览器，代价是出口 IP 与引擎存活。
- 我们的**md 排版输出**（省 29–40%）+ **真浏览器反爬**（CAPTCHA 无感）这两项在基准里没有对位物。

## 6. 本轮已实施的运维变更（SearXNG UA 固定）

### 6.1 变更内容

**问题**：SearXNG 默认 UA 模板是 **Firefox**（`searx/data/useragents.json`：`Mozilla/5.0 ({os}; rv:{version}) Gecko/20100101 Firefox/{version}`，版本 159.0/158.0），而它的网络层用 **curl_cffi impersonate（Chrome）** 做 TLS 指纹 → **UA 声称 Firefox、TLS 说是 Chrome** 的自相矛盾；且 UA 每次请求**随机**（`utils.py:105` `choice(USER_AGENTS['os'])` + `choice(USER_AGENTS['versions'])`），而 DDG 引擎的 vqd 缓存键包含 UA（`duckduckgo.py:232` `secret_hash(query + UA)`）→ **UA 漂移 = vqd 失配 + 缓存永不命中**。

**变更**：UA 恒定为 gsearch-rs 自己的 Chrome（`src/browser.rs:12` 同款）：
```
Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.8078.80 Safari/537.36
```

> **版本更正（2026-11-13 晚）**：初版写死 `Chrome/138.0.0.0`——那是我照抄的旧版本号，**落后真实 Chrome 18 个大版本**（本机 Chrome 实为 `156.0.8078.80`，2026-11 当前版）。已改为真实版本号，改前锚点留在 `useragents.json.bak-20261113-chrome138`。
> **但要说清**：这次更正的意义是"别用假版本号"，**不是"修好了 bing"**——§6.5 证明 bing 错位的分水岭是 **TLS 指纹**，UA 字符串的版本新旧不影响它。

**落地方式**（持久化，watchtower 重建镜像不丢）：
- 文件：`/mnt/docker/searxng/useragents.json`（新）
- compose 增只读挂载：`- ./useragents.json:/usr/local/searxng/searx/data/useragents.json:ro`
- 回滚锚点：`docker-compose.yml.bak-20261113-ua`（删挂载行即回滚）；容器内原文件备份 `useragents.json.bak-20261113`（容器重建后已随旧容器销毁）

### 6.2 隔离变量实测（同容器 / 同出口 IP / 同端点，只变 UA）

DDG html 端点：

| UA | 响应 | 结果数 | 风控 |
|---|---|---|---|
| Chrome 138 Windows（本方案） | 33 KB | **10** | 无 |
| Chrome 60 Linux（老） | 28 KB | **10** | 无 |
| `Wget/1.21` | 14 KB | 0 | **anomaly** |
| Nokia（google 引擎用的） | 14 KB | 0 | **anomaly** |

→ **浏览器 UA 通过、非浏览器 UA 必被拦**；UA 新旧版本影响不大，但"是不是浏览器"是分水岭。

### 6.3 引擎存活前后对比（`!engine` 单引擎探测）

| 引擎 | 改前 | 改后（第 2 轮） | 改后（第 3 轮） |
|---|---|---|---|
| brave / google | 0 / 0 | 0 / 0 | 0 / 0 |
| ddg / bing / github | 10 / 10 / 30 | 10 / 10 / 30 | 10 / 10 / 30 |
| mojeek / yahoo / startpage / qwant | 28 / 25 / 25 / 25 | 28 / **34** / 27 / **34** | 20 / 20 / 20 / 20 |

→ **诚实结论**：UA 修复消除了一致性缺陷（UA/TLS 矛盾 + vqd 漂移），**单轮曾见 yahoo/qwant 召回 +36%，但多轮回归落在 20–34 波动区间，收益不显著**；**brave/google 依然死**——它们的死因是出口 IP 信誉（8.219.85.68 阿里云机房段）+ 上游 searxng#6835，**指纹救不了**。

### 6.4 仍未解决（需另立项）

1. **brave / google 引擎**：必须换出口 IP（加不同后端节点或让容器走本机 v2rayN 的 us1/us2 节点）。本机 v2rayN 有 96 节点 / 4 后端，但**内核未启动**（GUI 在跑、xray 子进程与 10808 监听都没有）。
2. ~~**中文查询错位**~~ → **已解决（2026-11-13，见 §6.5）**：根因不是"中文分词"而是 **bing 引擎整体返回无关 SERP**；把故障的 `bing` 从 keep_only 白名单摘除后，同一查询连跑 3 次：25 条结果前 5 条全部相关、**零无关条目**、三次结果稳定（改前每次混 1 条 Apache Gluten / Mount Faber Leisure / Sheraton Singapore）。
3. **本机代理链路**：v2rayN 内核没起 → 本机 Chrome 无法访问墙外站 → gsearch 的 Google 真浏览器兜底当前不可用（实测 `net::ERR_CONNECTION_TIMED_OUT`）。

### 6.5 bing 引擎返回无关结果：完整诊断链（本轮最有价值的技术产出）

**现象**：SearXNG 的 `bing` 引擎对同一个 query 连续返回**与 query 完全无关**的结果集，且**每次都不同**：

| 连续请求 | 返回内容 |
|---|---|
| run 1 | 新加坡掏耳诊所（11 Best Ear Wax Removal in Singapore…） |
| run 2 | Bloomberg（Bloomberg Anywhere / Log in to BAUTH / 彭博專業服務…） |
| run 3 | Lee Dong Wook 影视资料（MyDramaList / Wikipedia…） |
| run 4 | 知乎《日瓦戈医生》译本 / 长期在纽约生活是怎样一种体验 |
| run 5 | Chess.com |

**排除链（每条都有原始证据）**：

| 怀疑 | 结论 | 证据 |
|---|---|---|
| 查询没送达 bing | **排除** | 原始 HTML 的 `<title>` = `how to install docker on debian - Search`，搜索框 `sb_form_q` 原样回显 query，页面含 'docker' 20 次 |
| SearXNG 解析错位 | **排除** | 直接抓同一批 `li.b_algo`，内容与 SearXNG 输出逐条一致 |
| 广告落地页混入 | **排除** | 那些 `b_algo` 内 `b_ad` / `b_adTop` / `sb_ad` 标记计数 = **0** |
| UA 伪造/过时 | **排除** | 随机 Firefox、固定 Chrome 156、不覆盖 UA 三组行为相同 |
| 语言/市场不匹配 | **排除** | `mkt=en-SG/en-US/zh-CN` × `language=en/zh-CN/zh-hans` 六个组合全部复现 |
| CDN 缓存键不含 query | **排除** | 加 `&cvid=<随机>`、`&form=QBLH&sp=-1` 无改善 |

**分水岭 = TLS 指纹**（同容器、同 IP、同 URL、同 UA，只换客户端）：

| 客户端 | 结果 |
|---|---|
| `curl_cffi` `impersonate="chrome"` | **正确** —— Install Docker Engine on Debian \| Docker Docs、How to Install Docker on Debian 13, 12 and 11… |
| `curl_cffi` + UA 覆盖为 Chrome 156 / Firefox 159 / 不覆盖 | 均**正确** |
| Python 原生 `urllib`（无 impersonation） | **垃圾** —— 上面那批无关结果 |
| **SearXNG 自己的** `searx.network.get(url)`（默认 impersonate=chrome） | **正确** |
| **SearXNG 自己的** `searx.network.get(url, impersonate="none")` | **垃圾** —— DeFi Yield Aggregators… |

**上游 issue**：`searxng/searxng#6937`（2026-11-13 提交，OPEN）——已把上述证据链贴进去，并指出 `bing.py` **没有**像 `google.py`（`chrome99_android`）、`duckduckgo_web.py`（`firefox`）那样显式 pin `params["impersonate"]`，而是依赖全局默认；请 maintainer 查引擎实际发出的 impersonate 值。相关：#4964（同一症状、closed 但从未定位）、#5461、#2977、#6931、#5882。

**本地处置（已执行）**：把 `bing` 从 `keep_only` 白名单摘除（**只摘 web 主力那条**，`bing images/videos/news` 保留——那三条路径今天未测，证据不足不扩面）。
- 备份：`settings.yml.bak-20261113-bing`；回滚 = 拷回 + `docker restart searxng`
- 实测：摘除后同一中文查询连跑 3 次 → 25 条结果前 5 条全部相关、零无关条目、三次稳定
- 待上游回复后再决定是否加回

**这条诊断对 gsearch 自己的意义**：这类"**看起来完全正常的无关结果**"是结果层诚实标注的典型场景——已立项 `bd gsearch-rs-d8s`（P1）：复用仓内 `title_overlap`/`tokenize` 做相关性自检，结果带 `relevance` 标记、整集可疑时用**新 status 值**（不与 `searxng_degraded`/`filtered_empty`/`no_results` 混用）、`meta` 带计数；回归 fixture 已落盘 `tests/fixtures/off_topic_searxng_20261113.json`（4 case / 40 条真实 off_topic）。

## 7. 托管 API 侧机制（10 家，2026 主流）

> 机制提取自各家官方 docs（2026 版）。**注意**：这些机制大多是**托管服务内部实现**，我们不能照搬服务，但**参数形态与分流逻辑可以照搬**。

### 7.1 最值得抄的参数设计（成本低、且与 §3.2 的 query 侧证据完全吻合）

| 家 | 机制 | 为什么提升质量 | 对 gsearch-rs 的可借鉴点（成本） |
|---|---|---|---|
| **Parallel** | `objective`（自然语言目标）+ `search_queries`（1–5 个 3–6 词关键词）**双字段分离**；官方明说最常见错误是同一串既当目标又当唯一查询 | 把"要什么"和"搜什么"解耦 → 查询词更聚焦，召回更准 | **加一个"目标/关键词"分离的查询参数**（低）——正好落在 §3.2「retrieval gap 来自 query 侧」这条最确定的收益线上 |
| **Parallel** | 四档 mode：`turbo`/`fast`(~200–700ms) → `basic`（扩展摘要）→ `advanced`（多跳 + 更强召回压缩管线） | 延迟/成本/质量三维旋钮 | **档位化**：我们已有 `fast/standard` 分档雏形（openserp 也有），可做 `--fast` / `--deep`（低） |
| **Exa** | `contents.highlights`：用抽取模型**只回与 query 相关的段落**——官方测 500 字符 highlights 准确度 ≈ 前 8000 字符全文而 **token 少 16 倍**；长技术文档 500 字符 highlights 达 60% 准确而全文仅 6% | 从"截全文"升级为"按 query 抽取" | **把 `--snippet-len` 从定长截断升级为 query 相关抽取**（中）——这是我们省 token 硬指标的下一台阶 |
| **Tavily** | `max_age_hours`（**抓取时效**）与 `time_range`（**发布时间**）分离；`include_domains_mode` = restrict（硬过滤）/ prefer（软偏好）双模 | `time_range` 匹配发布日会误杀持续更新页（天气/价格）；域名可软硬分级 | **我们已有 `time_range`，缺"抓取时效"与"域名软偏好"**（低）——`site:` 被部分引擎忽略那个坑（P1 盲测八）也能靠 prefer 缓解 |
| **Linkup** | 查询写法分两段：「要检索什么证据」与「如何据此推理」；`depth` 四档（flash/fast 不调 LLM 直查索引 → standard 单轮 agentic → deep 多轮迭代 + JS 渲染） | 官方明说分两段比"直接问答案式问题"命中率高 | 与 Parallel 的 objective/queries 同源（低）；**单轮/多轮分级**（中）——但 §3.2 说多轮 fan-out 收益可疑，**只做单轮** |
| **Firecrawl** | `categories` 走**专用索引**：research（限 arxiv/nature/pubmed）/ developer（GitHub issue+PR+README+文档站）/ gov（法条判例） | 垂直索引比通用 SERP 准得多 | **领域路由**（低）——§2.2 argo 的规则路由同款，落到 SearXNG 侧就是"按查询类型选引擎子集" |
| **Firecrawl** | Search Highlights 默认开：把站点 description 换成 query 相关正文段落，**取不到高亮自动回退原摘要** | 既提质又不因高亮失败丢结果 | **"默认高亮 + 失败回退"契约**（中）——我们 md 输出已有"缺席语义"，可加一层 |
| **Brave** | **Goggles 自定义重排**：内联 `$site=` / `$discard` / `$boost=` / `$downrank=`，可只留官方文档站并排除 UGC | 不改引擎就能改排序 | **站点加权/排除规则**（低）——直接缓解"site: 被静默吞掉"与中文错位 |
| **Brave** | 上下文预算**四维可调**（urls / tokens / snippets / 每 URL tokens+snippets）+ `context_threshold_mode` strict/balanced/lenient | 按查询复杂度缩放预算 | 我们 `--limit`/`--snippet-len` 是二维，**可加"每 URL 预算"与"总量预算"**（低） |
| **TinyFish** | `purpose` 参数：额外传"为什么搜/用途"作为排序信号 | 意图入检索 | 与 objective 同族（低） |
| **You.com** | `extraction_mode` 三档：snippets（100–200 词）/ highlights（query 感知抽取）/ full_page | 内容深度分级暴露 | 与 Exa highlights 同族（中） |
| **Perplexity** | `search_context_size`（low/medium/high）控抽取量；`search_mode="academic"` 学术路由；`search_recency_filter`/`search_domain_filter`（≤20 域） | 深度与时效可控 | context_size 档位 ↔ 我们抓取深度（低） |

### 7.2 代价与门槛（决定哪些不可抄）

| 家 | 定价/门槛 |
|---|---|
| Parallel | turbo/fast $1/1000、basic/advanced $5/1000、额外结果 $1/1000；advanced ~3s；EU 端点仅 basic |
| Exa | API key + 新号免费额度；lite 只吃索引不实时抓 |
| Tavily | basic/fast/ultra-fast 1 credit、advanced 2 credits；**keyless 免费但限流**，注册后 1000 credits/月 |
| Linkup | flash/fast/standard $0.005，sourcedAnswer/structured $0.006，deep $0.05 |
| Firecrawl | 无 key 可起步（限流更严）；gov 类别不加 credit |
| Brave | 需 Search 计划 API key |
| Perplexity | $5/1000 请求，Fast $1/1000；Agent API web_search $0.0025/次 |
| Serper | 一次性 2500 次免费；预付 $50 起，credit 6 个月过期 |
| You.com | $5/1000 calls，默认 10 req/s，新号 $100 额度 |
| TinyFish | **每天免费 12,000 次**（00:00 UTC 重置），个人 30 req/min |

→ 全部需要 key 或额度；**唯一与我们的"零 key 单 exe"冲突的就是它们**。

### 7.3 独立基准的量化对照（2026-09-15 快照，100 私有 ticket × 3 轮，模型固定 gpt-5.6-sol）

- **search & fetch**：Exa deep 83.0±1.0｜Exa auto 81.7｜TinyFish 79.0｜Perplexity high 77.7｜Parallel advanced 77.0｜Firecrawl 76.0｜Tavily advanced 60.0｜Linkup standard 48.3
- **search only**：Perplexity low **77.3**｜Firecrawl 70.3｜Parallel fast 66.7｜Exa fast 66.3｜**Brave LLM Context 43.0**
- **token 效率**：Firecrawl 7,456 最省，TinyFish 7,469，Perplexity 8,765；Linkup 57,791（≈4.5×）却垫底
- **multi-hop**：Parallel basic F1 46.5｜Exa deep 45.4｜**raw Google SERP 0.4±0.6（precision 0.7%）**

### 7.4 未查到 / 未核实

- Parallel / Exa 的具体检索算法未公开（只给行为不给排序模型）；Brave Goggles 的权重实现未公开。
- Perplexity / You.com / Serper 的官方文档站在本机直读失败，相关条目依据官方文档索引摘要 + changelog + 第三方实测交叉拼出，**字段名与额度建议二次核对**。
- Exa/Brave 的 highlights 效果数字为**厂商自测**，跨引擎不可直接比较。

## 8. 未做 / 风险

- **未实测**：所有竞品的机制均来自 README/官方文档，**未本地跑通**；fastembed-rs/ONNX rerank 未做原型（模型体积与"单 exe"分发策略冲突，需先定分发方式）。
- **未验证**：SearXNG 侧"绕过透明代理直连家宽"是否真的改善 DDG/Google——需实跑（本报告只证伪了"换节点"这条路）。
- **风险**：改 passwall 透明代理规则会影响路由器上**所有**设备（不只容器），需谨慎并先备份。
- **风险**：arXiv 2608.01913 的结论基于 BrowseComp-Plus 任务集，与我们的真实查询分布可能不同。
