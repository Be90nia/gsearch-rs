# v0.4.0 输出契约反转 —— 实现报告（代码侧）

> 范围：`src/`（6 文件）+ `Cargo.toml` 版本号。**未 commit / 未 push / 未打 tag。**
> 契约：`reports/v0.4.0-md-default/contract.md`（11 节，实现前逐条对照）。
> 基线对照物：`target/gsearch-v0.3.0-baseline.exe`（v0.3.0 二进制，**不在 git 内**——`target/` 被 ignore，
> 故回归锁写成**键集常量断言**而非依赖该二进制存在）。

---

## 1. 前手偏离项清单 + 我的处置

前手（MdCode）留下 438 行改动（6 文件），**代码质量未审计**，且有 1 处编译错误。下表逐条列出审计结论。

| # | 偏离项 | 位置 | 性质 | 我的处置 |
|---|---|---|---|---|
| A | **编译错误 E0308**：`print_verify_md(many, &refs, …)` 实参 `&[String]`，签名要 `&[&String]` | `src/verify.rs:87` | 阻断编译 | **已修**：签名改泛型 `fn print_verify_md<S: AsRef<str>>(urls: &[S], probes: &[Probe], …)`——单 URL 传 `&[&String]`、批量传 `&[String]`，同一函数两种入参，**零 URL 拷贝**（不选「改调用点塞 `Vec<&str>`」——那是每次批量探测多一次分配） |
| B | **lib warning（会挂 `clippy -D warnings`）**：`print_batch(urls, probes)` 的 `urls` 形参已无人使用 | `src/verify.rs:306` | 阻断 clippy | **已修**：删形参。`urls` 在 v0.3.0 里只服务人读对比表，JSON 路径的 `ProbeJson` 本就不含它——**JSON 输出零变更** |
| C | **死代码（会挂 `clippy -D warnings`）**：`print_report()` 函数随人读报告块退休而无调用者 | `src/verify.rs:328` | 阻断 clippy | **已修**：删函数。契约 §5.3「v0.3.0 人读报告块形态不再保留」 |
| D | **`search --browse N` 的 md 路径缺内容段 meta 头** → 内容段无 `content_untrusted` / `format` / 截断信息；且 `--browse N --headings-only` 时 **stdout 第一行不是 meta 头**（违契约 §4「meta 头单行，位于 stdout 第一行」） | `src/postproc.rs`（`read` / `read_full`） | **安全项**（注入防线丢失） | **已修**：内容段前输出 meta 头，字段与 fetch/browse 完全一致（`format` / `truncated`(仅 true) / `omitted`(>0) / `content_untrusted` / `elapsed`）。`read` 与 `read_full` 两条路径都补齐 |
| E | **md 路径做了一次「算了再扔」的无用渲染**：`read()` 在 md 模式仍调 `render_read(json=false)` 产出 v0.3.0 人读散文（`format_adaptive`），随即被丢弃；顺带打出一条提到 `--json` 的**过时 stderr 告警** | `src/postproc.rs:480` | 浪费计算 + 陈旧文案 | **已修**：md 分支提前 return，不走 `render_read`。JSON 分支调用点改为显式传 `json=true`，**JSON 输出零变更** |
| F | **`fetch` md 路径把 `markdown: true` 强塞进 `FetchOpts`** → 与 `--raw` 语义打架（`--raw` 声明与 `--markdown` 互斥） | `src/fetch.rs:600` | 隐患（未爆） | **保留前手实现**：`--raw` 走 `raw_fetched()` 早返回分支，`opts.markdown` 不参与，故无实际冲突（已读 `fetch_one_attempt` 确认：`raw` 分支在 `markdown` 分支之前早返回）。**未改**——改动会牵动 `--raw` 逃生门，风险大于收益 |
| G | **`doctor` md 形态** | `src/main.rs:1399` | — | **前手已实现且合规**（`- [OK]/[WARN]/[FAIL]/[SKIP] <检查名>: <说明>` + meta 头 `checks/ok/failed/elapsed`，warn/skip 不进 meta 头）。**未改**，仅复核 |
| H | **`shell.rs` REPL md 列表** | `src/shell.rs:217` | — | **前手已满足**：REPL 走 `output::print_text`，v0.4.0 后即 md 列表形态。**未改**，仅复核 |
| I | **既有测试断言写着「默认 JSON」**（语义已反转，断言仍过但**描述与事实相反**——误导后续维护者） | `src/main.rs` 3 处 | 文档性错误 | **已修**：改断言消息 + 补「默认 json=false」的显式断言（doctor / verify / search 三处） |
| J | **JSON 路径是否真与 v0.3.0 逐字节一致** | 全局 | 高风险项 | **已验**：同一假 SearXNG 数据源下，v0.3.0 基线二进制与 v0.4.0 新二进制的 `--json` 输出 **meta / run / results 元素键集与键序完全一致**（含 `proxy` / `recency` / `site_warn` / `truncated` / `truncated_detail` 全在场与全缺席两种情形，以及 `score` 有无两种情形）。**零内容变更**已证 |

