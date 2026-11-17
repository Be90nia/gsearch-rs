# llms.txt + CLAUDE.md 交付报告（bd gsearch-rs-4n4② / gsearch-rs-3vu）

**结论**：两份产物完成，双验收全过。`VERDICT: PASS`

日期：2026-11-17 ｜ 执行：LlmsAndClaudeDoc ｜ 目标：`llms.txt`（repo 根）、`CLAUDE.md`（repo 根，新建）

---

## 0. 交付物

| 文件 | 行数 | 字节 | 状态 |
|---|---|---|---|
| `D:/Project/gsearch-rs/llms.txt` | **115**（目标 100–160） | 8,309 | 新建 |
| `D:/Project/gsearch-rs/CLAUDE.md` | **118** | 7,261 | 新建 |
| `D:/Project/gsearch-rs/reports/2026-11-17-llms-txt-and-claude-md.md` | — | — | 本报告 |

产品代码零改动（`src/`、`tests/` 的工作树改动来自并行 agent，非本任务）。

---

## 1. llms.txt 核对表（逐条对照 `target/release/gsearch.exe` 实测）

二进制版本：`gsearch 0.4.1 (git:v0.4.1-6-g4eb9caf)`（`./target/release/gsearch.exe --version` 实跑）。

### 1.1 子命令（10 个，help 全量抓取后逐条比对）

| llms.txt 中出现的命令 | `--help` 中存在 | 处置 |
|---|---|---|
| `search` | ✓ | 写 |
| `browse`（别名 `read`） | ✓ `[alias: read]` | 写 |
| `fetch` | ✓ | 写 |
| `verify` | ✓ | 写 |
| `doctor` | ✓ | 写 |
| `login` | ✓ | 写 |
| `dl` | ✓ | 写 |
| `shell` | ✓ | 写 |
| `similar` | ✓ | 写 |
| `update` | ✓ | 写 |

`read` 别名实测：`./target/release/gsearch.exe read --help` → 输出 `Usage: gsearch.exe browse [OPTIONS] <URL>`（别名生效）。

### 1.2 Flag（机器核对：零缺失）

```
$ for c in "" search browse fetch verify shell doctor dl login similar update; do
    ./target/release/gsearch.exe $c --help 2>&1; done > $T/allhelp.txt
$ for f in $(grep -oE '\-\-[a-z][a-z0-9-]+' llms.txt | sort -u); do
    grep -q -- "$f" $T/allhelp.txt || echo "MISSING: $f"; done
MISSING: --bind      ← 非 gsearch flag：python -m http.server 的参数
MISSING: --path      ← 非 gsearch flag：cargo install --path . 的参数
=== done ===
```

llms.txt 中出现的 gsearch flag 全部存在：`--json` / `--from` / `--config` / `--read` / `--browse` /
`--allow-private` / `--help`。两个 "MISSING" 是 python / cargo 自己的参数，非 gsearch flag（**不修**）。

### 1.3 环境变量（源码核实：`src/config.rs` / `src/browser.rs` / `src/main.rs` / `src/fetch.rs`）

| 变量 | llms.txt 用途 | 源码位置 |
|---|---|---|
| `GSEARCH_SEARXNG_URL` | SearXNG base URL 覆盖 | `src/config.rs:23,60-61,118`（env > 文件 > None） |
| `GSEARCH_PROFILE` | profile 名 | `src/browser.rs:431`；`src/config.rs:19` |
| `GSEARCH_CHROME` | 浏览器路径 | `src/browser.rs:147,198` |
| `GSEARCH_PROXY` | 代理 | `src/main.rs:379`；`src/browser.rs:527` |
| `GSEARCH_FETCH_ALLOW_PRIVATE` | fetch 私网门 | `src/fetch.rs:9,160` |
| `GSEARCH_LOG` | 日志级别（compact-meta 强制全量的门） | `src/main.rs:388` |

### 1.4 配置查找顺序（源码核实 `src/config.rs:69-90`）

`--config <path>` → `./gsearch.json` → exe 同目录 `gsearch.json` → `~/.gsearch/config.json`。
llms.txt 写的顺序与源码一致。配置键：`profile` / `chrome` / `searxng_url` / `read_max_chars`
（`src/config.rs:19-27`，llms.txt 三键 + 提了 read_max_chars）。

### 1.5 输出契约（实测 stdout 摘录）

