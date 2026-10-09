# gsearch-rs 盲测六——真 AI 受试者体验盲测（用户视角：烧不烧 token；AI 视角：爽不爽）

**方法**：不再模拟"AI 会怎么想"——派 3 个真 AI 子代理当受试者，只给 exe + README + USER_GUIDE + help，各领一个真实任务，逐命令记账（字节数实测、每步满意/无用/误导、逐条扣分）。PM 不参与打分。

## 受试者成绩单

| 受试者 | 任务 | 完成 | UX 分 | Token 分 | 总消耗 | 扣分 |
|---|---|---|---|---|---|---|
| A | tokio 最新版本+新特性调研 | ✅ | **9** | **9** | 116KB（自切片后实际进上下文 ~20KB） | fetch 大页面无字符预算（-1） |
| B | 读 GitHub issue 正文+讨论 | ✅ | **7** | **7** | 22.2KB | browse 超时不提示 --proxy（-1）；profile 锁 38s 裸失败无 fetch 降级出口（-1）；16KB 单行 JSON 行式读取摩擦（-1） |
| C | serde_json from_str 文档提取 | ✅ | **5** | **6** | 13.6KB | 默认 humanize 慢档 89.3s vs 5.7s（-2）；裸环境静默回退 Google 无提示（-1）；goto 重定向壳 URL 不可消费（-1）；--read 漏 Example 段（-1） |

**均分：UX 7.0 / Token 7.33。任务成功率 3/3，零任务失败。**

## AI 摩擦榜（按烧钱/烧时间排序，全部带受试者实测证据）

1. **fetch/read 大输出无字符预算**（A/B/C 三人独立命中=最实锤）：A 的 fetch 单条 93.6KB 正文仅 ~7% 信噪比，直接进上下文=烧 2 万+ token，它靠把 stdout 重定向进文件再 python 切片才躲过；B/C 的 16KB 单行 JSON 让行式读取 agent 多花 3-5 个来回。**这是"烧不烧 token"的第一答案：搜索很省（2.7KB/8 条），fetch 是放血口。** → `n76` P2
2. **裸环境 humanize 慢档**（C）：没配 SearXNG 的用户首查 89.3s（humanize warmup+CAPTCHA 处理），加 --no-humanize 只要 5.7s——15 倍差价，且静默回退 Google 无一行提示。配了 SearXNG 的用户（A）完全无感（search 3.0s）。 → `9gb` P2
3. **goto 壳 URL**（C）：Google 回退路径 results.url=google.com/goto?url=... agent 无法引用/fetch 真实链接。 → `54c` P2
4. **browse 失败无出口引导**（B）：timeout 不提示 --proxy、profile 锁干等 38s 不提示"fetch 无需 Chrome"。 → `h90` P3
5. **--read 漏代码段**（C）：AdaptiveRead 给了 Errors 全文却在 §Example 前止步，逼二跑 --full。 → `9as` P3

## AI 爽点（受试者原话归纳）

- **help 是写给 agent 看的**（A/C 都点名）："Agent 反复调时建议加 --no-humanize"、"limit 上限 100，更大值翻页白耗时（实测 10000→18s）"——零试错成本
- **search 默认 JSON + SearXNG 命中**：一次命中、2.7KB/8 条、3s、零浏览器零弹窗
- **fetch 多 URL batch + --markdown 标题层级**：3 条并发 2.2s，按标题精确切片
- **契约安全意识**：content_untrusted 注入面标记被受试者当加分项
- **similar 三信号一致**（FixB5 附赠反馈）

## 实验设计备注（诚实边界）

- C 的环境变量未生效（task agent 独立 shell），意外构成"裸环境用户"对照组——因祸得福，挖出 1/2/3 号发现；配了 SearXNG 的路径（A）体验 9 分，说明**配置好的用户很爽，裸用户在流血**。
- B 的 profile 锁撞上了三受试者并发的 Chrome 单例（实验自扰），但"5 轮重试注定失败还干等"在真实并发场景同样成立，扣分维持、注明来源。
- FixB5（修复 agent）附赠撞点：browse meta.provider 硬编码 "google"——已并入 `h90`。

## 结论（回答用户两问）

- **烧不烧 token？** 搜索路径很省（这轮设计到位）；**fetch 大页面是唯一放血口**（n76 修掉后 token 分可上 9）。
- **AI 用得爽不爽？** 配好环境+看 help 的 AI：9 分很爽；裸环境 AI：5 分在流血。三个 P2 修完，两极分化收窄。

## 打分轨迹（盲测系列）

| 轮 | 方法 | 分 |
|---|---|---|
| 四 | PM 自测（工程视角） | 9.2 |
| 五 | 独立杠精审计（契约/安全视角） | 7.0 |
| **六** | **真 AI 受试者（体验/token 视角）** | **UX 7.0 / Token 7.33** |

三个视角互不替代：契约对、安全够，不代表 AI 用着爽。n76/54c/9gb 三个 P2 修完预计 UX 8.5+ / Token 8.5+。
