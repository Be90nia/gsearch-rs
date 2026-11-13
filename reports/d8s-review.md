VERDICT: CONDITIONAL

# bd gsearch-rs-d8s 独立代码审查报告（搜索结果相关性自检与诚实标注）

- **审查对象**：`D:/Project/gsearch-rs` 工作区未提交改动 —— `git diff`（11 文件，+392/−19）+ 4 个未跟踪新文件（`tests/relevance_off_topic.rs`、`tests/json_golden_compat.rs`、`tests/fixtures/on_topic_control_20261113.json`、`reports/off-topic-relevance-receipt.md`）
- **基准契约**：bd `gsearch-rs-d8s` 六条验收（相关性标签 / 新 run.status / bing 实锤 100% 判出 / 正常查询零误杀 / 零新依赖且体积 <100KB / JSON+人读双标注）+ README「输出契约（v0.4.0：默认 md）」+ `docs/research/search-competitors-2026-11.md` §6.5
- **审查方法**：只读 diff + 契约原文 → 静态推演判定内核 → **独立 Rust 探针**（把仓内 `gsearch` 当 path 依赖调用真实 `judge_relevance`/`apply_relevance`，不改仓内任何文件）→ 用 Python 复刻同一算法做交叉校验（复刻在两个 ground truth 上与 Rust 实现逐行一致：正样本 40/40 全 off_topic、对照组内部命中恰好 1 行 `('kubernetes pdb yaml', 3)`，与仓内测试断言完全吻合）
- **未重跑**：`cargo clippy` / `cargo test` / 真实 `search` e2e（PM 已独立跑过：0 warning、296 passed、真实查询 `relevance` 键 0 个、集成测试 40/40）——本次只补 PM 未覆盖的面

---

## 一、结论摘要

**做对的部分（独立复核通过）**

1. **缺席语义是结构性的，不是约定性的**：`relevance` 字段用 `#[serde(skip_serializing_if = "Option::is_none")]`（`src/types.rs:20-21`）挂在 `SearchResult` **末尾**，且 `apply_relevance` 只在整集可疑时写回标签（`src/search.rs:484-497`）→ 未触发判定时键整体缺席、既有键序不动。这一点是**逐字节兼容的真支点**，不是靠测试盯出来的运气。
2. **新状态与既有三态结构性互斥**：`degraded`/`healthy_empty` 都要求 `results.is_empty()`（`src/main.rs:680-681`），而 `suspicious` 要求 `off_topic ≥ 2`（`src/search.rs:328`）→ 物理上不可能同时成立，**不存在混用**。batch 路径同构（`src/main.rs:985-993`）。
3. **阈值护栏零网络**：`--relevance-threshold` 在任何触网前校验 `0.0..=1.0`，`NaN`/`inf` 一并被拒（`src/main.rs:534-542`）；范围检查用闭区间 `contains`，NaN 落入 false 分支。
4. **零新依赖成立**：`git status` 无 `Cargo.toml` 变更；判定内核只用 `std`（`String`/`Vec`/`char`），无分词库、无浮点比较护栏库（`ratio >= threshold` 是普通 `f64` 比较，阈值已被前置区间钳住，NaN 不可能进入）。
5. **契约越界文件全部是编译必需的机械修复**：`SearchResult` 只 `derive(Serialize)`（`src/types.rs:5`），新增字段必须补齐**全部**字面量构造点 → `src/searxng.rs`(3) / `src/parse.rs`(1) / `src/duckduckgo.rs`(1) / `src/postproc.rs`(2，均在 `mod tests` 内) / `src/shell.rs`(1，在 `mod tests` 内) 的 `relevance: None` 无一多余、无一改变既有语义；漏改会直接编译失败，**不存在漏改的构造点**。
6. **文档与实现逐字一致**（我逐字核对 README/CHANGELOG 与源码）：flag 名 `--relevance-threshold` ✓、默认 `0.5` ✓（`src/search.rs:331`）、状态名 `off_topic_results` ✓、字段名 `relevance` ✓、档位 `off_topic`/`weak`/`ok` ✓、md meta 键 `off_topic: N/M` ✓、可疑集退出码仍为 0 且已写明 ✓。
7. **`Relevance::as_str()` 与 serde 命名漂移有守卫**（`src/types.rs:412-417` 的 `relevance_label_text_matches_serde_name`）——这是好实践（md 与 JSON 不会各说各话）。

**核心缺口（按严重级见下）**

