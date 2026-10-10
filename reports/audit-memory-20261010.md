# gsearch-rs 内存专项深挖审计（2026-10-10）

范围：全库深浅拷贝逐点 + 泄漏面 + 动态实测。第一轮 PerfAudit 已报的 F-7（strip_summary_elements 全页克隆，fetch.rs:1189-1213）、F-8（collapse_blank 分配链，fetch.rs:1391-1449）不重复立项，本文仅做延伸量化。负优化预审：`git log --grep` perf/cache/revert/clone 零命中、`bd search` 优化/缓存零命中 → 无历史回退约束。

- 构建口径：`cargo build --release`（3m14s，当前 HEAD 全新构建，非缓存旧 exe）。
- 动态方法：PowerShell 5.1 harness（`_memtest/measure.ps1`，报告完成后已删）：`Start-Process -PassThru` 起进程，`Get-Process` 每 100ms 读 `PeakWorkingSet64`（内核生命周期峰值计数器，非采样近似），每 2s 记 `WorkingSet64` 曲线；wall 时间与裸跑交叉核对。
- 网络口径：本机直连出网全断（curl 全部 000:0，与 9-29 passwall NAT 教训同型）→ fetch 走 `--proxy http://127.0.0.1:10808`（实测 200）；浏览器路径走 Windows 系统代理（同端口，ProxyEnable=1）。**浏览器（chrome.exe 进程树）内存不在测量内**，只测 gsearch.exe。
- 页面样本：curl 实测 body——WWII 2,073,126B、Earth 1,763,957B、Mathematics 1,286,489B、History_of_China 1,034,664B、Language 903,460B、GitHub issue 440-518KB。
- 噪声口径：单进程峰值 WS 同一命令波动 ±6MB（14.5-38.8MB 首跑离群），关键点位均 ×3 取中位数；首跑（冷 exe）单独标注。

---

## 1. 克隆点三档表（全库 clone/to_owned/to_string/collect/into 逐点）

### 必要档（所有权/生命周期所需，不动）

| 位置 | 拷贝 | 说明 |
|---|---|---|
| fetch.rs:419-420 | `buf:Vec<u8>` → `from_utf8_lossy().into_owned()` | 1×body；happy path 可用 `String::from_utf8(buf)` 免拷，但 lossy 是对坏编码页的刻意容错，收益 1×B 不值改 |
| fetch.rs:1225 / skeleton.rs:84 / postproc 等处 `Html::parse_document` | scraper DOM arena | ~4-8×HTML 的树内存，DOM 解析固有成本，全库最大乘数；换流式解析是架构级改动，不属本次 |
| fetch.rs:1253 `tree_text` 输出 | 正文 String | 有 `with_capacity(4096)` 起步，产出必需 |
| postproc.rs:208 cap_chars / cap_chars_json | 截断产物拷贝 | ≤50KB（READ_BODY_MAX_CHARS，postproc.rs:28）有界 |
| search.rs:214/228/407、parse.rs:57、searxng.rs:156、duckduckgo.rs:129 | `seen.insert(r.url.clone())` | HashSet 需 owned key 且 r 本体随后保留，必要 |
| shell.rs:267 | `find_snap_elem(..)?.clone()` | 单个 SnapElem ~100B，微小 |
| fetch.rs:922/948 `Segment::Field(n).clone()` | 路径段名 | 字节级小串 |

### 可疑档（可免拷贝，量化后收益小或路径冷）

| # | 位置 | 拷贝路径 | 触发场景 | 量化 | 最小修复 |
|---|---|---|---|---|---|
| M-1 | fetch.rs:1400-1404（collapse_preserving_code）+ fetch.rs:1391-1449（collapse_blank 本体，F-8 延伸） | 每 prose 段 `seg.to_string()` → `collapse_blank` 内 `out` 重建 → 尾部 `out.trim().to_string()` 三连拷 | 每次 fetch/read 正文规整；无 pre/code 的典型页整段正文=一个 seg | 3×T 瞬时（T≈500KB 的 2MB 页 → ~1.5MB 瞬时）+ fetch.rs:1228 非 HTML 分支 `collapse_blank(html.to_string())` 再 +1×B（2MB） | `collapse_blank` 改收 `&str`；`trim().to_string()` 改为写回前算 trim 边界只 `push_str` 保留段——链路降到 1×T |
| M-2 | fetch.rs:771-773 | `f.text.clone().into()`（投影命中/JSON 路径把 text 克隆喂 serde） | `--json-keys`/JSON 源单命令一次 | ≤50KB 一次 | 后续不再用 f.text 时改 move（调整 f.title 取用顺序） |
| M-3 | fetch.rs:926/945 | `project_json_paths` 每命中路径 `node.clone()` | `--json-keys` 通配（GitHub comments N 条） | ≤50KB × 命中路径数，json-keys 源有界 | 叶子节点可借 `&Value` 序列化输出，免 Value 克隆；仅 json-keys 路径受益 |

