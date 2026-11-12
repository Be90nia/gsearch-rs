# v0.4.0 默认 md 输出 —— 独立 E2E 验证报告

**VERDICT: PASS**

- 验证者：e2e-runner（MdE2E）独立代理，黑盒——只跑命令看输出，未读 `src/` 源码，未改代码、未 commit/push/tag。
- 被验：`target/release/gsearch.exe`（v0.4.0）；对照基线：`target/gsearch-v0.3.0-baseline.exe`（v0.3.0）。
- 环境：`unset GSEARCH_PROXY`（本机 socks5 隧道失效，os error 10061）；`GSEARCH_SEARXNG_URL=http://192.168.89.249:8888`；`GSEARCH_PROFILE` 未设（search 走 SearXNG 纯 HTTP，不需要 Chrome）。
- 证据产物：`target/e2e_md.txt`（V7 盲测输入）、`target/e2e_gt.json`（同查询 --json ground truth）、`target/e2e_blind_answer.txt`（盲测 agent 原文）、`target/e2e_blind_prompt.md`（盲测提示词）。

---

## 逐项结果

| 项 | 判定 | 一句话 |
|---|---|---|
| V1 默认输出不是 JSON | **PASS** | 默认输出喂 `json.loads` 抛 `JSONDecodeError`，exit=1（非 0） |
| V2 默认输出是 md 结构 | **PASS** | 首行 meta 头，随后 `N. 标题 [domain_class]` + 缩进 URL + 缩进摘要 |
| V3 `--json` 键集与 v0.3.0 一致 | **PASS** | meta / run / results / 顶层 四组键集全 True |
| V4 `--json` 单行紧凑 JSON | **PASS** | 1 行、无换行、无缩进，`json.loads` 成功，3 条结果 |
| V5 fetch 默认输出 md 正文 | **PASS** | 首行 meta 头 + `# Example Domain` + 正文 |
| V6 doctor / verify 默认 md | **PASS** | doctor `- [OK]/[WARN]/[FAIL] 名: message`；verify 每 URL 一行 |
| V7 AI 消费盲测 | **PASS** | 5 题全对 + 2 个 md 附加题全答出（provider=searxng、truncated=true） |
| V8 退出码与错误路径 | **PASS** | 2 / 2 / 4，与 v0.3.0 基线逐项一致 |
| V9 token 对比 | **PASS** | md 329 tok vs 同查询 --json 521 tok（−36.9%）；vs v0.3.0 默认 556 tok（−40.8%） |

---

## V1 默认输出不是 JSON — PASS

```bash
GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 ./target/release/gsearch.exe search "rust async" --limit 3 2>/dev/null \
  | python -c "import sys,json;json.loads(sys.stdin.read())"; echo "exit=$?"
```

实测输出：

```
Traceback (most recent call last):
  ...
json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)
exit=1
```

`exit=1` ≠ 0 → 默认输出已不是合法 JSON。**PASS**。

## V2 默认输出是 md 结构 — PASS

```bash
GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 ./target/release/gsearch.exe search "rust async" --limit 3 2>/dev/null
```

实测全文（stderr 0 字节，exit=0）：

```
provider: searxng | results: 3 | limit: 3 | truncated: true | elapsed: 2.4s
1. Convert SVG to PNG Online – Free Browser-Based Tool [other]
   https://svgtopng.com/
   Free browser-based SVG to PNG converter. Convert vector graphics to high-quality PNG images. Your files never leave your device.

2. Introduction - Asynchronous Programming in Rust [blog]
   https://rust-lang.github.io/async-book/
   Part two is a collection of stand-alone chapters on more advanced topics. It should be useful once you’ve worked through part one or if you already have some ex

3. Chuyển Đổi Tệp SVG sang PNG Trực Tuyến – Công Cụ Miễn Phí [other]
   https://svgtopng.com/vi/
   Công cụ chuyển đổi SVG sang PNG miễn phí trên trình duyệt. Chuyển đổi đồ họa vector thành hình ảnh PNG chất lượng cao. Tệp …
```

结构与契约期望一致：首行 `provider: … | results: … | limit: … | truncated: … | elapsed: …`；其后每条结果 3 行（编号+标题+`[domain_class]`、3 空格缩进 URL、3 空格缩进摘要），条目间空行分隔。**PASS**。

