# 测试缺口静态审计（第二轮 · tests-gap）2026-10-10

- 范围：src/ 22 文件，静态分析（只读，未跑 cargo test / 未构建，遵守并行 target/ 纪律）
- 工具：serena-cli 8 次（overview ×5、函数清单 grep 降级 ×3——overview 输出含变量噪声过大，函数清单类查询用 grep 更省）；其余 read 分段
- 口径：一个 good test, not coverage。每条缺口 = 风险位置 + 缺什么 + 会漏的 bug 案例 + 建议测法

---

## 一、缺口清单（按风险降序）

### P0-1 `search.rs:322-364` try_searxng —— 回退链五态决策矩阵零锁定
- **风险逻辑**：SearXNG→DDG→Google 回退链的全部切换条件都嵌在 async fn 里：NotConfigured 门 / Results / DDG 非空→provider="duckduckgo" / DDG 空+precheck 通→FallbackGoogle（fault=仅 SourceError）/ HealthyEmpty+precheck 不通→HealthyEmpty（不熔断）/ SourceError+precheck 不通→CircuitBroken。`google_fallback_precheck`、`duckduckgo::collect`、`searxng_collect` 三个硬网络依赖让决策体不可直接单测。
- **会漏的 bug**：`fault` 改成 `Some(reason)` 无条件赋值（去掉 `matches!(fail, SourceError(_))` 限定）→ 源健康零结果在 Google 回退也空时被误标 `searxng_degraded`，直接破坏 zc6 元审计硬约束（熔断状态 snake_case 已锁但归因布尔没锁）；或 DDG Err 分支误写成 return → DDG 瞬时失败跳过 Google 预检直接熔断。现有 15 个 search.rs 测试全在决策体外围（circuit_diag/SearxFail 映射/is_captcha）。
- **建议测法**：把 Err 分支决策体抽成纯函数 `decide_after_fail(fail: SearxFail, ddg: DdgOutcome, precheck_ok: bool) -> SearxngAttempt`（DdgOutcome=枚举：Hit(vec)/Empty/Err(String)），5 行决策表各一测；网络层原样留在 try_searxng 壳里。

### P0-2 `search.rs:383-428` searxng_collect —— 翻页/部分结果/健康归因内核零锁定
- **风险逻辑**：① pageno 1..=MAX_PAGES 凑 limit；② json Err→degrade_html 并拼全真因链 `"{jr}；HTML 层: {h}"`；③ **双空但已有部分结果 → break（有多少用多少）**；④ 循环耗尽仍空 → `SearxFail::HealthyEmpty`（把"各页结果全被去重掉"归源健康，是 o1p 三态的输入）；⑤ truncate(limit) 收口。
- **会漏的 bug**：分支③回归成 `return Err(reason)` → 第 2 页起任何失败会丢光第 1 页已得结果（batch/单查同时丢数据）；分支④回归成 SourceError → 健康实例的重复结果页误触发熔断，整个回退链语义翻转。这些都是纯控制流，一行改动即坏，且没有任何测试能红。
- **建议测法**：抽 collect 内核 `collect_pages(fetch_page: impl Fn(u32) -> Future<Result<Vec<SearchResult>, String>>, limit)`，闭包注入假分页序列（页1有3条/页2Err/页1重复URL），断言部分结果保留与 HealthyEmpty 归因。

### P1-3 `search.rs:474-505` degrade_html + warn_searxng_fallback —— 真因链/提示文案无锁
- **风险逻辑**：`SourceError(format!("{e:#}"))` 分类判定（HTTP 200 空=HealthyEmpty、其余=SourceError）+ json/html 真因链拼接格式 + 403 时追加的 format=json 运维提示文案。这些字符串进 run.message 与 stderr（盲测系列逐字对照过的诊断行契约）。
- **会漏的 bug**：degrade_html 把 `Ok(_) => Err(HealthyEmpty)` 误改成 SourceError → 每次健康空结果都打熔断诊断行；403 hint 子串（`err.contains("403")`）被错误链格式变化破坏后静默消失。
- **建议测法**：warn 文案拼装抽纯函数 `fallback_warn(base, err) -> String` 表格测试；degrade 分类随 P0-2 内核注入覆盖。