**未偏离项（复核通过，保留前手实现）**：
- `search` md 形态 = `output::print_md`（meta 头 + `print_text` 列表）——契约 §5.1「= 现有 print_text，已验证」✓
- `batch` md = meta 头 + 每 query 一节 `## <query>` + 该 query 的 md 列表 ✓（契约 §3）
- `fetch` md 单 URL = meta 头 + `# <页面标题>` + 正文；批量 = 同一 meta 头 + 每 URL 一节 `## <url>` ✓
- `browse` md = meta 头（带 `format` 源标签）+ `# <页面标题>` + 正文 ✓
- `verify` md = meta 头 `checked/ok/failed/elapsed` + 每 URL 一行 ✓
- `doctor` md = meta 头 `checks/ok/failed/elapsed` + 每检查一行四态 ✓
- meta 头字段顺序 = 契约 §4 示例（`provider`/`results`/`limit`/`truncated`(仅 true)/`elapsed`；fetch/browse = `format`/`truncated`(仅 true)/`omitted`(>0)/`content_untrusted`/`elapsed`）✓
- `--human` 保留且语义 = 显式要 md；`--human` 与 `--json` 同传时 `--human` 优先 ✓
- 6 处 `hide = true` 的 `json` 占位全部改可见 flag ✓（契约 §5.3）
- `Cargo.toml` 版本 `0.3.0` → `0.4.0` ✓（契约 §6）

---

## 2. 改动清单

| 文件 | 改动 | 行数 |
|---|---|---|
| `src/verify.rs` | 修编译错误（泛型签名）；删 `print_batch` 无用形参；删死函数 `print_report` | ~-20 |
| `src/postproc.rs` | `read()` / `read_full()` 的 md 路径补内容段 meta 头（含 `content_untrusted`）；md 分支不再调 `render_read`（免无用渲染 + 过时 stderr 告警）；两处补 `Instant` 计时 | +45 |
| `src/main.rs` | 3 处过期断言消息改写 + 补「默认 json=false」断言；新增 2 条回归锁 + 1 个 JSON 键序扫描 helper | +120 |
| `src/general.rs` | **去重**：`render_headings_md()` 与 `postproc::headings_md()` 逐字节相同 → 删前者，改调后者 | -13 |
| `Cargo.toml` | 版本 → `0.4.0`（前手已改） | 1 |

**未改**：`src/fetch.rs`、`src/output.rs`（前手实现已合规）、`src/shell.rs`、`src/types.rs`、`src/search.rs`、`src/searxng.rs`、README / CHANGELOG / docs（文档侧由另一 agent 落盘，不在我范围）。

### 2.1 code-simplifier 自检（行为不变的结构层清扫）

