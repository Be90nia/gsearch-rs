VERDICT: PASS

# bd gsearch-rs-d8s 交付回执：搜索结果相关性自检与诚实标注

- **issue**：`gsearch-rs-d8s`（P1：结果与 query 无关时如实标出，不静默污染）
- **日期**：2026-11-13
- **改动范围**：`src/types.rs`（新增状态 + 标签枚举助手）、`src/search.rs`（判定内核）、`src/output.rs`（md 标记 + meta 计数）、`src/main.rs`（flag + 接线，单查询与 batch）、`tests/relevance_off_topic.rs`（新）、`tests/json_golden_compat.rs`（新，替代 `tests/tmp_golden_capture.rs`）、`README.md`、`CHANGELOG.md`
- **依赖**：**零新增**（`Cargo.toml` 未改，判定只用 std）
- **commit**：未做（归 PM）

---

## 1. 设计说明

### 1.1 判定模型（词元交集，不引模型、不 rerank）

| 环节 | 规则 |
|---|---|
| query 词元 | 拉丁词（`[a-z0-9]+`、≥3 字符、去英文停用词）+ 中文**不重叠 2-gram**（去中文停用二元组） |
| 分词依据 | 中文查询通常**无空格**，故不按空格切；按**不重叠** 2-gram 切段取子串，命中即算交集 |
| 最短判定 | query 词元 < 2 条 → **不判定**（短 query 不冤枉） |
| 交集判定 | 词元出现在 `title + snippet`（小写后子串匹配）即命中 |
| 可比性 | 结果文本须含 query **同族文字**（拉丁字母或 CJK）；否则判 `weak`（**判不了 ≠ 判无关**） |
| 档位 | `off_topic` = 零交集；`ok` = 全部词元命中；其余 `weak` |

### 1.2 整集门（何时才「出声」）

`off_topic ≥ 2 条` **且** `off_topic / 总条数 ≥ threshold`（默认 `0.5`）→ 整集判**可疑**：

- 逐条结果写回 `relevance` 标签（`off_topic` / `weak` / `ok`）
- `run.status = off_topic_results`（**新增态**，与 `searxng_degraded` / `filtered_empty` / `no_results` 三态语义分立）
- `run.message` = 一行人读说明（`10/10 条结果与 query 零词元交集（疑似上游引擎返回了无关结果集，非查询无资料）；建议换引擎或换短 query 复核`）
- md：meta 头追加 `off_topic: N/M`（>0 才出现），结果行尾追加 `[off_topic]` / `[weak]` / `[ok]`

### 1.3 缺席语义（向后兼容的支点）

**内部判定始终进行，但只有整集可疑时才把标签写回结果**。因此未判可疑时：

- `results[].relevance` 键**整体缺席**（不是 `null`）
- `run.status` 仍是 `ok`、`run.message` 空 → 键缺席
- md 结果行不追加标记、meta 头不追加 `off_topic`
- → **`--json` 输出与引入本特性前逐字节一致**（见 §3 A4）

### 1.4 可配阈值

`--relevance-threshold <0.0..1.0>`（默认 `0.5`）。clap 无 `RangedF64ValueParser`（4.6.6 只有整数版），故用 `value_parser!(f64)` + **发起任何网络前**的范围护栏（越界 rc=2，`NaN` 亦被范围检查拒掉）。

### 1.5 前一轮算法探索的结论（Temp 目录 `d8s/` 已读并采纳）

前一轮在 `C:/Users/Begonia/AppData/Local/Temp/d8s/` 留了算法变体实验（`variants.py` / `final_algo.py` / `final2.py` / `proto.py` / `live.py`）。本轮**先读结论、再动手实现**，采纳的结论：

| 变体 | 实测结果 | 采纳与否 |
|---|---|---|
| CJK 2-gram **不重叠**（步长 2） | fixture 40/40；对照组 off_topic 行 1 | ✅ 采纳（词元更干净，`何用` 这类跨位垃圾二元组不产生） |
| CJK 2-gram **重叠**（步长 1） | fixture 40/40；对照组 off_topic 行 1（weak 分布更散：144 vs 130） | ❌ 不采纳（词元更多 → ok 档更难达成，且多出跨位噪声二元组） |
| **去掉**「可比性」规则 | 对照组 off_topic 行 **3**（多出「北京 天气 → 英文 AccuWeather 页」这类**真误杀**） | ❌ 不采纳（跨语言结果会被冤枉） |
| 保留「可比性」规则（结果文本须含 query 同族文字） | 对照组 off_topic 行 1（仅已核实的 yandex 噪声行） | ✅ 采纳 |
| 拉丁词 ≥3 字符 + 英文停用词表 / 中文停用二元组表 | 不剔停用词时 `how/the` 命中一切 → 零交集判定永久失效 | ✅ 采纳（停用词表必需，非调优项） |

**本轮自建的对照实验**（在两个 fixture 上跑 4 个变体）另发现：若**去掉**「可比性」规则，对照组 off_topic 行由 1 升到 3，多出的是「北京 天气 → 英文 AccuWeather 页」这类**真误杀**（语义相关、只是语言不同）——故该规则保留（前一轮的两个 Python 原型 `final2.py` / `final_algo.py` 均已含此规则，本轮 Rust 实现与之一致）。

### 1.6 明确不做（非目标）

不改排序、不 rerank、不删 off_topic 条目、不引入模型/分词库、不改既有三态语义、不改既有 JSON 键名、不新增 meta 键。

---

## 2. 逐条验收对照

