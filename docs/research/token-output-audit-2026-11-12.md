# gsearch 输出 token 审计复盘（2026-11-12）

用户提问：「返回的是不是直接 JSON 结构？AI 直接拿 JSON 不如给处理好的数据，省 token」。

结论先行（2026-11-12 白天）：**是 JSON，且这正是设计决定；该省的已在 10-08 两轮审计 + 22 轮盲测（编号 5–22，17 份 final 报告）中省完，今天的实测证明剩余肥点趋近于零；改默认值有当日翻车的 revert 先例。**

> **后续（2026-11-12 晚，本文件末尾「本轮结论」节）**：用户随后拍板**默认输出直接反转成 md 排版**（v0.4.0，breaking）——上面的「剩余肥点趋近于零」结论成立，但**结论的前提错了**：瓶颈从来不是字段值而是**每条结果的键名税**，换个形态整体消掉（−29%，limit 10 时 −51%），且盲测证明 md 不损提取准确率。本文件上半部分保留为「在 JSON 前提下能榨出多少」的记录。

## 依据档案

- `reports/ai-token-saver-2026-10-08.md`（AI 视角审计，9 场景实证 + 元审计 20 issue 冲突矩阵）
- `reports/devil-token-2026-10-08.md`（杠精黑盒审计，T-01~T-10 浪费点全量化）
- v0.3.1 翻车实录：18fa36f（fetch --max-chars 默认 50000→8000）发布当天被 caaa0c2 revert

## T-01~T-10 落地状态对照（2026-11-12 实测）

| # | 当年发现（10-08 数字） | 今天实测 | 状态 |
|---|---|---|---|
| T-01 snippet 不截断（机器路径比人读贵 24%） | snippet 单条 avg 199B/max 396B | [160,135,67,113,141] cap 160 生效 | ✅ 已落地 |
| T-02 pretty JSON 未统一（12-25% 缩进税） | search 4422B→3877B 可省 545B | pretty 税 = **1 byte** | ✅ 已落地 |
| T-05 空值占位照常序列化 | run 59B 空值税 | run 只剩 `{"status":"ok"}` | ✅ 已落地 |
| T-07 可推导计数器 | results_count 20B | meta 已无 results_count | ✅ 已落地 |
| T-10 doctor 散文 message | 383B | doctor --json 结构化已存在 | ✅ 已落地 |
| T-04 meta 冗余键全量输出 | 14 键 343B（browser_path 77B…） | 只剩 10 键 232B，冗余键已删；compact 5 键仍为 opt-in | ✂️ 部分落地（剩余差异是拍板过的取舍） |
| T-03 read paragraph_index 冗余 | 944B/page | 未复测（浏览器路径） | ⏳ 盲测 FixG20 已加 dup_paragraphs 标注，默认形态未翻转 |
| T-06 http 谎报 ssl_valid | 正确性 bug | `verify http://example.com --json` → `ssl_valid = null`（不再谎报 true） | ✅ 已落地 |
| T-08 fetch batch message 重复 url | 30B/失败元素 | 未复测 | ⏳ 待查 |
| T-09 人读比 JSON 省（当时 2608 vs 4422B） | --json 默认开主张 | 被拍板原则否决（默认零破坏） | ↩️ 后被 v0.4.0 反转（默认改 md，见文末「本轮结论」） |

## 核心数据（2026-11-12 实测，SearXNG provider）

- `search "rust async runtime" --limit 5`：**1723–1836 bytes ≈ 430–460 tokens**（字节随 SERP 内容漂移；当时同场景 2345B → 已再降 ~25%）
  - results 1416B（snippet 699B = 内容本体，不可省）/ meta 232B / run 16B
  - `--compact-meta` 再省 **135B ≈ 33 tokens**（default meta 10 键：tool/version/query/profile/humanize/limit/elapsed_ms/truncated/provider/truncated_detail → compact 5 键：query/elapsed_ms/truncated/provider/truncated_detail）
  - results 键固定 5 个：title/url/snippet/score/domain_class
- `fetch README` 实测 9610B ≈ 2400 tokens（50000 是 cap 上限，非实际输出；正文长度=页面长度）

## 拍板原则（10-08 已定，v0.3.1 违反当天翻车）

> **默认行为零破坏 > opt-in 新能力 > 省一点 token。** 所有"省 token 但改默认行为"的项全部退到 opt-in——存量 agent 的契约稳定性是本工具的生命线。

（2026-11-12 晚更新：该原则**未被推翻**，而是在「输出形态反转」这一条上被更高优先级的目标覆盖——理由与 v0.3.1 翻车的区别见文末「本轮结论」。）

## 拍板项（2026-11-12）

