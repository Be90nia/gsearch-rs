# 回退链代理感知 + 决策层纯函数（bd gsearch-rs-1uc / gsearch-rs-aws）

**VERDICT: PASS**（两单同源一次做完；验收 1–4 全绿）

- 日期：2026-11-17
- 范围：`src/search.rs` + `src/duckduckgo.rs`（单一 owner）+ 新测试文件 `tests/fallback_decide.rs` + `README.md` / `CHANGELOG.md` 同步
- 提交：归 PM（本代理零 commit / 零 push）

---

## 1. 问题与根因（PM 2026-11-17 实测，非推测）

回退链 = **SearXNG → DDG html → Google**。故障形态：`GSEARCH_SEARXNG_URL` 指向不可达时，
stderr「DDG html 直连失败（curl exit 28：DDG 直连超时（5s 预算））」→「SearXNG 零结果已熔断…」，
结果 `provider=searxng / results=0 / run.status=searxng_degraded / rc=2 / elapsed=10.6s`。

根因：**回退链两跳都裸连**，只有浏览器路径带代理（M17 起 `launch_with_kind_proxy`）。

| 跳 | 原实现 | 为什么裸连 |
|---|---|---|
| DDG html 直连 | `src/duckduckgo.rs` 走 `curl` 子进程，**不带 `--proxy`** | curl 只原生读 `HTTPS_PROXY` 系环境变量，**不认 `GSEARCH_PROXY`** → 回退链这一跳在墙内必超时 |
| Google 回退预检 | `src/search.rs` 硬直连 `www.google.com:443`（TCP 1.5s） | 从不读代理配置 → 墙内必然不通 → **假熔断** |

旁证：`reports/2026-11-17-doctor-net-and-full-from-fix.md:171` 已把这条登记为观察项 1
（「`src/search.rs:651` 的 Google 回退预检仍只直连 TCP（不读 `GSEARCH_PROXY`）…故 `search`
在 SearXNG 故障后的 Google 回退仍会熔断」）——本单闭环该观察项。

---

## 2. 改动清单

### 2.1 `src/search.rs`（+~250 行）

| 位置 | 改动 | 作用 |
|---|---|---|
| `try_searxng` / `fallback_after_failure` | 回退链重构：网络 I/O（`FallbackNet::probe` → DDG → 预检）留在调用方，决策交给纯函数 `decide_after_fail`；回退段从 `try_searxng` 抽出成独立函数（52 行 → 13 行，纯搬移） | 决策层可单测（aws）+ 可读性 |
| `fallback_proxy` / `proxy_from_env_value` | 回退链代理来源 = `GSEARCH_PROXY`（空白 = 未设） | 代理感知入口（1uc） |
| `curl_proxy_url` | `socks5://` → `socks5h://`（域名交代理侧解析，与 Chrome SOCKS5 一致） | house 同款（main.rs doctor 同款内联副本） |
| `proxy_endpoint` | 代理 host:port 解析（剥 userinfo / IPv6 方括号 / 无端口补 curl 默认 1080） | TCP 探针前置 |
| `ProxyReach` / `FallbackNet` | 代理端点可达性**只探一次**（TCP 1s 封顶），DDG 与预检共用同一结论 | 归因：代理不可达 ≠ 目标站不可达 |
| `Precheck` | 预检结果三态：`Ok` / `Unreachable(归因)` | 归因可区分 |
| `google_fallback_precheck` | 无代理 = 原直连 TCP 1.5s；**有代理 = 经代理探测**（curl `--proxy`，house 同款 verify.rs/doctor，零新依赖） | 消除假熔断 |
| `DdgOutcome` / `decide_after_fail` | **纯函数**决策层（输入 = SearXNG 失败分类 + DDG 结果 + 预检结果；输出 = `SearxngAttempt`） | 决策表可自动化（aws） |

### 2.2 `src/duckduckgo.rs`（+~30 行）