- **F1（最重要）**：混合 query（中文 + 拉丁词，如「如何安装 Docker 容器」）遇到**全英文结果集**时，会被判成 `off_topic_results` 并把每条结果标 `off_topic` —— 即**对正常查询作出「上游引擎返回了无关结果集」的误指控**。这是本特性最核心价值的镜像面伤害，且**对照组 61 个 case 完全没有覆盖这一形态**（22 个中文 case 的结果集全含 CJK）。
- **F2/F3**：逐字节兼容的**证据强度不足**——golden 是手写常量 + 假数据（`example.com`）、只测信封序列化层；真正的 CLI stdout 字节对比脚本随 Temp 目录删除，**不可复跑**。
- **F4**：`weak` / `ok` 两档**零测试覆盖**（md 的 `[weak]`/`[ok]` 标记与 JSON 的 `"weak"`/`"ok"` 值从未被断言）。
- **F6**：内部 oracle「恰好一行」锁的是**字形族产物**而非语义；派单点名的「yandex 噪声 = 真阳性」我**独立复核成立**（该行确与 query 毫无关系、`engines=["yandex"]` 属实），但同一 case 的第 1 行（同样是 yandex 噪声、同样毫无关系）却被判 `weak` 而非 `off_topic` —— 论证只覆盖了一半。

---

## 二、五查结果

| # | 检查项 | 结论 | 证据 |
|---|---|---|---|
| 1 | 正确性（断言强度 / 算法） | ✗ | 判定内核忠实实现契约要求的「词元交集」，但**可比性门用 OR**（`src/search.rs:436-437`）导致混合语言 query 的误指控路径（F1）；判定用**已被 160 字符截断**的 snippet（F5）；子串匹配非词边界（F11） |
| 2 | 边界 | ✗ | 短/标识符/版本号 query **静默不判定**（`≥3 字符` + `≥2 词元`）：`rust` / `Go 语言` / `rust 1.75` / `C++ 智能指针`（F10）；`--limit 1` 或单条结果集**永不触发**（`off_topic ≥ 2` 硬门）；部分污染永不标注（F8） |
| 3 | 错误处理 | ✓（无吞异常；但有信息损失路径） | 判定内核是纯函数、无 `Result`、无 catch；失败模式不是「吞错」而是「**门控后静默不输出**」——`off_topic=1/10` 时标签全部丢弃（F8）、query 词元不足时完全不判（F10）。这两处不是异常，但同样属「用户看不到发生了什么」 |
| 4 | 测试质量 | ✗ | golden = 手写常量 + `example.com` 假数据 + 只测序列化层（F2）；端到端字节对比不可复跑（F3）；`weak`/`ok` 零覆盖（F4）；内部 oracle 自证循环且锁字形族产物（F6）；对照组缺「混合 query + 全英文结果」形态（F1 的根因） |
| 5 | 接线 | ✓（主路径全接，旁路未接且未写进消费方文档） | `apply_relevance` 接在 `cmd_search`（`src/main.rs:708`）与 `cmd_search_batch`（`src/main.rs:985`）→ JSON（`results[].relevance`）与人读（md 行尾 `[off_topic]` + meta 头 `off_topic: N/M`）双路径均可见；`shell` 交互路径与 `similar` 命令**不参与标注**（receipt §6.2 已列，**README 未写** → F9） |

---

## 三、Findings（按严重级排序）

> 严重级：**建议** = 应修（合并可接受，发布前必修/须在文档中显式声明）；**次要** = 可不改，但应记账。

### 建议