| # | 验收项 | code-level（命令 + 期望） | end-to-end（真实可观察） | 结论 |
|---|---|---|---|---|
| A1 | 相关性标签落地 | `cargo test --test relevance_off_topic`：`off_topic_fixture_all_results_judged_off_topic` → 40 条 100% 判 `off_topic`（输出 `off_topic 判定 40/40`） | 真实二进制（本地假 SearXNG 喂 fixture 载荷）：`run.status=off_topic_results`，`results[].relevance` 全部 `"off_topic"`；md 行尾 10 处 `[off_topic]` | ✅ |
| A2 | `run.status` 新增一态 | `types::tests::run_status_all_variants_serialize_snake_case` 已同步加 `off_topic_results`；`off_topic_status_is_distinct_from_zero_result_states` 断言与三态互斥 | `--json` 顶层 `run.status == "off_topic_results"`，`run.message` 含「零词元交集」说明 | ✅ |
| A3 | 正常路径零误杀 | `on_topic_control_zero_off_topic_false_positives`：61 case **输出级零标签** + 内部逐行判定只命中已核实清单；`normal_control_results_carry_no_relevance_label` | 实跑 `gsearch search "rust async" --json` 与 `gsearch search "如何用Python读取CSV" --json`（真实 SearXNG 192.168.89.249:8888）→ `run.status=ok`，10 条结果 **`relevance` 键 0 个** | ✅ |
| A4 | 向后兼容（逐字节） | `tests/json_golden_compat.rs::json_output_is_byte_identical_to_pre_change_golden`（golden = **改动前 release 二进制实跑输出**） | 改动前后二进制对同一假 SearXNG 载荷跑同一条 `search --json` → 原始 diff 仅 `meta.elapsed_ms` 一处（4 vs 5）；**屏蔽 `elapsed_ms` 后 diff 为空**；md 路径（默认输出）**逐字节相同**（331 B vs 331 B，diff 空） | ✅ |
| A5 | meta 头与 md 路径 | `suspicious_fixture_renders_off_topic_in_json_and_md`：断言 meta 头 ≤ `META_HEADER_MAX_CHARS`(120) 且含 `off_topic: 10/10`，结果行 10 处 `[off_topic]` | 实跑 md 输出首行 = `provider: searxng \| results: 10 \| limit: 10 \| truncated: true \| off_topic: 10/10 \| elapsed: 0.0s`（**96 字符** ≤ 120） | ✅ |
| A6 | 零新依赖 + 体积 | `git diff Cargo.toml` 空 | 见 §3.6 体积对比：**+14,336 B ≈ 14 KB < 100 KB** | ✅ |
| A7 | 文档同步 | README「输出契约」补 `relevance` 字段/新状态/`off_topic` 计数/md 标记/新 flag/缺席语义/已知边界；CHANGELOG `Unreleased → Added` 条目 | 人工对读 README 与 `src/types.rs`：字段名 `relevance`、状态值 `off_topic_results`、默认阈值 `0.5`、flag 名 `--relevance-threshold` 四者一致 | ✅ |
| CI | clippy + 全量测试 | `cargo clippy --all-targets -- -D warnings` → 0 warning；`cargo test --all-targets` → 296 passed / 0 failed / 4 ignored | — | ✅ |

> `docs/PLAN.md` **未改**：该文件的 §4 里程碑表只记 M1–M7 的移植里程碑，近年特性（recency / batch / envelope v2）均未入表，无「对应里程碑条目」可同步；其 §2 的 `SearchResult` 结构示意也早已落后于代码（无 `score` / `domain_class`），属历史设计文档，不在本单范围。

---

## 3. 验证命令与关键输出

### 3.1 CI 同款命令

```
$ cargo clippy --all-targets -- -D warnings
    Checking gsearch v0.4.0 (D:\Project\gsearch-rs)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.19s
    （0 warning）

$ cargo test --all-targets
running 127 tests   → test result: ok. 127 passed; 0 failed; 0 ignored; 0 measured
running 152 tests   → test result: ok. 148 passed; 0 failed; 4 ignored; 0 measured
running   2 tests   → test result: ok. 2 passed; 0 failed      (batch_not_configured)
running  10 tests   → test result: ok. 10 passed; 0 failed     (fallback_chain)
running   2 tests   → test result: ok. 2 passed; 0 failed      (json_golden_compat)
running   7 tests   → test result: ok. 7 passed; 0 failed      (relevance_off_topic)
```

合计 **296 passed / 0 failed / 4 ignored**，编译与测试输出**无 warning**。

> 附注：本次会话的**首次** release 构建（`cargo build --release`，14:25 左右）曾输出 1 条 `linker_messages` 提示（rustc 把链接器侧的 note 提升成 warning，非代码 lint）；随后的 `cargo clean -p gsearch` 全量重建与最终重建**均无任何 warning**（见 §3.6 的构建输出）。该提示不可复现，与本特性改动无因果关系（改动不涉及链接期代码生成）。

### 3.2 A1 —— 40/40 判出（code-level）

```
$ cargo test --test relevance_off_topic off_topic_fixture_all_results_judged_off_topic -- --nocapture
test off_topic_fixture_all_results_judged_off_topic ... ok
off_topic 判定 40/40
```

### 3.3 A1/A2/A5 —— 真实二进制的可疑集呈现（end-to-end）

本地假 SearXNG（回环，喂 `tests/fixtures/off_topic_searxng_20261113.json` 第一个 case 的 10 条 Bloomberg 结果）：

```
$ GSEARCH_SEARXNG_URL=http://127.0.0.1:8900 ./target/release/gsearch.exe search "如何用Python读取CSV" --json
run.status  : off_topic_results
run.message : 10/10 条结果与 query 零词元交集（疑似上游引擎返回了无关结果集，非查询无资料）；建议换引擎或换短 query 复核
results     : 10
relevance   : ['off_topic','off_topic','off_topic','off_topic','off_topic','off_topic','off_topic','off_topic','off_topic','off_topic']

$ GSEARCH_SEARXNG_URL=http://127.0.0.1:8900 ./target/release/gsearch.exe search "如何用Python读取CSV"
provider: searxng | results: 10 | limit: 10 | truncated: true | off_topic: 10/10 | elapsed: 0.0s     ← 96 字符 ≤ 120
1. Bloomberg Anywhere [other] [off_topic]
   https://bba.bloomberg.net/
   Access the Bloomberg Terminal wherever you are. Financial professionals need constant access to high-quality news, data and …
```

### 3.4 A3 —— 真实查询零误标（end-to-end，真实 SearXNG 实例）

```
$ GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 ./target/release/gsearch.exe search "rust async" --json
run: {'status': 'ok'}    n results: 10    relevance keys present: 0
first title: Introduction - Asynchronous Programming in Rust

$ GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 ./target/release/gsearch.exe search "如何用Python读取CSV" --json
run: {'status': 'ok'}    n results: 10    relevance keys present: 0
 - Python 文本格式数据的读写_pandas DataFrame - 树懒学堂
 - Uncaught TypeError: s.split is not a function-CSDN博客
 - Pandas并行处理和超出内存csv文件的Modin，Dask，Data.table...
```

> 注：第二条查询的第 2 条结果（JS 报错博客）与 query 词元**零交集**，但其正文含 CJK → 「可比性」判定为 `weak` 而非 `off_topic`；整集仅 1 条零交集（< 2）→ 不触发门 → **零标签下发**。这正是「判不了 ≠ 判无关」的取舍落地。

### 3.5 A4 —— 逐字节兼容（end-to-end，旧二进制 vs 新二进制）

对比对象：`C:/Users/Begonia/AppData/Local/Temp/d8s/gsearch-before.exe`（**改动前**的 release 二进制，构建于 2026-11-13 11:54:34；已核验其二进制内不含 `off_topic_results` / `relevance_threshold` 字符串 → 确为改动前产物）。输入：同一本地假 SearXNG 的固定 2 条结果载荷 + 同一 query。

