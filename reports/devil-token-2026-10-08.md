# Devil's Advocate Token 审计 — gsearch v0.2.9（黑盒，AI agent 视角）

**结论：gsearch 的输出契约对 AI 不友好不是风格问题，是钱的问题。单次 search+read 研究周期实测可省 ~2.3KB（约占合并载荷 25-30%）；最大三宗罪：JSON 模式 snippet 不截断（比人读模式更费）、pretty JSON 未统一（fetch 单 URL/browse 已是 compact，其余 5 条路径全是 pretty）、read 的 paragraph_index 默认输出里 43% 是同载荷内重复信息。**

VERDICT: 有罪——7 条命令输出路径中 5 条 pretty、机器路径比人读路径多付 24% snippet 字节、read 导航结构一半是冗余；全部可零破坏修复（默认值翻转 + 已有 flag 扶正），无需新协议。

审计方法：黑盒（不读 src/），本机 debug exe + 真实 SearXNG（http://192.168.89.249:8888），Windows Git Bash，2026-10-08。字节均 UTF-8 实测；compact 值 = `json.dumps(separators=(',',':'))` 同数据重序列化。

---

## T-01 SERP snippet 不截断——机器路径比人读路径更费（最严重）

- **复现**：`gsearch search "rust async runtime" --limit 10 --json --no-humanize`
- **实测**：10 条 snippet 共 **1992B = 全文件 4422B 的 45.0%**；单条 avg 199B / max 396B。
- **反向实锤**：同查询人读模式把 snippet 截到 **160 字符**（实测 10 行全 ≤160，4 处 `…`）。给 AI 的 JSON 路径反而比给人看的路径每条多付 ~24% 字节。给机器的格式比给人的格式更浪费，这是契约倒挂。
- **可省**：默认 160 字符 cap → 省 **499B/query（11.3%）**；cap 120 → ~790B（18%）。无 `--snippet-len`，`--excerpt` 只管 read，search 侧无任何控制手段。
- **修法主张**：**默认截断 160 字符**（与人读渲染器对齐）+ `--snippet-len N` 逃生口。不必 opt-in——agent 在前 160 字符里找不到答案就会 `--read N` 开页（一次 read ~5KB），chars 160-400 的边际价值≈0。
- **伤害场景**：agent 一次任务 20 连搜 = 白烧 ~10KB 上下文；批量 batch 模式（--envelope v2）下乘以查询数更狠。

## T-02 pretty JSON 未统一——同一工具两套序列化约定

- **复现实测**（同数据 compact 重序列化）：

| 命令 | raw | compact | 可省 | % |
|---|---|---|---|---|
| `search "rust async runtime" --limit 10 --json` | 4422 | 3877 | **545B** | 12.3% |
| `search q1 q2 q3 --json --envelope v2`（batch 3） | 5778 | 4601 | **1177B** | 20.4% |
| `doctor --json` | 1026 | 767 | **259B** | 25.2% |
| `verify https://example.com --json` | 125 | 103 | 22B | 17.6% |
| `verify <redirect/3> --json` | 199 | 159 | 40B | 20.1% |
| `fetch <url> --json`（批量 3 URL） | 3308 | 3152 | 156B | 4.7% |
| `fetch https://example.com --json` | 301 | 300 | 1B | **已是 compact** |
| `browse <url> --json`（read） | 4943 | 4942 | 1B | **已是 compact** |

- **实锤内部矛盾**：`fetch` 单 URL 和 `browse` 两条路径已经是 compact JSON，search/search-batch/verify/doctor/fetch-batch 五条路径全是 2 空格缩进 pretty。不是技术做不到，是没统一。
- **修法主张**：**默认 compact**，无需新 flag——消费者是机器，`--json` 的契约就是"给我可解析的最小结构"；人要看 pretty 会自己 `jq`。零破坏（JSON 语义不变）。
- **伤害场景**：纯纳税人。batch 3 查询每次调用白付 1.2KB；doctor 是低频命令但 25% 全是缩进。

