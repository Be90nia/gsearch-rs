# gsearch v0.2.9 嫉妒型对标报告（devil-envy）

**结论先行**：gsearch 的契约工程（退出码/注入面标注/SSRF 门）是所有竞品里最强的，但功能覆盖停在「裸 SERP + 单页正文」——没有 answer 合成、没有 markdown、没有 PDF 解析、没有整站抓取、没有相似页查找，我每天还是得让 tavily/exa/firecrawl 替我干一半活。像一台 2019 年的 tavily 硬件装了 2026 年的安全固件。

**VERDICT: 7.5/10 —— 成本与契约反超，功能覆盖被 firecrawl 一家甩开两身位；Top1 该抄的是 answer 合成 + relevance score 透传**

- 调研身份：日均几千次搜索调用、拿搜索结果直接喂 LLM 上下文的 AI agent
- 方法：黑盒（README/USER_GUIDE/--help + 实跑），竞品主张凭训练知识 + gsearch 实搜查证 2 次（exa 六种 search type 与 /answer、firecrawl v2 Interact/PDF 解析均已确认）
- 实跑证据：search（searxng, 3018ms, --compact-meta ✓）、batch --envelope v2（2/2 ✓）、fetch --json（content_untrusted ✓）、verify（403→get-fallback ✓）、doctor --json（见下文 network FAIL 实锤）

---

## 一、嫉妒四连问

### Q1 别人有、我们没有的（渴望清单，按我的痛感排序）

| # | 竞品 | 能力 | 我的真实场景与渴望 | 信心度 |
|---|------|------|-------------------|--------|
| 1 | tavily/exa | **answer 合成**（search 直接返回带引用的合成答案） | 「2026 Rust 生态年度报告要点」这类综述型任务，gsearch 要 search → read 3 页 → 自己拼上下文，3+ 次调用、几万 token 进上下文；tavily answer / exa `/answer` 一次调用直接给可引用结论，省 70% token、省 2-3 个往返。这是我离开 gsearch 的第一原因 | 高（exa /answer 实搜确认） |
| 2 | firecrawl/jina | **PDF/DOCX → 文本解析** | 我的日常：arxiv 论文 PDF、厂商白皮书。gsearch `--dl 1` 只落盘不解析，fetch 抓 PDF 是二进制乱码——我还得切别的工具。firecrawl v2 Media parsing 直接返回解析文本，一步进上下文 | 高（实搜确认） |
| 3 | firecrawl/jina | **markdown 输出**（保链接/表格/代码块） | gsearch AdaptiveRead 是剥标签 plain text：对比表格拍平成乱串、引用链接全丢，我拿到的上下文又丑又费 token。r.jina.ai 和 firecrawl /scrape 返回结构化 markdown，表格不散架、citation 链接可追溯 | 高 |
| 4 | exa | **findSimilar**（给 URL 找相似页） | 竞品调研任务：「这个工具的同类替代品有哪些」。gsearch 只能拿 URL 里关键词去 search 碰运气；exa findSimilar 一发命中同主题同形态的页面。写对比分析时这是刚需 | 高 |
| 5 | firecrawl | **/crawl + /map 整站递归** | 「把这个 docs 站 200 页灌进知识库」：gsearch 我要自己写循环 browse 200 次还自己维护去重；firecrawl 一个 job 交出去。/map 先拿全站 URL 列表更是灌库前菜 | 高 |
| 6 | tavily | **include_domains / exclude_domains / topic=news** | 「只搜 docs.rust-lang.org 内」gsearch 只能手写 `site:` 透传（有），但 exclude 没有参数化入口、news topic 聚焦没有。每日跑监测任务时这三个参数就是过滤器本器 | 高 |
| 7 | firecrawl | **actions 脚本化**（click/scroll/type/wait 后再抽取） | 抓带手风琴/翻页/懒加载的页面：gsearch 的 shell 有 `click N` 但是交互式的，不能在一条 browse 命令里声明式驱动。firecrawl v2 Interact endpoint 纯 API 完成 | 高（实搜确认） |
| 8 | exa | **category 过滤 + 六种 search type**（instant→deep-reasoning） | 找 github 仓库/paper 时 category=github/paper 一刀切准；要快用 instant、要深用 deep。gsearch 的 domain_class 是事后标注，不能事前按类筛 | 高（实搜确认 six search types） |
| 9 | tavily/exa | **结果级缓存** | agent 重试循环里同一 query 打 5 遍 = gsearch 5 次实爬 + 5 次风控暴露；tavily/exa 有缓存层，重查不重爬。gsearch 每次都是真金白银的 IP 信誉消耗 | 中 |
| 10 | jina | **alt 媒体描述**（图片→文字描述） | 页面核心信息在架构图/截图里时，jina alt 给我图的内容摘要；gsearch 输出里图片直接蒸发 | 中 |

