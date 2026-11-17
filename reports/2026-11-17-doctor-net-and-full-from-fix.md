# bd gsearch-rs-u45 + bd gsearch-rs-0z8：doctor network 分级 + search 侧 `--from` 不静默

**结论**：两项全部完成，双验收全过（code-level + end-to-end 各四条）。`VERDICT: PASS`

日期：2026-11-17 ｜ 执行：DoctorNetFix（实现子代理）｜ commit 归 PM

> **提交状态提示**：本任务的代码改动已被 PM 的行级 commit 工具误并入 `0209caca`（PM 操作失误，与本代理无关）。
> 「已被 0209caca 收录」= `src/main.rs` + `CHANGELOG.md` + `USER_GUIDE.md`；
> 「仍待 PM 提交」= `README.md`（2 处）+ `CHANGELOG.md`（把 0209caca 里错位在文件头的 3 条 entry 移回 `### Fixed` 段）。

---

## 1. 改动清单

### 1.1 已被 0209caca 收录

#### src/main.rs（doctor network 分级 —— u45）
| 位置 | 改动 |
|---|---|
| `main()`（:414） | `Command::Doctor` 分支 → `cmd_doctor(json && !human, proxy.clone())`（doctor 此前**完全拿不到**代理） |
| `cmd_doctor`（:1261） | 签名加 `proxy: Option<String>`；第 5 项 network 检查重写为「探测 + 分级」两段 |
| `probe_searxng_check`（新，:1449） | 原第 7 项 SearXNG 探测块抽成函数，返回 `(status, message, value, reachable)`；`reachable` = 主路径可用（端点有响应，零结果也算）。**探测提前执行**（分级需要），**记录仍在第 7 位** → 检查项输出顺序不变 |
| `probe_network_reach`（新，:1497） | 探测：直连 TCP `www.google.com:443`（2s，沿用旧目标与预算）→ 不通且配了代理则经代理再探一次 |
| `probe_via_proxy`（新，:1541） | 经代理探测 = 系统 curl 子进程（**house 同款**：`verify.rs` 已依赖系统 curl 走 `--proxy`，零新依赖）；`socks5://` → `socks5h://`（交代理侧解析域名，与 Chrome SOCKS5 行为一致）；`--connect-timeout 2 --max-time 4` |
| `classify_network`（新，:1582） | 分级纯函数：可达 → OK；不可达 + 主路径健康 → **WARN**（文案「需代理或出口受限」）；不可达 + 主路径亦不可用 → **FAIL** |
| `doctor_check_line`（新，:1425） | 人读行渲染抽成函数 → 人读形态与 `--json` 形态**同源**（同一 `DoctorCheck`），可单测 |
| `NET_TARGET` / `NetReach` / `SearxngProbe`（新） | 常量与探测结论类型 |

#### src/main.rs（search 侧 `--from` 不静默 —— 0z8）
| 位置 | 改动 |
|---|---|
| `from_is_consumed`（新，:490） | 纯函数：仅 `--browse N` 默认 md 形态消费 `--from`；`--full` / `--json` / `--headings-only` / `--read N`（snippet）/ batch 都不消费 |
| `warn_from_not_consumed`（新，:495） | stderr 一行提示（与 browse 侧 `general.rs:136` 同款文案口径） |
| `cmd_search`（:548） | `--from K > 0` 且当前形态不消费 → 打提示（单查询路径） |
| `cmd_search_batch`（:941） | batch 无浏览器路径 → `--from` 恒不消费，同样打提示（禁静默） |

> `src/postproc.rs:582` 的 `read_full`（`md_page_section(&title, &txt, 0)` 硬编码 0）**未改**：按 browse 侧既定口径，`--full` 形态不消费 `--from`（innerText 无空行分块），改为**出提示**而非消费——stdout 因此逐字节不变。

