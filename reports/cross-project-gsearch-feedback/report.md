# 跨项目 gsearch 使用反馈收集报告（2026-11-13）

> 范围：**gsearch-rs 之外**的其他项目里，agent / 用户对 gsearch 的使用与问题反馈。
> 方法：`history.db` SQL（6707 条 prompt + 714 条 recap）+ `sessions/` 全量 JSONL 扫描（其他 9 个项目目录，141 个会话，713 MB）+ 各项目记忆目录 grep + PATH/安装实测 + 独立 agent 复核。

## 0. 结论

**跨项目对 gsearch 的"问题反馈"几乎为零——因为其他项目基本没用过它。** 全库搜遍后，真实存在的只有 3 条：

| # | 类型 | 问题 | 证据强度 | 影响 |
|---|---|---|---|---|
| **B1** | **真 bug（静默误行为）** | `--humanize false` 这类 Python 风格布尔写法**不报错**，clap 把 `false` 当成第二个 QUERY → 静默变 batch 搜索 | 别的项目会话原文 + 今天实测复现 | 别的 agent 拿到**误导性的双倍结果** |
| **B2** | **部署缺口** | PATH 里的 `D:\Sdk\gsearch\gsearch.exe` 是 **v0.3.0**（2026-10-10 构建），其他项目拿不到 v0.4.0 | `which gsearch` + `--version` 实测 | v0.4.0 的 md 默认输出 / doctor 探针 8s 修复 / 安全硬化**对它们全部无效**；且 skill 现在写"默认 md"，对这些项目是**错的** |
| **B3** | **依赖风险** | eliquid-kb 把 gsearch 写进 AGENTS.md 铁律：「联网搜索一律 `bash` 调 gsearch CLI，**禁用 web_search 工具**」 | 项目记忆原文 | gsearch 已是至少 1 个项目的**搜索底座**，任何回归直接波及它 |

其余 11 个项目**未发现任何 gsearch 使用或问题**（名单见 §4）。

## 1. 跨项目使用实况（硬数据：会话里 `gsearch <子命令>` 调用次数）

