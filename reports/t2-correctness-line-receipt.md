# T2 正确性线回执（q01 / ih1 / 6a6 / vw2 / day + 8lp 协作项）

**结论：五 issue 全部落地并双验收通过；额外完成 T1 契约的 8lp A（JSON compact ×3）/②（fetch message 去 URL）/③（doctor 值类结构化）。**
**VERDICT: DONE — code-level（cargo check --lib 0 error；cargo test verify:: 17/17、config:: 8/8 全绿）+ end-to-end（真实 exe 五组命令输出见下）均通过。**

## 改动清单（owner 区内）

| 文件 | 内容 |
|---|---|
| build.rs（新建） | q01：vergen-gitcl 10 注入 VERGEN_GIT_DESCRIBE（tags=true, --always 兜底）+ VERGEN_GIT_SHA（short）；Emitter 默认无 .git 不 fail |
| src/build.rs（新建） | q01：`version_line()` = `{ver} (git:{describe})`，env 缺席（tarball）`{ver} (dev)`；LazyLock 缓存 |
| src/lib.rs | +1 行 `pub mod build;` |
| Cargo.toml | +[build-dependencies] vergen-gitcl = "10" |
| src/verify.rs | ih1：ssl_valid 输出改 Option 派生（Probe::ssl_output，按最终 URL scheme / 失败类别），http 整键缺席/人读 n/a；6a6：ProbeJson 显式字段序（不 flatten）+ verdict/error_detail，单条与批量同构，传输层失败 JSON 不再只出 stderr；人读批量表加 verdict 列；8lpA：print_json compact |
| src/config.rs | vw2：自动发现路径解析失败 → stderr WARN + PARSE_FAILURE 登记 + 回退默认（显式 --config 保持硬错误）；`parse_failure()` 供 doctor |
| src/main.rs（仅 cmd_doctor + Doctor 结构区） | vw2：searxng 项解析失败显式 FAIL（rc=1 走既有 fail 汇总）；8lp③：DoctorCheck 加 value 字段（message 空则缺席），ok 值类检查数据进 value，人读经 human_line 拼回旧格式；8lpA：DoctorOutput compact |
| src/fetch.rs | day：selector 错误 Display 人话化（去 EmptySelector/Please report）；8lpA：批量 JSON compact；8lp②：JsShell message 去重复 URL |
| （T1 侧）main.rs Cli derive | `version,` → `version = gsearch::build::version_line()`（我禁碰区，经 T1 落地） |

## 端到端验证（真实跑，cwd=D:/Project/gsearch-rs）

### a. 版本烧录（q01）
```
$ ./target/debug/gsearch.exe --version
gsearch 0.2.9 (git:v0.2.9-7-g977d041)
rc=0
```
（首跑曾输出 `gsearch gsearch 0.2.9 ...` 双名——clap 自拼 name 前缀，version_line 去掉程序名后修复。tarball 兜底路径：`option_env!` 缺席 → `gsearch 0.2.9 (dev)`，构建不炸。）

### b. verify ssl_valid / 分类（ih1 + 6a6）
```
$ gsearch verify "http://neverssl.com" --json
{"status":200,"final_url":"http://neverssl.com/","redirect_chain":[],"latency_ms":1440,"verdict":"ok"}   ← 无 ssl_valid 键 ✓

$ gsearch verify "https://no-such-host.invalid" --json ; echo rc=$?
{"status":0,...,"verdict":"dns_error","error_detail":"DNS 失败 (curl exit 6)"}
verify DNS 失败 (curl exit 6)
rc=4

$ gsearch verify "https://api.github.com/zen" "https://no-such-host.invalid" --json
[{"status":200,...,"ssl_valid":true,"verdict":"ok"},{"status":0,...,"verdict":"dns_error","error_detail":"DNS 失败 (curl exit 6)"}]   rc=1

$ gsearch verify "http://example.com" --human
ssl_valid:   n/a          ← http 不再谎报 true ✓

$ gsearch verify "https://api.github.com/zen" "https://no-such-host.invalid" --human
url                           status    ssl      verdict  latency_ms
https://api.github.com/zen       200   true           ok         363
https://no-such-host.invalid       0    n/a    dns_error          63    ← status:0 坍缩消除 ✓
```
timeout 分类另证：neverssl 抖动窗口 `verify http://neverssl.com --human` → `verify 超时 (curl exit 28)` rc=5。

### c. 毒化 gsearch.json（vw2）
```
$ printf '{"searxng_url": (broken' > gsearch.json && gsearch doctor 2>&1
WARN: 配置文件存在但解析失败，已忽略: gsearch.json (expected value at line 1 column 17)
{"checks":[...,{"name":"searxng","status":"fail","message":"配置文件存在但解析失败（已忽略，回退默认）: gsearch.json (expected value at line 1 column 17)"}],...}
rc=1（fail_count=2，另一项为当轮 network 超时抖动）

$ gsearch doctor --human   # 人读分支同样 [FAIL] 该项；值类检查拼回旧格式：[ OK ] Chrome: C:\...、[ OK ] 出口 IP: 61.144.188.80

$ cp gsearch.json.t2bak gsearch.json && gsearch doctor --human
[ OK ] profile 来自配置文件: default
[ OK ] SearXNG: HTTP 200, results=46, unresponsive_engines=3 (http://192.168.89.249:8888)   ← WARN 消失、探测恢复 ✓
（备份已删，文件复原）
```

