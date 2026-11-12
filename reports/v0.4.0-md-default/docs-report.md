# v0.4.0 默认输出反转——文档侧交付报告

> 范围：只改 3 个文档文件（README / CHANGELOG / token 审计复盘）。**未碰 `src/`、`Cargo.toml`，未 commit / push / tag。**
> 依据：`reports/v0.4.0-md-default/contract.md` 第 8 节；实测数据由 PM 提供（同源严格对照，tiktoken cl100k_base 精确计数 + 独立 agent 盲测）。

## 改动清单

### 1. `README.md`

| 位置 | 改动 |
|---|---|
| 顶部导语 + blockquote | 默认输出表述从「紧凑 JSON」改为 **md 排版（v0.4.0 起）**，写明省 ~29% token、盲测不损准确率、要程序解析加 `--json`；blockquote 记录反转语义（`--json` = 显式开关且与 v0.3.0 逐字节一致 / `--human` = 显式要 md / 缺席语义在两条路径上的对应） |
| `search` 用法示例 | 默认行注释改 md；新增 `--json` 示例行（显式 JSON），保留 `--human` 示例行（显式要 md） |
| **输出契约章节** | 标题从「JSON 输出契约」+「（默认）」后缀，改为「输出契约（v0.4.0：默认 md）」；新增 `##### 默认 md 形态` 小节（meta 头规格 + 各命令 md 形态表 + search 结果列表样例 + 「为什么默认是 md」实测表与键名税成因）；原 JSON 信封字段清单**全部保留**，下沉到 `##### --json 路径（显式 JSON，内容 = v0.3.0 逐字节一致）` |
| `read 失败显式化` | 改为三路径表格：默认 md（stdout 出已取结果 + 错误链走 stderr）/ `--json`（顶层 `read_error`）/ `--human`（stderr + exit 1）；**恒 exit 1** 写明 |
| `batch` 段 | 「默认输出裸数组（紧凑 JSON）」→ 默认 md 分节（meta 头 + `## <query>` + 结果列表）；`--json` 才是 JSON 数组 |
| `browse 输出契约` | 「默认输出 AdaptiveRead 结构化 JSON」→ 默认 md（meta 头带 `format` 源标签 + `# 标题` + md 正文），AdaptiveRead 字段清单归到 `--json`；`browse --human` 说明改为「显式要 md（等价默认）」 |
| `--markdown` 段 | 「JSON 消费分离 stderr」→「stdout 契约通道单一」（md 正文 / `--json` 信封两种口径）；补一段说明：md 路径正文默认就是 markdown，`--markdown` 在 md 路径等价默认、保留只为兼容，在 `--json` 路径仍按旧语义换字段源 + `meta.format:"markdown"` |
| `fetch` 用法 + 形态说明 | 默认 md（meta 头 + `# 标题` + 正文）；单 URL = meta 头 + `# 标题` + 正文（不分节），批量 = 同一 meta 头 + 每 URL 一节 `## <url>` + 正文 + 失败行 |
| **消费姿势章节** | 标题 `### 单行 JSON 消费姿势` → `### 消费姿势`；重写为「AI 直接吃 md（默认，零解析）」+「要程序解析加 `--json`」+ 保留禁 `2>&1` 合并流；省 token 杠杆表改为标注 **JSON 路径**口径 |
| 退出码章节 | exit 1 行补 md 路径下的 read 失败呈现；末尾加「注意（v0.4.0）：默认输出已不是 JSON，`json.loads(stdout)` 会抛异常」 |
| `browse/login/dl`、`doctor`、`verify`、`similar` 用法与说明 | 默认输出描述同步改为 md / `--json` 显式 JSON；doctor 章节写明 md 行四态 token（`[OK]/[WARN]/[FAIL]/[SKIP]`，warn/skip 不进 meta 头）；`--json` 路径节补「`--human` 与 `--json` 同传时 `--human` 优先」 |

### 2. `CHANGELOG.md`

新增 `## [0.4.0]` 段落（位于 `[Unreleased]` 之后、`[0.3.0]` 之前）：

