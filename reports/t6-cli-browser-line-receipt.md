# T6b CLI + Browser 线回执（gsearch-rs-745 / 276 / yvz / 5a5-工具侧）

**VERDICT: 四条全部实现完成并有真实 e2e 证据；cargo check 0 error、本模块过滤测试 4/4 绿。待 PM 合并态验收（全量 clippy/test 归 PM）。**

基线 6c52a9b → 工作树。同库并行 T5b 区（search.rs/types.rs/lib.rs/main.rs Similar 子命令）非本线改动，未触碰。

## 改动清单

| Issue | 文件 | 内容 |
|---|---|---|
| 745 | src/update.rs（新建 ~100 行） | `gsearch update`：GET `api.github.com/repos/Be90nia/gsearch-rs/releases/latest`，tag 剥 v 前缀手写 semver 三段比对 CARGO_PKG_VERSION（**零新依赖**，self_update crate 按派单裁掉——不做自替换）；已是最新 / 有新版 vX.Y.Z（本地 A.B.C）+ release 页 URL + `cargo install --git` 指引；网络失败 anyhow 一行 stderr rc=1；10s 超时；尊重 GSEARCH_PROXY |
| 745 | main.rs | `mod update;` + `Command::Update` + 派发 `update::cmd_update(proxy.clone())` |
| 276 | src/browser.rs | ① cleanup_stale_locks os-32 告警改「被浏览器进程或杀软持有」+ 锁文件路径 + handle.exe/资源监视器指引（原「被活 Chrome 持有」误导，实报是 msedge）；② 新增 `diagnose_lock_holders()`：PowerShell Get-CimInstance 列命令行含 profile 路径的 chrome/msedge PID；③ `launch_with_retry(config, profile)` 每轮重试打出**真实错误**（可见进度）+ 打尽后列持锁 PID（禁 taskkill /IM 全杀）/ 无命中指向杀软句柄排查 |
| yvz | src/general.rs | `dl_direct`：gate_check DNS 类失败 → 经（可能带代理的）client **复核一次 HEAD**，复核仍 DNS 类失败才 `bail!("域名不存在（DNS 解析失败），跳过浏览器下载")` rc=1；复核成功/超时/SSL 一律回退 browser 老链路（防纯代理解析环境误伤）；新增 `is_gate_dns_error` / `is_dns_error`（错误链遍历 + WSA 11001/11002/11004 兜底，locale 无关） |
| 5a5 | main.rs | `check_exit_ip_drift()`：当前 IP 存 `<profile>/last_exit_ip`；首跑无记录→静默并落盘；同 IP→静默；变化→附加 `exit_ip_drift` 检查项 WARN「出口 IP 自上次检查已变化（A→B）——VPN/代理切换或 IP 信誉重置信号」 |
| 文档 | README.md | 新增「### `gsearch update`（版本检查）」节；doctor 节出口 IP 条目补 drift 行为说明 |

测试：update.rs 2 个（semver 解析/数值比较）、general.rs 1 个（gate DNS 分类）、main.rs 1 个（漂移三态含无 profile 目录分支）。

## 验证（真实命令输出）

```
$ cargo check                                    → Finished dev profile, 0 error 0 warning
$ cargo test --bin gsearch -- semver3 gate_dns exit_ip_drift
  → 4 passed; 0 failed; 54 filtered out
```

**e2e-a update**：`./target/debug/gsearch.exe update` → `已是最新（本地 v0.2.9，远端 v0.2.9）`，0.78s，rc=0（真实 GitHub API）。

**e2e-b dl DNS fail-fast**：`dl https://no-such-zzz.invalid/file.pdf` → **84ms**（修复前 6.1s 全链），rc=1，`error: 域名不存在（DNS 解析失败），跳过浏览器下载: …`，chrome.exe 进程数 0→0。

**e2e-c profile 锁**：真实持锁 Chrome（--user-data-dir=测试 profile）下 browse →
- `[WARN] 残留锁被浏览器进程或杀软持有: D:\…\t6-lock-test\lockfile (os error 32)。排查: handle.exe "…", 或资源监视器（性能→CPU→关联的句柄）搜索 profile 路径`
- 5 轮重试各打真实错误 `ExitStatus(21)`（38s 全程可见进度）
- `error: 启动浏览器失败（已重试 5 轮共 35s）…` + `持有该 profile 的进程（按 PID 精确处理，禁 taskkill /IM 全杀）: PID=34108 chrome.exe …` 共 8 个 PID
测试后按列出的 PID 精确清理，进程数归零，临时 profile 已删。

**e2e-d doctor 漂移三态**：第 1 次（无记录）→ checks 无 exit_ip_drift、last_exit_ip 落盘 61.144.188.80；第 2 次（同 IP）→ 静默（warn_count 1 项来自既有 edge/network 检查，与改动无关）；第 3 次（伪造 last_exit_ip=205.251.242.103）→ `[WARN] 出口 IP 自上次检查已变化（205.251.242.103 → 61.144.188.80）——VPN/代理切换或 IP 信誉重置信号`，且该次运行自身把记录刷新为真实 IP（再次运行恢复静默——状态机自洽）。二次分支另有代码级单测 exit_ip_drift_three_states 覆盖（含无 profile 目录分支）。

## Side effects 三态

- **预期外文件改动**：无（本线仅动 update.rs 新建 / browser.rs / general.rs dl_direct 区 / main.rs 四区 / README 两节；git diff 中 search.rs +137 / types.rs / lib.rs / main.rs Similar / AGENTS.md 等为 T5b 与仓库级变更，非本线）
- **残留进程/临时物**：无（测试 Chrome 已按 PID 清零、t6-lock-test 已删、last_exit_ip 为真实 IP 无需清理）
- **配置/依赖变更**：无（零新依赖；Cargo.toml/lock 未动）

## 未做（明确）

- 745 issue 内「doctor 顺带查 latest（24h 缓存）stderr 一行提示」——派单未含，未做，issue 已留评论待拍板
- 5a5 路由器层（passwall NAT 注入）——工具侧无法覆盖，bd close reason 已写明需人工
- 全量 clippy/test / commit——按并行纪律归 PM 合并态

## 沉淀

`~/.omp/agent/rules/rust-pitfalls.md`「运行时陷阱」← DNS 失败分类禁只匹配英文错误串（Windows getaddrinfo 文案随 locale 变化，须错误链遍历 + WSA 11001/11002/11004 码兜底）。