| # | 严重级 | 位置 | 问题（一句话） | 期望行为 | 怀疑原因 | 验证命令 |
|---|---|---|---|---|---|---|
| F1 | 建议 | `src/search.rs:436-437`（可比性门 OR）+ `src/search.rs:441-443`（`matched==0` → OffTopic）；接线 `src/main.rs:708` | 混合 query（中文+拉丁词）对**全英文结果集**会被判 `off_topic_results` 且逐条标 `off_topic` —— 对正常查询误指控「上游引擎返回了无关结果集」 | 中文+拉丁混合 query 的英文结果集不应被指控；至少不得在无同族文本证据时指控 | 可比性门按「**任一族**存在」放行（OR）；CJK 词元在英文文本里**永不可能命中** → 「零交集」退化为「拉丁词元也没命中」→ 误指控。纯中文 query 因 `terms.latin` 为空而被门挡住（探针 S3 ✓），混合 query 则从门缝漏过（探针 S1/S2/S4 ✗） | 附录 A 探针：`cargo run` 输出 `[S1] suspicious=true off_topic=10/10` / `[S3] suspicious=false`（Rust 真实实现，非复刻）；对照组 61 case 中 22 个中文 case 的结果集**全部含 CJK** → 该形态零覆盖 |
| F2 | 建议 | `tests/json_golden_compat.rs:12-17`（GOLDEN_JSON 常量）、`:19-42`（`golden_envelope` 手工装配）、`:67-77`（断言） | golden 只锁「**serde 序列化结果**」，不锁「**CLI 实际 stdout 字节**」；且数据是 `example.com/example.org` 占位 URL，与「改动前二进制实跑输出」的宣称不可核验 | 锁住「正常态 `gsearch search --json` 的 stdout 逐字节不变（含键序、含 `println!` 形态）」 | 断言对象是手工拼装的 `OutputEnvelope`；`main.rs` 的装配（meta 取值、`print_envelope_json` 的输出形态、stdout 上有无多余行）**完全不在断言范围**；文档字符串与 CHANGELOG 均宣称 golden 来自改动前二进制实跑，但基准二进制随 Temp 删除、URL 为占位域名 | `cargo test --test json_golden_compat`（只证明序列化层）；`git log -1 --format=%H` 后 `git show HEAD:src/types.rs` 对比字段集可知改动前无 `relevance` |
| F3 | 建议 | `reports/off-topic-relevance-receipt.md` §3.5 / §5.1 | 端到端「旧/新二进制逐字节对比」的**脚本与基准二进制随 Temp 目录删除 → 证据链断点，不可复跑** | 逐字节兼容结论必须由仓内可复跑的回归承载 | 基准是「**未提交的工作区**」（= HEAD 之外的改动前源码树）→ 一旦本次改动 commit，基准即永久不可重建；一次性脚本未入库 | `ls C:/Users/Begonia/AppData/Local/Temp/d8s` → 不存在；receipt §5.1 自述「基准可重建：git archive HEAD」——**只在 commit 前成立** |
| F4 | 建议 | `tests/relevance_off_topic.rs` 全文（只断言 `off_topic`）；`src/output.rs:64-77`（md 标记分支） | `weak` / `ok` 两档**零断言**：md 行尾 `[weak]`/`[ok]` 与 JSON 的 `"weak"`/`"ok"` 从未被测 | 三档各自有断言（分布 + 渲染 + 序列化） | 正样本 40 条 100% `off_topic`；对照组只断言「**不是** off_topic」，从不断言「是 weak/ok」→ 两个档位即使永远返回 `weak` 也会全绿 | `grep -n "weak\|\[ok\]" tests/*.rs` → 仅命中注释与 fixture 内容；`grep -n "Relevance::Weak\|Relevance::Ok" tests/` → 0 命中 |
| F5 | 建议 | `src/main.rs:675-677`（先截断 snippet）→ `src/main.rs:708`（后判定）；batch 同序 `src/main.rs:981-986` | 判定用的是**已被 160 字符截断**的 snippet → 查询词只出现在 snippet 尾部的结果会被误判 `off_topic` | 判定基于完整 `title + snippet`（契约原文写的是 title+snippet，未提截断） | snippet 截断在判定之前（代码顺序即证据）；SearXNG 的 `content` 常超过 160 字符且查询词未必在头部 | 代码顺序核对（`src/main.rs:675` vs `:708`）即可，无需运行 |
| F6 | 建议 | `tests/relevance_off_topic.rs:24-30`（`KNOWN_UNRELATED_CONTROL_ROWS`）+ `:109-116`（`flagged == known` 断言）+ `:118-130`（must-flag 断言） | 内部 oracle 锁的是**字形族产物**而非语义：「恰好只有第 3 行」是「标题里有没有 ASCII 字母」的实现细节 | oracle 表达语义属性（被判 `off_topic` 的行必须：与 query **零词元交集** ∧ 文本与 query **同族**） | 同一 case 的第 1 行（yandex 返回的俄语 СберБанк 首页，**同样与 kubernetes/pdb/yaml 毫无关系**）因标题/摘要无 ASCII 字母 → 落 `weak` 分支 → 不算 `off_topic`。探针 S12 直接给出 `[OffTopic, Weak]`：两条同样无关的行因字形不同而分档。把可比性门从「ASCII 字母」放宽为「任意字母」（语义不变、覆盖面更广）→ 该断言立即变红 | 探针 `[S12 internal labels] [Some(OffTopic), Some(Weak)]`；fixture 原文：`tests/fixtures/on_topic_control_20261113.json` case `kubernetes pdb yaml` row 1（`engines:["yandex"]`，俄语银行首页） |

### 次要