### P1-4 `search.rs:447-470` run_batch —— 并发帽 32 + slots 手工保序无测试
- **风险逻辑**：`buffer_unordered(cap)` 后靠 `slots[pos]` 重排恢复输入序（批 JSON 数组顺序契约：与输入一致）。types.rs 锁了 BatchEntry 序列化但没锁顺序——顺序是 run_batch 的行为。
- **会漏的 bug**：slots 重排被删（直接 collect 流序）→ 批输出顺序漂移，按序对位消费的 agent 全错；`Option::unwrap` 改动会 panic。batch_one 在未配置 searxng_url 时立即 Err（不走网络），是天然快速路径。
- **建议测法**：`run_batch(&["a","b","c"].…, ...)`（确保 CWD 无 gsearch.json / GSEARCH_SEARXNG_URL 未设——repo 根当前无 gsearch.json，cargo test CWD=包根满足），断言返回 Vec 与输入逐位对齐且全为 SourceError 未配置文案。

### P1-5 `shell.rs:234-251` parse_shell_read_opts —— 纯函数零测试
- **风险逻辑**：read/browse 共用的 flag 解析：--full/--json/--headings-only/--from K、未知 flag 报错、--from 缺值/非数字报错。与顶层 CLI flag 同名是明示契约（"agent 心智统一"）。
- **会漏的 bug**：match 改成 `_ => {}` 吞掉未知 flag → `read --ful`（打错字）静默按默认行为执行而非报错；--from 解析回归成字节截断。
- **建议测法**：10 行表格测试（合法四 flag 组合、未知 flag Err 含 cmd 名、--from 0 合法、--from 缺值 Err）。

### P1-6 `shell.rs:359-402` cmd_dl 参数解析 —— 内联解析器零测试
- **风险逻辑**：-o/--output 取值、多余位置参数拒绝（"不暗中吞掉"明示契约）、N 越界/0 报错、无 N 落 current_url、current_url 空报错、非数字 token 视为 URL 直下。
- **会漏的 bug**：`else if n_token.is_some()` 拒绝分支被删 → `dl 1 2` 静默下载第 2 条（错目标落盘）；N 越界检查回归 → panic/错位下载。
- **建议测法**：解析段抽 `parse_dl_args(&[&str]) -> Result<(Option<usize>, Option<&Path>)>` 纯函数后表格测试（M13 三处一致性基线已在 util.rs 锁住落盘段，解析段是缺口）。

### P1-7 `shell.rs:65-306` REPL 状态整替语义（last_results/last_snap/current_url）—— context 点名回归敏感区，零锁定
- **风险逻辑**：① `cmd_search` 空结果 → `last_results = []`（整替非保留）；② CaptchaTimeout → swap headless + page 重建 + `current_url.clear()`；③ `cmd_snap` 空列表也整替（旧 @eN ref 失效）；④ `cmd_click` N 越界/0 报错；⑤ page.evaluate("1") 探活失败重建 page。
- **会漏的 bug**：「空 search 后 `click 1` 用旧结果跳转」（旧结果复活）——只要 cmd_search 整替语义回归成"空时保留旧 results"，④ 的越界报错就不触发，agent 会点到上一条查询的死链接。这正是 REPL 状态类最经典的回归。
- **建议测法**：状态转移抽 `apply_search_outcome(&mut ShellState, outcome)`（ShellState 去掉 Browser 字段后的纯状态体）单测整替/清空；或最低限度加 `#[ignore]` live test：search 空词路径后 click 1 必 Err。

### P1-8 `fetch.rs:211-242` build_client redirect 每跳 SSRF 门 —— 接线无测试
- **风险逻辑**：`Policy::custom` 闭包对**重定向每一跳**做 resolve_host+is_private_ip 判定（门的存在意义就是防 302 中转 SSRF）。gate_check/is_private_ip 本体有全段表测试，但"每跳真的会走 Policy"这一接线零测试。
- **会漏的 bug**：Policy 被改成只查首跳（或 `attempt.follow()` 分支漏了私网判定）→ 公网 URL 302 到 169.254.169.254 静默放行，SSRF 门名存实亡——所有 gate_check 单测仍全绿。
- **建议测法**：仓内已有先例（verify.rs timeout 测试起 TcpListener）。起本地回环双跳 302（127.0.0.1 本身即私网，命中 Policy 拒绝路径），断言 client.get 首跳 200 正常、302 跳另一回环地址被 Policy 拒。

