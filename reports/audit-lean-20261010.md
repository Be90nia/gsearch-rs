# gsearch-rs lean/bloat 专项审计（2026-10-10，只读）

**首行结论：代码库经多轮清理后已相当精简——真死代码仅 2 处（`find_chrome`、lib.rs 的 `SearchResult` re-export），另有 ~30 行可安全合并的重复/透传层；0 个依赖可整删；tokio 可从 `full` 缩到 7 个 feature（但受 feature 统一化影响收益≈0）。预计可删 **~32 行 prod 代码（占 prod 9017 行的 0.35%，占 src 总量 0.23%）**，谨慎合并再省 ~40 行。**

```
VERDICT: LEAN（接近下限；残余为小颗粒清理项）
SCOPE:   src 22 文件 13649 行（prod 9017 + tests 4632）+ build.rs 17 + Cargo.toml 12 直接依赖
METHOD:  serena-cli overview 符号表（rust LS）→ 全树 grep 逐符号验证（含 tests/reports/docs/beads 字符串引用）→ 逐文件读体对照
```

---

## 1. 死代码清单（全树 grep 验证，含字符串引用）

### 1.1 确认死代码（可删）

| # | 符号 | 位置 | 证据 | 可删行数 |
|---|------|------|------|---------|
| D1 | `pub fn find_chrome` | `src/browser.rs:233-237` | 全树唯一命中即定义本身；调用方已迁移到 `find_browser()`/`find_specific()`（launch: browser.rs:586/588，doctor: main.rs:1189/1202）。lib-crate `pub` 项无 dead_code lint 所以编译器不报 | ~6（含 doc） |
| D2 | `pub use types::SearchResult` | `src/lib.rs:13` | 全树 0 处 `gsearch::SearchResult` 引用（全部走 `gsearch::types::SearchResult` 全路径）；唯一消费者是本仓 bin，无外部 API 破坏面 | 1 |

### 1.2 不可达配置旋钮（YAGNI）

| # | 项 | 位置 | 证据 |
|---|-----|------|------|
| Y1 | `FetchOpts.anchor_pad_lines` | `src/fetch.rs:73-74`（定义）、`fetch.rs:90`（Default 0）、`src/main.rs:418`（唯一构造点硬编码 `anchor_pad_lines: 0`） | 字段被读（fetch.rs:528 `let pad = opts.anchor_pad_lines`）但**没有任何 CLI flag / env / config 键能把它设成非 0**——唯一构造方 main.rs 恒传 0，测试还断言恒 0（fetch.rs:2548）。`#L10-L20` 锚点裁剪功能本身活着，pad 恒 0。可删字段并把 `crop_text_lines` 的 `pad` 参数定值化，省 ~5 行；或补 CLI 暴露（属新功能，非本审计建议） |

### 1.3 逐项排查后**判活**的疑似项（防误杀记录）

- `update.rs`（90 行）：`cmd_update` 在 main.rs:422 派发，`parse_semver3` 有 2 组单测。活。
- `skeleton.rs`（764 行）：名字像脚手架，实为 **AdaptiveRead 正文摘要引擎**（extract_adaptive/format_adaptive/github_comment_html），被 postproc/general/shell/fetch 四方消费；391-765 行是 374 行单测。活且是核心资产。
- `build.rs` + `src/build.rs`（17+17 行）：vergen-gitcl 编译期注入 → `version_line()` 供 clap `--version`（main.rs:37）。活。
- `verify.rs`（660 行）：`Verify` 子命令整条链（main.rs:406 派发），427 行起为单测。活。
- `b64_decode`（util.rs:75）：prod 调用点 `postproc::fetch_in_page`（postproc.rs:747，页内 fetch base64 通道）。活。
- `searxng::search_html`（搜索 JSON 降级，search.rs:482）、`searxng::probe`（doctor，main.rs:1336）、`stealth::{install_init_script,warmup}`（--humanize 路径，main.rs:608-609）、`duckduckgo::collect`（回退链第二层）、`profile_name_only`（main.rs 5 处 meta 装配）、`browser::UA`（launch 602/667）、`wait_dom_complete`（content_retry/eval_string_retry 重试前导）——全部有 prod 调用点。活。
- `GITHUB_CONTAINER_CHAIN`（fetch.rs:1076）：注释自称「仅用于标注」，但实际在 `host_default_include`（fetch.rs:1066）返回、`--include` 未命中回退路径（apply_host_route_on_include_fallback）真实消费。活。