| # | 严重级 | 位置 | 问题 | 期望行为 | 怀疑原因 | 验证命令 |
|---|---|---|---|---|---|---|
| F7 | 次要 | `src/main.rs:774-779`（md 头有计数）vs `src/main.rs:749-757`（JSON meta 无计数）；需求来源 `docs/research/search-competitors-2026-11.md:252`（「**meta 带计数**」） | `--json` 路径**没有**计数键（只有 md meta 头有 `off_topic: N/M`）→ JSON 消费方须自行统计 `results[].relevance=="off_topic"` | JSON meta 侧也带一个缺席语义的计数字段（可疑时才出现） | 需求来源文档明确要求「meta 带计数」，实现只在 md 侧兑现；bd 验收只要求「JSON 与人读模式均如实标注」（逐条标签已满足）→ 属**与来源文档的偏离**，不违反 bd 验收 | `grep -n "off_topic" src/main.rs` → 只有 md 分支；`grep -n "meta 带计数" docs/research/search-competitors-2026-11.md` |
| F8 | 次要 | `src/search.rs:471-477`（`suspicious` 门）+ `src/search.rs:484-497`（只在可疑时写回） | **部分污染永不标注**：`off_topic=1/10` 或 `4/10`（占比 <0.5）时全部标签被丢弃、status 仍 `ok` —— 「静默污染」只是从「全量」缩到「多数」，残留部分照样静默 | 要么在 ≥1 条 off_topic 时也下发逐条标签（需重新评估逐字节兼容），要么在 README **显式声明**「只有整集 ≥50% 明显无关时才标注，20% 噪声不报」 | 探针 S11（1/2 off_topic）→ `suspicious=false` 且无标签；README 只写了整集门规则，没写「部分污染不报」这一后果 | 探针 `[S11 single off_topic row] suspicious=false off_topic=1/2 labels=["-","-"]` |
| F9 | 次要 | `src/shell.rs`（未改）、`src/main.rs:901-905`（`similar` 路径未接线）；README 无相关说明 | `shell` 交互路径与 `similar` 命令**不参与标注**，README（面向消费方的契约文档）**未写** | 消费方文档应写明覆盖边界（哪些入口会带 `relevance`、哪些不会） | receipt §6.2 已列为遗留风险，但 README/CHANGELOG 无对应「已知边界」条目（CHANGELOG 只写了「纯中文 query 对全英文结果集不判 off_topic」这一条边界） | `grep -n "relevance" README.md` → 无 shell/similar 说明；`grep -rn "apply_relevance" src/` → 仅 `main.rs` 两处 |
| F10 | 次要 | `src/search.rs:403-424`（`≥3 字符` 规则）、`src/search.rs:430-432`（`MIN_RELEVANCE_TERMS=2`） | 短词 / 版本号 / 标识符 query **静默永不判定**：`rust`（1 词元）、`Go 语言`（`go` 仅 2 字符被丢 → 1 词元）、`rust 1.75`（`1`/`75` 被丢 → 1 词元）、`C++ 智能指针`（`c` 被丢） | 至少让这类 query 的「不判定」可被观察（当前完全静默：无标签、无状态、无提示） | `≥3 字符` 过滤 + `≥2 词元` 下限叠加后，`Go 语言` 这类常见形态永不进入判定 | 探针 `[S6b] query="Go 语言" labels=["-"]`、`[S6c] query="rust 1.75" labels=["-"]` |
| F11 | 次要 | `src/search.rs:441`（`hay.contains(t)` 子串匹配） | 子串匹配非词边界 → 假命中：`rust` 命中 `Distrust`（探针 S7 该行判 `weak` 而非 `off_topic`）；同理 `cat` 会命中 `education` | 词边界匹配（拉丁词按非字母数字切分后整词比对） | 契约原文只说「求交集」，子串是宽松解读；影响方向是**漏判**（少指控），危害小于误指控 | 探针 `[S7 substring false match] off_topic=1/2 labels=["weak","off_topic"]`（`Distrust in certificates` 因含 "rust" 子串被判 weak） |
| F12 | 次要 | `tests/fixtures/on_topic_control_20261113.json` 的 `_comment` 字段 | fixture 元数据与实际不符：`_comment` 写「**50 条**真实查询（**中日各半**）」，实际 **61 case**（CJK 22 / 纯拉丁 39）；receipt 与 CHANGELOG 写的是 61 | 文档数字与数据一致 | 抓取脚本的元数据按初版 50 条写，后续补到 61 未回改注释 —— 与本仓「文档写语义不写数量 / 数量漂移必被察觉」的历史教训同型 | `python -c "import json;d=json.load(open('tests/fixtures/on_topic_control_20261113.json',encoding='utf-8'));print(len(d['cases']), d['_comment'][:40])"` → `61` vs 「50 条真实查询」 |
| F13 | 次要 | `src/main.rs:715-724`（分支顺序：`suspicious` 在 `captcha_solved` 之前） | 可疑集与「本次经过人工 CAPTCHA」同时成立时，CAPTCHA 的人读提示被静默丢弃（`run.captcha_solved` 布尔仍在，但 message 没了） | 两个提示可同时出现，或至少 message 里都保留 | 分支互斥写法（`else if` 链）天然吞掉次要信息 | 代码顺序核对 `src/main.rs:715-724` |
| F14 | 次要 | `tests/json_golden_compat.rs:3-10`（docstring）、`CHANGELOG.md:12`（「golden = 改动前 release 二进制实跑输出」） | golden 的**出处不可核验**：文档宣称来自改动前二进制实跑，但数据是 `example.com/example.org` 占位 URL，基准二进制已删（Temp 清零） | 把原始捕获输出连同**来源（commit sha + 复现命令）**落盘成仓内 fixture，使宣称可审计 | 无法区分「实跑捕获」与「按输出格式手写」——两种情况下断言强度相同，但可信度差一个量级 | `grep -n "改动前的 release 二进制" tests/json_golden_compat.rs`；`ls C:/Users/Begonia/AppData/Local/Temp/d8s` → 不存在 |

