# bd gsearch-rs-b39：search.rs 回退链五态决策矩阵 + searxng_collect 内核测试覆盖

- 日期：2026-11-17
- 交付物：`tests/fallback_matrix.rs`（新增，10 个用例，独立测试二进制）
- 结论：**VERDICT: PASS**（新文件 10/10 绿；全量 329 passed / 0 failed；clippy rc=0）
- 工具：serena-cli 8 次（1 版本探测 + 1 `warm rust` + 3 `overview` + 3 `symbol-body`，定位回退链与内核符号）+ grep/read 读函数体；未改任何 `src/` 实现代码

---

## 一、五态决策矩阵覆盖对照表

回退链现状（SearXNG JSON → SearXNG HTML 降级 → **DuckDuckGo html**（af9 后插层）→ Google 真浏览器）。
下表「态」按派单的五态编号；每态的**可自动化部分**与**不可自动化部分**分列。

| 态 | 场景 | 用例（tests/fallback_matrix.rs） | 断的是什么回归 | 观测点（可观察行为） |
|---|---|---|---|---|
| ① | SearXNG JSON 端点成功 | `state_01_json_hit_marks_provider_searxng_without_html_degrade` | provider 标错源 / captcha 标记误置 / 命中路径误入降级分支 | `provider=="searxng"`、`captcha_solved==false`、结果集==JSON 响应、**HTML 层零请求**、JSON 层只请求首页 |
| ② | JSON 层 HTTP 故障 → 降级 HTML | `state_02_json_http_error_degrades_to_html_keeping_provider_searxng` | 降级结果被标成 google/duckduckgo（agent 分流错）/ 降级路径没走到 | `provider=="searxng"`（降级不改来源标注）、URL 来自 HTML 层、JSON+HTML 两层各请求一次 |
| ②b | JSON 层 200 但响应体非 JSON → 降级 HTML | `state_02b_json_non_json_body_degrades_to_html_keeping_provider_searxng` | 非 JSON 响应被当成「零结果」（语义错）/ 直接报错不上降级 | 同上 + HTML 层确有请求（降级发生） |
| ③ | JSON+HTML 双层故障 → 走 Google 分支 | `state_03_both_layers_failed_is_fault_with_two_layer_cause_chain` + `collect_attributes_failure_to_the_layer_that_failed` | 真因链缺层/串层；`fault` 无条件赋值（源健康零结果被误标 degraded）；健康空被归成故障 | 分类=`SourceError`；真因链「json 真因；HTML 层: html 真因」双层拼接且**逐层归位**（json 段带 `format=json`、html 段不带）；`circuit_diag(reason)`（回退分支的 run.message/stderr 同源诊断行）携真因 |
| ④ | SearXNG 零结果（≠ 引擎故障） | `state_04_zero_results_is_healthy_empty_not_fault_or_degraded` | 健康空被归成 SourceError（误打熔断诊断行）；零结果被标 `searxng_degraded`（agent 误以为基础设施故障） | 分类=`HealthyEmpty`、`describe()=="查询无结果（源健康）"`、`empty_status(None)==(NoResults, NO_RESULTS_MSG)`、`empty_status(Some(Day))==(FilteredEmpty, FILTERED_EMPTY_MSG)`、两者均 ≠ `SearxngDegraded` |
| ⑤ | 部分引擎挂 / 部分页失败但有结果 | `state_05_partial_engine_failure_still_returns_available_results` + `collect_keeps_partial_results_when_later_page_fails` | 见到失败引擎/失败页就整集判失败（可用结果被丢光） | `provider=="searxng"`、结果集==可用结果、**不报错**（`Results` 而非 `Err`） |

### 态③的自动化盲区（**未覆盖，需另开 src 改动单**）

「双层失败 → **选中 Google 分支**（`provider=="google"`）」这一*分支选择*在本仓**无法自动化**：

- `try_searxng` 的 Err 分支直接调两个**硬网络依赖**：`crate::duckduckgo::collect`（`src/duckduckgo.rs:23-27`，URL 硬编码 `https://html.duckduckgo.com/html/`）与 `google_fallback_precheck`（`src/search.rs:653`，直连 `www.google.com:443`）。二者都**无注入边界**。
- 若写用例自动化它，结果随网络环境漂移（DDG 命中→`provider="duckduckgo"`；否则 precheck 通→`FallbackGoogle`、不通→`CircuitBroken`），且会**打真外网**——违反本单「不打外网 / 零外网」约束，且 CI（GitHub Actions 可直连 google）与本机（google:443 不通）结论相反 → 必 flaky。
- 本文件锁的是该分支的**输入与归因**（双层故障 → `SourceError` + 双层真因链 → 回退决策必须携真因、不得归为源健康零结果），分支选择本身留白。
- **要补齐需先改 src**（本单非目标明令禁止，故未动）：按本仓审计报告 `reports/audit-tests-20261010.md` P0-1 的原处方，把 Err 分支决策体抽成纯函数
  `decide_after_fail(fail: SearxFail, ddg: DdgOutcome, precheck_ok: bool) -> SearxngAttempt`
  （`DdgOutcome = Hit(Vec<SearchResult>) | Empty | Err(String)`），网络层原样留在 `try_searxng` 壳里，然后 5 行决策表各一测（DDG 命中→duckduckgo / precheck 通→FallbackGoogle(Some|None) / 健康空+precheck 不通→HealthyEmpty / 故障+precheck 不通→CircuitBroken）。
  → **建议 PM 单开一张 src 改动单**（一行纯函数抽取，零行为变更）。

