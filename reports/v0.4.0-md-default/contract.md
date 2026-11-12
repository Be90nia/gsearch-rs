# v0.4.0 输出契约反转：默认 md，--json 显式 JSON

> 本文件是**实现契约**，执行者开工必读全文。规格逐条写死，**禁止自由发挥**。

## 1. 目标

gsearch v0.4.0 把默认输出从 JSON 反转为 **md 排版**；`--json` 变成显式要 JSON 的开关。

用户拍板原话：「你也别加什么 --md 了..直接就所有数据就是 md 不就好了么?」

## 2. 实测依据（已实证，别再重新论证方向）

- 同源严格对照（同一批 5 条结果，只换渲染形态，`tiktoken` cl100k_base 精确计数）：
  JSON 全量 **436 tok** / md 排版 **308 tok**（**−29%**）/ md + 1 行 meta **337 tok**（−23%）。
- JSON 的浪费是结构性的：每条结果固定多付 `"title":` `"url":` `"snippet":` `"domain_class":` 共 **47B ≈ 12 tok** 键名，条数越多亏得越多。
- 独立 agent 盲测（各只读一种形态，答案由 PM 按 JSON 源逐字核对）：JSON 组 5/5 全对，md 组 5/5 全对且**答案逐字一致**（含带 `msockid` 的长 URL、40 字符截断边界）。→ **md 形态不损 AI 提取准确率**。
- 现有 `--human`（search）已是 md 排版且经上述盲测验证 → **search 的 md 形态就是现有 print_text 输出，不要重新设计**。

## 3. 默认输出形态矩阵

| 命令 | 默认（md） | `--json`（显式） |
|---|---|---|
| `search` | meta 头 1 行 + md 结果列表 | 原 JSON 信封 `{meta,run,results}` |
| `batch` | meta 头 + 每 query 一节（`## <query>` + 该 query 的 md 列表） | 原 JSON 数组 |
| `fetch` | meta 头 + `# <页面标题>` + md 正文 | 原 JSON 信封 |
| `browse` / `search --browse N` | meta 头 + `# <页面标题>` + md 正文 | 原 JSON 信封 |
| `verify` | meta 头 + 每 URL 一行检查结果 | 原 JSON |
| `doctor` | meta 头 + `- [OK]/[FAIL] <检查名>: <message>` 清单 | 原 JSON |
| `dl` | 沿用现状（下载文件，无正文输出） | 原 JSON |
| `shell` | REPL 内 search 结果用 md 列表 | — |

## 4. meta 头硬规格

- **单行**，位于 stdout **第一行**（stderr 只放诊断行，禁止混流）。
- 格式：`<key>: <value> | <key>: <value>`，字段用 ` | ` 分隔。
- search/batch 示例：`provider: searxng | results: 5 | limit: 5 | truncated: true | elapsed: 2.7s`
  - `provider`：search/batch 有；其他命令用适用字段（**不要**编造 provider 语义）。
  - `results`：本次返回条数。
  - `limit`：本次生效的 limit。
  - `truncated`：**只在 true 时出现**（false 是默认态，缺席 = 未截断，与现有 JSON 缺席语义一致）。
  - `elapsed`：保留 1 位小数秒（`2.7s`）。
- fetch/browse：`truncated`(true 时) | `omitted`(>0 时) | `elapsed`。
- verify：`checked` | `ok` | `failed` | `elapsed`。
- doctor：`checks` | `ok` | `failed` | `elapsed`。
- **整行 ≤ 120 字符**。

## 5. 各形态细节

### 5.1 search md 列表（= 现有 `output::print_text`，已验证）

```
1. 标题 [domain_class]
   https://example.com/
   摘要前 160 字符
```

- 每条后空行（现有行为）。
- snippet 截断语义**不变**：`--snippet-len`，默认 160。

### 5.2 fetch / browse md 正文

- 复用现有 `crate::convert::html_to_markdown`（`--markdown` 路径已有该能力，**不要另写转换器**）。
- 结构：`# <页面标题>` + 空行 + 正文。
- `--full`（innerText 源）与 md 源的差别在 meta 头体现（如 `format: full` / `format: markdown`）。
- 正文截断语义不变（`--max-chars`，`meta.truncated` / `omitted` 语义照旧，只是搬到 md 的 meta 头）。