- `### Changed（breaking）`：默认输出 = md 排版（各命令形态逐条列）、`--json` 语义反转（与 v0.3.0 逐字节一致）、`--human` 语义变为「显式要 md」、`truncated` 缺席语义沿用
- `### 迁移指引（存量脚本）`：破/好两行对照代码块 + **5 行「存量用法 → v0.4.0 行为 → 要不要改」表**（依赖默认 JSON 的要改加 `--json`；`--json` / search 的 `--human` 存量脚本不用改；verify / doctor 的 `--human` 旧表格形态被 md 取代；AI 直接吃 stdout 更好）
- `### Performance`：29% 省 token 实测（436 → 308 tok，md+meta 337 tok）、47B ≈ 12 tok 键名税的结构性成因（limit 10 时 JSON 996 vs md 484）、盲测 5/5 结论
- `### Documentation`：补 README 输出契约章节重写 + token 审计复盘追加本轮结论两条

另修正 `[Unreleased]` Documentation 条目里对 README 旧章节名的引用（「单行 JSON 消费姿势」→「消费姿势」）。

### 3. `docs/research/token-output-audit-2026-11-12.md`

- 顶部结论加「后续」提示框：原结论的前提错了，指向文末新节
- T-09 行状态从「🚫 有意不做」改为「↩️ 后被 v0.4.0 反转」
- 拍板原则节加注：该原则未被推翻，而是在「输出形态反转」这条上被更高优先级目标覆盖
- 追加 `## 本轮结论（2026-11-12 晚，v0.4.0 默认输出反转）`：决定（默认反转 / 不新增 `--md` / `--human` 保留）、省 token 实测表、**结构性成因**（键名税是 per-result 的，limit 5 → −29%、limit 10 → −51%，并解释「T-01~T-10 是边际优化 vs 换形态是消税」的量级差）、盲测 5/5 结论、与「默认行为零破坏」原则的关系（**响亮的破坏优于静默的破坏**——v0.3.1 翻车是静默改数据，本次是格式变了立刻报错且自带修复路径）

## 我替你拍板了什么

规格未写死、我自行决定的地方（**已与代码 agent MdCode 对齐**的项标 ✅）：

1. **README 章节标题层级**：契约只说「标题改为『输出契约（v0.4.0：默认 md）』」。JSON 信封字段清单不能丢（派单要求「保留 JSON 信封字段说明但明确它现在是 `--json` 路径」），所以我在新标题下加了一层 `##### 默认 md 形态` 与 `##### --json 路径（显式 JSON…）` 两个子节承载两套形态，避免一节里混型。契约第 3 节的形态矩阵则直接转成 README 里的「各命令 md 形态」表。

2. **meta 头字段清单的读法**：契约第 4 节给了 search/batch 的示例与 fetch/browse、verify、doctor 的字段名。我按契约原样落，未增删字段；`provider` 一项明确写「仅 search / batch 有」并加「不编造 provider 语义」——契约第 4 节原话。

3. **md 路径下 `score` 不出现**：md 结果列表复用 `print_text` 形态（契约第 5.1 节「= 现有 `output::print_text`」），该函数不渲染 `score`。这是「md 形态 = 现有 print_text」的直接后果，不是新设计——但它意味着**按相关性分筛序必须走 `--json`**，所以我在 README 显式写了一行提示（信息丢失必须写明，不能让用户自己撞）。

4. **read 失败的 md 呈现**：派单给了二选一（走 stderr / 另写 md 错误段），并要求拍板时标注。我选**走 stderr**——理由：契约第 4 节硬规格规定「stderr 只放诊断行，禁止混流」，且 md 输出契约没有定义错误段；另起错误段等于在输出契约外发明新形态。默认 md 路径下 stdout 仍出 meta 头 + 已取到的结果，错误链（全文）走 stderr，**exit 1 恒定**。