| 选项 | 内容 | 收益 | 代价 | 推荐 |
|---|---|---|---|---|
| A 维持现状 | 不改代码；skill 文档引导 AI 用 `--compact-meta` / `--max-chars` | 0 风险 | 无 | ⭐ 推荐 |
| B compact 默认翻转 | `--compact-meta` 变默认，`--full-meta` 才全量 | 135B ≈ 33 tokens/query | 违约 + CHANGELOG/README/skill/盲测基线全量重验（v0.3.1 同类翻车） | 不推荐 |
| C fetch cap 砍默认 | 50000→8000 | 超长页省 ~10000 tokens | **正是 revert 过的操作** | 🚫 除非用户明确要求 |

## 本轮结论（2026-11-12 晚，v0.4.0 默认输出反转）

上表的 A/B/C 是在「保持默认 JSON」前提下问的问题。**用户随后直接拍板了输出形态本身要反转**——「你也别加什么 --md 了..直接就所有数据就是 md 不就好了么?」于是问题从「怎么在 JSON 里省」变成「默认该不该是 JSON」，答案：**不该**。

### 决定

- **默认输出从 JSON 反转为 md 排版**（v0.4.0，breaking）；`--json` 从 no-op 隐藏占位变为**显式 JSON 开关**，输出内容与 v0.3.0 **逐字节一致**（零内容变更，只改入口）。
- **不新增 `--md` flag**（用户明确否决）——md 就是默认，不需要第二个开关去选它。
- **`--human` 保留**，语义变为「显式要 md」（等价于默认）——v0.3.0 的人读对比表 / 报告块形态不再保留（保留会与「默认 md」语义打架），md 是唯一的非 JSON 形态。

### 省 token 实测（tiktoken cl100k_base 精确计数，同一批 5 条结果，只换渲染形态）

| 形态 | tokens | 相对 JSON |
|---|---|---|
| JSON 全量 | 436 | — |
| md 排版 | 308 | **−29%** |
| md + 1 行 meta 头 | 337 | −23% |

### 结构性成因：JSON 的键名税是 per-result 的

每条结果固定多付 `"title":` `"url":` `"snippet":` `"domain_class":` 共 **47B ≈ 12 tok** 键名——这是**每条结果**都要付的固定开销，与内容长度无关，因此**条数越多亏得越多**：

| limit | JSON | md | 省 |
|---|---|---|---|
| 5 | 436 tok | 308 tok | −29% |
| 10 | 996 tok | 484 tok | −51% |

换句话说：上表 T-01~T-10 那些「在 JSON 里省 4-8%」的项，都是在**已经付了键名税的载荷上做边际优化**；换形态是**把税整个消掉**，量级差一个数量级。这解释了为什么前面 17 轮盲测把 JSON 榨到「剩余肥点趋近于零」却仍然显著贵——瓶颈从来不是字段值，是字段名。

### 盲测结论：md 形态不损 AI 提取准确率

独立 agent 盲测（各只读一种形态，答案由 PM 按 JSON 源逐字核对）：

- **JSON 组 5/5 全对**
- **md 组 5/5 全对，且答案逐字一致**——含带 `msockid` 的长 URL、40 字符截断边界这两个易错点

即：md 不是「省 token 换精度」的权衡，而是**纯赚**。这是本次反转成立的前提——若 md 组出现任何一处答案偏差或改写，默认反转就得重估。

### 与「默认行为零破坏」原则的关系

「默认行为零破坏 > opt-in 新能力 > 省一点 token」这条 10-08 定的原则**没有被推翻，而是被更高优先级的目标覆盖**：本工具的唯一消费者是 AI/agent，**输出形态本身是消费协议**——协议层的破坏（md 换 JSON）有明确、零成本的迁移路径（加 `--json`，JSON 内容逐字节不变），与「悄悄改一个默认值让存量脚本静默拿到不同结果」不是一类事。

参照 v0.3.1 的 revert 先例（fetch `--max-chars` 默认 50000→8000，发布当天被 revert）：那次翻车的原因是**存量脚本静默拿到截断更狠的正文**（数据变了，调用方不知情）；本次反转是**输出格式变了，调用方立刻报错**（`json.loads` 抛 JSONDecodeError），且报错信息自带修复路径（加 `--json`）。**响亮的破坏优于静默的破坏**——这是本次拍板与 v0.3.1 翻车的关键区别。

迁移指引（README / CHANGELOG 已落）：只有「依赖默认 JSON」的脚本会破，加 `--json` 即完全兼容。`--human` 侧的可见变化只有一处：v0.3.0 的人读对比表 / 报告块形态（verify 批量表、doctor 人读检查表）被 md 形态取代——search 的 `--human` 仍是 md 列表，只多一行 meta 头。