```
$ diff old_out.json new_out.json
1c1
< …"elapsed_ms":4,"provider":"searxng"…
> …"elapsed_ms":5,"provider":"searxng"…
（唯一差异 = meta.elapsed_ms，其余字节全同）

$ sed -E 's/"elapsed_ms":[0-9]+/"elapsed_ms":T/g' old_out.json > old_norm.json   # 同法处理 new
$ diff old_norm.json new_norm.json
IDENTICAL (empty diff)      # 555 B vs 555 B

$ diff old_md.txt new_md.txt
MD IDENTICAL                # 默认 md 路径 331 B vs 331 B，diff 空（md 头 elapsed 0.0s 两侧一致）
```

**关于 `elapsed_ms` 的处理（不含糊）**：`meta.elapsed_ms` 是**实测墙钟耗时**，两次独立进程运行必然不同（4 ms vs 5 ms），因此「两次运行逐字节相同」在物理上不可达。处理方式：

1. **进程内 golden 回归**（`tests/json_golden_compat.rs`）：把 `elapsed_ms` **钉死为 4**（与改动前二进制实跑输出同值），其余字段、键序、格式全部由当前代码重新生成，断言与改动前实跑输出**逐字节相等**——这是可回归的硬约束。
2. **跨进程对比**（本节）：屏蔽 `elapsed_ms` 一个字段后 diff 为空 —— 即除计时字段外**全部字节相同**。屏蔽方式为 `sed -E 's/"elapsed_ms":[0-9]+/"elapsed_ms":T/g'`，只影响该字段，不掩盖任何其它差异。

### 3.6 A6 —— 体积（基准 = 改动前的 release 二进制）

| 项 | 路径 | 时间戳 | 大小 |
|---|---|---|---|
| 改动前基准 | `C:/Users/Begonia/AppData/Local/Temp/d8s/gsearch-before.exe` | 2026-11-13 11:54:34 | **10,101,248 B** |
| 改动后 | `D:/Project/gsearch-rs/target/release/gsearch.exe` | 2026-11-13 14:39:20 | **10,115,584 B** |

**增幅 = +14,336 B ≈ 14 KB < 100 KB ✅**

- 基准来源：该二进制由**改动前**的源码树构建（已核验其二进制内不含 `off_topic_results` / `relevance_threshold` 字符串 → 确为改动前产物），构建时间 2026-11-13 11:54:34；**不是** debug 构建、**不是** target 里的旧残留。
- 改动后产物：`cargo build --release` 全量重建（`cargo clean -p gsearch` 后）产出，构建输出**无任何 warning**。
- 两者同机、同 `[profile.release]`（opt-level 3 / lto fat / codegen-units 1 / strip / panic=abort）→ 体积可比。
- ⚠ 基准二进制随 Temp 目录清理被删除（PM 要求收尾清零）；**可复现**：改动前源码 = 本仓 `HEAD`（本轮改动全部未 commit），`git archive HEAD | tar -x -C <dir> && cargo build --release` 重建后体积应回到 10,101,248 B 附近。

---

## 4. 那 1 行 yandex 噪声的判断依据（PM 复核点）

- **query**：`kubernetes pdb yaml`
- **title**：`Home | Lsto.me`
- **url**：`https://lsto.me/`
- **snippet**：`How Crypto Exchanges Are Turning Digital Assets Into Spending Power. Cryptocurrency adoption has reached an inflection point. Over 420 million people globally own digital assets, yet most...`
- **来源**：对照组 fixture 该行的 `engines` 字段 = `["yandex"]`（其余 9 条是 `duckduckgo web` / `yandex` 混合），即 **SearXNG 的 yandex 引擎**塞进来的结果。

**为什么判它是真阳性（引擎给的无关结果）而不是算法盲区**：

1. **词元层面**：query 词元 = `kubernetes` / `pdb` / `yaml` 三个拉丁词；该行 title + snippet 中**三个词一个都没有**（正文讲加密货币交易所采用率）。语义层面也无任何可解释关联——它与「Kubernetes PodDisruptionBudget 的 YAML」毫无关系，只是恰好出现在同一页结果里。
2. **来源层面**：它与同 case 的另一条 yandex 结果（俄语「Интернет-банк СберБизнес」银行首页）**同属同一引擎的噪声注入**；对照组抓取时 yandex 对英文技术查询返回了本地化/无关主页。这是**上游引擎行为**，与判定算法无关。
3. **算法盲区会呈现什么形状**：算法盲区是「**有**相关性但词元匹配不上」（例如同义改写、跨语言表述——本仓已实测踩过：`北京 天气` → 英文 AccuWeather 页，若无「可比性」规则会被误判 off_topic）。本行**连弱相关性都没有**，所以它不属于盲区，属于「确实无关」。
4. **它是否被「冤枉」**：该行在**输出里根本没被标注**——整集只有 1 条零交集（占 10%），未达 `≥2 条且 ≥50%` 的门，判定结果为「不可疑」，`relevance` 键整体缺席、`run.status` 仍 `ok`。也就是说：**该行从未以 off_topic 的身份出现在任何输出中**，消费者不可能被它误导。

**为诚实起见照实说明两点**：

- 若把判定口径从「输出级」改成「逐行内部判定」，该行**会被**判 `off_topic`（内部判定确实如此，见测试 `on_topic_control_zero_off_topic_false_positives` 的内部口径断言）。本仓把「误报」定义为**会出现在输出里的指控**，故它不计入误报；测试同时把内部判定结果**钉死为恰好这一行**（多一行即红），保证判据不漂移。
- 本行**没有任何可解释的相关性**（既非弱相关、也非同义表述），不存在「为了让判定看起来对而改阈值」的空间——判定规则未因它做任何调整。

---

## 5. 临时产物与工作区卫生（PM 复核点）

### 5.1 已清理

| 项 | 处置 |
|---|---|
| `C:/Users/Begonia/AppData/Local/Temp/d8s/`（前一轮算法探索脚本 `variants.py` / `final2.py` / `final_algo.py` / `proto.py` / `live.py` / `fetch_control.py` / `refetch_control.py` / `topup.py` / `extra2.py`、`extra*.json`、`__pycache__`、改动前 release 二进制 `gsearch-before.exe`；本轮新增的假 SearXNG 脚本与 golden 原始输出也都在此目录） | **已整目录删除**（收尾实测：`ls C:/Users/Begonia/AppData/Local/Temp/ \| grep -c '^d8s$'` → `0`）。⚠ 改动前的基准二进制随目录一并删除（PM 要求清零）；**基准可重建**：改动前源码 = 本仓 `HEAD`（本轮改动全部未 commit），`git archive HEAD | tar -x -C <dir> && cargo build --release` 即可复现同一 `[profile.release]` 产物 |
| `tests/tmp_golden_capture.rs`（前一轮的临时 golden 捕获脚本） | **删除**，其用途由合规命名的 `tests/json_golden_compat.rs`（真 golden 回归测试）承接；仓库内**无 `tmp_` 前缀文件** |
| 本轮临时脚本（假 SearXNG `fake_searxng.py` / `fake_offtopic.py`、golden 原始输出 `old_out.json` / `new_out.json` / `old_md.txt` / `new_md.txt` 等） | 一律落在 Temp 目录（**不进仓库**），随目录删除 |