#### src/main.rs（回归锁，3 个新单测）
| 测试 | 锁的契约 |
|---|---|
| `classify_network_grades_by_main_path` | 直连通 → OK（message 缺席、value 承载）；直连不通 + 主路径健康 → WARN 且文案含「需代理或出口受限」；直连不通 + 主路径不可用 → FAIL 且文案含「经代理也不通」 |
| `doctor_human_line_and_json_status_agree` | 人读行标签 ↔ JSON `status` 一致；ok 值类检查 JSON 侧 `message` 键真缺席 |
| `from_is_consumed_only_in_browse_default_md` | `--from` 仅 `--browse N` 默认 md 形态消费 |

#### USER_GUIDE.md（§8 doctor 子命令）
新增「网络连通项分级」段（直连 → 代理 → 按主路径健康度分 WARN/FAIL）。

### 1.2 仍待 PM 提交

| 文件 | 改动 |
|---|---|
| `README.md`（:441, :445） | doctor 章节：「网络连通」条目改写为分级语义；退出码段补「`network` 分级后，仅直连被封但代理可用/主路径健康时 doctor rc=0（此前恒 1）」 |
| `CHANGELOG.md` | `### Fixed`（`[0.4.1]`）段首两条：**u45**（doctor network 分级 + 退出码语义变化 + JSON 键无新增/改名说明）、**0z8**（search `--from` 不再静默）。⚠ 该 diff 同时把 0209caca 错位在**文件头**（`# Changelog` 之后、`格式参考` 之前）的 3 条 entry（u45 / 0z8 / p8r）**移回 `### Fixed` 段**——PM 的行级工具按行号套补丁导致落点错位，本次一并纠正 |

---

## 2. 验证证据（真实命令 + 关键输出）

### 2.1 code-level：`cargo test`（全量）
```
test result: ok. 130 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # lib
test result: ok. 151 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out   # bin（含 3 个新单测）
test result: ok.  6 passed; 0 failed; 1 ignored                              # cli_golden_bytes
test result: ok. 10 passed; 0 failed                                          # fallback_chain
test result: ok. 10 passed; 0 failed                                          # fallback_matrix（同伴代理新增）
test result: ok.  3 passed / 2 passed / 11 passed / 4 passed                  # fetch_redirect_flush / json_golden_compat / relevance_off_topic / site_qualifier_diag
```
全量 **0 failed**；4+1 个 `#[ignore]` live 测试按惯例未跑。

> 一次中途观察：并行全量跑时 `tests/fallback_chain.rs` 曾 8 failed（假 SearXNG 返 500）——**非本改动引起**：单跑该文件 10/10 通过（两次），全量重跑亦 10/10；成因是并发构建/测试负载下假服务器线程被饿到超时（同伴代理并发跑 cargo + 我的 release 构建）。非本改动回归。

### 2.2 code-level：`cargo clippy --all-targets`
```
Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.34s
```
rc=0，无新增 warning。

### 2.3 改动前基线（代码级实证，非推测）
`git show 4eb9caf:src/main.rs`（本改动前 HEAD）：
```
1338-        Err(_) => (
1339-            DoctorStatus::Fail,          ← 直连超时无条件 FAIL（不读 GSEARCH_PROXY）
1340-            "网络连接超时 (www.google.com:443)".to_string(),
...
1466:    if fail > 0 { Ok(ExitCode::from(1)) } else { Ok(ExitCode::SUCCESS) }   ← FAIL ⇒ rc=1
```

### 2.4 end-to-end：doctor（验收 1 / 2 / 4）

**A1 —— 配了代理**（`GSEARCH_PROXY=socks5://127.0.0.1:10808`）：
```
$ GSEARCH_PROXY=socks5://127.0.0.1:10808 ./target/release/gsearch.exe doctor
checks: 7 | ok: 6 | failed: 0 | elapsed: 7.5s
- [OK] chrome: C:\Program Files\Google\Chrome\Application\chrome.exe
- [WARN] edge: Edge 不可用（msedge.exe 未找到；仅 Chrome 可跑）
- [OK] profile_writable: profile 可写: C:\Users\Begonia\.gsearch\profiles\default
- [OK] exit_ip: 61.144.188.80
- [OK] network: www.google.com:443（经代理可达：socks5://127.0.0.1:10808）
- [OK] profile_source: GSEARCH_PROFILE 未设置（默认 ~/.gsearch/profiles/default/）
- [OK] searxng: SearXNG: HTTP 200, results=32, unresponsive_engines=4 (http://192.168.89.249:8888)
RC=0
```
✅ **PASS**：无与 `[OK] searxng` 并列的裸 network FAIL；代理可达时 network 显示可达（value 标注经代理）。

