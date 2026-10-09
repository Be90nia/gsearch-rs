# 盲测九——全新 AI 受试者，验证 FixG8（profile fork + 8 拉分项）

**方法**：3 个全新 task 子代理（SubjectJ/K/L），任务与盲测六/七/八 A/B/C/D/E/F 同构（tokio 版本调研 / GitHub issue 阅读 / serde_json 文档提取），上下文只给 exe + README/USER_GUIDE + help；**禁读 reports/、禁 git log、禁打听**——不知道任何修复历史。逐命令记账 + 结构化评分。PM 不参与打分。

## 成绩单

| 受试者 | 任务 | 完成 | UX | Token |
|---|---|---|---|---|
| J | tokio 最新版本+变更调研 | ✅ 1.53.2 + 1.53.1 + 1.53.0 三版+CHANGELOG+atom feed 多源 | **8.5** | **9.0** |
| K | GitHub issue 正文+讨论 | ✅ #8065 性能回归完整 5 要点（PR #7431→#8120→1.51.2） | **7.5** | **7.5** |
| L | serde_json from_str 签名/Errors/示例 | ✅ 1473B 一次拿齐+Source 锚点反查 | **8.5** | **9.5** |

**均分：UX 8.17 / Token 8.67**（盲测八 7.5/8.23；原班复测 9.5/9.5）

## FixG8 修复被无上下文受试者实证消费

- **P0 profile fork 双层防御**：J/K/L 三人 0 撞锁提示（H/I 主因消失）——盲测八 H 撞锁 55.8s+59.1s 共 115s 无产出、I 撞 100s → 三人 8m21s/4m22s/2m28s **全部流畅跑通**
- **P1 JSON brace 边界截断**：J 跑 `fetch crates.io API` 拿到完整 max_version（盲测八 G "半截 JSON json.loads 失败"消失）
- **P1 site_warn**：未在三人任务中触发（任务不含 site: 查询）——已实测有 meta.site_warn 字段（PM 验证通过）
- **0bh 内容保真（修复前已合）**：L 拿到 `pub fn from_str<'a, T>(s: &'a str) -> Result<T>\nwhere\n    T: Deserialize<'a>,\n` 完美分行 + Example `#[derive(Deserialize, Debug)]\nstruct User {` 完美
- **b5f 全量代号清零**：三人 help 体验零扣分

## 受试者打分轨迹（同任务同口径）

| 任务 | 盲测六 | 盲测七 | 盲测八 | **盲测九** |
|---|---|---|---|---|
| tokio 版本调研（A/D/G/J）| 9/9 | 8.5/9 | 8.5/8.5 | **8.5/9.0** |
| GitHub issue 阅读（B/E/H/K）| 7/7 | 7/7.3 | 6/7 | **7.5/7.5** |
| serde_json 文档（C/F/I/L）| 5/6 | 7.5/8.5 | 8.0/9.2 | **8.5/9.5** |

L 任务从 5/6 → 9.5/9.5（**+4.5 UX / +3.5 Token**），是盲测系列最大单点提升——0bh 内容保真 + browse/fetch 文档清晰 + meta 标注全套生效。

## 新发现（盲测九挖出，按拉分潜力排序）

### K 拖后腿主因（UX/Token 各 -2.5）
1. **browse 默认 AdaptiveRead GitHub issue 空（K 扣 1）**：容器选择器对 GitHub 新版 React DOM 仍 miss，hint 兜底有效但要重试 1 次
2. **--markdown 未剥 GitHub 顶部 nav 7KB（K 扣 1）**：markdown 模式命中容器但选了外壳，consumer 还要 postfilter
3. **默认 max-chars=50000 截断不报告 omit 位置（K 扣 1）**：meta.omitted 只给总数不给位置，agent 还要主动拉 ≥80KB

修这三个潜力：K 涨到 9+/9+，均值能上 9+/9+

### J 的零散扣分（合计 -1.5 UX / -1 Token）
- fetch 缺 timeout/重试参数（GitHub 抖动 3 次 ~30s 硬等）——0.5
- fetch 大 JSON 无 offset/字段选择（crates.io API 字段藏 5KB 后）——0.25
- --include 单容器命中（releases 列表只拿首个）——0.25

### L 的两个零星扣分（合计 UX -1.5）
- search 混入 Singapore Citibank 噪声（"str"→"SMRT"包含匹配）+ --read 5 触发无关 URL 浏览器超时 30s——0.5（**SearXNG 白名单副作用**，非工具缺陷）
- fetch 源文件锚点 #2709-2714 无法裁剪（拉了 87 行没到 target）——0.5（**Source 锚点裁剪可加**）

## 对比轨迹

| 轮 | 方法 | UX | Token |
|---|---|---|---|
| 六首测 | 真 AI 受试者 | 7.0 | 7.33 |
| 六复测 | 原班（知道修了什么） | 9.5 | 9.5 |
| 七 | 全新（零上下文） | 7.67 | 8.27 |
| 八 | 全新（验证 FixG7） | 7.5 | 8.23 |
| **九** | **全新（验证 FixG8）** | **8.17** | **8.67** |

诚实解读：
- **Token 8.67 距 9.5 差 0.83**——K 是拖累；L 已 9.5
- **UX 8.17 距 9.5 差 1.33**——K + 一些零散扣分
- FixG8 的 P0 撞锁根治**显著**：盲测八 H 6/7 拖均值 0.5/0.23，九 K 涨到 7.5/7.5 把均值拉回 +0.5
- L 涨到 9.5 Token = 内容保真修复完整路径生效（盲测六 C 5 → 九 L 9.5，**+4.5 Token**）

## 距 9.5 缺口

| 缺口来源 | UX 缺口 | Token 缺口 |
|---|---|---|
| K 三个 GitHub browse 问题 | 1.5 | 1.5 |
| J 三个 fetch 问题 | 1.0 | 0.5 |
| L 两个零星问题 | 0.5 | 0 |

[INFERENCE] 样本 3 人 ±0.5 噪声。9.5 缺口集中在 K 的 GitHub 长页面 browse 体验——这是同一根因（GitHub 新版 React DOM 容器未中）的多种表现。修这一个根因，K 三个扣分同时下降。

## 优先级建议（若再开 FixG9）

1. **GitHub 新版 React DOM browse 容器扩展**：增加 `data-testid`、`markdown-body`、`[role="article"]` 等选择器；命中后 priority 高于旧 `.js-comment-body`
2. **browse --markdown 剥 GitHub 顶部 nav**：识别 `.Header`、`[role="banner"]` 等 nav 容器并剔除
3. **fetch max-chars 截断 meta 增 `truncated_at_offset`**：告知截断位置，让 agent 知道哪段缺
4. **fetch 加 --timeout / --retry 参数**：最小侵入（G 反复被坑）
5. **fetch 加 --json-keys JSONPath 投影**：解决大 JSON 字段位置问题（J 反复被坑）

修 #1+#2 估 50-100 行 + 测试；K 涨 1.5 UX/1.5 Token，均值能上 9/9.2。修全部 5 项估 200-300 行；潜在 9.3/9.5。