| 断言 | 实测命令 | 实测 stdout 首行/键 |
|---|---|---|
| search md meta 头 | `gsearch search "rust async book" --limit 2` | `provider: searxng \| results: 2 \| limit: 2 \| truncated: true \| elapsed: 1.8s` |
| fetch/browse md meta 头 | `gsearch fetch https://example.com` | `format: markdown \| content_untrusted: true \| elapsed: 0.6s` |
| doctor md meta 头 | `gsearch doctor` | `checks: 7 \| ok: 5 \| failed: 0 \| elapsed: 4.3s` |
| verify md meta 头 | `gsearch verify https://example.com/nope404` | `checked: 1 \| ok: 0 \| failed: 1 \| elapsed: 0.8s` |
| search `--json` 顶层键 | `jq -r 'keys\|join(", ")'` | `meta, results, run` |
| fetch `--json` 顶层键 | 同上 | `meta, status, text, title, url`（扁平对象 + `status:"ok"`） |
| browse `--json` 顶层键 | 同上 | `headings, meta, paragraph_index, summary_paragraphs, title, url` |
| fetch `--json` 的 content_untrusted | `jq -c '.meta'` | `{"content_untrusted":true,...}` |
| provider 缺席语义 | browse/fetch 的 meta | 无 `provider` 键；search / similar 的 meta 有 `provider` |

### 1.6 退出码（全部实测，0/1/2/3 + verify 4/5）

| 码 | 触发场景（实测） | 实测 exit |
|---|---|---|
| 0 | search / fetch / browse / verify ok / doctor（仅 WARN）/ similar / update | 0 |
| 1 | `fetch` 私网门拒 | 1 |
| 1 | `fetch` 非 http(s) scheme（`file://`） | 1 |
| 1 | `fetch` 目标不可达（DNS 失败） | 1 |
| 1 | `fetch` JS 壳页（stderr：`该页无服务端正文（JS 壳），需渲染：用 gsearch browse …`） | 1 |
| 1 | `browse` 代理不可达（`net::ERR_PROXY_CONNECTION_FAILED`） | 1 |
| 1 | fetch / verify 批量部分失败 | 1 |
| 1 | doctor 有 FAIL（配置文件解析失败 → 2 个 FAIL） | 1 |
| 2 | 空 query | 2 |
| 2 | `similar` 输入非 URL 形态 | 2 |
| 2 | `browse` 非 http/https scheme | 2 |
| 2 | `browse` 私网默认拒 | 2 |
| 2 | `search --read 5 --limit 3`（越界静态拒绝） | 2 |
| 2 | verify 单 URL HTTP 404（`http_error`） | 2 |
| 2 | fetch / verify 批量全部失败 | 2 |
| 2 | 显式 `--config` 指向解析失败的文件 | 2 |
| 3 | verify SSL 握手失败（`https://self-signed.badssl.com`） | 3 |
| 4 | verify DNS 失败 | 4 |
| 5 | verify 请求超时（`--timeout 1` 打 5s 慢端点） | 5 |

**exit 3 的 search 侧（CAPTCHA 亲解超时）未端到端触发**（需真撞验证码）：源码映射 `src/main.rs:648`
`return Ok(ExitCode::from(3))` 在 `SearchOutcome::CaptchaTimeout` 分支。llms.txt 对该码的措辞是
「撞 CAPTCHA 人工亲解超时」，与源码分支一致。

### 1.7 性能与链路断言

| llms.txt 断言 | 实测证据 |
|---|---|
| 配 SearXNG → 1.8–4.0s、零浏览器、`provider=searxng` | `gsearch search "rust async book" --limit 2` → `real 0m1.801s`，`provider: searxng`，stderr 空（无「使用浏览器」行） |
| 未配 SearXNG → 回落 Google 真浏览器直爬、撞无头风控弹窗 | `--config` 指向无 `searxng_url` 的配置 → 8.9s，`provider: google`，stderr 有「Google 对无头浏览器有独立风控…弹出窗口验证后本会话将切回无头继续」 |
| SearXNG 局域网直连，**不经**代理 | 死代理 `socks5://127.0.0.1:1` 下 `search` 仍 rc=0 4.1s `provider=searxng`；源码 `src/searxng.rs:6`「不走 GSEARCH_PROXY / 系统代理」 |
| 代理不可达 → 浏览器路径快速失败 | `browse https://example.org` 死代理 → 0.87s / rc=1 / `net::ERR_PROXY_CONNECTION_FAILED`；`fetch` 死代理 → 2.08s / rc=1 / `tunnel error` |
| 代理死端口时 Chrome 缓存造成假成功 → 必须破缓存重测 | `browse https://example.com` 死代理 → rc=0（缓存命中）；加 `?cb=<随机>` → rc=1；`browse https://www.iana.org/…` → rc=1 |