### P2-9 `verify.rs:299-341` 人读输出（print_report/print_batch/ssl_cell/transport_exit）—— stdout 逐字节契约只锁了 JSON 半边
- **风险逻辑**：--human 表格列宽（`w = max(url.len())`）、`ssl_valid` 单元格 n/a/false/true、传输失败人读仅 SSL 出报告、`transport_kind` 分类名。本仓 stdout 逐字节断言是保命惯例（17 轮盲测正确性零 bug 的基础），verify 的 --json 已锁而 --human 完全裸奔。
- **会漏的 bug**：列宽算式或 verdict 列名改动 → 批量人读输出变形，脚本对列消费断裂；`transport_exit` 的 `verdict == 3` 条件回归 → SSL 失败人读路径不再出报告。
- **建议测法**：print_* 改为 render 返回 String + 薄打印壳（与 fetch fetched_json 同手法），对 render 逐字节断言；ssl_cell 直接表格测试（纯函数，现在就能测）。

### P2-10 `config.rs:68-119` load_from_disk 候选顺序 + PARSE_FAILURE 记录 —— 发现链零锁定
- **风险逻辑**：三级发现顺序（CWD gsearch.json → exe 旁 → ~/.gsearch/config.json，首个存在者胜）+「自动发现解析失败 → warn+忽略+`parse_failure()` 记录（doctor 显式 FAIL 的数据源，vw2）」。现有 8 个 config 测试全在单键解析层；顺序测试自述"EXPLICIT 是全局静态喂不进"而复刻了两步逻辑——复刻不是锁定。
- **会漏的 bug**：候选顺序回归（exe 旁优先于 CWD）→ 开发者目录的调试配置被 exe 旁旧配置压住且无任何测试红；PARSE_FAILURE 忘 set → doctor 对坏配置静默 PASS（vw2 修复失效且不可见）。
- **建议测法**：load_from_disk 抽 `load_from_candidates(candidates: &[(PathBuf, bool)])` 内核（现签名变包壳），tempdir fixture 测：三级顺序、坏 JSON+自动发现→默认值+parse_failure=Some、显式坏 JSON→Err。

### P2-11 `searxng.rs:210-251` probe() 响应解析 —— doctor 第 5 项数据源无解析测试
- **风险逻辑**：ProbeResponse 的 `unresponsive_engines` 是 **[engine, error] 对数组**（实测形态，只数个数）；非 JSON 时错误串带响应头 120 字符（区分 format=json 未启用 vs 风控页）。doctor 的「TCP 活但查询零结果」盲区检测全靠这个解析。
- **会漏的 bug**：SearXNG 升级后 unresponsive_engines 从数组变计数器/对象 → serde 解析 Err → probe 恒 Err → doctor 把健康实例报成探测失败（或反之静默 0），watchtower 更新场景正是本仓高频事故源。
- **建议测法**：解析段抽 `parse_probe_json(text) -> Result<SearxngProbe, String>`，喂三种 payload：正常数组形态（含 [engine,error] 对）、unresponsive 缺失、HTML 挑战页（断言错误串含响应头片段）。

### P2-12 `browser.rs:521-553` swap_to_headed/headless —— handler abort 顺序不变量零锁定
- **风险逻辑**：M16 死锁修复的核心顺序「graceful_close → launch → **abort 旧 handler** → spawn 新 handler → 赋值 browser」。顺序错 = chromiumoxide 0.9.1 "receiver is gone" 挂死（项目实测事故）。纯时序，无断言可离线写。
- **会漏的 bug**：重构时 abort 与 spawn 换位（或丢 abort）→ 编译过、单测全绿、运行时按条件概率挂死——正是 v0.2.3 带死锁发布的同类形态。
- **建议测法**：`#[ignore]` live test（postproc_live 先例）：launch(headless)→swap_to_headed→swap_to_headless→open_page→evaluate("1") 全链 Ok。CI 不跑但手工验证有锚；顺带把「abort 必须在 spawn 前」写成注释级不变量。