### 1.4 刻意保留、不计入 bloat（有明确注释背书）

`json: bool` 兼容占位 flag ×6（Browse/Doctor/Verify/Fetch/Similar/Search，「解析但无效果，存量脚本零破坏」，hide=true）；`chaser-stealth` feature 空实现开关；`native-tls`（PM 已知，不重报）。这些是**故意的**，删了就是破坏契约。

---

## 2. 重复与近似拷贝对照

### 2.1 可合并（低风险，建议做）

**R1 `browser_alive` 双胞胎（完全同体）**

```rust
// src/general.rs:352-354 —— 已是 pub(crate)
pub(crate) async fn browser_alive(browser: &chromiumoxide::Browser) -> bool {
    browser.version().await.is_ok()
}
// src/shell.rs:511-513 —— 私有副本
async fn browser_alive(browser: &Browser) -> bool {
    browser.version().await.is_ok()
}
```

shell.rs 与 general.rs 同为 bin-crate 模块，直接 `use crate::general::browser_alive` 即可。省 3 行 + 消除一处定义漂移点。

**R2 `swap_to_headed` 透传 wrapper ×2（单行转调）**

```rust
// src/search.rs:608-611 与 src/shell.rs:505-508 —— 逐字符相同
async fn swap_to_headed(browser: &mut Browser, h_slot: &mut Option<tokio::task::JoinHandle<()>>) -> Result<()> {
    browser::swap_to_headed(browser, h_slot).await
}
```

两个文件都已引用 browser 模块（search.rs:195 就直接调了 `browser::swap_to_headless`，证明直调无障碍），调用点各仅 1 处（search.rs:155 / shell.rs:444）。直调省 ~10 行（含 doc 注释）。

**R3 `launch_with_kind` 只有 1 个调用方**

```rust
// src/browser.rs:514-516 → 554-557
pub async fn launch(headless: bool) -> Result<(Browser, Handler)> {
    launch_with_kind(headless, None).await          // 唯一调用 launch_with_kind 的地方
}
pub async fn launch_with_kind(headless: bool, kind: Option<BrowserKind>) -> Result<(Browser, Handler)> {
    let proxy = std::env::var("GSEARCH_PROXY").ok().filter(|s| !s.is_empty());
    launch_with_kind_proxy(headless, kind, proxy).await
}
```

`launch_with_kind` 的调用方只有 `launch` 自己（其余调用方全部直调 `launch_with_kind_proxy`：general.rs:136/279/394、main.rs:596/1022）。把 env 读取内联进 `launch`，砍掉中间层，省 ~4 行。

### 2.2 结构性重复（可合并但收益/风险比一般，建议记录不强推）

**R4 goto+timeout+map_err 模式 ×4**（~30 行）：

- general.rs:139-142、150-153（经 `goto_timeout_error`/`goto_nav_error` helper，文案带 `--proxy` hint——b5/h90 特意加的）
- shell.rs:556-561（`goto()`，裸超时文案）
- search.rs:664-670（`load()`，goto+content 合取）
- postproc.rs:548-551（goto_page 内联，文案「浏览器被手关？」）

四处 `tokio::time::timeout(PAGE_TIMEOUT_SECS, page.goto(url))` 骨架相同、**错误文案刻意分场景**（browse 路径带代理自救 hint，shell 提示手关浏览器）。合并进 browser.rs 一个带 error-suffix 参数的 helper 可省 ~12 行，代价是四处文案语义要显式保留（回归面：盲测对 stderr 文案敏感）。

**R5 SERP HTML「流式配对」手法 ×3**（各 ~20-25 行）：parse.rs `SEL_WALK`（h3+VwiC3b）、searxng.rs `extract_walk`（h3>a + p.content，147-186）、duckduckgo.rs `parse` 回退路径。选择器集、URL 修复（absolutize vs real_url+percent_decode vs unwrap_google_redirect）、评分规则三处全不同，只有「h3 开新条目、后续元素配 snippet」的骨架同源（duckduckgo.rs:135 注释自己承认「同 searxng extract_walk 手法」）。抽泛型 pairing 核心省 ~30 行但引入一层闭包参数抽象——三个消费方的选择器/修复函数签名不一致，合并后可读性下降。**不建议现在做**；若第四个 provider 再抄一次，届时抽。