---

## 二、searxng_collect 内核三项（经 `run_batch` 观测——`batch_one` 与单查共用同一内核）

| 内核行为 | 用例 | 断的是什么回归 | 观测点 |
|---|---|---|---|
| 翻页（多页拼接） | `collect_concatenates_pages_in_order_and_truncates_to_limit` | 跨页顺序错乱（页内/页间混排）/ 截断丢失 / 凑满后继续翻页 | 首页 2 + 第 2 页 2 + 第 3 页 2、limit=5 → 结果序列逐条等于 `kp/1/a, kp/1/b, kp/2/a, kp/2/b, kp/3/a`；请求页码序列 == `[1,2,3]`（凑满即停） |
| 翻页（页上限） | `collect_stops_at_page_cap_without_overfetching` | 上限被改大（空转更多轮）/ 提前断（少取）/ 收口时丢末页结果 | 每页 1 条、limit=100 → 10 条结果；页码序列严格 `1..=10`；**无第 11 页请求**（不越界多取）；末页结果 `kc/10` 在列 |
| 部分结果 | `collect_keeps_partial_results_when_later_page_fails` | 后续页失败回归成 `return Err` → 已得结果丢光 | 首页 2 条 + 第 2 页双层故障 → `Ok(Results)` 且 `provider=="searxng"`，结果集 == 首页 2 条 |
| 健康归因（失败引擎归到哪一层） | `collect_attributes_failure_to_the_layer_that_failed` | 真因链缺层/串层（运维按诊断行定位不到哪层挂的） | json 段含 `SearXNG 返回错误状态`+`format=json`；html 段以 `SearXNG HTML 返回错误状态` 开头且**不含** `format=json` |

---

## 三、红-绿验证（每个用例都验过「能红」——变异测试）

方法：临时改 `src/` 实现 → 跑本文件 → 记录变红用例 → **逐条还原**（还原后 `git diff src/search.rs src/searxng.rs` 为空）。

| # | 变异（临时） | 变红的用例 | 结论 |
|---|---|---|---|
| 1 | `try_searxng` Ok 分支 `provider:"searxng"` → `"google"` | state_01 / state_02 / state_02b / state_05（4 红） | provider 标注被锁 |
| 2 | `Err(_) if !collected.is_empty() => break` → `Err(reason) => return Err(reason)` | collect_keeps_partial_results（1 红） | 部分结果分支被锁 |
| 3 | `degrade_html` 的 `Ok(_) => HealthyEmpty` → `SourceError` | state_04（1 红） | 健康空归因被锁 |
| 4 | `MAX_PAGES` 10 → 3 | collect_stops_at_page_cap（1 红） | 页上限被锁 |
| 5 | 真因链 `"{jr}；HTML 层: {h}"` → `"{jr}; html: {h}"` | state_03 / collect_attributes（2 红） | 真因链格式+分层被锁 |
| 6 | `searxng::parse` 遇 `unresponsive_engines` 返回空集 | state_05（1 红） | 部分引擎挂不得丢结果被锁 |
| 7 | 去掉 `collected.truncate(cfg.limit)` | collect_concatenates（1 红） | limit 收口被锁 |
| 8 | JSON 命中也走 `degrade_html` | state_01 / state_05 / collect_keeps_partial_results / collect_stops_at_page_cap / collect_concatenates（5 红） | 命中路径不得误入降级被锁 |

8/8 变异均被至少一个用例抓住；无「恒绿钉子」。

---

## 四、验证命令与关键输出

```
$ cargo test --test fallback_matrix -- --nocapture
running 10 tests
test state_01_json_hit_marks_provider_searxng_without_html_degrade ... ok
test state_05_partial_engine_failure_still_returns_available_results ... ok
test collect_attributes_failure_to_the_layer_that_failed ... ok
test state_03_both_layers_failed_is_fault_with_two_layer_cause_chain ... ok
test state_04_zero_results_is_healthy_empty_not_fault_or_degraded ... ok
test state_02_json_http_error_degrades_to_html_keeping_provider_searxng ... ok
test state_02b_json_non_json_body_degrades_to_html_keeping_provider_searxng ... ok
test collect_keeps_partial_results_when_later_page_fails ... ok
test collect_concatenates_pages_in_order_and_truncates_to_limit ... ok
test collect_stops_at_page_cap_without_overfetching ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
```