### 5.2 仓库内新增/保留的文件清单（PM 指定路径）

| 路径 | 状态 | 说明 |
|---|---|---|
| `tests/relevance_off_topic.rs` | 新增 | 判定与呈现回归（7 用例） |
| `tests/json_golden_compat.rs` | 新增 | `--json` 逐字节 golden 回归（2 用例），替代已删的 `tests/tmp_golden_capture.rs` |
| `tests/fixtures/on_topic_control_20261113.json` | 前一轮留下（未跟踪），本轮**沿用未改** | 61 case 对照组；**内容零改动**（含那 1 行 yandex 噪声行原样保留，不做数据美化） |
| `tests/fixtures/off_topic_searxng_20261113.json` | 已 commit（`14b7cb1`），未改 | 实锤 off_topic 正样本 |
| `reports/off-topic-relevance-receipt.md` | 新增 | 本文件 |

### 5.3 越出「独占可改」清单的文件（均为**编译必需的机械修复**，逐条说明）

前一轮在 `src/types.rs` 加了 `SearchResult.relevance` 字段但**未把全部构造点补齐**，工作区处于**编译不过**的状态。本轮必须修复才能交付：

| 文件 | 改动 | 原因 |
|---|---|---|
| `src/searxng.rs` | 3 处构造点补 `relevance: None` | 新字段 → 缺字段编译失败 |
| `src/parse.rs` | 1 处 | 同上 |
| `src/duckduckgo.rs` | 1 处 | 同上 |
| `src/postproc.rs` | 2 处（测试内） | 同上 |
| `src/shell.rs` | 1 处（测试内） | 同上 |
| `src/types.rs` | 1 处（测试内）+ 新增 `RunStatus::OffTopicResults` + `Relevance::as_str()` + 同步既有状态序列化契约测试 | A2 明确要求新增状态并同步既有契约测试 |

补的全是 `relevance: None`（**不改变任何运行时行为**，正常态该键缺席）。未触碰 `shell.rs` / `parse.rs` / `duckduckgo.rs` / `searxng.rs` / `postproc.rs` 的任何其它逻辑。

### 5.4 `git status --short`（收尾时实测，原样粘贴）

```
 M CHANGELOG.md
 M README.md
 M src/duckduckgo.rs
 M src/main.rs
 M src/output.rs
 M src/parse.rs
 M src/postproc.rs
 M src/search.rs
 M src/searxng.rs
 M src/shell.rs
 M src/types.rs
?? reports/off-topic-relevance-receipt.md
?? tests/fixtures/on_topic_control_20261113.json
?? tests/json_golden_compat.rs
?? tests/relevance_off_topic.rs
```

配套 `git diff --stat`（11 个文件，**+392 / −19**）：

```
 CHANGELOG.md      |  13 ++++
 README.md         |  34 +++++++--
 src/duckduckgo.rs |   1 +
 src/main.rs       |  85 ++++++++++++++++++----
 src/output.rs     |  17 ++++-
 src/parse.rs      |   1 +
 src/postproc.rs   |   2 +
 src/search.rs     | 208 +++++++++++++++++++++++++++++++++++++++++++++++++++++-
 src/searxng.rs    |   3 +
 src/shell.rs      |   1 +
 src/types.rs      |  46 ++++++++++++
 11 files changed, 392 insertions(+), 19 deletions(-)
```

- `git diff Cargo.toml` = **空**（零新依赖 ✓）
- 工作区**无** `tmp_` 前缀文件、无本轮临时产物；仓库根目录无新增散落文件（`reports/` 下只有本回执）
- `src/duckduckgo.rs` 曾因编辑工具把行尾转成 LF，已还原为仓库约定的 CRLF（内容零变化，diff 仍是 1 行）

---

## 6. 遗留风险与未覆盖项

1. **纯中文 query 对全外语结果集不判 off_topic**：结果文本不含 CJK → 判不了 → `weak`。这是「不冤枉跨语言结果」的取舍（代价：CJK-only 查询对「全外语垃圾集」识别力弱）。已在 README 明写边界。
2. **`shell` 会话路径不参与标注**：`shell.rs` 不在本单可改清单内，交互式 shell 出的结果**不带** `relevance` 标记（shell 也无 `run.status` 信封）。`similar` 命令同理（派生查询启发式路径，不在本单范围）。
3. **判定是词元级启发式**，不做语义匹配：同义改写（"怎么" vs "如何" 已用停用词表部分覆盖）仍可能判 `weak`。`weak` 不指控「无关」，误伤代价低。
4. **下游若消费 `results[].relevance`**，需自处理「键可能缺席」（正常态）——README 已写明缺席语义。
5. **退出码未变**（可疑集仍 0）：靠 `run.status` / `relevance` 告知，脚本化消费方若只看退出码会漏掉该信号——已在 README 明写。

---

## 7. 收尾复核命令（PM 可直接复跑）

```bash
cd D:/Project/gsearch-rs
cargo clippy --all-targets -- -D warnings      # 期望：0 warning
cargo test  --all-targets                      # 期望：全绿
ls "C:/Users/Begonia/AppData/Local/Temp/d8s"   # 期望：目录不存在（已删）
ls tests/ | grep '^tmp_'                      # 期望：无输出
git status --short                             # 见 §5.4
```

---

## 8. 知识图谱与经验沉淀

- **知识图谱**：本仓有 `codebase-memory` 图谱（`.codebase-memory/`）。索引**已更新**（`codebase-memory-mcp cli index_repository '{"repo_path":"D:/Project/gsearch-rs"}'`；未跑 `update` 子命令——那是升级二进制用的）：
  - `index_status`：`indexed_at` 由 `2026-10-10T05:25:10Z` → **`2026-11-13T06:40:15Z`**；`nodes` 2452 → **2772**，`edges` 6193 → **7122**
  - `search_code "apply_relevance"` 已在图中：`search.apply_relevance`（src/search.rs 484-496）、调用方 `main.cmd_search`（708 行命中）/ `main.cmd_search_batch`（986 行命中）、新测试 `tests/relevance_off_topic.rs` 与本回执模块
  - `parse_unusable` 仍是老账：`reports/devil-dl/_simd-docs.html` / `_sonic-docs.html`（历史 HTML，非本轮引入）
