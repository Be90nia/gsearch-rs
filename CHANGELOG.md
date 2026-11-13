# Changelog

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)；版本遵循语义化版本。

## [Unreleased]

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