### P2-13 `browser.rs:712-767` launch_with_retry 第二层 fork 接线 —— 条件组合无测试
- **风险逻辑**：退避表 [1,3,6,10,15] ×5 轮后，`is_lock_collision_error && is_default_profile_path && try_fork_profile 命中` 三条件与才 fork-rebuild。三因子各自有测试，**合取接线**没有。
- **会漏的 bug**：`is_default_profile_path` 条件被删 → 用户自定义 profile（work，含私有登录态）被误 fork 拷贝；fork 失败被吞成 Ok。
- **建议测法**：把 `Browser::launch` 依赖经闭包注入（再抽一层 `retry_loop(launch_fn, rebuild_fn, ...)`），用 always-Err launch 桩断言 fork 分支调用次数与最终 lock_failure_msg；BACKOFF 表长度进断言防误改总时长。

### P3-14 `search.rs:508-560` similar() 加权重排 —— 算子有测、合成无测
- tokenize/host_of/title_overlap/split_site_keys 均已锁；但「词重合×2 + 同域×1 + stable 保 SearXNG 序」的合成与超采样 limit×3、8..=15 窗无测试。会漏：两权重对调（同域压过词重合）不红。建议抽 `rerank(hits, src_host, keywords) -> Vec<SimilarHit>` 纯函数测试。

### P3-15 `postproc.rs:100-180` SNAPSHOT_JS / page_snapshot / wait_content_stable —— j44 判稳本体零锁定
- marker 连续两次相同判稳（-32000/晚跳转修复本体）+ SNAPSHOT_MAX_TEXT_CHARS 占位符替换无测试。会漏：占位符名漂移 → JS 原样注入（运行时 JS 语法错，报错点远离根因）。最低成本：静态断言替换后 JS 无 `{max}` 残留 + 含双读 marker 逻辑；轮询时序留给 postproc_live。

### P3-16 `main.rs:1147-1179` record_check / human_line —— doctor 人读逐字节契约无测试
- 注释明示「格式与旧版逐字节一致」，JSON 侧有解析测试，人读侧 render 零锁定。会漏：doctor --human 输出格式漂移。建议 human_line 纯函数表格测试（message 非空透传 / value 拼回散文两分支）。

### P4-17 `output.rs:13-27` truncate_snippet + print_text 人读格式
- snippet 按字符截断（160，多字节字符边界）无人测；print_text 的 `N. title [class]\n url\n snippet` 人读格式无人测。成本极低：truncate_snippet 3 行断言（中文 161 字、恰好 160、空串）；print_text 可暂缓。

### P4-18 `update.rs:41-66` cmd_update 比较分支 / `general.rs:549` is_dns_error
- cmd_update 的「本地≥远端不降级 / 非 semver 不可比对」分支需网络，建议抽 `compare_versions(local, remote_tag) -> Decision` 纯函数后一行测试。is_dns_error 的 WSA 11001/11002/11004 兜底需构造 reqwest 错误链，成本高、收益低，建议仅加注释标注未测。

---

## 二、已覆盖 / 缺口对照表