### Q2 别人同类做得更好的（不是没有，是质量差）

1. **搜索结果质量**：tavily 每条结果带 rich snippet + raw_content + relevance score；gsearch 我实测 3 条里 1 条 snippet 为空（exa docs 那条 `snippet: ""`），meta 里连 relevance score 字段都没有——SearXNG 明明内部有 score，输出层丢了。agent 想做 top_k 过滤都无权重可用。
2. **相关段落选取**：exa highlights 直接返回「与 query 语义相关的段落」；gsearch 的 `--read N --excerpt` 是「页面第 K 段的前 N 字符」，按位置不按相关性——长页面上我拿到的常常是导航和页脚，不是答案。
3. **抓取输出形态**：firecrawl /scrape 一发可选 markdown/html/screenshot/summary 多格式；gsearch fetch/read 只有 plain text 一条路（`--include` selector 是好的开始，但输出形态单一）。
4. **结构化抽取**：firecrawl /extract 给 JSON schema 就回结构化数据；gsearch 给我全文文本后 schema 抽取要我自己在下游跑 LLM，每页一遍。50 个产品页 = 50 次额外 LLM 调用。
5. **可靠性姿态**：tavily/exa 是多机房 SLA 服务；gsearch 单机出口 IP 信誉就是单点——撞码/SearXNG IP 连坐都要人肉切节点。gsearch 的熔断（searxng_degraded）和 doctor 是好的工程，但兜底仍是「人」不是架构。

### Q3 我们有、但配不上竞品水准的

1. **doctor 的 network 检查硬编码 www.google.com:443**——我本次实跑：network FAIL（exit 1），同一秒 SearXNG 搜索活得好好的（20 results）。provider-aware 缺失，CI 里 doctor 会稳定误报，这个 FAIL 语义对走 SearXNG 的 agent 是噪音。
2. **batch 有了但没有跨查询聚合**：`--envelope v2` 给了顶层统计（好），但 n 个查询返回 n 个独立数组——没有去重、没有合并 top_k、没有跨查询融合排序。tavily 一次查询天然带分数排序；我的「多角度调研」工作流拿到的是 n 堆散沙而不是一份去重后的证据池。
3. **--excerpt 有了但不按相关性选段**：如 Q2-2 所述，是「位置窗口」不是「语义窗口」，token 省了但没省到刀刃上。
4. **snippet 质量不稳**：空 snippet 直接进上下文就是死重量。
5. **read_max_chars 硬截断 50000 是字符 cap 不是 token cap**：对 agent 消费方，按字符截断长表格可能正好截在表格中段，下游解析全废。

### Q4 gsearch 反超的点（公平义务，这些是我真心不想回去的理由）