5. **消费姿势章节的省 token 表口径**：原表的数字全部是 JSON 口径（1811B/452tok 等）。反转后表格若继续标「默认」会误导，所以整表标注 **JSON 路径**口径，并加一句「默认 md 路径已省掉 JSON 键名开销（每条结果固定 ~12 tok），`--compact-meta` 只对 `--json` 路径生效」。✅ 已与代码 agent 确认：`--compact-meta` **只对 `--json` 路径生效**，md 路径的 meta 头字段固定（契约第 4 节写死），不吃裁剪。

6. **`--human` 的定位表述**：写为「等价于默认（显式要 md）」，不是「切回旧格式」——因为 md 默认就是 `print_text` 形态，旧人读文本与它同形。✅ 另补一条实现确认：**`--human` 与 `--json` 同传时 `--human` 优先**（保 v0.3.0 `--human --json` 仍给人读的兼容行为），已写进 README 与 CHANGELOG。

7. **CHANGELOG 位置与条目顺序**：放在 `[Unreleased]` 之后、`[0.3.0]` 之前（Keep a Changelog 惯例：未发布在最上，已发布版本倒序）。`[0.4.0]` 不写日期——PM 负责打 tag / 发 release，日期由发版时补。
8. **`[Unreleased]` 下的两条旧条目归入 `[0.4.0]`**：原 `[Unreleased]` 里的 `Fixed`（doctor 探针超时 3s→8s）与 `Documentation`（search 省 token 指引）都是**工作区未提交**的改动，会随 v0.4.0 一起发版——所以把 `[0.4.0]` 段落插在 `[Unreleased]` 与这两条之间，它们自然落到 `[0.4.0]` 下（`[Unreleased]` 留空作占位段）。未删任何内容，只改了归属版本。

9. **审计复盘文件里原结论不删**：只加「后续」提示框与 T-09 状态翻转，原表与拍板原则原样保留——它是「在 JSON 前提下能榨出多少」的记录，删掉会丢失决策链。

10. ✅ **doctor 的 md 行状态 token 用四态**（代码 agent 决定，已落文档）：契约第 3 节只举了 `[OK]/[FAIL]` 两个示例，但 doctor 既有 JSON 状态是 ok/warn/fail/skip 四态，砍掉 warn/skip 会丢信息 → md 行用 `- [OK]/[WARN]/[FAIL]/[SKIP] <检查名>: <说明>`；meta 头 `checks / ok / failed / elapsed` 只计 ok 与 fail，warn / skip 不进 meta 头（只在行里可见）。README「默认 md 形态」表与 doctor 章节均已按此写。

11. ✅ **`format` 键进 fetch / browse 的 md meta 头**（代码 agent 决定）：契约第 5.2 节明说「`--full` 与 md 源的差别在 meta 头体现」→ md meta 头 = `format: <源标签> | truncated(仅 true) | omitted(仅 >0) | elapsed`；`format` 取值：`fetch` = `markdown` / `raw`（`--raw`），`browse` = `markdown` / `full`（`--full`）/ `headings`（`--headings-only`）。README 已补该字段说明。

12. ✅ **fetch 单 URL vs 多 URL 的 md 形态**（代码 agent 决定）：**同一 meta 头单行**；单 URL 时 meta 头 + `# <页面标题>` + 正文（不额外分节），多 URL 时 meta 头之后每 URL 一节 `## <url>` + 该 URL 正文 + 失败行。README「fetch」章节已按此写。

13. ✅ **`--markdown` flag 在 md 路径的角色**（代码 agent 决定）：md 路径正文**默认就是 markdown**（复用既有 `html_to_markdown`），所以 `--markdown` 在 md 路径**等价于默认**（flag 保留只为兼容）；它在 `--json` 路径下仍按旧语义换 `text`/`content_text` 源 + `meta.format:"markdown"` 标注。README `--markdown` 章节已按此写。

