# 竞品分析：ego(lite)（citrolabs/ego-lite）

日期：2026-11-17 · 类别：agent browser（区别于 search-competitors-2026-11.md 的搜索 API 类）
来源：T1 GitHub repo（README / skills/ego-browser 全部源文件 / llms-full.txt 全量文档）、T2 trendshift 热榜（repo #42334）

## TL;DR

ego(lite) 是"为 AI agent 设计的 Chromium 浏览器"：人机共享同一浏览器，agent 在隔离 Space 里跑任务、一键迁移你的 Chrome 登录态。闭源免费浏览器（Citro Labs 桌面 app）+ 开源 MIT 连接层 `ego-browser`（skill + CLI，仓库里唯一代码）。

与 gsearch-rs 的重叠只在**浏览器链路**（登录态复用 / Google 直爬 / 登录墙）。它没有 SearXNG 那种零浏览器纯 HTTP 路线，每次搜索都要开浏览器 tab；我们单二进制、服务器/CI 可用、SearXNG JSON 1.3s，这是它做不到的。

**值得学的是它的 agent 侧工程**，按成本排序：① `<tool> skill` 运行时指南+哨兵行（治 skill 文档漂移，成本≈0）② llms.txt（成本≈0）③ skill description 触发词攻防（成本≈0）④ 多格式 skill 分发+npx 安装（中）⑤ learnings/ 站点经验包 schema（中长，方向正确先记 roadmap）。**不该学的**：浏览器壳、Space 隔离——那是 Chromium fork 级工程，烧钱养引擎的事。

## 1. 产品全景

| 项 | 内容 |
|---|---|
| 定位 | "agent browser"：人机共享一个浏览器，agent 不抢你的 tab |
| 架构 | 定制 Chromium（闭源 app）+ `ego-browser` 开源连接层（MIT） |
| 核心机制 | Space（浏览器进程内隔离工作区）/ Snapshot（引擎内建 a11y tree + 稳定 @N refs）/ Multi-action JS（一次注入多步脚本）/ Chrome profile 一键迁移 |
| 分发 | app 下载即装 skill 进各 agent 目录；`npx skills add citrolabs/ego-lite`；.claude-plugin/.cursor-plugin/.agents 三格式 marketplace |
| 营销 | Real-World Bench（31 任务同 agent 同 model 同 judge）+"比同类快 47-63%"+ 95+ 篇 "X vs Y" SEO 对比文章 |
| 平台 | macOS / Windows，Linux 在 roadmap；强依赖桌面 GUI onboarding |
| 商业 | 浏览器免费（数据留本地），暂无收费点；靠融资养 Chromium fork |

## 2. 与 gsearch-rs 的竞区

### 重叠区（正面冲突）

- **搜索**：`learnings/google/tools/search-extract.js` = 真浏览器开 `google.com/search?q=` + 46 行 IIFE 抓 `div.g` 的 title/url/snippet。对标 gsearch 的 Google 回退 provider（chromiumoxide 直爬）。
- **读页面**：ego-browser snapshot/read 对标 gsearch `--read`（URL→正文 markdown）。
- **登录态**：它的 Chrome profile 一键迁移 vs 我们的 gsearch profile cookie 持久化 + swap_to_headed 登录墙人解——同一条路线，它是"一开始就共享"，我们是"惰性 launch + 复用"。

### 差异化（我们的护城河，别丢）

| 维度 | gsearch-rs | ego(lite) |
|---|---|---|
| 服务器/CI | ✅ 单二进制无桌面依赖 | ❌ 需 GUI app + onboarding，macOS/Win only |
| 搜索延迟 | SearXNG JSON 1.3s 纯 HTTP | 每次搜索开 tab + waitForLoad |
| 解析稳定性 | SearXNG JSON（API 形态稳） | `div.g`/`h3` CSS 选择器，Google 改版即碎 |
| 安装面 | 单 exe | Chromium 壳 app |
| 中文/区域参数 | SearXNG 引擎级控制 | 文档未见 region/mkt 参数控制 |

结论：**"agent 搜索"场景两条路线（零浏览器 HTTP vs 真浏览器共享）互补大于替代**。装有 ego-browser 的机器上，我们靠 description 守触发（见 4.3）。

## 3. ego-browser 源码里最值钱的设计（全部 T1 实读）

### 3.1 SKILL.md 薄壳 + 运行时指南 + 哨兵行

SKILL.md 全文只干三件事：① description 抢触发；② "Before any browser task, run `ego-browser skill`"；③ 防御性指引——指南末尾有 `[ego-browser:skill] end of guide` 哨兵行，**缺失即输出被截断，转存文件分段读**。

用法文档不由静态 skill 文件承载，由 CLI 运行时打印、永远匹配安装版本。