---

## 2. llms.txt 薄度核对（验收项 2）

```
$ wc -l llms.txt
115 llms.txt
```

≤160 ✓，且在 100–160 目标区间内。

参数默认值残留检查：

```
$ grep -nE "默认 ?[0-9]{2,}|\[default:|50000|默认 10|默认 160" llms.txt
No matches found
```

- 无「默认 50000」类参数默认值清单残留 ✓
- 无 `[default: ...]` 形式的版本化参数清单 ✓
- 无 `--limit` / `--max-chars` / `--snippet-len` / `--timeout` / `--retry` 的默认值列举 ✓
- 全文**不含**任何「参数默认值全表」；参数真相统一指向 `gsearch --help` / `gsearch <cmd> --help`
  （llms.txt 第 6 行、第 111–112 行的「边界声明」段）

**保留的决策信息**（非参数表）：provider 链、输出形态选择（md vs --json）、退出码语义、
`--read`/`--browse` 两义、stdout/stderr 分离、`content_untrusted`、缺席语义、私网门、
代理分工（SearXNG 不经代理 vs 浏览器/fetch 经代理）、缓存假成功坑。

---

## 3. CLAUDE.md 九条纪律逐条定位

| # | 纪律 | CLAUDE.md 位置 | 可执行性（含具体命令） |
|---|---|---|---|
| ① | 构建与测试 | §1「构建与测试」（L11） | `cargo test --lib --bins --tests -- --skip live_tests` + `cargo clippy --all-targets -- -D warnings`（与 `.github/workflows/ci.yml` 逐字同款）；`tests/cli_golden_bytes.rs` 逐字节 stdout 契约 + 改输出形态必须同步该文件与 golden fixture |
| ② | 产物落盘路由 | §2「产物落盘路由」（L27） | 表格：报告 → `reports/`、深度调研 → `docs/research/`（二者入库，实测 `git ls-files` = 125 / 4 个文件）、临时脚本 → 系统临时目录；明确「临时文件禁落 repo 根或工作区根」，点名 `.probe/` 60+ 文件事故 |
| ③ | 行尾纪律 | §3「行尾纪律（Windows 工作树）」（L41） | `git config core.autocrlf=true` 实测；`0c9b8ae` 的 `2161/2159`、`813/764` numstat 实锤；命令：`git diff --stat` + `git diff -- src/foo.rs` |
| ④ | 浏览器测试环境 | §4（L58） | `"D:/Program Files/Python 3.14/python.exe" -m http.server 8931 --bind 127.0.0.1`（实测跑通，`D:\Program Files\Python 3.14` 在 PATH，Python 3.14.6）；`browse --allow-private` / `fetch` 私网同理；测完停伺服 |
| ⑤ | 零 token 纪律 | §5（L71） | `timeout: 0` 一次调用跑完；禁 bash→wait→bash 两段式；输出用文件重定向落临时目录一次性读回 |
| ⑥ | commit 规范 | §6（L78） | `<type>: <description>`（类型枚举 + 中文 description + bd 单号写法）；逐文件 `git add <file>`，禁 `git add -A` / `git add .`；子代理禁 commit/push |
| ⑦ | bd 用法 | §7「bd（beads）用法」（L87） | 禁 `bd edit`（它开 `$EDITOR`，实测 `bd edit --help` 首行即此），改用 `bd update` / `bd note` / `bd comment`（三者 `bd --help` 均列出且实测存在）；`bd create "<标题>" --parent gsearch-rs-<父单号>`（`bd create --help` 有 `--parent string`）；`bd close gsearch-rs-<单号> --reason "…"`（`bd close --help` 有 `-r, --reason string`） |
| ⑧ | 文档同步纪律 | §8（L96） | 同批次：CHANGELOG `## [Unreleased]`（实测已存在于文件最前）+ README/USER_GUIDE/docs/PLAN/llms.txt/CLAUDE.md 按需；禁事后补；禁参数默认值全表 |
| ⑨ | 不得替用户改环境 | §9（L111） | 禁擅自 `setx` / 改 `~/.gsearch/config.json` / 改系统代理 / 动注册表环境变量值；换代理用单次 `--proxy` / `--config` flag；只读探测命令 `reg query "HKCU\Environment" /v GSEARCH_PROXY`（实测可用） |