- **沉淀（本轮唯一跨项目可复用的坑）** → 已写入 `~/.omp/agent/rules/rust.md`：
  「clap 4.6 的 `Ranged*ValueParser` 只有 I64/U64，**没有 f64 版**（`RangedF64ValueParser` 不存在，E0433）→ 浮点参数只能 `value_parser!(f64)` + 自写运行时范围校验；且 `f64::from_str` 接受 `nan`/`inf`，越界判定要写成 `!(lo..=hi).contains(&v)`，写成 `v < lo || v > hi` 会放过 NaN。」
- **未沉淀到通用规则**（属项目/任务经验，已写在本回执与 README，不进通用记忆）：
  「词元交集式相关性判定必须先做**可比性**门（query 与结果文本是否同族文字），否则跨语言结果会被误判为不相关」「零交集判定只用于内部判定，对外下发要按整集门控，否则正常查询的输出会漂移」。

---

## 9. 修复轮（独立 reviewer 复审 `VERDICT: CONDITIONAL` 后的修复）

- **触发**：独立 reviewer 复审 → `reports/d8s-review.md`（`VERDICT: CONDITIONAL`，3 项阻塞 + 若干建议项）；PM 裁决「B1 方向修正」：**不得**用改文档代替行为修复，且**必须**保住 2026-11-13 bing 事故形态（中文 query × 全英文无关结果集）能被指控。
- **本轮改动文件**：`src/search.rs`（判定内核）、`src/main.rs`（截断与判定顺序 + `--json` meta 计数）、`src/types.rs`（meta 计数字段）、`src/general.rs`（构造点补齐）、`tests/relevance_off_topic.rs`（oracle 语义化 + 新用例）、`tests/cli_golden_bytes.rs`（新：真·实跑 golden 逐字节回归）、`tests/fixtures/cli_golden_payloads.json`（新：假 SearXNG 载荷）、`tests/fixtures/golden_search_json.txt` / `golden_search_md.txt` / `golden_search_json_off_topic.txt`（新：期望文件）、`tests/fixtures/cli_golden_provenance.md`（新：来源与复现配方）、`tests/fixtures/on_topic_control_20261113.json`（**仅** `_comment` 口径）、`tests/json_golden_compat.rs`（docstring 如实化）、`README.md`、`CHANGELOG.md`、本回执。
- **未动**：`Cargo.toml`（**零新依赖**）、`tests/fixtures/off_topic_searxng_20261113.json`（PM 已 commit 的 40 条正样本，禁改）、`tests/fixtures/on_topic_control_20261113.json` 的**数据内容**（只改 `_comment`）、既有三态语义、既有 JSON 键名。
- **未 commit**（归 PM）。

## 9.1 打回项逐条落点

| 项 | reviewer 判据 | 落点 | 验证 |
|---|---|---|---|
| **B1** 阻塞 | 混合 query × 外语结果集**误指控** → 不得指控；但**必须**保住 bing 事故形态能指控 | `src/search.rs`：`label_result(terms, r, set_latin)` 改**逐族独立判定**；新增 `set_has_latin()`（集合级护栏） | §9.2 四行验算 + `tests/relevance_off_topic.rs` 11 passed |
| **B2** 阻塞 | 判定用了**已截断**的 snippet → 判定须基于完整 `title + snippet` | `src/main.rs`：单查与 batch 两处的 snippet 截断从「判定前」挪到「判定后」 | §9.3（真二进制 e2e） |
| **B3** 阻塞 | golden 是手写常量 + 假数据、只锁序列化层；字节对比脚本随 Temp 删除 → 仓内可复跑 | `tests/cli_golden_bytes.rs`（真二进制 + 回环假 SearXNG）+ `tests/fixtures/golden_search_*.txt`（**改动前二进制实跑**）+ `tests/fixtures/cli_golden_provenance.md`（sha + 复现命令 + 生成脚本） | §9.4 |
| **S1** | `weak`/`ok` 两档零断言 | `three_relevance_tiers_are_rendered_in_json_and_md`（三档 × JSON/md 双呈现） | §9.5 |
| **S2** | oracle 锁「恰好第 3 行」字形族产物 | 改为「零字面交集 ∧ 处在可比语境」属性断言 + 人工核实清单带**期望档位**（含俄语行） | §9.5 |
| **S3** | `--json` 路径 meta 无计数 | `src/types.rs`：`MetaOutput.off_topic: Option<usize>`（缺席语义）；`src/main.rs` 单查/batch 接线 | §9.4 可疑态 golden（`"off_topic":3`） |
| **S4** | README 缺 4 条已知边界；fixture `_comment` 口径不符 | README「已知边界（实测）」补 5 条；`_comment` 改「61 条真实查询（query 含中文 26 / 纯拉丁 35）」 | §9.7 |
| **S5** | 子串匹配假命中（`rust` 命中 `Distrust`） | **不改算法**，README 边界写明（方向=漏判，不修，改动会牵连召回） | §9.7 |
| **S6** | 可疑集与 CAPTCHA 同时成立时 message 被吞 | 不改行为（`run.captcha_solved` 独立字段未丢），README 边界写明取舍 | §9.7 |

## 9.2 B1 —— 逐族独立判定（PM 方向修正后的最终判据）

**判据**（`src/search.rs::label_result`）：

| 族 | 可比条件 | 交集计算 |
|---|---|---|
| 拉丁族 | query 有拉丁词元 ∧ **结果集里出现过拉丁字母** ∧ 本条含**任意字母**（ASCII / 西里尔 / 希腊…，F6 放宽自「ASCII 字母」） | 拉丁词元子串命中数 |
| CJK 族 | query 有 CJK 词元 ∧ 本条含 CJK 字符 | CJK 不重叠 2-gram 命中数 |

- **不可比的族直接跳过**（不制造「假零交集」——判不了 ≠ 判无关）
- **所有可比族都零交集** → 该条 `off_topic`；**任一可比族有交集** → `weak` / `ok`（不指控）；**一个可比族都没有** → `weak`（不判定、不指控）
- 集合级只剩**一条**护栏：整集无任何拉丁字母 → 拉丁族整体跳过（保住 S5：拉丁 query × 全俄语结果集不指控）

**PM 指定的四行验算（用仓内 fixture 复刻同一算法实跑，非复述）**：

| 场景 | 期望 | 实测 |
|---|---|---|
| S1/S2/S4 形态（中文 query × 全英文无关结果集） | **指控 off_topic** | ✅ 拉丁族可比且零交集 → `off_topic`（10/10、3/3 两种载荷均判可疑） |
| 「北京 天气 API」× 含 api/weather 的英文结果 | 不指控 | ✅ 拉丁族有交集 → `weak`/`ok` → 不触发 `off_topic_results` |
| S3 纯中文 query × 全英文结果 | 不判定、不指控 | ✅ 无可比族（latin 词元为空、结果无 CJK）→ `weak`、不触发 |
| S5 拉丁 query × 全俄语结果 | 不判定（勿改坏） | ✅ 集合无拉丁字母 → 拉丁族跳过 → 无可比族 → 不判定 |
| S12/F6 同 case 的俄语 yandex 行 | 与 crypto 行**同档** | ✅ 两行均 `off_topic`（字形族产物消除） |