**A2 —— 代理缺失**（`env -u GSEARCH_PROXY`，且 shell 内该变量本就为空）：
```
$ env -u GSEARCH_PROXY ./target/release/gsearch.exe doctor
checks: 7 | ok: 5 | failed: 0 | elapsed: 5.3s
...
- [WARN] network: 直连不可达 (www.google.com:443): 连接超时：需代理或出口受限——SearXNG 主路径可达，整体网络可用
- [OK] searxng: SearXNG: HTTP 200, results=39, unresponsive_engines=3 (http://192.168.89.249:8888)
RC=0
```
✅ **PASS**：表达为「直连不可达（需代理/出口受限）」级别的提示；与 `[OK] searxng` 并存时**整体不判 FAIL**（`failed: 0`，rc=0）。

**A3 —— `--json` 形态语义与人读一致**：
```
keys: ['checks', 'elapsed_ms', 'fail_count', 'warn_count']
fail_count= 0 warn_count= 1
  chrome ok | message= None | value= C:\Program Files\Google\Chrome\Application\chrome.exe
  edge warn | message= Edge 不可用（msedge.exe 未找到；仅 Chrome 可跑） | value= None
  profile_writable ok | message= profile 可写: ... | value= None
  exit_ip ok | message= None | value= 61.144.188.80
  network ok | message= None | value= www.google.com:443（经代理可达：socks5://127.0.0.1:10808）
  profile_source ok | message= GSEARCH_PROFILE 未设置（默认 ~/.gsearch/profiles/default/） | value= None
  searxng ok | message= SearXNG: HTTP 200, results=39, unresponsive_engines=3 (http://192.168.89.249:8888) | value= None
```
✅ **PASS**：**键无新增/改名**（`checks[].name` 恒为 `network`；`status` 仍四态 ok/warn/fail/skip；`message`/`value` 缺席语义不变）；人读行标签 ↔ JSON `status` 同源（单测 `doctor_human_line_and_json_status_agree` 锁）。

### 2.5 end-to-end：search `--from` 提示（验收 3）

```
$ ./target/release/gsearch.exe search "rust async" --read 1 --full --from 3   # A
$ ./target/release/gsearch.exe search "rust async" --read 1 --full           # B
$ ./target/release/gsearch.exe search "rust async" --read 1 --full           # C（同 B 再跑一次，量上游抖动）

stderr：A=203 字节 / B=0 字节 / C=0 字节
stdout：A=B=C=341 字节
$ cmp n_a.out n_b.out   → IDENTICAL     # 归一 elapsed（sed 's/elapsed: [0-9.]+s/elapsed: Xs/'）后
$ cmp n_b.out n_c.out   → IDENTICAL     # 上游无抖动
$ diff s_a.out s_b.out  → 仅 elapsed 一行（4.0s vs 1.2s），其余逐字节一致
```
A 的 stderr 全文：
```
[hint] --from K 仅 --browse N 默认 md 形态生效（跳过正文前 K 个空行分隔块）；当前形态（--read N snippet / --full / --json / --headings-only / batch 多查询）不消费 --from
```
✅ **PASS**：提示出现、stdout 与不带 `--from` 逐字节一致（归一 elapsed 后）。

**反向验证（不误发提示）**：
```
$ GSEARCH_PROXY=… ./target/release/gsearch.exe search "rust async" --browse 1 --from 3
stderr = 182 字节，仅 Chrome INFO 两行，**无 [hint]** ✅（默认 md 形态消费 --from，不误报）
$ GSEARCH_PROXY=… ./target/release/gsearch.exe search "rust async" --browse 1 --full --from 3
stderr 首行 = [hint] --from K 仅 --browse N 默认 md 形态生效… ✅（--full 形态不消费，出提示）
```

### 2.6 构建
`cargo build --release` rc=0（2m03s），`target/release/gsearch.exe` = 10,127,872 B（较 0.4.1 基线 10,115,584 B **+12,288 B ≈ 12 KB**；零新依赖）。