段落起始行号由 `grep -nE "^## " CLAUDE.md` 实测：`1`@L11、`2`@L27、`3`@L41、`4`@L58、`5`@L71、
`6`@L78、`7`@L87、`8`@L96、`9`@L111（文件共 118 行）。

### 3.1 CLAUDE.md 提到的文件路径 / 命令 / 脚本逐条核

```
$ for p in README.md CHANGELOG.md USER_GUIDE.md docs/PLAN.md llms.txt CLAUDE.md \
    tests/cli_golden_bytes.rs tests/fixtures/golden_search_json.txt \
    tests/fixtures/golden_search_md.txt tests/fixtures/cli_golden_provenance.md \
    .gitattributes reports docs/research; do … done
OK   README.md
OK   CHANGELOG.md
OK   USER_GUIDE.md
OK   docs/PLAN.md
OK   llms.txt
OK   CLAUDE.md
OK   tests/cli_golden_bytes.rs
OK   tests/fixtures/golden_search_json.txt
OK   tests/fixtures/golden_search_md.txt
OK   tests/fixtures/cli_golden_provenance.md
OK   .gitattributes
OK   reports
OK   docs/research
```

命令与工具存在性：`cargo` 1.98.0-nightly ✓、`cargo clippy` 0.1.97 ✓、`bd`（`C:\Users\Begonia\AppData\Local\Programs\bd\bd.exe`）✓、
`python` 3.14.6（`D:\Program Files\Python 3.14\python.exe`）✓、`git` ✓。

---

## 4. 无虚构内容核对（每个事实性断言的取事实命令）

| 断言出处 | 取事实命令 | 输出摘录 |
|---|---|---|
| 10 个子命令 + 别名 | `for c in …; do gsearch $c --help; done` | 10 个 `Usage:` 行 + browse 的 `[alias: read]` |
| search md meta 头形态 | `gsearch search "rust async book" --limit 2` | `provider: searxng \| results: 2 \| limit: 2 \| truncated: true \| elapsed: 1.8s` |
| fetch/browse md meta 头形态 | `gsearch fetch https://example.com` | `format: markdown \| content_untrusted: true \| elapsed: 0.6s` |
| doctor / verify meta 头形态 | `gsearch doctor` / `gsearch verify https://example.com/nope404` | `checks: 7 \| ok: 5 \| failed: 0 \| elapsed: 4.3s` / `checked: 1 \| ok: 0 \| failed: 1 \| elapsed: 0.8s` |
| `--json` 三种信封形态 | `jq -r 'keys\|join(", ")'` | `meta,results,run` / `meta,status,text,title,url` / `headings,meta,paragraph_index,summary_paragraphs,title,url` |
| 退出码 0/1/2/3/4/5 全部语义 | 见 §1.6 表（每行一条实跑命令 + exit 码） | 见 §1.6 |
| 缺席语义（`truncated:false` 等键不出现） | `jq` 读 search `--json` 的 meta | 键集仅 `tool,version,query,profile,humanize,limit,elapsed_ms,provider,truncated,truncated_detail`——无 `null` 值键、无 `message` 空串键 |
| `run.status` 五个取值 | 源码 `src/types.rs:152-169` + `types.rs:361-369` 单测断言 | `Ok/NoResults/FilteredEmpty/SearxngDegraded/OffTopicResults` → `"ok"/"no_results"/"filtered_empty"/"searxng_degraded"/"off_topic_results"` |
| `dl`/`update`/`login` 输出人读文本 | `gsearch update` 实跑 | `已是最新（本地 v0.4.1，远端 v0.4.1）`（非 JSON） |
| 配置查找顺序与键 | 源码 `src/config.rs:19-27,69-90` | 见 §1.3 / §1.4 |
| 环境变量清单与优先级 | 源码 `src/config.rs:60-61,118` + `src/main.rs:379,388` + `src/browser.rs:147,431,527` + `src/fetch.rs:160` | 见 §1.3 |
| SearXNG 不经代理 | 源码 `src/searxng.rs:6` + 实跑（死代理下 search 仍 rc=0 provider=searxng） | 见 §1.7 |
| 代理死 → 快速失败（不重试） | 死代理实跑 browse / fetch | 0.87s rc=1 / 2.08s rc=1 |
| Chrome 缓存假成功 | 死代理下 browse example.com / example.com?cb=rand / iana.org | rc=0 / rc=1 / rc=1 |
| `search --read` 两义 | `gsearch search --help` | `--read N`：「只取前 N 条的 snippet 进输出（纯 HTTP 路径，不启动浏览器）」；`--browse N`：「用 Chrome 渲染前 N 条结果的 URL」；browse 别名说明「与 `search --read N` 是另一语义」 |
| JS 壳 → 换 browse 不是重试 | 本地 SPA fixture（`<div id="root">` + 空正文） | fetch rc=1 stderr「该页无服务端正文（JS 壳），需渲染：用 gsearch browse …」；同页 browse rc=0 |
| 私网默认拒（SSRF 门） | `gsearch browse http://127.0.0.1:8931/x` | rc=2（默认拒）；加 `--allow-private` 后进入渲染流程（连接失败 rc=1） |
| 越界静态拒绝 | `gsearch search "…" --limit 3 --read 5` | rc=2，零网络 |
| `GSEARCH_PROXY` 持久化在用户级环境变量 | `reg query "HKCU\Environment" /v GSEARCH_PROXY` | `GSEARCH_PROXY REG_SZ socks5://127.0.0.1:10808` |
| doctor network 分级 | `gsearch doctor`（无代理）/ `GSEARCH_PROXY=…10808 gsearch doctor` | 无代理：`[WARN] network: 直连不可达…SearXNG 主路径可达` + rc=0；配代理：`[OK] network: …（经代理可达：socks5://127.0.0.1:10808）` + rc=0 |
| doctor FAIL → rc=1 | cwd 放解析失败的 `gsearch.json` 后跑 doctor | `[FAIL] network` + `[FAIL] searxng` → rc=1 |

