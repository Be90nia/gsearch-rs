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
Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36
```

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
2. **中文查询错位**：三次连跑「如何用Python读取CSV」，**每次混入 1 条完全无关结果**（Apache Gluten / Mount Faber Leisure / Sheraton Singapore）——即 SearXNG bing 引擎的**中文查询错位**（老缺陷，记忆里 2026-10-22 已实证）。英文查询两次跑完全一致，无此问题。对应动作：给 bing 降权 / 补中文垂直引擎（sogou、csdn、baidu）。
3. **本机代理链路**：v2rayN 内核没起 → 本机 Chrome 无法访问墙外站 → gsearch 的 Google 真浏览器兜底当前不可用（实测 `net::ERR_CONNECTION_TIMED_OUT`）。

## 7. 托管 API 侧机制（未完成）

本轮第三路 librarian（托管 API：Parallel/Exa/Tavily/Perplexity/Brave/Firecrawl/Linkup）跑满 35 分钟未收敛，已终止。**该节缺失**——但硬数据已从独立基准 README 直接取得（Exa deep 83.0% / TinyFish 79.0% / Perplexity low 77.3% / Firecrawl 76.0% search-only 70.3%；token 效率 Firecrawl 7,456 最省、Linkup 57,791 垫底），且反方对榜单可信度的证伪（§3.1）不依赖托管 API 机制细节。

## 8. 未做 / 风险

- **未实测**：所有竞品的机制均来自 README/官方文档，**未本地跑通**；fastembed-rs/ONNX rerank 未做原型（模型体积与"单 exe"分发策略冲突，需先定分发方式）。
- **未验证**：SearXNG 侧"绕过透明代理直连家宽"是否真的改善 DDG/Google——需实跑（本报告只证伪了"换节点"这条路）。
- **风险**：改 passwall 透明代理规则会影响路由器上**所有**设备（不只容器），需谨慎并先备份。
- **风险**：arXiv 2608.01913 的结论基于 BrowseComp-Plus 任务集，与我们的真实查询分布可能不同。