---

## 四、逐条回应派单的 7 类审查点

### 4.1 判定算法的正确性与误杀风险（最高优先）

- **中文分词**：CJK 段按**不重叠 2-gram** 取子串（`src/search.rs:403-418`），停用二元组表剔除「如何/怎么/使用」等 → 「如何用Python读取CSV」的 CJK 词元只剩 `读取`（探针词元表实测：`['python','csv'] + ['读取']`）。**语义相关但字面不重叠**的中文结果，只要**含任意 CJK 字符**就走「可比」分支，不会被判 off_topic；只有「中文 query + 全外语结果」才落到 `weak`（探针 S3）。这是本实现最稳的一块。
- **拉丁路径漏短词**：`≥3 字符` 会丢掉 `C`/`R`/`Go`/`JS`/版本号/`--flag` 名（探针 S6/S6b/S6c）→ 这些 query **静默不判定**（F10），方向是漏判不是误杀；但 `Go 语言` 这种「短拉丁词 + 中文」的常见形态因此整类不参与判定。
- **可比性门**：机制 = 「结果文本必须含与 query **同族**的文字（拉丁字母 / CJK）」，否则判 `weak`（「判不了 ≠ 判无关」）。**但它是 OR 门**（`src/search.rs:436-437`），query 同时含两族时，任一族存在即放行 → 混合 query 的英文结果集被放行后必然「零交集」→ **误指控**（F1）。纯中文 query、纯拉丁 query 都被正确保护（探针 S3/S5）。
- **阈值 0.5 边界**：0/1 词元的 query 根本不进入判定（`MIN_RELEVANCE_TERMS=2`）；2 词元 query 的整集占比按 `off_topic/total` 计，2/3 = 0.67 ≥ 0.5 → 可疑（探针 S10）；阈值 1.0 时 2/3 → 不可疑（S10b）；`off_topic=1` 时因 `MIN_OFF_TOPIC_RESULTS=2` 永不可疑（S11）。
- **`weak` 档是否有实质区别**：**有**，但只有两档被测：`weak` = 有交集但未全覆盖 ∨ 文本不含 query 同族文字；`ok` = 词元全覆盖（探针 S10 的第三条结果得 `ok`）。二者都**不出现在输出里**除非整集可疑；且 `weak`/`ok` 零测试覆盖（F4）。

### 4.2 缺席语义与向后兼容

- `relevance` 键**整体缺席**（非 null）✓：`#[serde(skip_serializing_if = "Option::is_none")]` + 只在可疑时写回。
- `meta` 正常态不出现 off_topic 相关键 ✓（`src/main.rs:774` 的 `if relevance.suspicious` 守卫；JSON meta 根本无此字段，F7）。
- 键序：字段**追加在 `SearchResult` 末尾**（`src/types.rs:21`）→ 既有键序不动 ✓。
- `RunStatus` 新值序列化 = `off_topic_results` ✓（`src/types.rs:362-363` 契约测试已同步）。
- **但「逐字节一致」的证据强度不足**：golden 只测序列化层 + 假数据（F2），真 CLI 字节对比不可复跑（F3），md 路径（README 明确宣称「md 输出逐字节不变」）**零回归锁**。

**最小补强方案（可复跑、可审计）**

1. **字节级 CLI 回归（首选，兼替 F3）**：复用 `tests/fallback_chain.rs` 已有的回环假 SearXNG（`TcpListener` + `results_json()` 载荷），在 `tests/` 新增 `cli_golden_bytes.rs`：
   - 用 `env!("CARGO_BIN_EXE_gsearch")` spawn 真实二进制，`GSEARCH_SEARXNG_URL` 指向假服务器（`src/config.rs:117-118` 证实 env 覆盖优先于配置文件）；
   - 载荷固定为 `tests/fixtures/` 里的现成 fixture（`off_topic_searxng_20261113.json` 第一个 case），query 固定；
   - stdout 归一 `meta.elapsed_ms` 后与**仓内落盘的期望文件**逐字节比对（`--json` 一份、默认 md 一份）；
   - 期望文件由本次审查时的**改动前**行为产出（现在 `git stash` 式重建：`git archive HEAD | tar -x -C <tmp> && cargo build --release`，或直接在本次合并前用旧二进制产出）→ commit 之后仍然永久可复跑。
2. **零进程成本的次选**：把 `cmd_search` 的信封装配抽成一个纯函数（`search::build_envelope(query, results, ...)`），让 golden 测试走**真实装配路径**而非手工拼 `OutputEnvelope`；同时加一条 `render_md` 的正常态 golden（锁 md 逐字节不变）。
3. **golden 出处**：把原始捕获 stdout 落盘为 `tests/fixtures/golden_search_json_pre_d8s.txt`，文件头注明来源 commit sha + 复现命令（消 F14）。