| 文件（行数） | 已锁契约（测试数） | 缺口 |
|---|---|---|
| types.rs (~500) | JSON 信封逐键：meta 缺席语义、run status snake_case 全枚举、domain_class 末键、BatchEntry/BatchEnvelopeV2 键契约、null 缺席、score 透传、truncated_detail、browser 键移除（11） | — |
| output.rs (82) | strip_ansi（1） | P4-17 |
| parse.rs (223) | parse_serp 单条/流式配对/去重保首/空页、壳 URL 解包表、/url?q= 端到端（6） | absolutize 无直接表测（经 searxng HTML 测试间接覆盖，可接受） |
| searxng.rs (393) | JSON 解析真实形态/null 兜底/空 results/非 JSON 报错、HTML 容器+walk 兜底、build_url 逐字节含 time_range（8） | P2-11 probe 解析；search/search_html HTTP 层（可接受，reqwest 薄壳） |
| duckduckgo.rs (342) | 容器+walk 解析、limit 截断、广告跳过、real_url 表、percent_decode、form_body df、classify_curl_result 回退语义、curl_args 指纹（9） | is_challenge 第二文案变体（"bots use DuckDuckGo too"）未见独立断言——低风险，classify 测试顺带补 |
| search.rs (860) | is_captcha/unusual_traffic（recaptcha 脚本不误判）、SearxFail 三态、circuit_diag、serp_url 逐字节、recency 映射、similar 派生词、urlencode fuzz（15） | **P0-1、P0-2、P1-3、P1-4、P3-14**（回退链与翻页内核是全仓最大无测区） |
| fetch.rs (~2900) | ~45 测：include 渲染、JSON path 全家、extract_text 边界、binary/pdf、JS 壳、process_html、**SSRF 门全段表**（gate_check 拒/放行、is_private_ip v4+v6、accumulate_chunk、should_retry 矩阵、scheme 白名单）、host 路由、collapse 保代码、raw 逐字、meta 键、默认值 | P1-8 redirect 每跳接线 |
| verify.rs (660) | ~19 测：headers 多 hop/CONNECT/尾 3xx、final_url marker、curl exit 分类、exit_for_status 表、真 curl 回环超时 28→5、args、403/405 GET 回退、probe JSON 键、verdict/ssl 不说谎 | P2-9 人读半边 |
| browser.rs (~1250) | ~15 测：profile_name、redact_proxy、user_scope、fork 三态+目录形态+递归拷贝、锁碰撞关键词、竞态第二层目录唯一、stealth 序列、lock_failure_msg、init_script | P2-12 swap 状态机、P2-13 retry 合取接线、find_browser 候选表（环境相关，可接受） |
| shell.rs (605) | b64/文件名/parse_search_args（3） | **P1-5、P1-6、P1-7**（605 行文件 3 个测试，dispatch 表/cmd_read CAPTCHA 门/goto 超时均裸奔） |
| shell_snap.rs (209) | parse_click_target、find_snap_elem、format_snap_line、ref_id 分配（5） | snap_page/click_snap_elem JS 串（page 依赖，可接受） |
| skeleton.rs (~660) | ~17 测：阈值边界 5/10/30/50/51/100、first_sentence 中英、headings、pi 折叠、--from、excerpt opt-in、code 收集/预算/渲染 | — |
| postproc.rs (~1110) | ~13 测 + 1 ignored live：pick、login_wall_hit、cap_chars UTF-8/offset、cap_chars_json brace、GitHub 容器、render_read meta/dup、pi 过滤、open scheme/平台 | P3-15 判稳本体 |
| config.rs (217) | 8 测：键解析、malformed、缺文件、显式缺失、env 优先 | P2-10 发现链 |
| convert.rs (298) | ~13 测：表格/标题/链接/跳过标签/围栏保真（换行/撇号/where）/language、clean_markdown 全家、行首锚点 | — |
| util.rs (340) | 8 测：b64 向量+1MB roundtrip、**I9 安全门全表**、domain_class 表、dl 三处一致性基线 | — |
| general.rs (~615) | 13 测：DNS 分类、超时文案、settle 阈值、empty hint、scheme 白名单、私网门表、.. 穿越、resolve_dl_target、login_poll_decision 5 案例 | P4-18 is_dns_error |
| main.rs (~1460) | ~20 测：CLI 解析全家（batch/recency/范围/互斥/doctor/verify/similar/fetch）、site: 警告、blank query、humanize 三态、ip_drift 三态、help 无泄漏码 | P3-16 human_line |
| update.rs (90) | parse_semver3 表+数值序（2） | P4-18 cmd_update 分支 |
| stealth.rs / lib.rs / build.rs | 标记/jitter（2）；无逻辑可测 | — |

**结构结论**：纯函数层（解析器/分类器/URL 构造/安全门/序列化）覆盖扎实，与「stdout 逐字节断言」惯例一致；缺口集中在两类——**(a) 回退链/翻页/批处理等 async 编排层**（search.rs 最大，零锁定），**(b) REPL/浏览器状态机**（shell.rs 605 行 3 测、browser.rs swap/retry 接线）。两类共同的修法是抽注入边界把决策体从 I/O 里剥出来（本仓已有成熟先例：curl_args/classify_curl_result/login_poll_decision/human_line 全是这么抽出来锁住的）。

## Verdict

审计完成：18 条缺口（P0×2 / P1×6 / P2×5 / P3×3 / P4×2），已覆盖对照 22 文件。最高风险集中在 `search.rs` 回退链与翻页内核（状态/归因逻辑完全无测，一行改动可翻转熔断语义且不可见）与 `shell.rs` REPL 状态整替。未跑任何测试/构建（并行纪律），全部结论来自静态阅读源码与现有测试比对。