### 5.3 `--json` 语义反转

- src/main.rs 有 **6 处** `#[arg(long, hide = true)] json: bool` 占位（注释写着「兼容占位：JSON 已是默认输出，此 flag 解析但无效果」）。
- 全部改为**可见 flag**，语义 = 强制 JSON 输出。
- **JSON 输出内容必须与 v0.3.0 逐字节一致**（零内容变更，只改默认开关）。
- `--human` 保留，语义 = 显式要 md（等价于默认），不再是"切换到另一种"。

## 6. 版本

- Cargo.toml `version = "0.3.0"` → `"0.4.0"`（breaking 变更）。
- **不要**打 tag、不要推 release、不要 commit（PM 负责）。

## 7. 测试

- 现有测试全部适配新默认（断言 JSON 输出的改为走 `--json` 路径）。
- 新增回归锁：
  - 默认输出**不是**合法 JSON（证明默认已切）。
  - `--json` 输出的 **meta 键集 / run 键集 / results 元素键集** 与 v0.3.0 完全一致（对照基线二进制 `target/gsearch-v0.3.0-baseline.exe`，**只比键集不比结果内容**——SERP 内容随时间漂移，逐字节比必假失败）。
- `cargo test --all-targets` 全绿（当前基线 282 passed / 0 failed）。
- `cargo clippy --all-targets -- -D warnings` 0 warning。

## 8. 文档（仓库内文件，PM 负责 skill）

- **README.md**：输出契约章节重写（默认 md + `--json` 显式）+ 消费姿势更新（AI 直接吃 md；要程序解析加 `--json`）+ 退出码章节不动。
- **CHANGELOG.md**：新增 `## [0.4.0]` breaking 条目——默认输出反转 + **迁移指引**（存量 `json.loads` 脚本必须加 `--json`）。
- **docs/research/token-output-audit-2026-11-12.md**：追加本轮实测结论（29% 数字 + 盲测 5/5）。

## 9. 非目标（禁止做）

- 不改网络/搜索/provider 路径，不动 `src/searxng.rs`。
- 不改 `--max-chars` / `--snippet-len` / `--limit` / `--compact-meta` 语义。
- 不新增 flag（用户明确否决了 `--md`）。
- **不动** `~/.omp/agent/skills/gsearch-cli/SKILL.md`（PM 自己改）。
- 不 commit、不 push、不打 tag。

## 10. 验收命令（可执行，逐条贴输出）

```bash
cd D:/Project/gsearch-rs
cargo test --all-targets 2>&1 | grep "^test result"          # 全部 ok / 0 failed
cargo clippy --all-targets -- -D warnings 2>&1 | grep -c "^warning"   # 0
cargo build --release 2>&1 | tail -1

# 默认不再是 JSON
./target/release/gsearch.exe search "rust async" --limit 3 2>/dev/null | python -c "import sys,json;json.loads(sys.stdin.read())" ; echo "exit=$? (期望非 0)"
# --json 仍是合法 JSON
./target/release/gsearch.exe search "rust async" --limit 3 --json 2>/dev/null | python -c "import sys,json;d=json.loads(sys.stdin.read());print(d['meta']['provider'],len(d['results']))"
# JSON 键集与 v0.3.0 基线一致
./target/gsearch-v0.3.0-baseline.exe search "rust async" --limit 3 --json > target/a.txt 2>/dev/null
./target/release/gsearch.exe search "rust async" --limit 3 --json > target/b.txt 2>/dev/null
python -c "
import json
a=json.load(open('target/a.txt'));b=json.load(open('target/b.txt'))
print('meta keys same:', list(a['meta'])==list(b['meta']))
print('run keys same:', list(a['run'])==list(b['run']))
print('result keys same:', list(a['results'][0])==list(b['results'][0]))
"
# md 默认形态肉眼可查
./target/release/gsearch.exe search "rust async" --limit 3 2>/dev/null | head -8
./target/release/gsearch.exe fetch https://example.com 2>/dev/null | head -5
./target/release/gsearch.exe doctor 2>/dev/null | head -6
./target/release/gsearch.exe verify https://example.com 2>/dev/null | head -5
```

## 11. 落盘

- 验收输出与自检报告写到 `reports/v0.4.0-md-default/`（本目录）。
- 代码改动在仓库工作区，**不 commit**。