### 4.3 `run.status` 新态语义

- **不混用 ✓**：见 §一.2（`results.is_empty()` 与 `off_topic ≥ 2` 互斥）。
- **触发条件**：整集占比 ≥ 阈值（默认 0.5）**且** `off_topic ≥ 2` —— 「部分可疑」不触发（F8），与契约「整集判定为可疑时」一致。
- **message 可操作性**：`"10/10 条结果与 query 零词元交集（疑似上游引擎返回了无关结果集，非查询无资料）；建议换引擎或换短 query 复核"` —— 「疑似」措辞恰当 ✓；但**「换短 query 复核」这条建议方向可疑**：query 变短 → 词元更少 → 更易落到「<2 词元不判定」或「更易凑齐词元命中」，两种都会让指控消失而非让问题暴露（探针 S6/S6b）。建议改成「换引擎 / 用英文 query 复核」。
- 退出码仍 0 且已写明 ✓（README + CHANGELOG）。

### 4.4 测试强度

- **`tests/json_golden_compat.rs` 能否挡住「往正常态 meta 加键/改键序」？** → **部分能**：
  - **加字段**：能挡住（Rust 结构体字面量缺字段 → 编译失败 → 测试二进制编译不过 → CI 红）。这是它最强的一点。
  - **改键序**：能挡住（`serde_json` 按声明序输出，golden 逐字节比对 → 红）。
  - **但挡不住**：`main.rs` 装配层的任何改动（meta 取值来源、输出形态、`--json` 模式下多打/少打一行、`print_envelope_json` 改 pretty-print），以及 **md 路径的任何字节变化**。也就是说它锁的是**类型层**，不是**CLI 输出层**。最小补强见 §4.2。
- **一次性 e2e 不可复跑 → 是否证据链断点？** → **是**：它是「逐字节兼容」这一验收项（A4）的**唯一端到端证据**，而脚本与基准二进制均已随 Temp 删除；receipt §5.1 给的「可重建」路径（`git archive HEAD`）**只在本次改动 commit 之前成立**。补强方案同 §4.2 第 1 条。
- **`tests/relevance_off_topic.rs` 是否自证循环？** → **部分是**：
  - 有独立 oracle 的部分：正样本用**真实抓取**的 fixture（`off_topic_searxng_20261113.json`，已 commit）+ 对照组 61 case 真实抓取；「内部判定命中集合必须与人工核实清单完全一致」是多出即红的强断言（`:109-116`）✓；
  - 自证的部分：oracle 清单 `KNOWN_UNRELATED_CONTROL_ROWS` 是**实现者自己人工核实的 1 行**，且该行之所以被选中恰恰因为当前算法会判它 off_topic（否则断言红）——**存在「先看实现输出再写 oracle」的循环风险**（F6 的另一半）。对照组缺「混合 query + 全英文结果」形态（F1）使这类误杀无从暴露。
  - 独立 oracle 的补强：用**与 query 明显同族、字面零交集**的人工构造噪声行（例如中文 query + 中文无关页）作为 must-flag，并加一条**人工可判定的反例**（中文 query + 相关中文页 → 必须**不是** off_topic）。
- **「yandex 噪声 = 真阳性」独立核验** → **成立，但论证不完整**：
  - 成立：该行 `Home | Lsto.me`（`https://lsto.me/`）+ 摘要讲加密货币交易所采用率，与 `kubernetes / pdb / yaml` **零词元交集、零语义关联**；fixture 该行 `engines:["yandex"]` 属实（我逐字段核对过）。
  - 不完整：同一 case 的**另一条 yandex 噪声**（row 1，yandex 返回的俄语 СберБанк 首页）**同样与 query 毫无关系**，却因文本无 ASCII 字母被判 `weak`（探针 S12：`[OffTopic, Weak]`）→ 「恰好一行」是字形族产物而非相关性产物（F6）。
  - 且该行**从未出现在任何输出里**（整集 1/10 未达门 → 不下发标签、status 仍 `ok`）——receipt §4 已如实说明这一点，态度可取；但据此把内部命中定义为「零误杀」是**自定义口径**（F8/§4.4），严格口径下对照组内部判定有 1 例误杀（真阳性，但非零误杀口径）。

### 4.5 契约越界与连带影响

- `src/duckduckgo.rs`(+1) / `src/parse.rs`(+1) / `src/postproc.rs`(+2，均在测试内) / `src/searxng.rs`(+3) / `src/shell.rs`(+1，测试内) —— **全部是「新增 `SearchResult` 字段 → 补齐字面量构造点」的编译必需修复**，逐行核对无任何语义改动 ✓（`SearchResult` 只 `derive(Serialize)`，故不存在「漏改但能编译」的构造点）。
- `src/types.rs`(+46)：新字段 + `Relevance` 枚举 + `as_str()` + `RunStatus::OffTopicResults` + 既有契约测试同步 ✓。
- `src/output.rs`(+17)：md 行尾标记 + `relevance_meta_fields` —— **均在 `relevance == None` 时产出空串/空 vec**，正常态 md 输出逐字节不变 ✓。
- `src/main.rs`(+85)：flag + 阈值护栏 + 单查询接线 + batch 接线 + v2 信封 `n_ok` 口径 + 非 JSON batch 分支 —— **注意 batch 的 `n_ok` 把 `OffTopicResults` 计为成功**（`src/main.rs:1030-1032`、`:1090-1092`），与 README「退出码语义不变」一致 ✓。