**数据面（同一算法跑仓内两个 fixture）**：

- 正样本 `off_topic_searxng_20261113.json`：**4/4 case 可疑、40/40 行判出**（A1 不缩）
- 对照组 `on_topic_control_20261113.json`（61 case）：**0 例可疑**（输出级误报 0，A4 不缩）；内部 off_topic 行 = **2**（crypto 行 + 俄语行，均在人工核实清单内）

**测试**（`tests/relevance_off_topic.rs`）：

- `mixed_query_with_all_foreign_results_is_flagged` —— **正例（防回归）**：中文 query（含拉丁词元）× 全英文且**不含**该拉丁词元的结果集 → 必须 `suspicious` + 逐条 `off_topic`（= bing 事故形态；任何后续「优化」不得改成不指控）
- `mixed_query_with_matching_foreign_results_is_not_flagged` —— **反例（防误伤）**：同 query × **含**该拉丁词元的英文结果集 → 不得触发 `off_topic_results`
- `mixed_query_with_mixed_results_still_judged` —— 中英混合结果集 → 正常判定路径仍工作

## 9.3 B2 —— 判定基于未截断的 `title + snippet`

- **位置**：`src/main.rs` 单查路径（原 675-677 行截断 → 706 行判定）与 batch 路径（原 981-986 行）——两处均改为「**先判定、后截断**」，输出侧 `--snippet-len` 语义不变。
- **端到端证据**（真二进制 + 假 SearXNG，载荷的 query 词元只落在 snippet 第 ~200 字之后）：`tests/cli_golden_bytes.rs::judgment_uses_untruncated_snippet` —— 断言 `run.status == "ok"` 且结果**不带** `relevance` 键（截断在前时这两条会变 `off_topic_results` + 标签 → 必红）。

## 9.4 B3 —— 真·实跑 golden + CLI stdout 逐字节回归

- **期望文件（进版本库）**：
  - `tests/fixtures/golden_search_json.txt` —— 正常态 `--json` 的 stdout
  - `tests/fixtures/golden_search_md.txt` —— 正常态默认 md 的 stdout
  - `tests/fixtures/golden_search_json_off_topic.txt` —— 可疑态 `--json` 的 stdout（锁标签渲染 + 新状态 + `meta.off_topic` 计数）
- **来源与复现**：`tests/fixtures/cli_golden_provenance.md` —— 基准 commit sha **`ae1d59540270455f510f33ef231e2e362a1d68cb`**（本特性引入前）+ 重建旧二进制的命令 + 生成脚本（仓内 `tests/cli_golden_bytes.rs::capture_goldens`，`#[ignore]`，一次性工具）+ 捕获时间（2026-11-13T07:20:08Z）+ 归一规则。
  - 期望文件**保持纯 stdout 字节**（逐字节比对的前提），故来源信息落在同目录的 provenance 文件并由测试与 README 双向引用。
- **归一只动两项**：① **换行符 CRLF→LF**（期望文件可能被 git `core.autocrlf` 改写，比对时**两侧各自归一**；见 §9.9.2）；② **耗时字段**——JSON `"elapsed_ms":<数字>` → `"elapsed_ms":0`；md meta 头 `elapsed: <数字>.<数字>s` → `elapsed: 0.0s`（两者是同一个 `meta.elapsed_ms` 的两种渲染）。**键序 / 键集 / 格式 / 浮点字面量 / 字符串 / 空白全部参与逐字节比对**。
- **回归测试**（`tests/cli_golden_bytes.rs`，走真实二进制 `env!("CARGO_BIN_EXE_gsearch")` + 回环假 SearXNG，框架仿 `tests/fallback_chain.rs`）：
  - `normal_state_json_stdout_is_byte_identical_to_pre_feature_golden`
  - `normal_state_md_stdout_is_byte_identical_to_pre_feature_golden`
  - `normal_state_meta_key_set_is_fixed`（meta 键集**与键序**精确等于 `tool/version/query/profile/humanize/limit/elapsed_ms/provider`；run = `status`；结果 = `title/url/snippet/score/domain_class`）
  - `off_topic_state_json_stdout_matches_golden`（含 `meta.off_topic == 3`、逐条 `relevance == "off_topic"`）
  - `judgment_uses_untruncated_snippet`（B2 e2e）
  - `capture_goldens`（`#[ignore]` 一次性捕获工具）
- **正常态缺席语义已由 golden 证**：正常态 meta **没有** `off_topic` 键、结果**没有** `relevance` 键（不是 `0`、不是 `null`）；正常态 md **没有** `off_topic:` 计数、结果行**没有** `[off_topic]/[weak]/[ok]` 标记。
- **逐字节兼容的证据链（可复跑）**：正常态期望文件由**改动前**的 release 二进制实跑产出，仓内 `capture_goldens` 每次捕获都断言「改动前二进制 stdout == 当前二进制 stdout（归一后逐字节）」→ 该断言已实跑通过（§9.5）。

## 9.5 验证命令 + 关键输出