```
$ cargo test        # 全量（改动后）
lib.rs       : 130 passed / 0 failed / 0 ignored
main.rs      : 151 passed / 0 failed / 4 ignored（live 浏览器用例，#[ignore]）
batch_not_configured : 2 passed / 0 failed
cli_golden_bytes    : 6 passed / 0 failed / 1 ignored
fallback_chain      : 10 passed / 0 failed
fallback_matrix     : 10 passed / 0 failed      ← 新增
fetch_redirect_flush: 3 passed / 0 failed
json_golden_compat  : 2 passed / 0 failed
relevance_off_topic : 11 passed / 0 failed
site_qualifier_diag : 4 passed / 0 failed
doc-tests           : 0 passed / 0 failed
合计：329 passed / 0 failed / 5 ignored

$ cargo test        # 全量（改动前基线：把 tests/fallback_matrix.rs 移出后再跑）
合计：319 passed / 0 failed / 5 ignored      ← 与改动后同 0 failed，差值恰为新增的 10 条

$ cargo clippy --all-targets -- -D warnings
Finished `dev` profile ... ; CLIPPY_RC=0
```

- **改动前后**：基线 319 passed / 0 failed（移出本文件实测）→ 改动后 329 passed / 0 failed（+10 = 本文件 10 条），既有套件未被本次改动打红。
- **确定性**：本文件连跑 10 遍全绿（10/10，0 flaky）。无 sleep 等待、无执行序依赖、每用例自带 query 场景（路由表按 `q` 一字一场景，互不串味）。

---

## 五、side-effects 三态

| 态 | 内容 | 证据 |
|---|---|---|
| ① **零外部副作用** | 不打外网（全部请求打 127.0.0.1 回环假 SearXNG）、不启浏览器、不碰真实 SearXNG/Google/DuckDuckGo 实例 | 假实例用 `std::net::TcpListener` 绑 `127.0.0.1:0`；SearXNG 客户端 `.no_proxy()`；成功路径不触发 DDG 分支；`--nocapture` 输出里只有降级提示行，无 DDG/Google 痕迹 |
| ② **进程内副作用（随进程退出归零）** | 回环监听端口（随测试进程退出释放）；`config::set_explicit_and_load` 写进程级 `CONFIG` OnceLock（只在本测试二进制进程内生效，不污染其他测试二进制/机器全局） | `tests/fallback_matrix.rs:54-78`；独立测试二进制 = 独立进程 |
| ③ **落盘副作用（已清零）** | 临时配置文件 `%TEMP%\gsearch-fallback-matrix-<pid>\gsearch.json`：写入 → 载入 CONFIG → **立即删文件+删目录** → 实测残留 0 | 代码内 `remove_file`+`remove_dir`；跑完 `ls %TEMP% \| grep gsearch-fallback-matrix` → 0 条 |

仓库内落盘仅两处：`tests/fallback_matrix.rs`（新增测试）与本报告。未改任何 `src/` 实现代码（`git diff src/search.rs src/searxng.rs` 为空）；未改 `tests/cli_golden_bytes.rs`（另一单负责）；未在仓库根留任何临时脚本。

---

## 六、与既有 `tests/fallback_chain.rs` 的关系

- 既有文件（v0.3.0 提交，10 个用例）已覆盖 collect 内核的部分主题（翻页上限/截断/跨页去重/部分结果/健康空/真因链）。
- 本文件的断言**与既有用例不重复**，落在既有文件没锁的观测面上：降级后 provider 不漂移、命中路径不误入降级（HTML 层零请求）、真因链**逐层归位**、部分引擎挂（`unresponsive_engines`）仍出结果、零结果 → `run.status` 的 no_results/filtered_empty 映射与「不得=degraded」、页上限**不越界多取**（无第 11 页请求）、跨页拼接的**完整序列**断言。
- 两文件各自独立进程、独立假实例，互不影响。

## 七、遗留建议（不在本单范围）

1. **态③ Google 分支自动化**：需 src 抽纯函数 `decide_after_fail(...)`（详见第一节盲区说明）——建议 PM 单开 src 改动单。
2. `try_searxng` 的 DDG 插层（`provider="duckduckgo"`）目前**零自动化**，与态③同因（无注入边界），可与上一条一并解决。
3. `MAX_PAGES=10` 是实现常量（`src/search.rs:21`，非 pub），本文件以「实测 10 页 + 无第 11 页请求」的方式锁行为；若该常量有意变更，需同步改本断言。