### 4.6 文档一致性

- **逐字一致 ✓**：flag 名 / 默认阈值 / 状态名 / 字段名 / 档位名 / meta 键名 / 退出码语义，全部与 `src/types.rs`、`src/search.rs`、`src/main.rs` 对得上（我逐条比对）。
- **遗漏的已知边界**：
  1. `shell` / `similar` 路径不参与标注（F9）——只在 receipt，README 未写；
  2. **部分污染不标注**（F8）——README 只写整集门规则，未写「20% 噪声不报」这一后果；
  3. **混合 query + 全外语结果集的误指控风险**（F1）——README/CHANGELOG 现表述为「含拉丁词的查询**不受此限**」，读起来像**优点**（混合 query 照样能判），实际是**误杀面扩大**，措辞需改；
  4. JSON 路径无计数（F7，与需求来源 §6.5「meta 带计数」有出入）。
- `USER_GUIDE.md` 未更新 —— 但该文件整体停留在 v0.2.9（默认输出即 JSON 的旧契约），recency/batch/envelope v2 等近年特性同样未收录 → **属既有文档债，非本次回归**（建议单独开条目）。

### 4.7 零新依赖

- `git status --short` 无 `Cargo.toml` ✓；判定内核只用 `std`（`String`/`Vec<char>`/`char::is_ascii_alphabetic`/`is_cjk`），**无 `ordered_float` 之类浮点护栏库** ✓；`f64` 仅一处比较（`ratio >= threshold`），阈值已被前置区间钳住，`NaN` 不可能进入比较 ✓。
- 体积：receipt 报 +14,336 B（10,101,248 → 10,115,584 B）。基准二进制已删 → **本次未能独立复核**；receipt 给了可复跑配方（`git archive HEAD` → rebuild），但如 F3 所述，**该配方在 commit 后失效** → 建议把体积对比也做成 CI 断言（`git diff HEAD~1` 前后 release 体积差 < 100KB）。

---

## 五、未覆盖范围（本次未审到 / 未复跑）

1. **未复跑 PM 已覆盖的项**：`cargo clippy --all-targets -- -D warnings`、`cargo test --all-targets`、真实 `search "rust async" --json`、集成测试 40/40 —— 按分工不重复实现方自证路径。
2. **未复跑端到端字节对比**：旧二进制已删，无法复现（见 F3）。
3. **未做真实 SearXNG 混合语言回归**：本次为避免污染实例出口 IP 信誉，未对真实实例发「中文 query + 英文结果」类查询；F1 的证据来自**真实实现**在受控输入上的判定结果（探针），未叠加真实抓取分布。F1 的**发生概率**取决于真实实例对中文 query 返回外语结果的比例（本次对照组 22/22 中文 case 均返回含 CJK 的结果 → 该比例在本实例当前为 0，但 bing 缺陷期曾出现 100% 外语结果）。
4. **未审**：`src/postproc.rs` / `src/shell.rs` / `src/duckduckgo.rs` 等文件的**既有**逻辑（本次 diff 仅补字段），以及与本特性无关的既有代码风格问题。
5. **未复核**：`docs/PLAN.md` 是否应同步（receipt §2 已说明理由，同意其判断——该文件只记 M1–M7 移植里程碑）。
6. **未验证**：`--relevance-threshold` 在 **batch** 模式下的越界护栏路径（`cmd_search_batch` 由 `cmd_search` 前置校验后转入，护栏在分支之前 ✓，但未实跑）。

---

## 六、side-effects

- **side-effects: none**（对被审查工作区）。
  - 未修改任何仓内既有文件；未 `git add` / `commit` / `checkout` / `stash`。
  - 唯一新增文件 = 本报告 `reports/d8s-review.md`（派单指定路径）。
  - 未在 `D:/Project/gsearch-rs/target` 内构建：探针以 `CARGO_TARGET_DIR=/d/...` 编译，被 Windows 解析到 **`C:\d\Project\gsearch-rs\target`**（仓外），仓内 `target/` 未被本次审查触碰。
  - 探针工程位于 `C:/Users/Begonia/AppData/Local/Temp/d8s-review-probe/`（仓外），源码已全文附于附录 A，可复跑。
  - 未向 SearXNG 实例发起任何请求（`192.168.89.249:8888` 未被触达，未单独请求 `engines=bing`）。
  - 未运行 `cargo test` / `cargo clippy`（PM 已覆盖，避免无谓构建与 CPU 争用）。

