# gsearch 输出 token 审计复盘（2026-11-12）

用户提问：「返回的是不是直接 JSON 结构？AI 直接拿 JSON 不如给处理好的数据，省 token」。

结论先行：**是 JSON，且这正是设计决定；该省的已在 10-08 两轮审计 + 22 轮盲测（编号 5–22，17 份 final 报告）中省完，今天的实测证明剩余肥点趋近于零；改默认值有当日翻车的 revert 先例。**

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
| T-09 人读比 JSON 省（当时 2608 vs 4422B） | --json 默认开主张 | 被拍板原则否决（默认零破坏） | 🚫 有意不做 |

## 核心数据（2026-11-12 实测，SearXNG provider）

- `search "rust async runtime" --limit 5`：**1723–1836 bytes ≈ 430–460 tokens**（字节随 SERP 内容漂移；当时同场景 2345B → 已再降 ~25%）
  - results 1416B（snippet 699B = 内容本体，不可省）/ meta 232B / run 16B
  - `--compact-meta` 再省 **135B ≈ 33 tokens**（default meta 10 键：tool/version/query/profile/humanize/limit/elapsed_ms/truncated/provider/truncated_detail → compact 5 键：query/elapsed_ms/truncated/provider/truncated_detail）
  - results 键固定 5 个：title/url/snippet/score/domain_class
- `fetch README` 实测 9610B ≈ 2400 tokens（50000 是 cap 上限，非实际输出；正文长度=页面长度）

## 拍板原则（10-08 已定，v0.3.1 违反当天翻车）

> **默认行为零破坏 > opt-in 新能力 > 省一点 token。** 所有"省 token 但改默认行为"的项全部退到 opt-in——存量 agent 的契约稳定性是本工具的生命线。

## 拍板项（2026-11-12）

| 选项 | 内容 | 收益 | 代价 | 推荐 |
|---|---|---|---|---|
| A 维持现状 | 不改代码；skill 文档引导 AI 用 `--compact-meta` / `--max-chars` | 0 风险 | 无 | ⭐ 推荐 |
| B compact 默认翻转 | `--compact-meta` 变默认，`--full-meta` 才全量 | 135B ≈ 33 tokens/query | 违约 + CHANGELOG/README/skill/盲测基线全量重验（v0.3.1 同类翻车） | 不推荐 |
| C fetch cap 砍默认 | 50000→8000 | 超长页省 ~10000 tokens | **正是 revert 过的操作** | 🚫 除非用户明确要求 |