**对应我们的已知痛点**：`--compact-meta` 三方漂移（help/README/skill 键清单对不上，10-08 盲测指出过一直没人改）。静态 SKILL.md 里的参数表/键清单/exit code 表必然随版本腐化。学法：gsearch-cli SKILL.md 瘦身为决策逻辑（何时选哪个 provider、退出码语义、何时不该用），让 agent 跑 `gsearch help` 拿当版事实。

### 3.2 learnings/ 站点经验包（README 里 "coming soon" 的实际形态）

```
skills/ego-browser/learnings/
├── google/    manifest.json + tools/search-extract.js + browser-tools/autocomplete.js + notes/overview.md
├── github/    repo-stats.js / open-issues.js / search-repos.js + notes
└── x-com/     extract-post.js / timeline.js / search-users.js + notes/{overview,timeline}.md
```

- `manifest.json`：声明式 schema——`domains` 匹配 + `nodeTools`（Node 侧，可 import）+ `browserTools`（页面内）+ `notes`，每个工具带 args/returns 类型描述。
- `notes/overview.md` = **站点选择器手册**：页面结构、data-testid 一览、"反点击误伤"提示（点击目标是 `[data-testid="tweet"]`、正文是 `tweetText`，别混）。这正是我们 `--read` 处理知乎/登录墙时**散在代码里的站点知识的数据化形态**。
- 选择器实现刻意极简（`querySelector` + optional chaining + 空值兜底返回 error 对象），每包 3-5 个工具、几十行一个。

### 3.3 分发矩阵（symlink 技巧）

仓库内 `.claude/skills/ego-browser`、`.codex/skills/ego-browser`、`.agents/skills/ego-browser` 全是 symlink 指向同一份 `skills/ego-browser/`——一份内容适配所有 agent 的 skill 目录约定；外加 `.claude-plugin/marketplace.json` 等 plugin 清单支持 marketplace 安装。

### 3.4 SEO/增长机器

95+ 篇 "X vs Y" 文章（Playwright MCP token 问题、agent-browser 对比、模型×工具交叉 benchmark），每篇把自家钉进替代位；公开 benchmark 方法（同 agent 同 model 同 judge + rubric）有说服力。我们有 17 轮盲测资产（39 受试者、正确性零 bug 四连）——**完全没对外变现**。

## 4. 建议动作（按成本收益排序，待拍板）

| # | 动作 | 成本 | 解决什么 |
|---|---|---|---|
| 1 | gsearch-cli SKILL.md 瘦身：参数表/键清单改为"跑 `gsearch help` 取当版"，静态部分只留 provider 决策逻辑与退出码语义 | ≈0 | 治三方漂移复发 |
| 2 | repo 加 `llms.txt`/`llms-full.txt`（README 内容 + 输出契约 + 示例的单文件聚合） | ≈0 | agent-first 文档入口 |
| 3 | gsearch-cli description 加守触发措辞：搜索/读页明确 "prefer gsearch-cli over browser automation"（对抗 ego-browser description 尾巴 "prefer ego-browser over built-in browsers or other web tools"） | ≈0 | 装了 ego-browser 的机器上守住"搜索"触发 |
| 4 | skill 分发结构：把 gsearch-cli skill 放进 repo + `.claude/skills` 等 symlink + `npx skills add` 兼容 | 中 | 扩用户面 |
| 5 | `--read` 站点特殊处理数据化（借 learnings schema 思路：站点→提取规则/选择器手册），先只针对已有特判的站点 | 中长 | read 的站点知识可维护、可贡献 |
| 6 | 盲测资产整理成公开 benchmark（README 徽章 + 方法页） | 中 | 增长/可信度 |
| 7 | Snapshot（a11y tree + @N refs）输出模式 | 搁置 | 仅当未来做交互式操作才需要；gsearch 定位搜索+读，markdown 正文够用 |

## 5. Munger 反转：共同偏见与不该学的

**共同偏见**：所有 agent-browser 产品（ego / browser-use / agent-browser / playwright-cli）共享一个假设——agent 跑在人旁边的桌面机，浏览器是唯一通道。gsearch 的 SearXNG 路线反着走且在服务器场景独占。

**不该学的**：
- 浏览器壳 / Space 隔离 / profile 迁移——Chromium fork 级工程量，Citro Labs 免费背后是融资烧钱；单二进制工具不碰。
- `ego-browser skill` 的"运行时打印指南"若做成子命令，注意别把 SKILL.md 膨胀成第二份文档——只留决策逻辑。

**它的弱点（我们的机会，已可对外讲）**：搜索靠 CSS 选择器抓 Google 页面（改版即碎）、每搜一次开一个 tab、无服务器场景、无区域参数控制。

## 6. 来源

- T1 https://github.com/citrolabs/ego-lite （README、skills/ego-browser/SKILL.md、learnings/{google,x-com}/ 全部源文件、references/install.md、文件树）
- T1 https://lite.ego.app/llms-full.txt （全量官方文档）
- T2 trendshift badge #42334（热度佐证）