---

## 附录 A：独立判定探针（可复跑）

**用途**：在不触碰仓内任何文件的前提下，调用**真实实现**（`gsearch::search::judge_relevance` / `apply_relevance`）验证判定内核的误杀/漏杀面。
**工程**：`C:/Users/Begonia/AppData/Local/Temp/d8s-review-probe/`（仓外）

```
Cargo.toml:
  [package] name = "d8s-review-probe"   edition = "2021"
  [dependencies]
  gsearch = { path = "D:/Project/gsearch-rs" }
```

```rust
// src/main.rs（要点；完整文件同上）
use gsearch::search::{apply_relevance, judge_relevance, DEFAULT_RELEVANCE_THRESHOLD};
use gsearch::types::{Relevance, SearchResult};

fn mk(title: &str, snippet: &str) -> SearchResult { /* title/url/snippet/score/domain_class/relevance */ }

fn show(name: &str, query: &str, results: Vec<SearchResult>) {
    let mut rs = results;
    let out = apply_relevance(query, &mut rs, DEFAULT_RELEVANCE_THRESHOLD);
    // 打印 suspicious / off_topic / total / 每条的标签（None → "-"）
}
// S1 中文+拉丁 query + 全英文结果集；S2 同上且无字面 token；S3 纯中文 query + 全英文结果集；
// S4 混合 query + 全英文结果集；S5 拉丁 query + 全俄语结果集；S6/S6b/S6c/S9 短/版本/标识符 query；
// S7 子串假命中；S8 site: 限定；S10 阈值边界（2/3 @0.5 与 @1.0）；S11 单条 off_topic；
// S12 crypto 行 vs 俄语银行行的内部标签对比
```

**复跑命令**（Windows / Git Bash）：

```bash
cd /c/Users/Begonia/AppData/Local/Temp/d8s-review-probe
CARGO_TARGET_DIR=/d/Project/gsearch-rs/target cargo run -q   # 注意：Windows 会把 /d/... 解析到 C:\d\...（仓外）
```

**实际输出（Rust 真实实现，非复刻）**：

```
[S1 zh+latin query / all-English results] query="如何安装 Docker 容器" suspicious=true  off_topic=10/10 labels=["off_topic"×10]
[S2 zh+latin query / English results w/o literal token] query="如何用Python读取CSV" suspicious=true off_topic=10/10 labels=["off_topic"×10]
[S3 pure-zh query / all-English results] query="北京 天气" suspicious=false off_topic=0/10 labels=["-"×10]
[S4 mixed query / all-English results] query="北京 天气 API" suspicious=true off_topic=10/10 labels=["off_topic"×10]
[S5 latin query / all-Cyrillic results] query="kubernetes pdb yaml" suspicious=false off_topic=0/10 labels=["-"×10]
[S6 short query] query="rust" suspicious=false off_topic=0/1 labels=["-"]
[S6b short query (Go + zh)] query="Go 语言" suspicious=false off_topic=0/1 labels=["-"]
[S6c version-ish query] query="rust 1.75" suspicious=false off_topic=0/1 labels=["-"]
[S7 substring false match] query="rust security" suspicious=false off_topic=1/2 labels=["-","-"]
[S8 site: limited query] query="site:docs.rs tokio" suspicious=false off_topic=1/2 labels=["-","-"]
[S9 one-term query vs unrelated set] query="docker" suspicious=false off_topic=0/1 labels=["-"]
[S10 2/3 off_topic @ default 0.5] query="kubernetes pdb yaml" suspicious=true off_topic=2/3 labels=["off_topic","off_topic","ok"]
[S10b threshold=1.0] suspicious=false off_topic=2
[S11 single off_topic row] query="kubernetes pdb yaml" suspicious=false off_topic=1/2 labels=["-","-"]
[S12 internal labels] [Some(OffTopic), Some(Weak)]
```

**读法**：
- `labels=["-"]` = `apply_relevance` 未写回标签（整集不可疑 → 缺席语义生效，正常态输出逐字节不变）。
- **S1/S2/S4 = F1 的直接证据**（误指控）；**S3/S5 = 可比性门在纯语言 query 上的正确保护**；**S11 = F8**（单条噪声被静默丢弃）；**S6/S6b/S6c/S9 = F10**（短 query 永不判定）；**S7 = F11**（子串假命中）；**S12 = F6**（两条同样无关的行因字形族不同而分档）。

**交叉校验**：同一算法用 Python 复刻后，在两个 ground truth 上与 Rust 实现**逐行一致**——正样本 40/40 全 `off_topic`、对照组内部命中集合恰好 `[('kubernetes pdb yaml', 3)]`（与仓内测试 `on_topic_control_zero_off_topic_false_positives` 的断言完全吻合）→ 复刻对新输入的预测可信，两条证据链互证。