- `collect(cfg, net)`：代理端点不可达 → 直接按「代理不可达」失败归因（不空烧请求预算）；否则 curl 显式 `--proxy`。
- `curl_args(url, body, proxy)`：配了代理 → 追加 `--proxy <socks5h://…>`，预算 5s → **10s**（隧道握手 + 出口延迟叠加，5s 会误杀可用请求）；**无代理时参数与旧版逐字一致**。
- 文案去「直连」限定（经代理时不再成立）：`curl exit 28：DDG 请求超时（预算内未返回）` / `curl exit {c}：DDG 传输失败`。

### 2.3 新增测试 `tests/fallback_decide.rs`（10 例，零外网 / 零浏览器 / 零 env 依赖）

决策表 6 态 + 代理纯函数 4 组。

### 2.4 文档同步

- `CHANGELOG.md`：新增 `## [Unreleased] → Fixed` 条目（1uc + aws 同款）。
- `README.md`：回退链段改为「代理感知」表述 + 「代理不可达与目标站不可达分立归因」。
- `llms.txt` / `USER_GUIDE.md` / `docs/PLAN.md`：**均无回退链/预检描述**（已 grep 确认）→ 无需改。

---

## 3. 决策表（`decide_after_fail` 纯函数）

| # | DDG | Google 预检 | SearXNG | 决策 | 自动化用例 |
|---|---|---|---|---|---|
| ① | 命中 | — | 任意 | `Results(provider="duckduckgo")` | `state_1_ddg_hit_returns_duckduckgo_provider` |
| ② | 空 | 通 | 健康零结果 | `FallbackGoogle(None)`（源健康，非降级） | `state_2_..._is_fallback_without_fault` |
| ③ | 空 | 通 | 故障 | `FallbackGoogle(Some(真因))`（真因透传） | `state_3_..._fault_carries_cause` |
| ④ | 空 | 不通 | 健康零结果 | `HealthyEmpty`（不熔断） | `state_4_..._is_healthy_empty_not_circuit` |
| ⑤ | 空 | 不通 | 故障 | `CircuitBroken(SearXNG 真因 + DDG 失败归因 + 预检归因)` | `state_5_..._with_cause_chain` |
| ⑥ | 失败 | 同②–⑤ | 同②–⑤ | 与「空」同语义，但 DDG 失败归因**不吞**（进真因链） | `state_6_ddg_failed_behaves_like_empty_but_keeps_cause` |

（bd aws 建议的签名是 `decide_after_fail(fail, ddg, precheck_ok: bool)`；实现把 `bool` 换成
`Precheck` 枚举——否则「代理不可达 ≠ 目标站不可达」的归因无处安放。）

---

## 4. 验证（命令 + 关键输出）

### 4.1 编译与静态门

```
$ cargo test --lib --bins --tests -- --skip live_tests
test result: ok. 131 passed; 0 failed  (lib)
test result: ok. 153 passed; 0 failed  (bin main)
test result: ok. 10 passed; 0 failed   (tests/fallback_decide.rs ← 新增)
… 共 12 个测试二进制，0 failed

$ cargo clippy --all-targets -- -D warnings
Finished `dev` profile …          clippy rc=0（零 warning）

$ cargo build --release
Finished `release` profile … target/release/gsearch.exe = 10,136,064 B（零新依赖）
```

### 4.2 验收 1：SearXNG 挂 + 代理可用 → 回退成功出真结果

```
$ GSEARCH_PROXY=socks5://127.0.0.1:10808 GSEARCH_SEARXNG_URL=http://127.0.0.1:9 \
    gsearch search "rust async runtime" --limit 2 --json
rc=0
provider= duckduckgo   status= ok   n= 2   elapsed_ms= 8088
stderr: SearXNG http://127.0.0.1:9 查询失败（…Connection refused…），已回退 DuckDuckGo html 直连
```

（修复前同一命令：`provider=searxng / results=0 / run.status=searxng_degraded / rc=2 / elapsed=10.6s`。）

### 4.3 验收 2：代理不可达 → 归因可区分 + 预算内退出

```
$ GSEARCH_PROXY=socks5://127.0.0.1:1 GSEARCH_SEARXNG_URL=http://127.0.0.1:9 \
    gsearch search "rust async runtime" --limit 2 --json
rc=2   wall=5s（不挂死）
stderr: DDG html 直连失败（代理不可达（socks5://127.0.0.1:1）：TCP 连接 127.0.0.1:1 超时（1s 预算）），继续 Google 回退链
stderr: SearXNG 零结果已熔断（…）；真因: …；DDG 插层失败：代理不可达（…）；Google 回退预检失败：代理不可达（…）
run.message 断言：has 代理不可达 = True | has 目标站不可达 = False
```