| 项目 | 用法 | 调用形态 |
|---|---|---|
| **Xray-core-rust** | **重度用户** | `browse` ×29、`search` ×10、`doctor` ×6、`--proxy` ×6、`shell` ×3、`fetch` ×2、`verify` ×1、`--version` ×2；用的是 `D:\Sdk\gsearch\gsearch.exe`（PATH 里的那份） |
| **eliquid-kb** | **铁律级依赖** | AGENTS.md 写死「主代理与所有子代理 联网搜索一律 `bash` 调 gsearch CLI …**禁用 web_search 工具**」（另见 memory 里的 agent 约束段）；实际调用以 `doctor` 自检为主 |
| **serena-rust** | 降级路径 | 「网络环境 中国 可能超时 无降级到 web_search/**gsearch-cli** 降级路径」——把 gsearch-cli 当 web_search 失败时的兜底 |
| **server_api** | 诞生期 + 周报 | 2026-08-26 在 `D:/Sdk/plsearch` 写出 Python 原型 `gsearch.py`（gsearch-rs 的前身）；后来 gsearch-rs 只是周报里的并列项目 |
| **searxng** | 替代方案评估 | 「能不能替代我们的 `D:\Project\gsearch-rs`」——评估 SearXNG 官方 CLI 能否取代 gsearch-rs |

## 2. B1：`--humanize false` 静默变 batch（真 bug，已复现）

**别的项目的会话原文**（Xray-core-rust，2026-10-14）：

```
error: unexpected value 'false' for '--humanize' found; no more were expected

Usage: gsearch.exe search <QUERY|--limit <LIMIT>|--recency <RECENCY>|--read <READ>|--browse <BROWSE>|--dl <DL>|--open <OPEN>|--humanize|--no-humanize|--full|--human|--json|...
```

**今天实测（v0.4.0）**：

| 写法 | 实际行为 | 是否符合预期 |
|---|---|---|
| `--humanize false` | **exit 0，静默按 batch 执行**：`search "test" "false"` → meta 头 `results: 20`，输出里出现 `batch 完成：## test` | ❌ **静默误行为**（`false` 被当作第二个 QUERY 位置参数） |
| `--humanize=false` | `error: unexpected value 'false' ...` | ❌ 报错（clap `SetTrue` flag 不接受显式值） |
| `--no-humanize` | 正常工作 | ✅ |

**成因**：`--humanize` / `--no-humanize` 是 clap `ArgAction::SetTrue` 布尔 flag——
- 传**空格分隔**的 `false` → 被 clap 当成位置参数（QUERY）→ 命令照常执行，**参数个数校验也过了**（多查询 = batch 模式）；
- 传**等号形式** `--humanize=false` → clap 报 "unexpected value"。

**为什么别的 agent 会这么写**：Python/argparse 习惯 `--flag false`；bash 里 `gsearch search "x" --humanize false` 看起来最自然。别的 agent 的自然写法 = 我们的静默错误输出。

**建议修法（零破坏，纯兼容增强）**：给这些布尔 flag 改成可显式带值——
```rust
#[arg(long, num_args = 0..=1, default_value_t = false, default_missing_value = "true",
      action = clap::ArgAction::Set)]
humanize: bool,
```
这样 `--humanize` / `--humanize true` / `--humanize=false` / `--humanize false` **四种写法全部可用**，且旧行为（裸 flag）不变。

**验收**：`gsearch search "x" --humanize false` 输出**单查询**结果（非 batch），`--humanize=false` 等价，`--humanize` 仍为 true。

## 3. B2：其他项目用的是 v0.3.0 旧版（部署缺口）

**实测**：

```
$ which -a gsearch
D:\Sdk\gsearch\gsearch.exe            ← 在 PATH 里，其他项目命中的是这份

$ D:\Sdk\gsearch\gsearch.exe --version
gsearch 0.3.0 (git:v0.3.0)             ← 2026-10-10 构建

$ D:\Project\gsearch-rs\target\release\gsearch.exe --version
gsearch 0.4.0 (git:v0.3.0-5-g4cd981d)  ← 本项目当前
```

**兼容性已实测**：v0.3.0 与 v0.4.0 的 `--json` 键集**完全一致**
（`meta` 10 键 / `run` 1 键 / result 5 键，两版逐项相同）→ **升级后加 `--json` 的脚本零破坏**，唯一行为差异是默认输出形态（JSON → md）。

**但现在有个真实的不一致**：昨天 v0.4.0 把默认输出反转成 md，而 skill（`~/.omp/agent/skills/gsearch-cli/SKILL.md`）已改写成「默认输出就是 md」——**其他项目的 agent 读到的是这份 skill，跑的是 v0.3.0**，拿到的是 JSON。文档与行为不符 → agent 可能按 md 假设去解析 JSON（或反向）。

**建议**：用 v0.4.0 覆盖 `D:\Sdk\gsearch\gsearch.exe`（保留其 `gsearch.json` 与 `profiles/`，先备份旧 exe；可秒级回滚）。**需用户拍板**——因为这是 breaking 变更落到其他项目的头上（虽然实测它们的用法是"读输出"，md 反而更省 token）。

## 3.5 立项原点：别的项目当时**根本没有可用的搜索**（第一手证据）

server_api 项目 2026-08-26 的会话 `01a03bac-7d51-7000-a712-45d5f1ef8027`（**同一 session 在 gsearch-rs 记忆里的标题是「规划 gsearch-rs 项目」**——即 gsearch-rs 的立项就发生在别的项目里）完整记录了搜索能力真空：

**① web_search 的免 key 引擎全被反爬拦截**（agent 复测两次，结果一致）：
> 第二次调用结果与第一次完全一致——**5 个免 key 引擎全部被反爬拦截**（Google 弹人机验证页，DDG/Mojeek 明确判定出口为机房/共享 IP）。失败可复现，不是瞬时抖动。

**② 本机无任何搜索凭据**：
> `webSearchOrder`/`webSearchExclude` 均为空 走内置默认链 - 环境变量无任何搜索 API key Brave/Tavily/Exa/Kagi 等全无 - `~/.omp` 下无已存的搜索凭据 - `web_search.enabled = true` 开关本身是开的 **任务终态 所有本机可达的排查与验证已穷尽**

**③ 唯一的"搜索 skill"依赖未接线的 MCP**（用户手动提示后 agent 才发现）：
> 用户：「你看看用skill呢..不是有搜索用的skill么?」
> agent：找到 `internet-research` skill，但它依赖两个 **MCP** 工具（`websearch_web_search_exa` = Exa 系、`grep_app_searchGitHub`）——**这些 MCP 服务器未接线**，所以内置 web_search 挂了它也用不上。

**结论**：gsearch-rs 的存在意义被这份证据直接证实——它是本机**唯一免 key、不依赖外部 MCP/凭据**的搜索路径。这也解释了为什么 eliquid-kb 会把「一律 `bash` 调 gsearch CLI，禁用 web_search」写进铁律。

**新增行动项 B4（触发率）**：那次是**用户手动提示**"你看看用skill"才让 agent 转向 skill。说明 gsearch-cli skill 在"web_search 搜不到/挂掉"这类场景下的**自动触发率可能不足**——skill description 里已有"web_search 经常搜索不到或搜索报错"这句线索，但 agent 当时没走通。建议用 `skill-comply` 的方差测试法实测触发率（单轮 10/10 可能是假阴性，见记忆里的教训），确认后再改 description 措辞。

## 4. B3：gsearch 是别的项目的搜索底座（依赖风险）
eliquid-kb 的 agent 约束原文（项目记忆 2026-09-28）：

```
- **联网搜索 铁律**
- **主代理与所有子代理 联网搜索一律 `bash` 调 gsearch CLI** 搜索/抓正文/下载 **禁用 web_search 工具** …
```

含义：**别的项目的 agent 已经把 gsearch 当唯一搜索入口**。gsearch 的任何回归（doctor 误报、search 空结果、browse 失败）会直接打穿它们的调研流程——而这些项目**不会**像 gsearch-rs 自己那样有 22 轮盲测兜底。

**建议**：给 skill 加一条"先 `gsearch doctor` 自检 + 失败时降级"提示；doctor 的 SearXNG 探针超时（v0.4.0 已从 3s 放宽到 8s）这类修复对其他项目同样重要 → 见 B2。

## 5. 明确查过但**未发现** gsearch 问题的项目

argosbx / cf-keepalive / tcpboost / codebase-memory-mcp-pro / server_api-docker / python_server-master / Xray-core / office_oxide / scandir-rs / jwalk-meta / Notes / tmp-hide-test
——搜了各自的 prompt（`history.db`）、会话 recap、session JSONL 全文、项目记忆目录，**零 gsearch 使用痕迹**。

## 6. 方法与可复核性

| 数据源 | 位置 | 覆盖 |
|---|---|---|
| 历史 prompt | `~/.omp/agent/history.db` → `history` 表（6707 行，含 `cwd`） | 按项目过滤 `prompt like '%gsearch%'` |
| 会话摘要 | 同库 `session_recaps` 表（714 行，含 `cwd`） | 搜 gsearch / web_search / 搜索工具 |
| 会话全文 | `~/.omp/agent/sessions/<项目>/*.jsonl`（其他 9 项目，141 文件，713 MB） | 逐行子串匹配 → 报错签名聚类 → 抽原文 |
| 项目记忆 | `~/.omp/agent/memories/--D--Project-*/`（9 个非 gsearch-rs 项目） | grep gsearch / web_search / 搜索工具 |
| 安装实测 | `which -a gsearch`、两处 `--version`、`--json` 键集对照 | PATH 解析与版本差 |

**噪声控制说明**：报错签名聚类命中 340+ 条，其中绝大多数是噪音（skill 描述/README/AGENTS.md 里引用 gsearch 时顺带出现 "CAPTCHA"/"失败"/"timeout" 字样、以及 `codebase-memory-mcp` 的项目名列表）。**经逐条回读原文，只保留 3 条真实问题**（B1/B2/B3）。

## 7. 行动建议（按优先级）

| 优先级 | 事项 | 类型 | 成本 |
|---|---|---|---|
| P1 | 修 `--humanize false` 静默变 batch（§2 修法） | 代码 | 小（clap 2 个 flag + 回归锁） |
| P1 | 覆盖 `D:\Sdk\gsearch\gsearch.exe` 到 v0.4.0（先备份） | 部署 | 秒级，**需拍板**（breaking） |
| P2 | skill 加「doctor 自检 + 失败降级」提示（服务 B3 的依赖方） | 文档 | 小 |
| P2 | skill 描述区分「本仓库 v0.4.0」与「PATH 上的 v0.3.0」，避免文档-行为不符 | 文档 | 小 |
| P3 | 观察其他项目反馈（Xray/eliquid-kb 用量起来后建立跨项目回归口径） | 流程 | — |