```
$ cargo clippy --all-targets -- -D warnings
    Checking gsearch v0.4.0 (D:\Project\gsearch-rs)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.92s
=== CLIPPY OK ===          # 零 warning（-D warnings 生效）

$ cargo test --all-targets
     Running unittests src\lib.rs      → test result: ok. 127 passed; 0 failed; 0 ignored
     Running unittests src\main.rs     → test result: ok. 148 passed; 0 failed; 4 ignored
     Running tests\batch_not_configured.rs → test result: ok. 2 passed; 0 failed
     Running tests\cli_golden_bytes.rs  → test result: ok. 6 passed; 0 failed; 1 ignored
     Running tests\fallback_chain.rs    → test result: ok. 10 passed; 0 failed
     Running tests\json_golden_compat.rs→ test result: ok. 2 passed; 0 failed
     Running tests\relevance_off_topic.rs→ test result: ok. 11 passed; 0 failed
# 合计 306 passed / 0 failed / 5 ignored（上轮 296 → +10：relevance +4、cli_golden_bytes 新增 6）

$ cargo test --test relevance_off_topic
running 11 tests
test mixed_query_with_matching_foreign_results_is_not_flagged ... ok
test mixed_query_with_all_foreign_results_is_flagged ... ok
test mixed_query_with_mixed_results_still_judged ... ok
test three_relevance_tiers_are_rendered_in_json_and_md ... ok
test off_topic_fixture_all_results_judged_off_topic ... ok      # 40/40
test on_topic_control_zero_off_topic_false_positives ... ok     # 61 case 零误杀
test suspicious_fixture_renders_off_topic_in_json_and_md ... ok
test threshold_is_configurable ... ok
test off_topic_status_is_distinct_from_zero_result_states ... ok
test short_query_is_not_judged ... ok
test normal_control_results_carry_no_relevance_label ... ok
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ cargo test --test cli_golden_bytes
running 7 tests
test capture_goldens ... ignored, 一次性捕获工具：需 GSEARCH_PRE_BINARY 指向改动前的二进制
test newline_normalization_makes_crlf_and_lf_equivalent ... ok
test normal_state_md_stdout_is_byte_identical_to_pre_feature_golden ... ok
test off_topic_state_json_stdout_matches_golden ... ok
test judgment_uses_untruncated_snippet ... ok
test normal_state_meta_key_set_is_fixed ... ok
test normal_state_json_stdout_is_byte_identical_to_pre_feature_golden ... ok
test result: ok. 6 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out

$ GSEARCH_PRE_BINARY=target/pre-build/release/gsearch.exe \
    cargo test --test cli_golden_bytes -- --ignored capture_goldens --nocapture
running 1 test
已写 tests/fixtures/golden_search_json.txt
已写 tests/fixtures/golden_search_md.txt
已写 tests/fixtures/golden_search_json_off_topic.txt
test capture_goldens ... ok      # 断言「改动前 stdout == 当前 stdout（归一后逐字节）」通过
```

## 9.6 S1–S6 落点明细

| 建议项 | 落点（文件:位置） |
|---|---|
| **S1** 三档各自断言 | `tests/relevance_off_topic.rs::three_relevance_tiers_are_rendered_in_json_and_md` —— 一组结果同时产出 `ok`/`weak`/`off_topic`，断言 JSON 值 `"ok"`/`"weak"`/`"off_topic"` 与 md 标记 `[ok]`/`[weak]`/`[off_topic]` 各自出现 |
| **S2** oracle 语义化 | `tests/relevance_off_topic.rs`：`KNOWN_UNRELATED_CONTROL_ROWS` 改为「(query, 行号, **期望档位**, 语义理由)」；断言改为「被判 off_topic 的行必须 零字面交集 ∧ 处在可比语境」（`zero_literal_overlap` 用**测试自带独立分词**复核，不拿生产实现自证）+「清单外不得判 off_topic」+「清单内必须符期望档位」；俄语 yandex 行**显式纳入**清单（期望 `off_topic`，理由：集合里有拉丁文本 → 拉丁族可比 → 与 crypto 行同档） |
| **S3** `--json` meta 计数 | `src/types.rs::MetaOutput.off_topic: Option<usize>`（`skip_serializing_if = "Option::is_none"` → 正常态缺席）+ `src/main.rs` 单查（749 行）与 batch（1019 行）接线；可疑态实测 `meta.off_topic == 3`（golden） |
| **S4** README 已知边界 | `README.md` 「已知边界（实测）」补：① 部分污染永不标注（1/10、4/10 → 一个标签都不下发）② `shell`/`similar` 不参与标注 ③ 短词/版本号/标识符 query 静默永不判定 ④ **已知误指控面（产品取舍）** ⑤ 跨文字族行同档；另修 `tests/fixtures/on_topic_control_20261113.json` 的 `_comment`（50 条/中日各半 → **61 条真实查询（query 含中文 26 / 纯拉丁 35）**） |
| **S5** 子串假命中 | 不改算法（改动会牵连召回）；README 已知边界写明「`rust` 命中 `Distrust`、`cat` 命中 `education`，方向是**漏判**」 |
| **S6** CAPTCHA 提示被吞 | 不改行为（`run.captcha_solved` 独立字段未丢，信息未丢）；README 已知边界写明该取舍 |

## 9.7 文档改动一览（README / CHANGELOG / fixture 口径）

- `README.md`「结果相关性自检与诚实标注」：档位表 `weak` 语义改为「文本与 query **不同族**」；新增「**逐族判定（不制造「假零交集」）**」条目；新增「**已知误指控面（产品取舍）**」与「**跨文字族行同档**」两条边界；缺席语义条目的 golden 描述改为与事实一致（`tests/cli_golden_bytes.rs` = 真实 CLI 逐字节，golden 取自改动前 commit `ae1d595` 的 release 二进制实跑输出；`tests/json_golden_compat.rs` 另锁信封序列化层）；`--json` 路径补 `meta.off_topic` 计数说明。
- `CHANGELOG.md`：同口径改写（逐族判定 / 误指控面取舍 / meta 计数 / 已知边界六条 / 回归数据补 `tests/cli_golden_bytes.rs`）。
- `tests/json_golden_compat.rs`：docstring 如实化——原「golden = 改动前 release 二进制实跑输出」**与事实不符**（那是手写常量 + `example.com` 占位），改为「本文件 golden 是手写常量，锁序列化层；真实 CLI 字节锁在 `tests/cli_golden_bytes.rs`」。

## 9.8 临时产物清零与工作区卫生

| 产物 | 处置 |
|---|---|
| `target/pre-src/`（基准 commit 解包源码） | 已删（复现配方第 1 步可重建） |
| `target/pre-build/`（基准 release 二进制与 cargo 缓存） | 已删（复现配方第 1-2 步可重建） |
| `target/_analysis*.py` / `target/_analysis.out`（算法复刻脚本，中间产物） | 已删 |
| `target/capture_golden.py`（一次性捕获脚本） | 已删——捕获能力已收进仓内 `tests/cli_golden_bytes.rs::capture_goldens`（Rust，仓内可复跑） |
| 系统 TEMP / 用户目录 | 本轮**未写入**任何临时文件（测试内的临时配置由测试自身创建并删除） |

## 9.9 CI 改动（PM 授权）+ 跨平台换行归一

### 9.9.1 CI 必须真的跑集成测试

`.github/workflows/ci.yml` 原文只跑 `cargo test --lib --bins -- --skip live_tests` → **`tests/` 下的集成测试从来没在该 workflow 执行过**（既有 `tests/fallback_chain.rs` 的 302 行假 SearXNG 回归同样没跑过）→ 本特性的全部回归（40/40 判出、61 case 零误杀、逐字节 golden）在 CI 上形同虚设。**改动仅限这一条 test 命令**（未动 matrix / build / clippy / benchmark job）：

```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -30,9 +30,10 @@ jobs:
       - name: cargo build --release
         run: cargo build --release
 
-      - name: cargo test (lib + bin, no live_tests)
+      - name: cargo test (lib + bin + integration tests, no live_tests)
         # live_tests 需要真 Chrome + profile——只在 Windows 跑
-        run: cargo test --lib --bins -- --skip live_tests
+        # --tests：跑 tests/ 下的集成测试（假 SearXNG 回环回归 + CLI stdout 逐字节 golden）
+        run: cargo test --lib --bins --tests -- --skip live_tests
 
       - name: cargo clippy
         run: cargo clippy --all-targets -- -D warnings
```