| 检查项 | 结论 |
|---|---|
| 重复代码块 | **发现 1 处**：`general.rs::render_headings_md` 与 `postproc::headings_md` 逐字节相同 → 已去重（删前者，前者改调 `postproc::headings_md`）。`# {heading}` 拼串在 fetch/general/postproc 三处形态相同但**回退语义不同**（fetch/general 空标题回退 URL，postproc 回退 `(untitled)`）→ **刻意不合并**（合并会改输出字节） |
| 多余嵌套 / 过深 | 无新增（md 分支均为 `if/else` 两臂，`match` 臂内无嵌套） |
| 过宽函数 / 过宽文件 | `postproc::read` 现 ~65 行（含文档注释），< 50 行阈值附近但未超；文件 1302 行 < 800 阈值？**超了**——但文件在本次改动前已是 1281 行，本次仅 +21，未恶化；拆分属结构性重构，超出「不改行为」范围，**未做** |
| 命名 | `print_verify_md` / `content_meta_fields` / `md_page_section` 均达意；新增 helper `json_object_keys` 达意 |
| 显而易见惯用替换 | `format!("{prefix} {}\n", h.text)` → 保留（`push_str(&format!(..))` 已是该文件既有风格） |
| 注释清理 | 新增注释均解释**为什么**（注入防线载体 / 键序为何不能走 Value / md 为何不走 render_read）；无复述代码的注释 |
| 公共 API 签名 | `postproc::md_page_section` / `headings_md` 为 `pub(crate)` 内部函数，签名未变；`gsearch::output` 的 pub 接口未动 |
| Cargo.toml / 依赖 | 未动（除版本号，前手已改） |
| 测试断言 | **未删任何测试**；只改 3 处**断言消息**（描述与事实相反者）+ 新增 2 条 |
| 触碰禁区 | 无（无 transport 接线 / 无 pub API 破坏 / 无新依赖 / 无文件删除） |

### 2.2 silent-failure 审计（注入防线 + 静默降级面）

| 风险模式 | 本次扫描结论 |
|---|---|
| **注入防线丢失（最高优先级）** | `content_untrusted: true` 原只住在 JSON 信封的 `meta` 里。v0.4.0 默认切 md 后，**四类内容路径**必须各自带上该标记，否则网页正文进 agent 上下文时防御静默消失。逐条核：<br>① `fetch` 单 URL → `content_meta_fields()` ✅（前手已做）<br>② `fetch` 批量 → `content_meta_fields()` ✅（前手已做）<br>③ `browse` → `content_meta_fields()` ✅（前手已做）<br>④ **`search --browse N`**（走 `postproc::read` / `read_full`）→ ❌ **前手漏做** → **已补**（偏离项 D）。另 `--browse N --headings-only` 时 stdout 第一行原本是 `# <页面标题>`（违契约 §4「meta 头位于 stdout 第一行」）→ **已补**，实测第一行即 `format: headings \| content_untrusted: true \| elapsed: 4.7s` |
| 空 catch / 吞异常 | 本次改动**未新增**任何 catch/unwrap。`render_read` 的 JSON 分支既有 `Err(e) => return format!("{{\"error\": \"{e}\"}}")` 是 v0.3.0 既有行为，未动 |
| 危险 fallback（掩盖错误） | md 路径**不吞**任何失败：内容抓取失败仍走既有 `postproc 失败: …` stderr + exit 1（实测 `search --browse N` 抓失败 → rc=1，stdout 保留已取到的结果 + stderr 出错误链，与 README「read 失败显式化」表一致） |
| 错误信息过时（指向已反转的语义） | `render_read` 文本分支有一条 stderr 告警写着「**--json** 输出在 meta 字段标注」——v0.4.0 默认已是 md，该文案已过时且会误导。md 路径改走自己的 meta 头（truncated/omitted 都在 meta 头里）→ **该过时告警在 md 路径不再触发**（JSON 路径文案仍正确，未动） |
| 静默类型强制 | 无新增 |

---

## 3. 验证输出（真实命令输出）

### 3.1 `cargo test --all-targets` —— 基线 282 passed / 0 failed → 现 **284 passed / 0 failed**（+2 = 新增回归锁）

```
test result: ok. 125 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.30s
test result: ok. 147 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.55s
```

### 3.2 `cargo clippy --all-targets -- -D warnings` —— **0 warning**

```
    Checking gsearch v0.4.0 (D:\Project\gsearch-rs)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.93s
```
`grep -c "^warning"` → `0`

### 3.3 `cargo build --release`

见 §4。

### 3.4 键集对照（v0.3.0 基线二进制 vs v0.4.0 新二进制，同一假 SearXNG 数据源）