## T-03 read 的 paragraph_index 默认输出 43% 是同载荷冗余

- **复现**：`gsearch browse https://tokio.rs/tokio/tutorial --json`（AdaptiveRead 默认形态）
- **实测**：输出 4943B；`paragraph_index` 17 项共 **2181B（44%）**，其中：
  - 已进 `summary_paragraphs`（默认 10 段）的段落，其 `first_sentence` 在同一载荷里**原文重复 749B**；
  - `index` 字段 = 数组位置 +1，agent 免费可算，~170B；
  - `char_count` 306B（唯一有点用的导航键）。
  - 冗余合计 **≈944B/页（43%）**。
- **修法主张**：默认形态 paragraph_index **只列未进摘要的段落**（11-17），或干脆 `--excerpt` 时才输出 pi；去掉 `index` 键（位置即索引）。
- **伤害场景**：read 是深读路径，agent 每读一页都要为"已在本载荷里的数据"付两次钱。20 页研究 = ~19KB。

## T-04 search meta 14 键默认全量——`--compact-meta` 存在但默认关

- **复现**：`search ... --json`（不加 `--compact-meta`）
- **实测**：full meta **343B**（14 键），compact-6 ≈125B → **可省 218B/query（4.9%）**。逐键点名：
  - `browser_path` **77B**：Chrome 绝对路径，每次调用原样重复的环境噪声，agent 永不消费；
  - `query` 30B：调用方自己刚传的参数；`limit` 12B：同上；`tool` 18B：常量 "gsearch"；
  - `proxy: null` 14B / `recency: null` 16B：空值占位；
  - `results_count` 20B：`len(results)` 可推导；
  - 真有信息量的：`provider` 22B、`elapsed_ms` 19B、`truncated` 18B、`version` 19B（缓存失效判断）。
- **修法主张**：**翻转默认**——compact-6 默认开，`--verbose debug` 强制全量排障（帮助文本里已有此语义，方向反了）。对"唯一消费者是 AI"的工具，排障形态不该是默认形态。
- **伤害场景**：agent 每 query 白付 218B，其中 77B 是同一条 Chrome 路径在第 N 次重复。

## T-05 空值占位照常序列化（null/""/false/0 全额付费）

- **复现实测**：
  - `search --json` 顶层 `run`：`{"status":"ok","captcha_solved":false,"message":""}` = **59B/query**，happy path 三字段全是默认态（没撞码、没消息、ok=没新闻）；
  - batch v2 每 ok 元素 `"message":""` = **13B × n**（3 查询 39B，10 查询 130B）；
  - `fetch`/`read` meta `omitted:0, truncated:false` = **26B/页**（`content_untrusted:true` 是安全标注，保留——这是对的）；
  - `verify http://...` 的 `redirect_chain: []` 20B。
- **修法主张**：**默认省略**默认态字段——`run` 只在 status≠ok 或 captcha_solved 时出现；`message` 只在 error 元素出现（fetch batch 失败元素已这么做：`{"message","status","url"}` 无 meta，证明省略模式代码里已存在）；`omitted/truncated` 仅非默认时输出。JSON 消费方按"缺席=正常"解析。
- **伤害场景**：10 查询 batch = ~190B 纯空值税；乘以整个 session 的调用次数。

## T-06 verify 对 http:// URL 谎报 `ssl_valid: true`（正确性缺陷，非纯浪费）

- **复现**：`gsearch verify http://example.com --json`
- **实测**：返回 `"ssl_valid": true`——对明文 HTTP 根本不存在 TLS，该值无意义且**误导 agent 安全推理**。
- **修法主张**：http:// 时该字段 `null` 或省略。
- **伤害场景**：agent 据此把 http 端点当"SSL 健康"，在登录/提交场景做出错误信任决策。省 17B 是小事，语义污染是大事。

## T-07 可推导计数器（agent 免费能算的数字不该付费）

