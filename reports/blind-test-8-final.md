# 盲测八——全新 AI 受试者，验证 FixG7 三修复 + 63 处代号清零

**方法**：3 个全新 task 子代理（SubjectG/H/I），任务与盲测六/七 A/B/C/D/E/F 同构（tokio 版本调研 / GitHub issue 阅读 / serde_json 文档提取），上下文只给 exe + README/USER_GUIDE + help；**禁读 reports/、禁 git log、禁打听**——不知道任何修复历史。逐命令记账 + 结构化评分。PM 不参与打分。

## 成绩单

| 受试者 | 任务 | 完成 | UX | Token |
|---|---|---|---|---|
| G | tokio 最新版本+变更调研 | ✅ 1.53.2 + 1.51.5 + 1.53.1 三版双源 | **8.5** | **8.5** |
| H | GitHub PR 正文+讨论 | ✅ PR#8135（5 commits + discussion 全摘） | **6** | **7** |
| I | serde_json from_str 签名/Errors/示例 | ✅ 1473B 一次拿齐+Source 锚点反查验证 | **8.0** | **9.2** |

**均分：UX 7.5 / Token 8.23**（盲测七 7.67/8.27；原班复测 9.5/9.5）

## 修复被无上下文受试者当原生能力消费（生效验证）

- **0bh fenced 代码块保真**：I 实战 `fetch --markdown --include main` 拿到 `#[derive(Deserialize, Debug)]\nstruct User {...}` 完美分行，无链接注入、无撇号偷换——F 受试者"工具单独输出不可直接交付"的结论被根治；I 进一步用 Source 锚点反查 de.rs 源码交叉验证签名原文——这是修复**让 agent 真正敢用**
- **b5f 全量代号清零**：G/H/I 三人无人扣 help 代号分（盲测七 E 扣过 0.2）
- **wqm GitHub 评论缺失信号**：H 报告"meta.github_comments_hint + missing:true 诚实告知评论区未含本文并给升级路径（browse / api.github.com/.../comments）"——首次作为 delight 点名表扬，标志从"扣分点"升格为"信任信号"
- **isatty 快档**：三人独立 shell 无人遇 89s 慢档（盲测六 C -2 坑消失）
- **--max-chars**：G 全程自控 + I 二次 fetch 源码 104948B 走 max_chars=200000——字节预算被当原生能力消费

## 新发现（盲测八挖出的下一层，按严重度）

### P0 多 agent 共享 Chrome profile 单例锁（H + I 都撞）
盲测设计本身让三受试者并行→共享 ~/.gsearch/profiles/default 撞锁：
- **H**：第一条 search 走 Google 回退触浏览器，lockfile 被他人持锁 → ExitStatus(21) ×5 重试耗 55.8s；等 60s 再跑又撞同锁再耗 59.1s——两条 search 共浪费 115s 无产出，被迫绕过浏览器改 fetch 直拼 GitHub URL
- **I**：首条 `search --read 5` 整 100s 报 ExitStatus(21) ×3 + Google ERR_CONNECTION_CLOSED，0 字节 stdout
- h90 修了 WARN 出口但**无法避免撞锁本身**——需要 `--no-browser` 强制 SearXNG-only 旗标，或并发 agent 自动 fork 子 profile

### P1 fetch 重定向陷阱（H 实锤）
`fetch URL --max-chars 100000 > file 2> err` 触发 10s timeout + 0 字节产出；同样的命令用 `2>&1 | tee file` 拿到 24581 字节全量。stdout/stderr 缓冲与 timeout 互作用导致——`>`/`<` redirect 行为与 tee 不一致是隐形坑。H 扣 15。

### P1 机器 JSON 截断痛点（G 实锤）
fetch 对 GitHub API list JSON（每条 release 挂几十个 asset 字段）按字节截断后 `json.loads` 失败（半截 JSON 无法解析）；meta.omitted=9262 诚实标注但 agent 仍要重新逐 tag fetch——无 JSONPath 投影、无"对象边界截断"开关。G 扣 0.5。

### P1 SearXNG site: 限定符支持不一致（H 实锤）
`site:github.com/tokio-rs/tokio refactor` whitelist 引擎 0 命中，但 search 错误只说"查询无结果"不解释为什么——site: 被默认静默吞掉，应至少警告"site: 被 N 引擎忽略"。H 扣 6。

### P2 残余打磨
- fetch 默认无 --markdown 对 docs.rs 几乎无用（必须 --markdown 才出 fence）——help 应明示契约（I 扣 2）
- browse --markdown 输出被 read 工具预览层截 768 chars，无 `truncated_at_read_tool` 标识——I 误以为命令本身截断（I 扣 1）
- fetch 命中 main 后没把 Source 锚点带进 meta——验证闭环比预期多一步（I 扣 1）
- fetch 连接级失败无降级提示（vs JS 壳页有 browse 提示）——GitHub 拒纯 HTTP 客户端是高频场景（G 扣 0.5）

## 对比轨迹

| 轮 | 方法 | UX | Token |
|---|---|---|---|
| 六首测 | 真 AI 受试者 | 7.0 | 7.33 |
| 六复测 | 原班（知道修了什么） | 9.5 | 9.5 |
| 七 | 全新（零上下文） | 7.67 | 8.27 |
| **八** | **全新（验证 FixG7 三修复）** | **7.5** | **8.23** |

诚实解读：均值略降 ≠ 修复失效，而是 **H 的 P0 多 agent 撞锁把均值拖下来**（单点 6/7 拉低 UX 0.17 / Token 0.04）。**修复单独看**：I 的 8.0/9.2（0bh 验证、F 升档 0.5/0.7）证明内容保真修复是杀手锏；G/H 的 search 路线修复链路稳。

[INFERENCE] 样本量 3，单人 ±0.5 噪声；H 撞锁是实验设计诱发（盲测并行共享 profile），非真实单 agent 用户场景。下一轮要解决：(1)给并发受试者各起独立 profile；(2)若不解决 P0，多 agent 用户的盲测分数天花板就是 7-8。

## 优先级建议

1. **P0：profile 撞锁**——加 `--no-browser` 强制 SearXNG-only 旗标（最小侵入），或并行 agent 自动 fork 子 profile
2. **P1：fetch 重定向陷阱**——查 stdout/stderr/timeout 互作用根因（chromiumoxide 链路还是 reqwest？），修在 fetch.rs
3. **P1：机器 JSON 截断**——加 `--json-keys` JSONPath 投影或"对象边界截断"开关
4. **P1：site: 静默吞掉**——search 错误明示"site: 被 N 引擎忽略"+建议拆词
5. **P2：fetch --markdown 默认值**——docs.rs 类结构化页面默认走 markdown（成本：每次多一次 scraper；收益：消除首测盲试）