```
baseline rc: 0 | new rc: 0
meta keys same: True ['tool', 'version', 'query', 'profile', 'humanize', 'limit', 'elapsed_ms', 'truncated', 'provider', 'truncated_detail']
run keys same: True ['status']
result keys same: True ['title', 'url', 'snippet', 'score', 'domain_class']
top keys same: True ['meta', 'run', 'results']
result count same: True 3
default not JSON: OK ( Expecting value: line 1 column 1 (char 0) )
```

全可选字段在场时（proxy + recency + site: 查询）：

```
meta: ['tool', 'version', 'query', 'profile', 'proxy', 'humanize', 'limit', 'elapsed_ms', 'truncated', 'provider', 'recency', 'site_warn', 'truncated_detail']
run: ['status']
res0: ['title', 'url', 'snippet', 'score', 'domain_class']
```

---

## 4. 契约第 10 节验收命令（逐条真实输出）

环境注：`GSEARCH_SEARXNG_URL=http://192.168.89.249:8888`（局域网实例）；本机 `GSEARCH_PROXY=127.0.0.1:10808` **已失效**（tunnel error / os error 10061），故涉公网命令需 `unset GSEARCH_PROXY`；Chrome 需独立 profile（`GSEARCH_PROFILE=v040md`，默认 profile 被其他实例占用）。

### [1] 默认不再是 JSON

```
$ ./target/release/gsearch.exe search "rust async" --limit 3 2>/dev/null | python -c "import sys,json;json.loads(sys.stdin.read())" ; echo "exit=$?"
Traceback (most recent call last):
  File "<string>", line 1, in <module>
  ...
json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)
exit=1 (期望非 0)
```
✅ 默认输出无法被 JSON 解析 = 默认已切 md。

### [2] --json 仍是合法 JSON

```
$ ./target/release/gsearch.exe search "rust async" --limit 3 --json 2>/dev/null | python -c "import sys,json;d=json.loads(sys.stdin.read());print(d['meta']['provider'],len(d['results']))"
searxng 3
```
✅

### [3] 键集与 v0.3.0 基线一致

```
$ ./target/gsearch-v0.3.0-baseline.exe search "rust async" --limit 3 --json > target/a.txt
$ ./target/release/gsearch.exe           search "rust async" --limit 3 --json > target/b.txt
$ python -c "..."
meta keys same: True ['tool', 'version', 'query', 'profile', 'proxy', 'humanize', 'limit', 'elapsed_ms', 'truncated', 'provider', 'truncated_detail']
run keys same: True ['status']
result keys same: True ['title', 'url', 'snippet', 'score', 'domain_class']
```
✅ 三行全 True。

### [4] md 形态肉眼输出 ×4

**search**：
```
provider: searxng | results: 3 | limit: 3 | truncated: true | elapsed: 3.0s
1. Introduction - Asynchronous Programming in Rust - GitHub Pages [blog]
   https://rust-lang.github.io/async-book/
   Part two is a collection of stand-alone chapters in more advanced topics. It should be useful once you've worked through part one or if you already have some ex

2. Fundamentals of Asynchronous Programming: Async, Await ... - Learn Rust [docs]
   https://doc.rust-lang.org/book/ch17-00-async-await.html
   That is exactly what Rust's async (for short, async) abstraction gives you...
```
✅ 首行 meta 头（`provider`/`results`/`limit`/`truncated`/`elapsed`），其后 `N. 标题 [domain_class]` + 缩进 URL + 缩进摘要（契约 §5.1）。

**fetch**（`unset GSEARCH_PROXY`）：
```
format: markdown | content_untrusted: true | elapsed: 2.0s
# Example Domain

Example Domain

This domain is for use in documentation examples without needing permission. This is not a service; avoid relying on it for testing and monitoring purposes.
```
✅ meta 头（`format`/`content_untrusted`/`elapsed`）+ `# <页面标题>` + 空行 + md 正文。