### 2.3 排查后判「非重复」的高嫌疑对

| 嫌疑对 | 结论 | 证据 |
|--------|------|------|
| searxng.rs vs duckduckgo.rs「JSON 解析/结果映射」 | **不重复** | DDG 无 JSON 路径（curl POST + HTML 解析）；SearXNG JSON 用 serde struct（searxng.rs:46-68）。结果映射共调 `util::domain_class` + `types::SearchResult`，复用已到位 |
| fetch.rs vs postproc.rs「正文提取 helper」 | **输出形态不同，解析器已统一** | kda 修复已删 fetch 手写状态机，`tree_text`（fetch.rs:1265-1386）与 `extract_adaptive`（skeleton.rs:83）同走 scraper 树；fetch 产纯文本（块边界→换行的逐字节契约），skeleton 产 AdaptiveRead 结构（标题/段落/代码块）。`cap_chars`/`github_comment_html`/`clean_text_anchors` 已跨模块共享（fetch.rs:460/1096、general.rs:170、postproc.rs:198）。强行合并会破坏任一侧输出契约 |
| general.rs vs util.rs 通用工具 | **职责已分干净** | util.rs 只剩 3 个跨模块纯函数（filename_from_url/b64_decode/domain_class，各有 ≥2 模块消费）；general.rs 其余是 browse/login/dl 命令体 + 私有 helper（is_dns_error 等仅 general 用） |
| duckduckgo curl 包装 vs verify curl 包装 | **传输形态不同，不合并** | DDG：tokio::process 异步 + `--data-raw` POST + 头序风控敏感（duckduckgo.rs:40-49）；verify：std::process 同步 + HEAD/-w dump + redirect 链（verify.rs:1-9 模块注释有零依赖拍板）。参数集交集≈0 |
| postproc::read vs shell::cmd_read | **旧审计的 ~30 行重复已收敛** | 现两者共享 `render_read`/`cap_chars`/`read_max_chars`/`wait_content_stable`（shell.rs:28 显式 import），残留差异是各自 5-10 行装配胶水 |

---

## 3. 依赖使用面表（12 直接依赖 + 1 build-dep）

| 依赖 | 实际用途（grep 实证） | 可否瘦身 |
|------|----------------------|---------|
| chromiumoxide 0.9 | browser.rs 全部 CDP 面 + 各命令 Page 操作 | 保留（核心） |
| tokio `features=["full"]` | 实用子集：`task`(JoinHandle/spawn)、`time`(sleep/timeout/Instant)、`select!`(browser.rs:869)、`process::Command`(duckduckgo.rs:33)、`io::{AsyncReadExt,AsyncWriteExt}`(main.rs:1436/general.rs:487)、`fs::File`(general.rs:527)、`net::TcpStream`(main.rs:1262/search.rs:371)、`#[tokio::main multi_thread]` | **可缩**：`["macros","rt-multi-thread","time","process","io-util","fs","net"]`，砍掉 sync/signal/io-std/parking_lot 4 个 feature。⚠ 收益注记：feature 全图统一化下 chromiumoxide 大概率已替全图开启其中若干，二进制/编译时间收益≈0，属「声明最小化」卫生项 |
| clap 4 derive | main.rs 全部 CLI | 保留 |
| anyhow | 全库错误链 | 保留 |
| serde / serde_json | 配置反序列化 + 全部 JSON 输出 + json_keys 投影 | 保留 |
| scraper 0.20 | parse/searxng/duckduckgo/skeleton/fetch 五处 HTML 解析 | 保留 |
| tracing 0.1 | 全库宏 | 保留 |
| tracing-subscriber env-filter | main.rs:373-381 `EnvFilter` | 保留 |
| reqwest 0.13 (json,rustls,stream,native-tls) | searxng（json+html+probe，no_proxy）、fetch（bytes_stream→stream feature、rustls）、update.rs（GitHub API）、general dl_direct。native-tls：PM 已知无消费者，**不重报** | feature 层已最小（Cargo 红线已知项除外） |
| htmd 0.5 | 仅 convert.rs 一个 import（`HtmlToMarkdown`），prod 调用 3 处：fetch.rs:460/1101（--markdown）、general.rs:170（browse --markdown）+ 8 个单测 | 保留（--markdown 特性唯一实现，自研替代=几百行） |
| futures 0.3 | 4 文件 6 处：StreamExt::next（browser.rs:8 handler 流、fetch.rs:409 bytes_stream、general.rs:486）+ `stream::iter→map→buffer_unordered`（search.rs:456、fetch.rs:648） | **不可划算替换**：chromiumoxide `Handler` 是 futures::Stream，tokio 自带不发布 Stream；buffer_unordered 换 JoinSet 需手工保序（search.rs 的 slots 重排逻辑重写）。换 tokio-stream 同量级。保留 |
| vergen-gitcl 10 (build-dep) | build.rs 版本烧录 | 保留 |