---

## 3. Side-effects 三态

- **预期内（修复面）**：
  - doctor `network` 项的 status 可能由 `fail` 变 `warn`/`ok`，message/value 文案改写；**键集合不变**。
  - **退出码语义变化**：仅「直连不通但代理可用 / SearXNG 主路径健康」时 doctor 由 **rc=1 → rc=0**（README 退出码表原本就写「doctor = 全 PASS 或仅 WARN」；此前恒 1 是 bug）。**其余退出码来源零改动**（chrome/profile/searxng 配置解析失败仍 FAIL → rc=1）。
  - `search` 在 `--from K > 0` 且形态不消费时**多一行 stderr 提示**；stdout 逐字节不变（stderr 不进 stdout 契约）。
- **无副作用面**：`search --browse N` 默认 md 形态消费 `--from` 的行为未变（不误发提示）；browse 子命令未动；`--json` 信封键无变化；golden bytes 6 passed；SearXNG 探针语义（`unresponsive_engines` / 零结果 WARN）未动；exit_ip / profile_source / chrome / edge 四项未动。
- **观察项（范围外，未动，留 PM 裁决）**：
  1. **`src/search.rs:651` 的 Google 回退预检仍只直连 TCP**（不读 `GSEARCH_PROXY`）——故 doctor 报「经代理可达」时，`search` 在 SearXNG 故障后的 Google 回退**仍会熔断**（预检直连不通）。这是既有的独立缺陷（源码内 ponytail 注释已登记），**不在本次范围**（非目标明列「不做代理回落新功能」），建议 PM 单开一条 bd 记录。doctor 的 network 项现在表达的是「公网出口可达性（含代理）」，不是「Google 回退预检会不会放行」。
  2. **doctor 墙钟 ~5–8s**，README 仍写「3 秒内完成 6 项自检」——**改动前就已超**（SearXNG 探针本身墙钟下界 ~3.03s + 直连 TCP 超时 2s + exit_ip）。本次新增的一次 curl 代理探测仅在直连失败时触发（~0.3–0.5s）。README 的「3 秒」属既有漂移，未在本次顺手改（避免扩大 diff）。
  3. **shell 子命令的 `read --full`** 同样不消费 `--from`（`src/shell.rs` 内联 innerText 路径，未走 `postproc::read_full`）——本次只覆盖顶层 `search`（契约范围），shell 侧同型缺口未动。
  4. **代理 URL 以命令行参数传给 curl**（`--proxy <url>`）→ 凭据形如 `http://user:pass@host` 时会出现在进程列表——与既有 `verify` 子命令同款暴露面（house 一致），未额外加码防护。

---

## 4. 临时产物清理

- 本代理的 e2e 产物（doctor/search 的 stdout/stderr 快照、归一文件）全部落在 `%LOCALAPPDATA%\Temp\gsearch-u45\`，验证后整目录删除（实测 `GONE`）；**repo 内零临时文件**。
- repo 根的 `.probe/`（73 个 `*.out`/`*.err`）为并发同伴代理（SkillSync）产物，本代理**未产生任何其中文件**；按 PM 指示已**逐文件清空**（`ls -a .probe | wc -l` = 2 = 仅 `.`/`..`）。目录本身因并发进程占用（Windows os error 32：某进程 CWD 在其中）暂时删不掉——**空目录 git 不跟踪**，故 `git status --untracked-files=all` 已无 `.probe` 条目，仓库视作已清。

---

## 5. 未做（明确不在本轮）

- 未改 `src/postproc.rs` 的 `read_full`（`--full` 不消费 `--from` 走「提示」而非「消费」，与 browse 侧口径一致）。
- 未改 `src/search.rs` 的 Google 回退预检（代理感知）——见 §3 观察项 1。
- 未动 shell 子命令的 `read --from`（见 §3 观察项 3）。
- 未改 doctor 的其它检查项、未改 exit code 的其它来源、未把任何代理地址/路径写死进代码或配置默认值。
- 未 commit（归 PM）；未动 bd issue 状态（归 PM）。