**未验证项（如实披露）**：
1. **search 侧 exit 3（CAPTCHA 亲解超时）未端到端触发**——需真撞 Google 验证码；依据是源码分支
   `src/main.rs:648` 与 README 退出码表。verify 侧的 3 已实测（self-signed.badssl.com）。
2. **代理不可达时"Google 回退预检经代理探测"的新行为未单独实测**——该改动由并行 agent 在本会话中
   落地（`src/search.rs` 工作树改动），当前 release 二进制（`git:v0.4.1-6-g4eb9caf`）尚未含它；
   llms.txt 因此**没有**写这条行为。

---

## 5. 副作用三态

**预期内（本次职责面）**：
- 新建 `llms.txt`（repo 根，115 行）
- 新建 `CLAUDE.md`（repo 根，118 行）
- 新建 `reports/2026-11-17-llms-txt-and-claude-md.md`（本报告）
- `bd close gsearch-rs-3vu`（bd 纪律⑦：完工核销）→ 实测 `bd show gsearch-rs-3vu` 显示 `CLOSED` + close reason
- `bd note gsearch-rs-4n4`（父单记 ② 项交付进度，未 close 父单——父单含 9 项竞品学习点，非本单全部）

**无副作用面**：
- `src/` / `tests/` / `README.md` / `CHANGELOG.md` / `USER_GUIDE.md` / `docs/` **零改动**
  （`git status` 中这些文件的改动来自并行 agent：DoctorNetFix / FallbackMatrixTest）
- `~/.gsearch/config.json` 未动（仍 `{"searxng_url": "http://192.168.89.249:8888"}`）
- 未设 / 未改任何 `GSEARCH_*` 环境变量；未改系统代理；未动注册表（用户既有授权值
  `GSEARCH_PROXY=socks5://127.0.0.1:10808` 原样保留）
- 未 commit / 未 push（契约规定 commit 归 PM）

**观察项（范围外，未动，留 PM 裁决）**：
1. **`cargo test --no-run` 当前编译失败**（并行 agent 的在途改动）：
   `src/duckduckgo.rs:28` 找不到 `crate::search::FallbackNet` + `src/search.rs:622` 参数数不匹配
   （E0425 / E0061 ×2）。**与本任务无关**（本任务零代码改动），但会挡住 CI。
2. **`README.md` 与 `CHANGELOG.md` 有并行 agent 的在途改动**（未提交）；本次两份文档按契约未触碰。
3. **`docs/PLAN.md` 未包含新特性**（`docs/PLAN.md` 停在 2026-08 的设计稿）。属 ⑧ 纪律范畴但不在
   本单验收内，未擅动。