**可整删依赖：0。可缩 feature：tokio 10→7（4 个可去，收益以声明卫生为主）。**

---

## 4. 抽象税 / YAGNI

| 项 | 位置 | 判定 |
|----|------|------|
| 单实现 trait | — | **0 个**（全库无 trait 定义，grep 实证） |
| 只透传 wrapper | R2/R3（swap_to_headed ×2、launch_with_kind） | 见 §2.1，~14 行可省 |
| 单调用方泛型 | postproc.rs:697 `goto_for_download<F,T,E>` | **合理**——为 start_paused 单测注入 pending future，注释明说，非税 |
| 定义未读的 CLI flag | `json` 兼容占位 ×6 | **刻意保留**（存量脚本契约），不计 bloat |
| 定义未读的 config 键 | **0 个**——GsearchConfig 4 键（profile/chrome/searxng_url/read_max_chars）全部有消费点（browser.rs:148、find_browser、search.rs:323、postproc.rs:181） |
| 定义未读的结构体字段 | `anchor_pad_lines`（Y1）恒 0 | 唯一一处 |
| 进程级全局 | types::COMPACT_META / config 3×OnceLock | 有注释拍板（serde skip_serializing_if 拿不到 self 的最小改动），合理 |

---

## 5. 量化结论

```
src 总行            13,649（22 文件）
  prod 代码          9,017（66%）
  测试代码           4,632（34%；fetch 1,377 / main 421 / postproc 361 / skeleton 374）

立即可删（D1+D2+Y1+R1+R2+R3）      ≈ 32 行 prod   ≈ 0.35% prod / 0.23% src
谨慎可合并（R4 goto-unify）         ≈ 12 行（文案回归面在盲测敏感区，需拍板）
不建议动（R5 SERP-pairing ×3）      名义 ~30 行，抽象税 > 收益
刻意保留（json 占位 flag ×6 等）     ~20 行，删=破坏契约

依赖：可整删 0 个；tokio feature 10→7（砍 sync/signal/io-std/parking_lot，
      受 feature 统一化影响，二进制收益≈0，纯声明卫生）
```

**总评：这是一个已经过至少 8 轮显式去重/抽取（util.rs M8、kda 提取管线统一、stealth 单一事实源 M14-2A）的代码库，lean 空间已接近榨干。剩余项全是 3-10 行颗粒度。真正值得动手的只有 D1/D2（真死代码）和 Y1（死旋钮）；R1-R3 顺手可做；其余记录即可。**

**仓库卫生（非代码，附带）**：git 追踪 151 个文件中 **104 个在 `reports/`（含一个 9.5MB 的 `reports/devil-dl/gsearch-v0.2.9.exe/…exe` 发布二进制）**——占仓库文件数 69%、仓库体积大头；e2e/、_search_runs/、devil-dl/ 根目录副本未 ignore。建议 reports 归档迁出或 gitignore，与代码 lean 无关但影响 clone 体积。

## 6. 复核命令

```bash
# D1 死代码
grep -rn "find_chrome" . --include="*.rs" --include="*.md"   # 仅 src/browser.rs 定义
# D2 re-export
grep -rn "gsearch::SearchResult" src/                        # 0 命中
# R1/R2 双胞胎
grep -rn "fn browser_alive\|fn swap_to_headed" src/          # general+shell 各一份
# tokio 子集
grep -rnoE "tokio::[a-z_]+" src/ | sort -u -t: -k3
# prod/test 分界
grep -rn "^#\[cfg(test)\]" src/
```

*检索纪律说明：结构侦察用 serena-cli（overview，rust LS 符号表）；使用面验证用全树 grep（覆盖 tests/reports/docs/.beads 字符串与动态引用）；未跑任何构建/测试（并行审计纪律）。*