### 可免档（零收益确认，列出防复查）

general.rs:182/main.rs:706 等 `proxy.clone()`/`query.clone()`——进程级一次性小串；launch_with_retry config.clone()——冷路径（仅启动失败重试）；shell_snap.rs:141-147 format_snap_line 小串克隆——SnapElem text 30 字符硬顶（shell_snap.rs JS `slice(0,30)`）；postproc.rs:167 marker `(s.title.clone(), s.visible_text.clone())`——6000 字符硬顶（postproc.rs:31）、≤10 轮判稳轮询，有界。

**结构布局**：SearchResult（3×String24 + Option<f64>16 + &'static str16 = 104B，8 对齐零填充）；MetaOutput、Fetched（fetch.rs:794）全 8 对齐字段无填充浪费；String→Box<str> 无收益场景（结果集 ≤ 数十条）。**无发现**。
**Vec 预分配**：tree_text（4096 起步）、collapse 双 out（`with_capacity(s.len())`）、strip_summary（`with_capacity(html.len())`）均已有；其余大 Vec 热路径未发现缺 with_capacity 的点。**无发现**。

## 2. 泄漏面清单

### 已验证安全（每项附证据点）

| 面 | 结论 | 证据 |
|---|---|---|
| `tokio::spawn` 全库唯一入口 | browser.rs:910-915 `spawn_handler`；handle 三类去向均确认 | ① h_slot 持有 + swap 时 `take()+abort()`（browser.rs:531-533/549-551，main.rs:552/600，postproc.rs:416/469/510，shell.rs:59）；② `let _h = spawn_handler(..)` detached（general.rs:137/280/395）——单命令进程生命周期 1 个，进程退出即清，有界；③ shell `ctx.handler_task`——会话 1 个，swap 即 abort，退出 graceful_close |
| 成功路径多次 swap 累积（本轮专项） | **不累积**：swap_to_headed/headless 每次先 `graceful_close` → `handler_slot.take()` → `h.abort()` 再换新（browser.rs:531-533/549-551）；旧 handler task 被 abort，旧 Browser 随 close+wait 释放。**失败路径豁免**：launch 失败时 abort 不执行（browser.rs:525-530）——属 SilentFailAudit P3 finding（swap 失败路径旧 handler 未 abort），本轮不重复报 | 静态读全路径 + shell 动态曲线无阶梯（§3 T3） |
| graceful_close | close → wait(5s 超时) → kill 兜底 → 再 wait（browser.rs:951-968），无 chrome.exe 残留路径 | browser.rs:951-968；shell 退出走同一路径（shell.rs:144） |
| Box::leak / mem::forget / ManuallyDrop / MemFile | **0 命中**（全库 grep） | 本轮 grep |
| CDP 事件订阅/监听器 | 全库 0 处 subscribe/EventStream/event_listener → 无反注册缺口面 | 本轮 grep |
| OnceLock/LazyLock 静态 | 5 处全部 set-once 有界：CONFIG/EXPLICIT/PARSE_FAILURE（config.rs:30-33）、COMPACT_META（AtomicBool，types.rs:78）、SHARED_CLIENT（单个 reqwest::Client，searxng.rs:22）、build.rs:9 版本串 | 静态读 |
| shell 会话状态 | `last_results`/`last_snap` 整替（shell.rs:204/304），行缓冲 `buf.clear()`（shell.rs:102）；SnapElem text 30 字符硬顶、SearchResult 集合 ≤ limit——**静态+动态双重确认**（T3 曲线平坦） | shell.rs:102/204/304 + §3 T3 |
| postproc 判稳轮询 | ≤read/browse+10s/click+4s 窗口，每轮快照 ≤6000 字符 marker 克隆，prev 整替 | postproc.rs:28-33/153-180 |
| 子进程 | explorer/open/xdg-open（postproc.rs:53-58）与 curl（verify.rs/duckduckgo.rs）均 `.output()`/`.spawn()` OS 级，非 tokio task；前者 detached 属功能语义（开浏览器） | 静态读 |

## 3. 动态实测

环境：Windows 11 x64，release 构建；峰值=PeakWorkingSet64；base≈10MB（tokio+reqwest 冷启动首采样 9.9MB）。

### T1 单命令峰值（fetch 2.07MB 页，×3 中位数）

| 指标 | 值 |
|---|---|
| 峰值 | 14.5 / 14.6 / 20.1 MB → **中位 14.6MB**（首跑冷 exe 离群 38.8MB，已标注） |
| 管线开销 | base 10MB + ~4-10MB ≈ **2-5×body 瞬时**（第一轮静态估 5-7×body 为上界，实测落在其下——峰值 WS 含堆未归还页噪声） |
| wall | ~1.7s；stderr 0 字节；输出 51,920B JSON（50KB cap+meta）✓ |