**经验沉淀（task-closing-ritual §1–§2）**：
- 写入项目长期记忆 `C:/Users/Begonia/.omp/agent/memories/--D--Project-gsearch-rs--/MEMORY.md`
  两行：①「repo 根已有 llms.txt + CLAUDE.md，参数真相源 = `--help`，不抄参数表」；
  ②「**断言『代理不可达』前必须破缓存**——Chrome profile 磁盘缓存命中会让同一 URL 返回 rc=0
  假成功，加 `?cb=<rand>` 才复现真失败」。第 ② 条是本次实测踩到的真坑（换项目/环境同样会踩），
   且原记忆无此条，属新增。
- **知识图谱未重建**：`codebase-memory-mcp cli index_status` 显示 `indexed_at=2026-11-17T09:55:50Z`，
  早于本次改动；但项目记忆明确「**subagent 禁止 reindex**」，且本次改动**只有 markdown 文档**
  （`llms.txt` / `CLAUDE.md` / 报告），不产生新代码符号 → 无需重建。

---

## 6. 临时产物清理确认

- 本任务**零**临时文件落在 repo 根或工作区根：所有探针输出（help 全量抓取、各命令 stdout/stderr、
  坏配置 fixture、SPA fixture、本地 HTML）全部落在 **系统临时目录**
  `C:/Users/Begonia/AppData/Local/Temp/gsearch-doc/`，完工前已整目录删除：
  ```
  $ taskkill /PID 26112 /F          # 停本任务起的 python -m http.server 8931
  成功: 已终止 PID 为 26112 的进程。
  $ rm -rf C:/Users/Begonia/AppData/Local/Temp/gsearch-doc
  $ ls -d .../gsearch-doc
  ls: cannot access '.../gsearch-doc': No such file or directory
  $ netstat -ano | grep ":8931" | grep LISTENING
  (无输出 = 端口已释放)
  ```
- repo 根**无** `.probe/`、**无** `tmp_fulltest/`、无其它临时残留（`ls -d` 均报不存在）。
- 最终 `git status --short`：
  ```
   M .beads/interactions.jsonl     ← 非本任务
   M CHANGELOG.md                   ← 非本任务（并行 agent）
   M README.md                      ← 非本任务（并行 agent）
   M src/duckduckgo.rs              ← 非本任务
   M src/general.rs                 ← 非本任务
   M src/main.rs                    ← 非本任务
   M src/postproc.rs                ← 非本任务
   M src/search.rs                  ← 非本任务
  ?? CLAUDE.md                      ← 本任务
  ?? llms.txt                       ← 本任务
  ?? reports/2026-11-17-gsearch-skill-sync.md   ← 非本任务（SkillSync）
  ?? tests/fallback_decide.rs      ← 非本任务
  ```

---

## 7. 验收自检

| 验收项 | 判定 | 证据 |
|---|---|---|
| ① llms.txt 每个命令/子命令/flag 与实测一致 | **PASS** | §1.1（10 命令）/ §1.2（机器核对零缺失，两个非 gsearch flag 已标注）；§1.3–§1.4（配置与环境变量源码核实）；实跑 help 全量比对 |
| ① CLAUDE.md 每个文件路径 / 命令 / 脚本真实存在 | **PASS** | §3.1（13 条路径逐条 OK + 5 个工具版本实测） |
| ② llms.txt 不含参数默认值全表 / 版本化参数清单，行数 ≤160 | **PASS** | §2：115 行；默认值残留 grep 零命中 |
| ③ CLAUDE.md 覆盖 9 条纪律，每条可执行 | **PASS** | §3 表（9/9 定位到行号，每条带具体命令） |
| ④ 两文件无虚构内容（无「可能/大概」式断言） | **PASS** | §4（每条断言附取事实命令 + 输出摘录）；未验证项 2 条已如实披露且未写进文档 |
| ⑤ 临时产物清零 | **PASS** | §6 |
| ⑥ 产品代码零改动 | **PASS** | §5（`src/` `tests/` `README.md` `CHANGELOG.md` `USER_GUIDE.md` 均未触碰） |

**未验证项（诚实披露）**：search 侧 exit 3（CAPTCHA 亲解超时）未端到端触发（需真撞验证码），
依据是源码分支 + README；verify 侧 exit 3 已实测。并行 agent 在途的「回退链代理感知」行为
未写进 llms.txt（当前 release 二进制尚未含该改动）。

---

## 8. 交付状态

**DONE**（`VERDICT: PASS`）—— 两份产物留在指定路径，验收全过，产品代码零改动，临时产物清零。