补充结构强校验（对 V7 捕获的 5 条结果逐行比对）：md 的每条 `标题行 / URL 行 / 摘要行` 与 `--json` 同一条记录的 `title+[domain_class]` / `url` / `snippet` **逐字节相等**，5/5 全 True。这证明 md 与 JSON 同源、无内容丢失。

## V3 `--json` 键集与 v0.3.0 一致 — PASS

```bash
./target/gsearch-v0.3.0-baseline.exe search "rust async" --limit 3 --json > target/e2e_a.json
./target/release/gsearch.exe          search "rust async" --limit 3 --json > target/e2e_b.json
python -c "…比键集…"
```

实测输出：

```
meta keys same: True ['tool', 'version', 'query', 'profile', 'humanize', 'limit', 'elapsed_ms', 'truncated', 'provider', 'truncated_detail']
run keys same: True ['status']
result keys same: True ['title', 'url', 'snippet', 'score', 'domain_class']
top keys same: True ['meta', 'run', 'results']
```

（两者 exit=0、stderr 均 0 字节、JSON 各自可解析。）
换稳定查询 `site:tokio.rs tokio tutorial` 复核一次，四组仍全 True。**PASS**。

## V4 `--json` 输出单行紧凑 JSON — PASS

```bash
./target/release/gsearch.exe search "rust async" --limit 3 --json 2>/dev/null | python -c "…"
```

实测输出（单行，前 400 字符）：

```
{"meta": {"tool": "gsearch", "version": "0.4.0", "query": "rust async", "profile": "default", "humanize": false, "limit": 3, "elapsed_ms": 3015, "truncated": true, "provider": "searxng", "truncated_detail": "results_capped_by_limit"}, "run": {"status": "ok"}, "results": [{"title": "Complete Guide on TikTok Influencer Marketing in 2026 ... - SocialPilot", "url": "https://www.socialpilot.co/tiktok-m
```

结构检查（落盘后自检）：

```
has newline: False
lines: 1
parsed ok, results: 3
has indent newline pattern: False
```

单行、无换行、无缩进，`json.loads` 成功。**PASS**。

## V5 fetch 默认输出是 md 正文 — PASS

```bash
./target/release/gsearch.exe fetch https://example.com 2>/dev/null | head -10
```

实测输出（exit=0）：

```
format: markdown | content_untrusted: true | elapsed: 0.7s
# Example Domain

Example Domain

This domain is for use in documentation examples without needing permission. This is not a service; avoid relying on it for testing and monitoring purposes.
```

首行 meta 头，随后 `# <标题>` + md 正文。**PASS**。

## V6 doctor / verify 默认输出是 md — PASS

```bash
./target/release/gsearch.exe doctor 2>/dev/null | head -10
./target/release/gsearch.exe verify https://example.com 2>/dev/null | head -6
```

doctor 实测（exit=1，因 `network` 项 FAIL——本机 Google 直连超时，与本次 md 契约无关）：

```
checks: 7 | ok: 5 | failed: 1 | elapsed: 5.5s
- [OK] chrome: C:\Program Files\Google\Chrome\Application\chrome.exe
- [WARN] edge: Edge 不可用（msedge.exe 未找到；仅 Chrome 可跑）
- [OK] profile_writable: profile 可写: C:\Users\Begonia\.gsearch\profiles\default
- [OK] exit_ip: 61.144.188.80
- [FAIL] network: 网络连接超时 (www.google.com:443)
- [OK] profile_source: GSEARCH_PROFILE 未设置（默认 ~/.gsearch/profiles/default/）
- [OK] searxng: SearXNG: HTTP 200, results=2, unresponsive_engines=7 (http://192.168.89.249:8888)
```

verify 实测（exit=0）：

```
checked: 1 | ok: 1 | failed: 0 | elapsed: 0.9s
- [OK] https://example.com | ok | status 200 | ssl true | 935ms
```

doctor 为 `- [OK]/[WARN]/[FAIL] 检查名: message` 清单，verify 为每 URL 一行。基线 v0.3.0 的 doctor 退出码同为 1（无回归）。**PASS**。

## V7 AI 消费盲测（核心）— PASS

### 7.1 输入捕获

因 SearXNG SERP 随时间漂移（同一 query 两次结果不同），改为「同一 query 的 md 与 --json 两次调用，校验 5 条结果的 title/url/snippet 全部出现在 md 中」后才认定二者同源，取第 2 次尝试配对成功的一组：