→ stderr 与 `run.message` 表达的是「**代理不可达**」，不是「目标站超时/不可达」；代理端点 TCP 探针 1s 封顶。

### 4.4 验收 3：stdout 契约零破坏（改动前基线 vs 改动后，逐字节）

基线构建方式（不动工作树）：`git archive HEAD`（= 3a652cf，改动前）→ 系统临时目录 → 加 `[[bin]]
name="gsearch_prefix"` 避开同名产物 → 共享 `CARGO_TARGET_DIR` 构建 → `gsearch-prefix.exe`。

确定性对照（本地回环假 SearXNG，`GSEARCH_SEARXNG_URL=http://127.0.0.1:8899`，归一 `elapsed_ms`）：

```
gsearch-prefix rc=0 / gsearch-fixed rc=0
prefix len 426   fixed len 426
BYTE IDENTICAL
{"meta":{"tool":"gsearch","version":"0.4.1","query":"rust async runtime","profile":"default",
"humanize":false,"limit":2,"elapsed_ms":N,"truncated":true,"provider":"searxng",
"truncated_detail":"results_capped_by_limit"},"run":{"status":"ok"},"results":[…]}
stderr：两版均空
```

- JSON 信封**键集/键序零变化**（`provider` / `run.status` 语义不变），另有仓内既有回归锁
  `tests/cli_golden_bytes.rs`（真实 CLI 二进制 + golden fixture 逐字节）6 passed。
- 活实例（当前 `~/.gsearch/config.json`）对照：两版 `provider=searxng` / `status=ok` / 结果 URL 集合相同、
  键集相同；**仅第 1 条 snippet 文本不同** —— 实例两次返回的 snippet 漂移（非本改动面；确定性对照见上）。

### 4.5 知识图谱

```
$ codebase-memory-mcp cli index_repository --repo-path D:/Project/gsearch-rs --mode fast --persistence true
$ codebase-memory-mcp cli index_status --project D-Project-gsearch-rs
nodes: 3118（3050 → +68）  edges: 8113  status: ready  indexed_at: 2026-11-17T10:20:45Z
$ codebase-memory-mcp cli search_code --pattern decide_after_fail → 命中 10 处
```

---

## 5. Side-effects 三态

- **预期内（修复面）**：
  - 配了代理时，DDG 与 Google 预检**改走代理**（行为面变化即修复面）；DDG 预算 5s → 10s（仅代理态）。
  - 失败路径 stderr 增行：源健康 + 预检不通时多一行「Google 回退预检失败（…）——SearXNG 源健康零结果，不熔断」（禁静默）。
  - 熔断真因链内容变长（SearXNG 真因 + DDG 失败归因 + 预检归因）；`run.message` 与 stderr 诊断行同源同步。
  - DDG 失败文案去「直连」限定（`DDG 请求超时（预算内未返回）` / `DDG 传输失败`）。
- **无副作用面**：正常 SearXNG 命中路径逐字节不变（§4.4）；**无代理时** curl 参数集与预算逐字不变
  （`--max-time 5`、无 `--proxy`）；`SearXNG → DDG → Google` 链序与 `provider` 值域
  （`searxng`/`duckduckgo`/`google`）不变；`batch` 路径（SearXNG-only，禁浏览器回退）未动；
  JSON 信封键集/键序零变化；`tests/fallback_matrix.rs` 既有断言未改未动；`src/main.rs` 未动；零新依赖。
  可读性清扫（纯搬移、行为不变）：回退链从 `try_searxng`（52 行）抽成 `fallback_after_failure`（40 行）；
  `decide_after_fail` 36 行、最深嵌套 3 层，均在 house 上限（函数 ≤50 行 / 嵌套 ≤4）内。