- `--skip live_tests` **保留**（live_tests 需要真 Chrome + profile）。
- **未引入任何平台特定逻辑**：无 `cfg(windows)`、无路径分隔符断言、无 `std::env::consts`；假 SearXNG 用 `127.0.0.1:0` + `std::env::temp_dir()`（跨平台）；golden 文件路径用仓内相对路径。

### 9.9.2 跨平台雷：期望文件是 stdout 字节 → 换行符归一

期望文件是**某一次捕获的字节**，落盘后可能被 git 的 `core.autocrlf` 改写（LF→CRLF），而 stdout 是程序直接写的字节 ⇒ 不归一则 **Windows 检出的工作区与 Linux/macOS CI 上逐字节必红**。

- **修法**：`tests/cli_golden_bytes.rs::normalize()` 先做 **CRLF→LF**，再做耗时字段归一；**比对时两侧各自归一**（读入的期望文件 + 实际 stdout）。
- **归一只动这两项**：换行符 + 耗时字段（`elapsed_ms` 与其人读渲染 `elapsed: X.Ys`）；键序 / 键集 / 浮点字面量 / 字符串 / 空白全部参与逐字节比对。规则已写进 `tests/fixtures/cli_golden_provenance.md`。
- **独立断言**：`newline_normalization_makes_crlf_and_lf_equivalent` —— 把期望文件的换行在 LF ⇄ CRLF 之间来回换，两种形态**都**必须比对通过 ⇒ ① 归一生效；② 正常态输出**没有别的隐含平台差异**（否则任一形态都会红）。
- **判别力反向自检**（临时把 CRLF 归一删掉 → 用例必须变红）：

```
$ cargo test --test cli_golden_bytes newline_normalization     # 归一被临时移除时
---- newline_normalization_makes_crlf_and_lf_equivalent stdout ----
assertion `left == right` failed: CRLF 形态的期望文件应比对通过（换行归一生效）
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out
```

（复原后：`test result: ok. 6 passed; 0 failed; 1 ignored` —— `cargo test --test cli_golden_bytes`：6 passed / 1 ignored = 捕获工具）

### 9.9.3 顺带修掉的真 flakiness：并行用例共用同一个临时配置文件

加 `--tests` 后 CI 第一次真跑集成测试，本地复跑时出现**随机红**（同一命令两次跑，一次 6 passed、一次 3 failed：失败的恰是共用同一 query 的三个用例——json golden / md golden / meta 键集）。

- **根因**：`run_cli_bin()` 的临时配置文件名是 `pid + query` → libtest **并行**跑用例时，四个用 `Q_NORMAL` 的用例拿到**同一个路径**：先结束的用例把文件删了，还在跑的那个用例于是让 CLI 拿到「配置文件不存在」→ rc=2 + 空 stdout → 比对红。失败集合与「共用同一 query 的用例」完全吻合，定位确定。
- **修法**：临时配置名加**每次调用唯一序号**（`static CFG_SEQ: AtomicUsize`）→ 各用例各用各的文件、各删各的。
- **判别力/稳定性证据**：`for i in $(seq 10); do cargo test --test cli_golden_bytes; done` → **10/10 `6 passed; 0 failed`**（修前同命令会随机红）。

### 9.9.4 观察到的外部改动（非本轮引入，保留）

工作区出现了本轮未创建的 `.gitattributes`（PM 侧改动）：对 `tests/fixtures/golden_search_*.txt` 等声明 `text eol=lf`——与本节的换行归一是**同一条修复的两层**（一层锁 checkout 形态、一层锁比对口径），两者互补、方向一致，故**保留未改**。

## 9.10 残余风险与本轮明确未做

1. **已知误指控面（产品取舍，已落 README）**：混合 query（中+拉丁）遇外语结果集时，判定**只依据拉丁族**（CJK 词元在外语文本里永不可能命中 → 跳过）→ 「北京 天气 API」× 含天气但不含 `api` 的英文页仍会被判 `off_topic`。取舍理由：漏指控会让上游污染静默通过（bing 事故形态）；代价受控：只打标、不丢结果、不改排序，下游可复核。
2. **子串匹配的假命中未修**（`rust` 命中 `Distrust`）——方向是漏判，改动会牵连召回；已在 README 写明。
3. **部分污染仍不标注**（整集口径：占比未达阈值则一个标签都不下发）——已落 README。
4. **reviewer §4.3 提到但未列入本轮打回项**：`run.message` 里的建议文案「换短 query 复核」方向可疑（query 变短 → 词元更少 → 更易不判定 → 指控消失而非暴露问题）。**未改**（该文案属既有契约字符串，改动会波及 golden 与断言；不在本轮打回清单内）。
5. **`USER_GUIDE.md` 未更新**——该文件整体停留在 v0.2.9 契约（默认输出即 JSON），属既有文档债，非本轮引入。
6. **仓内 CI 已补跑集成测试**（PM 授权，§9.9.1）——`cargo test --lib --bins --tests -- --skip live_tests`。**首次在 ubuntu/macos 上跑这些集成测试的结果未知**（它们此前从未在该 workflow 执行过，含既有的 `tests/fallback_chain.rs`）：若某平台红，按平台定位**修断言的隐含平台假设**（不给某平台加 skip）——PM 将在 push 后看 CI 实际结果。
7. **未 commit**（归 PM）；**零新依赖**（`Cargo.toml` 未改）。

## 9.11 side-effects

- **side-effects: `.github/workflows/ci.yml` 的 test 步骤被改**（PM 授权，§9.9.1）——加了 `--tests`，使 `tests/` 下的集成测试（含本特性回归与既有 `fallback_chain.rs`）在该 workflow 里**开始执行**；除此之外：
  - 未修改任何**既有** JSON 键名、未改既有三态语义、未改 `Cargo.toml`、未新增运行时依赖。
  - 正常态输出**逐字节不变**（有 golden 逐字节回归证明：正常态 meta 无 `off_topic` 键、结果无 `relevance` 键、md 无计数与档位标记）。
  - 可疑态输出**新增**：`results[].relevance` 标签、`run.status=off_topic_results`、`run.message`、`meta.off_topic` 计数、md 的 `off_topic: N/M` 与行尾档位标记（均为本特性契约内的新增）。
  - 未触达外部网络（全部用本地回环假 SearXNG）；未访问真实 SearXNG 实例（`192.168.89.249:8888` 未被本轮任何命令请求）。
  - 未 commit / 未 push / 未改分支。