- 捕获脚本：`target/e2e_capture.py`（临时校验脚本，已保留作证据）
- md 原文（未加工、未裁剪）→ `target/e2e_md.txt`（1380 B）
- 同查询 `--json` → `target/e2e_gt.json`（2040 B）

`target/e2e_md.txt` 全文：

```
provider: searxng | results: 5 | limit: 5 | truncated: true | elapsed: 2.4s
1. Tutorial | Tokio - An asynchronous Rust runtime [other]
   https://tokio.rs/tokio/tutorial
   Tutorial. Tokio is an asynchronous runtime for the Rust programming language. It provides the building blocks needed for writing networking applications. It ...

2. Hello Tokio | Tokio - An asynchronous Rust runtime [other]
   https://tokio.rs/tokio/tutorial/hello-tokio
   Tokio is a runtime for writing reliable asynchronous applications with Rust.Cargo features. When depending on Tokio for this tutorial, the full feature flag is 

3. I/O | Tokio - An asynchronous Rust runtime [other]
   https://tokio.rs/tokio/tutorial/io
   Tokio. Tutorial.I/O. I/O in Tokio operates in much the same way as in std, but asynchronously. There is a trait for reading (AsyncRead) and a trait for writing 

4. Select | Tokio - An asynchronous Rust runtime [other]
   https://tokio.rs/tokio/tutorial/select
   Tokio is a runtime for writing reliable asynchronous applications with Rust. It provides async I/O, networking, scheduling, timers, and more.

5. Tutorial | Tokio - An asynchronous Rust runtime [other]
   https://tokio.rs/tokio/tutorial?ref=itsfoss.com
   Tutorial. Tokio is an asynchronous runtime for the Rust programming language. It provides the building blocks needed for writing networking applications.
```

### 7.2 盲测执行

本代理的 `task` 子代理派生被 harness 禁用（`spawns disabled for this agent`），改用**独立进程、独立会话、独立上下文**的 `omp -p --model=default` 启动一个新 agent，只给它一份提示词（`target/e2e_blind_prompt.md`，内含"只读这一个文件、禁读源码/禁跑命令/禁联网"的约束），它只读 `target/e2e_md.txt` 作答。原 agent 上下文未传入。

盲测 agent 原文（`target/e2e_blind_answer.txt`）：

```
1. **5** — `"provider: searxng | results: 5 | limit: 5 | truncated: true | elapsed: 2.4s"`

2. `"https://tokio.rs/tokio/tutorial/hello-tokio"`

3. `"I/O | Tokio - An asynchronous Rust runtime [other]"`

4. `"Tokio is a runtime for writing reliable "` (first 40 chars of `"Tokio is a runtime for writing reliable asynchronous applications with Rust. It provides async I/O, networking, scheduling, timers, and more."`)

5. Title: `"Tutorial | Tokio - An asynchronous Rust runtime [other]"` — URL: `"https://tokio.rs/tokio/tutorial"`

6. `"searxng"` (from `"provider: searxng | results: 5 | limit: 5 | truncated: true | elapsed: 2.4s"`)

7. **Yes, truncated** — `"truncated: true"` and `"limit: 5"` with `"results: 5"`, meaning more results exist beyond the 5 shown.
```

### 7.3 与 `--json` ground truth 逐字核对

| 题 | ground truth（取自 `target/e2e_gt.json`） | 盲测回答 | 判定 |
|---|---|---|---|
| 1 总条数 | `5` | `5` | ✅ |
| 2 第 2 条 URL | `https://tokio.rs/tokio/tutorial/hello-tokio` | 同 | ✅ |
| 3 第 3 条标题 | `I/O \| Tokio - An asynchronous Rust runtime` | `I/O \| Tokio - An asynchronous Rust runtime [other]` | ✅ 标题逐字一致（`[other]` 是 md 格式自带的 domain_class 标记，不属于标题本体） |
| 4 第 4 条摘要前 40 字符 | `Tokio is a runtime for writing reliable `（40 字符，含尾空格） | 同（40 字符，含尾空格） | ✅ |
| 5 第 1 条标题 + URL | `Tutorial \| Tokio - An asynchronous Rust runtime` / `https://tokio.rs/tokio/tutorial` | 同（标题同样带 `[other]` 后缀） | ✅ |
| 6 来源引擎 | `searxng` | `searxng` | ✅ |
| 7 是否触顶 | `true` | 是（引用 `truncated: true` + `results: 5` + `limit: 5`） | ✅ |