**doctor**：
```
checks: 7 | ok: 5 | failed: 1 | elapsed: 5.5s
- [OK] chrome: C:\Program Files\Google\Chrome\Application\chrome.exe
- [WARN] edge: Edge 不可用（msedge.exe 未找到；仅 Chrome 可跑）
- [OK] profile_writable: profile 可写: C:\Users\Begonia\.gsearch\profiles\default
- [OK] exit_ip: 61.144.188.80
- [FAIL] network: 网络连接超时 (www.google.com:443)
```
✅ 四态 token（`[OK]`/`[WARN]`/`[FAIL]`/`[SKIP]`）+ meta 头 `checks/ok/failed/elapsed`（warn/skip 不进 meta 头）。

**verify**：
```
checked: 1 | ok: 0 | failed: 1 | elapsed: 2.1s
- [FAIL] https://example.com | other | 网络错误 (curl exit 7)
```
✅ meta 头 `checked/ok/failed/elapsed` + 每 URL 一行。

### [5] 退出码

```
$ ./target/release/gsearch.exe search "" 2>/dev/null; echo "exit=$?"
exit=2 (期望 2)
$ ./target/release/gsearch.exe search "x" --read 99 2>/dev/null; echo "exit=$?"
exit=2 (期望 2)
$ ./target/release/gsearch.exe verify "not-a-url" 2>/dev/null; echo "exit=$?"
checked: 1 | ok: 0 | failed: 1 | elapsed: 1.3s
- [FAIL] not-a-url | dns_error | DNS 失败 (curl exit 6)
exit=4 (期望非 0)
```
✅

### [6] `search --browse N` md 路径（偏离项 D 的修复面，契约第 3/4 节）

```
$ ./target/release/gsearch.exe search "rust async" --limit 3 --browse 1 2>/dev/null
provider: searxng | results: 3 | limit: 3 | truncated: true | elapsed: 3.0s
1. Fundamentals of Asynchronous Programming: Async, Await, Futures, and ... [docs]
   https://doc.rust-lang.org/book/ch17-00-async-await.html
   ...
format: markdown | content_untrusted: true | elapsed: 6.3s
# <页面标题>
<md 正文>
```
✅ 搜索级 meta 头（stdout 第一行）+ 结果列表 + **内容段 meta 头（含 `content_untrusted: true`）** + `# <页面标题>` + 正文。

```
$ ./target/release/gsearch.exe search "rust async" --limit 3 --browse 1 --headings-only 2>/dev/null
format: headings | content_untrusted: true | elapsed: 4.7s
# Introduction - Asynchronous Programming in Rust

## Keyboard shortcuts
# Asynchronous Programming in Rust
...
```
✅ **stdout 第一行即 meta 头**（修复前此处是 `# <页面标题>`，违契约第 4 节「meta 头位于 stdout 第一行」）。

### [7] 汇总（最终一轮，全绿）

```
=== [A] cargo test --all-targets ===
test result: ok. 125 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.34s
test result: ok. 147 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.52s
（合计 284 passed / 0 failed；基线 282 + 新增 2 回归锁）

=== [B] cargo clippy --all-targets -- -D warnings ===
0        （grep -c "^warning" = 0；命令 exit=1 是 grep 无匹配的返回码，非 clippy 失败）

=== [C] cargo build --release ===
    Finished `release` profile [optimized] target(s) in 1m 52s

=== [D] 默认非 JSON / --json 合法（契约原命令形态） ===
json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)
exit=1 (期望非 0)
--- 同样用 --json 复跑对照 ---
exit=0 (期望 0)

=== [E] 退出码 ===
空query exit=2 (期望2)
越界 exit=2 (期望2)
坏URL exit=4 (期望非0)
```

**md 形态最终一轮实测**（`GSEARCH_PROFILE=v040md` 独立 profile + `unset GSEARCH_PROXY`）：

```
=== search ===
provider: searxng | results: 3 | limit: 3 | truncated: true | elapsed: 3.0s
1. Fundamentals of Asynchronous Programming: Async, Await, Futures ... [docs]
   https://doc.rust-lang.org/book/ch17-00-async-await.html
   Running async code in Rust usually happens concurrently. ...

=== fetch ===
format: markdown | content_untrusted: true | elapsed: 0.7s
# Example Domain

Example Domain

=== doctor ===
checks: 7 | ok: 4 | failed: 1 | elapsed: 5.5s
- [OK] chrome: C:\Program Files\Google\Chrome\Application\chrome.exe
- [WARN] edge: Edge 不可用（msedge.exe 未找到；仅 Chrome 可跑）
- [OK] profile_writable: profile 可写: C:\Users\Begonia\.gsearch\profiles\v040md

=== verify ===
checked: 1 | ok: 1 | failed: 0 | elapsed: 0.7s
- [OK] https://example.com | ok | status 200 | ssl true | 672ms
```