14. ✅ **`--human` 只剩「显式要 md」一种行为，v0.3.0 的人读对比表 / 报告块形态不再保留**（代码 agent 决定）：契约第 5.3 节写死「`--human` 保留，语义 = 显式要 md（等价于默认），不再是『切换到另一种』」——既然 md 是默认形态，`--human` 就不可能还切到「另一种」形态。影响面：`verify`（v0.3.0 批量人读对比表 / 单 URL 报告块）、`doctor`（v0.3.0 `[ OK ]/[WARN]/[FAIL]/[SKIP]` 人读检查表）均改为 md 形态（`- [OK]/[WARN]/[FAIL]/[SKIP] <检查名>: <说明>`）。README 的 verify / doctor / browse 三段与 CHANGELOG 的 `--human` 条目 + 迁移指引表已按此改写，并显式写明「v0.3.0 的人读对比表 / 报告块形态不再保留」——**这是存量 `--human` 脚本唯一的可见变化**（search 的 `--human` 仍是 md 列表，只多一行 meta 头）。

## 发现的问题（文档与代码不符但我没动的点）

1. **`docs/PLAN.md` 未同步**（超出派单文件范围，未动）：PLAN.md 是仓库声明的「权威设计文档」，其中第 17 行仍写 `gsearch search "query" [--limit N] [--json] …`、第 130-131 行仍写「默认：`1. 标题\n url\n snippet前160字` / `--json`：serde_json 全量」——**默认与 `--json` 的主次已反**，与 v0.4.0 实现不符。派单只授权 3 个文件，故未改；建议 PM 或代码 agent 落地后一并同步（或明确 PLAN.md 冻结为历史设计）。
2. **`reports/b6-fix-receipt.md` 提到「单行 JSON 消费姿势」段**——那是历史修复回执，记录当时动作，不应改写；README 章节改名不影响其历史准确性。
3. **契约第 4 节与第 5.2 节的 fetch/browse meta 头字段口径不一致**：第 4 节列 `truncated`/`omitted`/`elapsed`，第 5.2 节要求体现 `format: full` / `format: markdown`。实现按第 5.2 节做了（带 `format`），README 也按实现写——**契约本身有歧义，已由实现裁决**。

## 验证清单

- 链接：README 内 2 个相对链接（`docs/PLAN.md`、`LICENSE`）——**均存在，已 `ls` 确认** ✅
- 示例：README 新增的 bash 示例共 9 条，均为**命令形态示例**（未逐条在本机执行；执行需要 SearXNG 实例 + 落地的 v0.4.0 二进制，由代码 agent 跑契约第 10 节验收命令，我按其贴回的默认 stdout 头几行做逐字校对——见「我替你拍板了什么」第 10-14 项，这些项已按代码 agent 的实现决定落文档）
- 过时表述：旧章节标题（全角括号「（默认）」版）在 README / CHANGELOG.md / docs/ 内 **0 命中** ✅（原标题已改）
- 其他过时表述清扫：`grep "默认输出紧凑|默认 JSON|JSON 已是默认|JSON 本就是默认|输出默认为"` 在 README 中 **0 命中** ✅
- CHANGELOG：`## [0.4.0]` 段落存在 ✅ + 迁移指引（代码块 + 5 行对照表）✅ + 数字（436/308/337 tok、47B≈12 tok、limit 10 时 996 vs 484、29%、5/5 盲测）✅
- 文档与实现一致性：md 形态的每个字段（doctor 四态 token、`format` 标签、verify `checked/ok/failed`、fetch 单/多 URL 分节方式、`--markdown` 在 md 路径的角色、`--human` 与 `--json` 同传优先级）**均已与代码 agent 对齐并落文档**，非我单方面推断
- **⏳ 最后一轮逐字校对待办**：代码 agent 跑完契约第 10 节验收命令后会把 search / fetch / browse / verify / doctor 的默认 stdout 头几行贴回来，我按实际输出逐字校对 README 的形态描述（meta 头字段名、token 形态、分节标题）。**本报告交付时 `src/` 仍只有 `output.rs` 一处改动（代码 agent 的第一刀），二进制尚未重建，因此文档形态是「按契约 + 与实现决定对齐」而非「按实测输出校对」**——若实际输出有出入，改的是文档不是代码。
- 代码文件：**我未改任何 `src/` 或 `Cargo.toml`**（`git status` 里 `src/output.rs` 的改动是代码 agent MdCode 的第一刀，非我的编辑）
- 未 commit / 未 push / 未打 tag ✅