- **实测**：`search meta.results_count`（20B，= len(results)）；`doctor fail_count/warn_count`（31B，= checks 数组计数）；batch meta `n_ok`（~13B，= n_total - n_fail）。
- **修法主张**：低优先级。`n_fail`/`fail_count` 编码退出码语义，**保留**；纯镜像计数（results_count/n_ok）可省，合计 ~50-60B/query。默认省略。
- **伤害场景**：小额但无理由——每次调用为"自己能 count 的东西"付费。

## T-08 fetch batch 失败元素 message 内重复 url

- **复现**：`fetch https://example.com https://httpbin.org/status/404 https://www.iana.org/help --json`
- **实测**：失败元素 `{"message":"HTTP 404 Not Found: https://httpbin.org/status/404","status":"error","url":"https://..."}` —— url 在 message 和 url 字段**各出现一次**，30B/失败元素。
- **修法主张**：message 只留 `"HTTP 404 Not Found"`，url 已有独立字段。
- **伤害场景**：批量 fetch 站点清单时，死链越多税越重。

## T-09 人读默认陷阱——反向发现：人读模式比 JSON 模式更省 token

- **复现**：`search "rust async runtime" --limit 10`（忘加 --json）= **2608B**，JSON 同查询 = **4422B**。
- **实测**：agent 忘加 --json 反而省 1814B（因为人读模式截 snippet 到 160，见 T-01）。但人读输出是 `1. 标题 [class]\n   url\n   摘要…` 无字段名格式——正则解析脆弱（编号、`…` 截断、`[class]` 尾注），**一次误解析 = 重试一次完整 --json = 4422B + 2.5s 延迟**，比省的 1814B 更贵。
- **修法主张**：对 AI-only CLI，**`--json` 应默认开**，人要人读加 `--human`；退一步至少 README 第一行加警告。这是 0 成本的调用量翻转。
- **伤害场景**：agent 默认踩进人读模式 → 要么解析出错重试双倍付费，要么用正则硬啃无字段名文本。

## T-10 doctor message 散文模板（低频命令，小额）

- **复现**：`doctor --json`
- **实测**：7 条 message 共 383B（37% 文件）。`profile_source` 的 message "profile 来自配置文件: default" 完整复述了自己的 name；`edge` warn 对 agent 无行动价值（Chrome 在 = 可跑）。
- **修法主张**：value 类检查改结构化字段（`{"name":"exit_ip","status":"ok","value":"61.144.188.80"}`），散文只留给 fail。省 ~150-200B/run。
- **伤害场景**：低（doctor 低频）。列仅为完整性。

---

## Top3 汇总（按可省字节排序）

| # | 浪费源 | 一次性可省 | 频率换算 | 修法 |
|---|---|---|---|---|
| 1 | T-01 snippet 不截断 | **499B/query**（limit 10，cap160） | 20 搜/session ≈ 10KB | 默认 cap 160 + `--snippet-len` |
| 2 | T-02 pretty JSON | **545B/query；1177B/batch-3；259B/doctor；22-40B/verify** | 每 JSON 调用 12-25% | 全局默认 compact |
| 3 | T-03 read paragraph_index 冗余 | **≈944B/page** | 20 页深读 ≈ 19KB | pi 只列未摘要段；去 index 键 |

**合计**：典型 search(10) + read(1页) 周期 ≈ **1.3KB/query + 1.0KB/page ≈ 2.3KB**，约占两命令合并载荷的 25-30%，全部零破坏可修。

## 与 AI 无关但顺手的正确性记录

- T-06（http 谎报 ssl_valid:true）是唯一涉及语义正确性的发现，建议与字节优化同批修。

## 审计边界

- browse/read 浏览器命令实跑 1 次（预算 ≤2）；batch search 的失败元素路径未在 search 侧复现（SearXNG 对乱串查询也返回结果），fetch 侧失败元素已覆盖该形态；Google 直爬路径（本机 google 不通）未测——已知基础设施，非目标。