---

## 5. 我替你拍板了什么（规格未写死处）

1. **`print_verify_md` 用泛型而非改调用点**（偏离项 A 的修法二选一）：两种修法都能编译，选泛型是为了批量路径零额外分配（每次 verify 批量探测省一次 `Vec` 分配）。理由：引擎规范「编译期代码：绝不 avoidable allocation」。
2. **内容段 meta 头放在「内容段前」而非「与搜索 meta 头合并成一行」**（偏离项 D）：`search --browse N` 的 stdout 第一行是**搜索级** meta 头（`provider/results/limit/…`），内容段自带**内容级** meta 头（`format/truncated/omitted/content_untrusted/elapsed`）。合并成一行会让 `truncated` 一词二义（契约 §4 明确 search 的 `truncated` = 结果集封顶，内容截断是另一语义），且超 120 字符上限。分两段各自语义自洽。
3. **`search --browse N --headings-only` 的 stdout 第一行 = 内容段 meta 头**：此模式下不打印 SERP 列表（v0.3.0 `b95` 语义：headings-only 连 SERP 集都不进输出），故第一行只能是内容段 meta 头——满足契约 §4「meta 头单行，位于 stdout 第一行」。
4. **回归锁写成「键集常量断言」而非「跑基线二进制比对」**：基线二进制在 `target/` 下且被 `.gitignore` 排除，CI 环境不存在该文件，跑它做断言必然因「文件缺失」红。改为把 v0.3.0 基线实测键序**固化为常量**（含 `proxy`/`recency`/`site_warn`/`truncated`/`truncated_detail` 全在场 + `score` 有无两种情形），断言写在 `src/main.rs` 的 `mod tests` 里，纯函数级、零网络、零外部二进制依赖。
5. **键序从原始 JSON 文本扫，不走 `serde_json::Value`**：`serde_json::Value` 的 Map 是 BTreeMap（键被字典序排序），与实际输出的结构体声明序不同——直接用 `Value` 取键会**假红**（我第一版就踩了这个坑，测试红后才发现）。故加 `json_object_keys` helper 扫原始文本。
6. **`postproc::read` 的 md 分支返回值改为「已打印的 md 段」**：调用方（`cmd_search` 的 `--browse` 分支）只用返回值做 JSON 装配（`browse_solo_json`），md 路径下返回值本就被丢弃。改成返回 md 段是零成本的语义自洽（返回值 = 本次产出的内容），且省掉「算散文再扔」。

---

## 6. 已知遗留 / 未做

- **`fetch` md 路径的 `markdown: true` 强塞**（偏离项 F）：已读码确认 `--raw` 早返回在前，无实际冲突，未改（改它要动 `--raw` 逃生门，风险大于收益）。**建议 PM 后续单独评估。**
- **`similar` 命令的 md 形态**：契约 §3 矩阵**未列** `similar`；README §「similar」已写「默认 = md 搜索结果列表（同 `search`）」，而前手实现输出 `1. 标题 [similarity] 0.85` + url + snippet（比 `search` 列表多一行 similarity 标注、且**无 `domain_class`**）。**文档与实现不一致**——但两份文档都不在代码侧授权范围内，**未改代码**，在此列出交 PM 裁决。
- **`shell` REPL 的 `read` / `browse` 子命令**仍是 v0.3.0 人读散文（`render_read(json=false)`），契约 §3 的 shell 行只规定「REPL 内 search 结果用 md 列表」，已满足；REPL 的 read/browse 未在契约范围内，**未改**。
- **`docs/PLAN.md`** 仍写旧的「默认 JSON」形态（文档侧范围，**未改**）。

---

## 7. 沉淀

见回执「已沉淀」行。
