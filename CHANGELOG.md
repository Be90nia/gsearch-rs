# Changelog

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)；版本遵循语义化版本。

## [Unreleased]

### Added

- **结果相关性自检与诚实标注（bd gsearch-rs-d8s）**：搜索结果与 query 零词元交集时**如实标出**，不再静默污染。实锤来源：SearXNG `bing` 引擎对中文查询返回完全无关的结果集（Bloomberg / 汇率 / 掏耳诊所），upstream [searxng#6937](https://github.com/searxng/searxng/issues/6937)（2026-11-13）
  - **判定**：query 词元（拉丁词 ≥3 字符去英文停用词 + 中文**不重叠 2-gram** 去中文停用二元组；中文查询通常无空格，不按空格分词）与每条结果 `title + snippet` 求交集 → `off_topic`（零交集）/ `weak`（未全覆盖，或结果文本与 query 不同族——**判不了 ≠ 判无关**，跨语言结果不冤枉）/ `ok`（全覆盖）；query 词元 < 2 条**不判定**（短 query 不冤枉）
  - **逐族判定（不制造「假零交集」）**：拉丁族与 CJK 族**分开**判定——某族在该条结果文本里不可比（如英文页里没有中文可比）→ 该族**跳过**，不拿它的零命中凑「零交集」（判不了 ≠ 判无关）；所有可比族都零交集 → `off_topic`；任一可比族有交集 → `weak`/`ok`（不指控）。跨文字族行（西里尔/希腊等）只要结果集里存在拉丁文本即同档判定，不再因字形族不同分档；整集无任何拉丁字母时拉丁族整体跳过（拉丁 query × 全俄语结果集不指控）
  - **整集判定**：off_topic ≥ 2 条**且**占结果集比例 ≥ `--relevance-threshold`（默认 `0.5`，可配 `0.0..1.0`，越界取值零网络 rc=2）→ 逐条下发 `relevance` 标签 + `run.status=off_topic_results`（**新增态**，与 `searxng_degraded` / `filtered_empty` / `no_results` 三态语义分立：有结果但结果无关 ≠ 没找到 ≠ 源故障）+ `run.message` 一行人读说明
  - **缺席语义**：未判可疑时 `relevance` 键**整体缺席**（不是 `null`）、`run.status` 仍 `ok`、md 行不追加标记 → **正常查询的 `--json` 输出与 v0.4.0 逐字节一致**（回归锁 `tests/cli_golden_bytes.rs`：跑**真实 CLI 二进制** + 本地回环假 SearXNG，stdout 归一后与仓内 golden 逐字节比对；golden 取自**本特性引入前**的 commit `ae1d595` release 二进制实跑输出；`tests/json_golden_compat.rs` 另锁信封序列化层）
  - **md 路径**：meta 头追加 `off_topic: N/M` 计数（>0 才出现，整行仍 ≤120 字符），结果行尾追加 `[off_topic]` / `[weak]` / `[ok]`；`--json` 路径对应计数在 `meta.off_topic`（**只在可疑时出现**，缺席语义；值 = 零交集条数，分母 = `len(results)`）
  - **只打标，不丢结果、不改排序**：首轮不做 rerank、不删 off_topic 条目（本地 rerank 已在 bd `8gp` 明确驳回）；可疑集退出码仍为 `0`（结果照给了，只是如实标注）
  - **batch 同规则**：单条 query 判可疑时该条目标 `status=off_topic_results`（结果照给 + 一行诊断），退出码语义不变
  - **回归数据**：`tests/fixtures/off_topic_searxng_20261113.json`（实锤 off_topic 正样本 40 条 → 判出率 **40/40**）+ `tests/fixtures/on_topic_control_20261113.json`（61 条真实查询对照组 → 输出级误报 **0**）；新增 `tests/relevance_off_topic.rs`、`tests/cli_golden_bytes.rs`
  - **零新依赖**：判定只用 std（无分词库、无模型、无 rerank）；release 二进制 10,101,248 B → 10,115,584 B（**+14,336 B ≈ 14 KB**）
  - **已知边界（实测）**：**已知误指控面（产品取舍）**——混合 query（中+拉丁）遇外语结果集时判定只依据拉丁族，含天气但不含 `api` 的英文页仍会被判 `off_topic`（取舍理由：漏指控会让上游污染静默通过；代价受控：只打标、不丢结果、不改排序，下游可复核）；部分污染（off_topic 占比未达阈值，如 1/10、4/10）**永不标注**；`shell` / `similar` 路径**不参与**标注；短词 / 版本号 / 标识符类 query（`rust`、`Go 语言`、`rust 1.75`、`C++`）**静默永不判定**；词元按子串匹配（不切词边界）→ `rust` 会命中 `Distrust`（方向是漏判，不修）；可疑集与人工 CAPTCHA 同时成立时 `run.message` 走 off_topic 分支（CAPTCHA 提示被吞，`run.captcha_solved` 独立字段仍在）

### Fixed

- **布尔 flag 显式值（跨项目实证 bug）**：此前所有布尔 flag 是 clap `SetTrue`，**空格形式的 `false` 会被当成第二个位置参数**——`gsearch search "x" --humanize false` 曾**静默变成 batch 搜索**（多查一个词、结果翻倍、不报错）；等号形式 `--humanize=false` 则报 "unexpected value"。成因是别的 agent 按 Python/argparse 习惯写 `--flag false`（证据：D:/Project/Xray-core-rust 项目会话原文 + v0.4.0 实测复现）。现全部布尔 flag 支持 `--flag` / `--flag true` / `--flag=true` / `--flag false` 四种写法（`num_args(0..=1)` + `default_missing_value("true")` + `ArgAction::Set`），裸 flag 语义不变；`--no-humanize false` = 不禁用（反向 flag 语义正确）。新增回归锁 `bool_flags_accept_explicit_values`（纯解析层、零网络）。⚠ 副作用：flag 写在查询词**前面**时（`search --humanize "x"`），查询词会被当作 flag 的值 → 明确报错；flag 写在查询词之后即可

## [0.4.0]

**breaking**：默认输出从 JSON 反转为 **md 排版**。`--json` 从「no-op 兼容占位」变为**显式 JSON 开关**。

### Changed（breaking）

- **默认输出 = md 排版**（v0.4.0 起）：stdout 第一行是 **meta 头**（`provider: searxng | results: 5 | limit: 5 | truncated: true | elapsed: 2.7s`，单行、` | ` 分隔、整行 ≤120 字符），其后是 md 正文
  - `search` = meta 头 + `N. 标题 [类别]` + 缩进 URL + 缩进摘要
  - `batch` = meta 头 + 每 query 一节（`## <query>` + 该 query 的结果列表）
  - `fetch` / `browse` = meta 头（含 `format` 源标签）+ `# <页面标题>` + 空行 + md 正文（复用既有 `--markdown` 转换器）
  - `verify` = meta 头 + 每 URL 一行检查结果
  - `doctor` = meta 头 + 每检查一行 `- [OK]/[WARN]/[FAIL]/[SKIP] <检查名>: <说明>`
- **`--json` 语义反转**：从「解析但无效果」的隐藏占位变为**显式 JSON 开关**（`--json` 输出与 v0.3.0 默认输出**逐字节一致**，零内容变更，只改入口）
- **`--human` 保留**：语义变为「显式要 md」（等价于默认），不再是「切换到另一种输出」；`--human` 与 `--json` 同传时 `--human` 优先（保 v0.3.0 兼容行为）。**v0.3.0 的人读对比表 / 报告块形态不再保留**（保留会与「默认 md」语义打架）——md 是唯一的非 JSON 形态
- **`truncated` 缺席语义沿用**：默认 md 路径下 `truncated` 只在 true 时出现在 meta 头；`--json` 路径的 `meta` 键集与缺席语义完全不变

### 迁移指引（存量脚本）

**只有「依赖默认 JSON」的脚本会破**，修法一行——加 `--json`：

```bash
# 破：v0.4.0 起 stdout 是 md，json.loads 会抛 JSONDecodeError
python -c "import json,sys;print(len(json.load(sys.stdin)['results']))" < <(gsearch search "rust async")

# 好：加 --json，输出与 v0.3.0 逐字节一致
python -c "import json,sys;print(len(json.load(sys.stdin)['results']))" < <(gsearch search "rust async" --json)
```

| 存量用法 | v0.4.0 行为 | 要不要改 |
|---|---|---|
| `gsearch search ... \| json.loads(...)` | stdout 变 md，解析报错 | **要改**：加 `--json` |
| `gsearch search ... --json \| json.loads(...)` | 与 v0.3.0 完全一致（`--json` 此前是 no-op 占位，现在真正生效） | 不用改 |
| `gsearch search ... --human` | 仍是 md 列表（等价于默认），仅多一行 meta 头 | 不用改 |
| `gsearch verify ... --human` / `gsearch doctor --human` | v0.3.0 的人读对比表 / 报告块形态被 md 形态取代（md 是唯一的非 JSON 形态） | 想保留旧表格输出请改用 `--json` 后自行排版 |
| AI/agent 直接把 stdout 拼进上下文 | 变成 md，更省 token | 不用改（更好） |

### Performance

- **默认输出省 token ~29%**（实测，tiktoken cl100k_base 精确计数，同一批 5 条结果只换渲染形态）：JSON 全量 **436 tok** → md 排版 **308 tok**；md + 1 行 meta 头 **337 tok**（−23%）
- **JSON 的浪费是结构性的**：每条结果固定多付 `"title":` `"url":` `"snippet":` `"domain_class":` 共 **47B ≈ 12 tok** 键名，条数越多亏得越多（limit 10 时 JSON 996 tok vs md 484 tok）
- **md 形态不损 AI 提取准确率**（独立 agent 盲测：各只读一种形态，答案由 PM 按 JSON 源逐字核对）：JSON 组 5/5 全对，md 组 5/5 全对且**答案逐字一致**（含带 `msockid` 的长 URL、40 字符截断边界）

### Fixed

- **`doctor` 的 SearXNG 探针超时贴边致间歇性误报**：探针超时 3s，而本机实例查询墙钟下界约 3.03s（引擎被上游熔断时 SearXNG 等满 `outgoing.request_timeout=3.0` 才回）→ 健康实例被报成「探测失败」（实测 4 跑 3 误报）。探针超时放宽到 8s，5 连跑全 OK

### Documentation

- **README 输出契约章节重写**：原「JSON 输出契约」章节（标注为默认）改为「输出契约（v0.4.0：默认 md）」，新增「默认 md 形态」小节（meta 头规格 + 各命令 md 形态表 + 省 token 实测与盲测结论），JSON 信封字段清单保留并下沉到「`--json` 路径」小节；「单行 JSON 消费姿势」→「消费姿势」（AI 直接吃 md / 要程序解析加 `--json`）；batch / browse / fetch / verify / doctor / similar 各段的默认输出描述同步改为 md
- **补 search 场景省 token 指引**：README 消费姿势章节新增第 4 姿势（`--limit` / `--snippet-len` / `--compact-meta` 组合）与实测杠杆排序表——条数 > 截断长度 > meta 精简（meta 只值 4-8%，snippet 占结果体积 60% 不可再省）。**零契约变更**：不改任何默认值（沿用「默认行为零破坏 > opt-in 新能力 > 省一点 token」拍板原则，v0.3.1 改默认值当天被 revert 的先例在案）
- **token 审计复盘追加本轮结论**（`docs/research/token-output-audit-2026-11-12.md`）：29% 省 token 实测、键名税（47B ≈ 12 tok/条）的结构性成因、盲测 5/5 结论、默认反转是用户拍板的决策记录

## [0.3.0] - 2026-10-10

安全审计硬化里程碑：全库三轮深度审计（安全 / 静默失败 / 测试缺口 / 内存 / 性能 / lean）后的集中修复批次。

### Security

- **修复 SSRF 门绕过 ×2**：`classify_url` 手工 host 切片不认 userinfo（`http://a.com:80@192.168.1.1/` 门判公网、实连私网）——改走 `reqwest::Url` 与连接层同一解析器；`is_private_ip` 补 IPv4-mapped IPv6（`[::ffff:192.168.x.x]`）与 CGNAT（100.64.0.0/10）判定
- **结果集导航门**：`search --browse N` 与 shell `click N` 的目标 URL 来自搜索结果 = 不可信输入，读前过 scheme 白名单 + 私网门；`search` 新增 `--allow-private`（语义与 fetch/browse 对齐）
- **shell 会话接门**：shell `browse` 过完整门（scheme + 私网），`login`/`dl` 过 scheme 白名单——此前 shell 会话完全绕过顶层门
- `meta.proxy` 输出信封凭据脱敏（`scheme://user:***@host`）
- `fetch_in_page` 页内 fetch base64 通道 Rust 侧长度复核（此前仅 JS 侧）
- `verify` curl 参数加 `--` 分隔与 `--globoff`（封死 URL 以 `-` 开头的注入面）
- `dl` 文件名命中 Windows 保留设备名（CON/PRN/AUX/NUL/COM1-9/LPT1-9）降级 `download.bin`
- `resolve_host` 支持 IPv6 字面量重定向（公网不再误拒，私网仍拒）

### Fixed

- **shell `read` 吞 CDP -32000 为空正文**（顶层路径已修、shell 路径漏网的 j44 同款事故模式）——改走 `content_retry` + 空正文 stderr `[hint]`
- **CDP evaluate 链无超时上界**：`content_retry`/`eval_string_retry`/`page_snapshot`/`fetch_in_page` 四处统一 30s 超时——恶意页同步死循环 JS 楔死 renderer 不再永久挂起 CLI
- **`search --browse N` / `browse` 对 head/nav 超重页面（>50KB）正文全空**：截断吃光正文时自动从源 HTML 尾部窗口重提取（内存有界）
- `swap_to_headed/headless` 失败路径旧 handler 未 abort 导致会话状态悬挂；shell REPL 异常早退跳过 `graceful_close` 残留 chrome.exe 持 profile 锁
- 重定向每跳 SSRF 门在 `fetch`/`dl`/`update` 间 Client 复用后语义不变（每 URL 独立过门）

### Performance

- `fetch`（含 batch）/`dl`/`update` 单命令单 `reqwest::Client`（此前每 URL×每 attempt 重建，丢同 host 连接复用）
- `search --read N` 免二次全页 DOM 序列化（captcha 检查正文带回复用）
- profile fork 跳过 Cache/Code Cache/GPUCache/Service Worker 等目录（fork 关键路径 I/O 降 1-2 个数量级）
- collapse 链三连拷与 `--json-keys` 路径深拷贝收敛（输出逐字节不变）

### Removed

- 死代码清理（-32 行）：`find_chrome`、未消费的 `SearchResult` re-export、恒为 0 的 `anchor_pad_lines` 旋钮、`browser_alive`/`swap_to_headed` 重复定义与透传层

### 文档

- README 增补：结果集导航门、`meta.proxy` 脱敏语义
- 测试补强：SSRF 门 userinfo/伪装用例、fork 跳过、CDP 超时注入、保留名降级、CGNAT/fe80 边界、尾部窗口补救、SearXNG collect 归因与回退链决策矩阵等定向测试