### d. fetch 非法 selector（day）
```
$ gsearch fetch https://example.com --include "###" ; rc=1
error: CSS selector 无效: "###"（语法错误，应为合法 CSS 选择器如 "#main, article"）
（用 --verbose debug 查详细）
```
无 EmptySelector / Please report this to the developer ✓

### code-level
- `cargo check --lib`：0 error 0 warning（我的 lib 侧文件；全量 --all-targets 曾被 T1 在途编辑打挂，错误全部位于其区域，修完后 bin 构建通过）
- `cargo test verify:: --lib`：**17 passed / 0 failed**（新增 4 测：verdict_names_match_exit_code_semantics / error_detail_classifies_failures / ssl_output_never_lies_for_http / probe_json_omits_ssl_valid_for_http）
- `cargo test config:: --lib`：**8 passed / 0 failed**（含显式路径缺失硬错误回归）
- `cargo build --bin gsearch`：debug exe 构建通过（e2e 载体）

### 附：fetch 批量 JSON compact + 失败元素结构（8lpA/② 载体）
```
$ gsearch fetch http://127.0.0.1:1/x http://127.0.0.1:2/y --json ; rc=2
[{"message":"fetch 拒绝私网地址 127.0.0.1（host=127.0.0.1）。如确需内网，请传 --allow-private 或设置 GSEARCH_FETCH_ALLOW_PRIVATE=1","status":"private_blocked","url":"http://127.0.0.1:1/x"},{...url=.../2/y}]
```
compact 单行数组 ✓、url/status/message 三键结构 ✓（JsShell 文案去 URL 为字面量改动，编译验证；未构造真实 JS 壳页）。

## side-effects 三态
- **预期内**：verify JSON 增 verdict 键（ssl_valid http 缺席）；doctor JSON 值类检查 message 缺席改 value；verify/fetch/doctor JSON 全部转 compact 单行；fetch JsShell message 不再重复 URL；批量人读表多一列。均为 8lp/6a6/ih1 契约要求，T1 已同步。
- **意外发现并已修**：--version 双名 bug（见 a）。
- **超出任务的新增**：8lp A/②/③ 三项（T1 在 IRC 明确派给我，带 spec）；src/lib.rs 1 行模块声明（build.rs 落位所需）。

## 未做 / 残余风险
- 全量 clippy/test/release 构建未跑（契约归 PM 合并态）；图谱索引未动（禁令）。
- profile_source 的 **env 来源** Ok 分支保留散文未值类化（与 config 来源人读 label 不同，无法从 name+value 无歧义拼回）——已向 T1 报备，如需覆盖需加 label 字段。
- doctor 的 network/SearXNG 两项在验证窗口出现外部依赖抖动（google:443 超时 / SearXNG 偶发探测失败；同窗口另一轮 SearXNG results=46 成功）。探测逻辑未动，判定为环境波动非回归。
- 刹车依赖：`cargo test verify::` 曾因 T1 在途编辑短暂受阻，T1 修复后重跑通过。
- SKIP「未配置」分支未被 e2e 直接覆盖（本机配置文件含 searxng_url；分支逻辑未改动，仅包入 else），由编译+毒化/恢复两侧入口覆盖。

## 汇报格式附件
已查：rule://rust（items-after-test-module/管道吞码/LazyLock 命中并遵守）+ bd show 五 id + 8lp + docs.rs vergen-gitcl（API 实证）+ bd memories（vergen 无历史）。
code-simplifier 自检：二次清扫改动 0 处（写作时已按清单执行：显式 match、无嵌套三元、guard 式早返回、注释讲为什么、测试断言原文未动）/ 触碰禁区 0 / 全程 diff +226/-60（功能实现体，非清扫 pass）。
silent-failure 备注：本任务主题即反静默失败——config 解析失败从「吞成默认」改为 WARN+登记+doctor FAIL；verify 传输失败从「仅 stderr」改为结构化 verdict+error_detail；无新增空 catch/危险 fallback（option_env!→dev 为合法场景的显式降级）。
自我评估：准确性 5/5（每条声明有上方真实命令输出或文件行号佐证，含一次自曝双名 bug 修复）；完整性 5/5（五 issue 全验收 + T1 契约三协作项 + 未做清单显式）；清晰度 4/5（profile_source env 例外与 network 抖动定性需读者对照 IRC 上下文）；可执行性 5/5（复现命令逐条给出）；简洁性 4/5（回执完整优先于紧凑）。
沉淀：C:/Users/Begonia/.omp/agent/rules/rust.md ← clap 自定义 version 双名拼接 + vergen-gitcl 10.x bon builder/--always/option_env! 兜底两条通用坑。