### T2 浏览器路径（gsearch.exe 进程，Chrome 不计）

| 测试 | 峰值 | wall | 备注 |
|---|---|---|---|
| search "World War II wikipedia" --read 1（google 回退路径） | 14.0MB | 11.0s | read 目标为 goto URL，正文 0 字符（功能面归 SilentFail F2/本次交叉 P3，内存口径不受影响） |
| browse WWII 2.07MB | **21.8MB** | 5.3s | ≈5.9×body——cap-first（50KB 切片再建 DOM）压住了峰值；副作用见 §4 附注 |

### T3 shell 会话 30 混合命令增长曲线（search×3/read×9/browse 大页×4/snap×4/back×5/status×5）

| 采样点 | RSS |
|---|---|
| 0-2s（启动+Chrome） | 7.5→9.8MB |
| 12-15s（首轮 search/read/snap/back） | 12.0-12.2MB 平台 |
| 16.6s（browse 2.07MB WWII） | 峰 16.8MB → 回落 13.6MB |
| 29-41s（Earth/Mathematics/History_of_China 连续大页轮替） | 13.1-14.5MB 振荡，**无单调增长** |
| 结束 | 13.1MB；全程峰值 22.1MB |

**结论：整替成立**——30 条命令含 4 次 1-2MB 页循环后 RSS 回到起点 +1MB，无会话级泄漏；会话峰值=最大单命令峰值，不叠加。

### T4 batch 并发 vs 串行（5 URL，合计 6.06MB body，×2/×5 重复）

| 模式 | 峰值（各次） | 中位 | 结论 |
|---|---|---|---|
| batch 并发（FETCH_CONCURRENCY=5） | 39.7 / 41.3 / 32.3 MB | **39.7MB** | 并发槽成本 ≈ 单页峰值 +25MB ≈ **+5MB/URL**（5 条 DOM/提取管线同活） |
| 串行 5×单发 | 13.5-21.6 MB | 14.5MB（max 21.6） | 单页峰值即串行峰值，命令间零累积 |

批量 5 条输出全验证（每条 text=50,000 字符，JSON list 5 entries ✓）。**内存口径结论：batch 并发把 5 页提取管线叠到同一时刻，约 40MB 封顶——FETCH_CONCURRENCY=5 即并发内存上界 ~40MB，无需调低。**

## 4. Finding 排序

| 优先级 | 编号 | 一句话 | 位置 | 量化 |
|---|---|---|---|---|
| P3（中） | M-1 | collapse 链三连拷（F-8 延伸：调用侧 seg.to_string() + trim().to_string() + 非 HTML 分支 to_string） | fetch.rs:1228/1391-1449 | 每次提取 ~3×T+1×B 瞬时可降到 1×T（2MB 页省 ~2.5MB 瞬时） |
| P4（低） | M-2 | json 源 f.text.clone() 可 move | fetch.rs:771-773 | ≤50KB/命令 |
| P4（低） | M-3 | project_json_paths 每路径 Value 克隆 | fetch.rs:926/945 | ≤50KB×路径数，仅 json-keys |
| — | 附注 | browse/read 对 head>50KB 页正文空、rc=0（omitted=2,423,514、stderr 双 hint 实测确认）——功能面，已交 SilentFailAudit 入库（其 F2 互补，P3） | postproc.rs:189-203 | 非内存问题；修复时注意保持 cap-first 的 21.8MB 峰值优势（尾部窗口重抽仍有界） |

无泄漏类 finding；无结构布局 finding；无需要调 FETCH_CONCURRENCY/BODY_LIMIT 的点。

### 回归保护建议（如采纳 M-1）

collapse_blank 改签名属函数级重构：现有 `collapse_preserving_code` 无哨兵时输出与 `collapse_blank(s)` 逐字节一致的单测（fetch.rs:1765-1769 tiny_static 一族）即回归网，建议修复 PR 直接跑 `cargo test fetch` + 盲测正文逐字节比对（GUI 盲测十三/十四的 R/X 案例是此函数的 historic 踩坑位）。

## 5. 方法局限声明

- 峰值 WorkingSet 含 Windows 堆未归还页与 ASLR/加载噪声，单次波动 ±6MB；关键结论均 ×2/×3 重复取中位数。
- 浏览器路径只测 gsearch.exe；真实用户感知峰值 = gsearch.exe + chrome.exe（Chrome 自身 ~150-300MB，属浏览器固有）。
- 测量经本地代理出网（直连全断），代理对内存无影响路径（reqwest Proxy 仅改连接层）。
- 测量脚手架 `_memtest/`（脚本+样本+输出）已按清理纪律删除，数据以本报告为准。