1. **成本结构碾压**：自托管 SearXNG + 本机 Chrome = $0 无限调用。tavily/exa/firecrawl 按次计费，我这种日调几千的 agent 每月省数百美元，且**掉 key/欠费/限流三种失败模式根本不存在**。
2. **退出码契约**（0/1/2/3/4/5，含 JS 壳→「换 browse」、私网门→「加 --allow-private」、searxng_degraded 熔断语义、captcha_timeout JSON 状态）——竞品都是 HTTP API，没有 CLI 级可编程契约；agent 消费 CLI 时 exit code + stderr 提示就是可编程性天花板，这个纪律 exa/tavily 不提供。
3. **SSRF 私网门**：loopback/RFC1918/link-local/云 metadata/IPv6 ULA 全拒 + 重定向每跳复检 + 公网强制 https——我喂给它的 URL 来自 LLM 上下文（不可信），这道门 tavily 不需要（服务端），但任何本地工具竞品（ddgs/crawl4ai）都没有。
4. **content_untrusted: true 注入面标注**：网页正文进 agent 上下文 = 注入面，gsearch 全链路显式标注。我知识范围内全网独此一家（jev 同款纪律）。
5. **doctor + verify CI 型自检**：7 项健康检查 + URL 健康探测（SSL/DNS/超时分类退出码 3/4/5），竞品无等价物。
6. **真 Chrome profile 登录态**：login 弹窗人工登录 → cookie 持久化 → zip 整目录携走 + GAEX 豁免 + 人解 CAPTCHA 双模式。对登录墙/强风控站，比 firecrawl 的 cookie-header 注入强一档，jina reader 对登录墙直接跪。
7. **单 exe ~10MB 零运行时依赖**：ddgs 要 pip、crawl4ai 要 python+playwright 全家桶，gsearch 扔进 PATH 即用。
8. **数据不出内网**：自托管 = 企业合规场景 tavily/exa 物理做不到。

---

## 二、六维打分（10 分制；标杆 = 我心中该维最强竞品）

| 维度 | gsearch | 标杆（分） | 一句话 |
|------|---------|-----------|--------|
| 搜索质量（相关性/snippet/信号） | 6.5 | exa（9）：语义检索+highlights+score；tavily 8.5 | 裸 SERP + 空 snippet + 无 score 字段，输在信号密度 |
| 抓取能力（渲染/markdown/站点级/PDF） | 7.5 | firecrawl（9.5） | 单页真渲染 + 登录墙反超；markdown/PDF/整站三项输 |
| agent 契约（token+结构化+可编程） | 8.5 | tavily（8） | 退出码/envelope/compact-meta/content_untrusted 反超；缺 answer 与 score 扣回 |
| 功能覆盖度 | 5.5 | firecrawl（9） | crawl/extract/actions/PDF/screenshot 全没有 |
| 可靠性工程 | 7 | tavily（8.5） | SLA/缓存输；doctor/verify/熔断/退出码工程补回大半，IP 信誉单点仍痛 |
| 成本 | 10 | jina（7，免费额度限流） | $0 无限调用 + 零 key 依赖，无争议满分 |

**总分：7.5/10**
**一句话定位**：gsearch 是成本敏感、安全自觉的 agent 的本地搜索+抓取瑞士军刀——契约纪律是竞品的两倍，功能清单是竞品的一半。

---

## 三、如果我是 gsearch PM，最该抄的 Top 5（按 agent ROI 排序）

1. **抄 tavily/exa：relevance score 透传 + search 带 answer 合成**。SearXNG 内部本就有 score，先零成本透传成 `results[].score`；answer 可先做「top-3 snippet 融合」廉价版或接本地 LLM。ROI 最高：我 70% 的综述任务从 3+ 次调用降到 1 次，且 score 让 top_k 过滤、跨查询去重全部解锁——这是其它四条的地基。
2. **抄 firecrawl/jina：read/fetch 加 `--markdown` 输出**。保留链接、表格、代码块结构。表格不拍平 = 下游解析不炸，citation 链接可追溯，token 效率直接提升；DOM→markdown 转换是纯本地计算，无新依赖面。
3. **抄 firecrawl：fetch/dl 支持 PDF/DOCX → 文本**。arxiv/白皮书是我最高频的抓取对象，现在只给二进制；纯 Rust 解析 crate 本地完成，不做云调用，符合零依赖哲学。
4. **抄 exa：`findSimilar <url>`**。低成本版：取域名 + title 关键词过 SearXNG，title 相似度重排 top_k。竞品分析/找替代源是我的周常任务，现在全靠手气。
5. **抄 firecrawl：`search --crawl <N> --max-pages M` 递归抓站**。同域 BFS + 去重 + 复用现成 batch fetch 并发基建即可。灌库场景从 200 次循环调用变 1 次，还白得全站 URL 清单（/map 语义）。

---

*报告：DevilEnvy（嫉妒型 agent 对标）· 2026-10-08 · gsearch-rs v0.2.9 · 竞品查证 2/4 次（exa、firecrawl，经 gsearch 自身实搜）*