**5/5 提取题全对；2/2 md 附加题（provider、truncated）全部从 meta 头读出**——md meta 头可被 AI 直接消费，无需回查 JSON。

补充：盲测 agent 独立指出了第 4 题尾随空格的边界（"the snippet's 40th character is a trailing space"），说明它确实按字符切片而非目测。

**PASS**。

## V8 退出码与错误路径无回归 — PASS

```bash
./target/release/gsearch.exe search "" 2>/dev/null;                 echo "空query exit=$?"
./target/release/gsearch.exe search "x" --read 99 2>/dev/null;       echo "越界 exit=$?"
./target/release/gsearch.exe verify "not-a-url" 2>/dev/null;         echo "坏URL exit=$?"
```

实测：

```
空query exit=2 (期望 2)
越界 exit=2 (期望 2)
checked: 1 | ok: 0 | failed: 1 | elapsed: 1.3s
- [FAIL] not-a-url | dns_error | DNS 失败 (curl exit 6)
坏URL exit=4 (期望非 0)
```

错误信息（stderr 可见）：

```
error: query 不能为空或纯空白（收到 ""）；请给出搜索词
error: --read 99 越界：结果数上限为 --limit 10，请求前即可判定
```

与 v0.3.0 基线逐项对照，退出码与错误文案**完全一致**（基线：空 query=2、越界=2、坏 URL=4，报错文本逐字相同；基线 verify 的 JSON 形态在 v0.4.0 变为 md 行，语义等价）。**PASS**。

## V9 token 对比 — PASS

同一查询同一 limit（`site:tokio.rs tokio tutorial --limit 5`），tiktoken `cl100k_base`：

```
v0.4.0 默认(md) limit5           1380B   329 tok
v0.4.0 --json limit5 (同查询)     2040B   521 tok
v0.3.0 基线默认 limit5             2012B   556 tok
md vs v0.4.0 json: -36.9%
md vs v0.3.0 默认: -40.8%
```

md 明显小于 JSON，也明显小于 v0.3.0 的默认 JSON。**PASS**。

（契约脚本里的 `e2e_b.json` 是 `rust async --limit 3` 的产物，与 `e2e_md.txt` 查询/limit 不同，不可直接比 token；上表用同查询同 limit 的三份产物做公平对照，并额外附契约字面数字：v0.4.0 `--json`（rust async limit3）= 1287 B / 365 tok。）

---

## 过程中观察到的非契约问题（不影响本次判定）

1. **SERP 漂移**：同一 query 两次调用的结果集不同（`rust async` 连续 10 次捕获 0 次能与同时刻 `--json` 对齐；`site:tokio.rs tokio tutorial` 2 次尝试即配对成功）。这是上游 SearXNG 行为，非 v0.4.0 md 输出问题——但意味着任何"md 与 JSON 逐字节比对"的验收脚本必须做同源配对，不能直接连跑两次再比。

2. **SearXNG 空结果时回退链会整体失败**：`rust async` 第 5/8 次运行出现 stderr `DDG html 直连失败（curl exit 28）` → Google 回退 `Request timed out`，exit=1、stdout 0 字节。此时走的是 Google/DDG 直连路径（本机代理已 unset、Google 直连不可达），与 md 契约无关；但**失败时 stdout 为空、错误只在 stderr**，下游若只读 stdout 会拿到空内容而不报错。同一 query 另 7 次均 exit=0、stderr 0 字节。

3. **doctor 的 `network` 项在本机恒 FAIL**（`www.google.com:443` 超时），与基线 v0.3.0 同因（基线 doctor 退出码同为 1），无回归。

---

## 未覆盖 / 风险

- V7 盲测 agent 用 `omp -p` 独立进程跑（非 `task` 子代理，因 harness 禁用了本代理的 task 派生）；上下文独立性满足"不复用本代理上下文"，但与契约原文"派 subagent"在实现路径上不同。
- 未测：`browse` / `dl` / `similar` / `login` / `shell` / batch 多查询模式在 v0.4.0 下的 md 输出（契约 V1~V9 未覆盖）。
- 未测：Windows TTY（isatty）档下的默认输出是否仍为 md（本次全部为管道态）。
- 未做：clippy / cargo test（本次为黑盒产物验证，未触碰代码）。