- **观察项（范围外，未动，留 PM 裁决）**：
  1. **CLI `--proxy` 仍未铺到回退链**——回退链只读 `GSEARCH_PROXY`（env）。接线要改 `main.rs`
     （`try_searxng` 的调用点在本单非目标内）。故「只给 `--proxy`、不设 env」时回退链仍裸连。
  2. **`HEAD` 的 `src/postproc.rs` 编译不过**（`4eb9caf` 起 `fn pick` 丢了闭合 `}`，一个 `#[test]` 函数
     被塞进函数体）→ 本单建「改动前基线」时实测报错 `expected one of `.`, `;`, `?`, `}`, or an operator,
     found doc comment`（postproc.rs:41）。工作树里已有修复版（未提交，且带整文件 CRLF 归一）。
     **与本单无关，但 main 编译不过 = CI 必红**，请 PM 优先处置。
  3. **DDG 直连路径本身是「时好时坏」的**——本次基线二进制（改动前）在同一环境下 DDG 直连**成功**了
     （rc=0 / provider=duckduckgo），而 PM 报告里是 curl exit 28。即直连是间歇性超时，不是稳定不通；
     本修复把「经代理」变成确定性路径，但不宣称直连恒坏。
  4. **构建期 vergen 偶发失败**（`Unable to set VERGEN_GIT_DESCRIBE` → `--version` 显示 `(dev)`）：
     发生在并发 git 操作（别的代理在提交）期间，`git describe --dirty` 拿不到 index；
     JSON 信封的 `version` 取 `CARGO_PKG_VERSION`（`0.4.1`）不受影响。

---

## 6. 临时产物清理确认

- 本代理全部 e2e 产物（stdout/stderr 快照、基线二进制、假 SearXNG 脚本、基线源码树）落在
  `%LOCALAPPDATA%\Temp\`（`gsearch-fixed.exe`、`gsearch-prefix.exe`、`fake_searxng.py`、
  `gsearch-prefix/` 源码树、`e2e*/cmp-*/live-*/f1.*/f2.*`）→ 验证后逐个删除，实测
  `my-artifacts-remaining=0`；**repo 内零临时文件**（`git status --short` 只有本单 4 个改动文件
  + 2 个新增文件，同伴代理的 `src/general.rs` / `src/main.rs` / `src/postproc.rs` 改动未被我触碰）。
- repo 根的 `.probe/`：收尾时 `ls -a .probe | wc -l` = **0**（已被清空，非本代理所删）；本代理未在其中产生任何文件。
- 假 SearXNG 本地伺服（`127.0.0.1:8899`）验证后已停（实测 `fake searxng stopped`，端口已释放）。

---

## 7. 未做（明确不在本轮）

- 不改 `src/main.rs`（故 CLI `--proxy` 未铺到回退链，见 §5 观察项 1）。
- 不改 `tests/fallback_matrix.rs` 既有断言（新增独立文件 `tests/fallback_decide.rs`）。
- 不动 doctor 已完成的网络分级实现；不改 SearXNG 实例 / 路由器 / 任何环境配置
  （`GSEARCH_PROXY` 与 `~/.gsearch/config.json` 全程只读，代理按单条命令临时传参）。
- 不 commit / 不 push（归 PM）。

---

## 8. 沉淀

- **项目内**：CHANGELOG `[Unreleased]` + README 回退链段（回退链的「代理感知」语义与归因口径）。
- **通用经验**（可迁出项目名）：
  1. `curl` **只原生读 `HTTPS_PROXY` 系环境变量，不读应用自定义的代理变量** → 用应用自有代理配置时
     必须显式 `--proxy`，否则「以为走了代理、实际裸连」，墙内表现为必然超时而非明确报错。
  2. **代理不可达 ≠ 目标站不可达**：先 TCP 探代理端点（1s 封顶）再打目标，才能给出可区分归因；
     否则 curl exit 7 这类码无法区分是代理口拒绝还是目标拒绝。
  3. **纯函数决策层 + I/O 留在调用方**：分支选择类逻辑（多跳回退链）一旦把网络调用内联进 `async fn`，
     决策表就永远无法自动化（CI 与本机网络相反 → 必 flaky）；把「命中/空/失败」「通/不通」做成数据入参后，
     决策表即可单测。
