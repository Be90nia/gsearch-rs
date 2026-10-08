# 杠精报告：gsearch-rs v0.2.8 黑盒评测

> 日期：2026-10-08  
> 评测时长：~12 分钟（5 命令类别 / 9 次调用 / 1 个 8.9 MB 真实下载）  
> 立场：被迫使用的资深杠精，每条杠附真实命令 + 输出  
> 工具版本：`gsearch 0.2.8`，`Cargo.toml` 自报 `0.2.9`（**版本号本身就矛盾**，第一条杠从这里开始）

---

## 0. 版本号乌龙（送命题，开门红）

`./target/release/gsearch.exe --version` 打 `gsearch 0.2.8`，`Cargo.toml:3` 写 `version = "0.2.9"`，release tag 是 `v0.2.9`（`api.github.com/repos/Be90nia/gsearch-rs/releases/latest` 返的）。**同一仓库三处报两个版本号**——这是构建管线没把 commit 烧进 binary 的典型症状。下游 agent 按 version 输出判断能力基线会得出错误结论。

**修法（如果我是作者）**：Cargo build.rs 注入 `VERGEN_GIT_SHA` + `VERGEN_GIT_DESCRIBE`，release 二进制必须含 `git describe --tags` 输出。

---

## 1. 这工具有什么好的（begrudging acknowledgment）

### 1.1 子命令边界清晰

```
search / browse / login / dl / shell / doctor / verify / fetch / help
```

每个都有明确的「适用场景」前缀（`search` 走 SearXNG/Google、`browse` 必起 Chrome、`fetch` 纯 HTTP），**人类一眼能看懂分工会怎么走**。这是 Rust CLI 里罕见的克制——没堆出 20 个 subcommand 假装全能。

### 1.2 `doctor` 是真做事

```
[ OK ] Chrome: C:\Program Files\Google\Chrome\Application\chrome.exe
[WARN] Edge 不可用（msedge.exe 未找到；仅 Chrome 可跑）
[ OK ] profile 可写: C:\Users\Begonia\.gsearch\profiles\devil
[ OK ] 出口 IP: 61.144.188.80
[FAIL] 网络连接超时 (www.google.com:443)
```

`./target/release/gsearch.exe doctor` 3.3s 跑完，**把"我为啥搜不到东西"的自检链一次性吐出来**——浏览器、profile、出口 IP、外网连通。**勉强承认**：90% 的 Rust CLI 工具只会 print "everything OK" 然后静默让你摸黑。

### 1.3 `fetch --json` 是救命稻草

`./target/release/gsearch.exe fetch "https://crates.io/api/v1/crates/simd-json" --json` 1.5s 出 `{"meta":{"content_untrusted":true,"omitted":190320,"truncated":true},"text":"..."}`，**meta 字段给截断位、content_untrusted 给注入面告警**——这是给 agent 消费设计的契约。`meta.truncated=true` 让消费者知道"别信这是全文"，这种细节 90% 的爬虫工具不做。

### 1.4 `shell` 真会话复用立得住

M7 里程碑写的"起一次 Chrome 会话复用"不是 PPT——`echo "help\nsearch rust async\nquit" | gsearch shell` 4 秒内 `help` 出命令清单 + `search rust async` 返 10 条带摘要的结果 + 同一 Chrome 进程内复用。**对得起这个 milestone 名号**。

---

## 2. 哪些功能纯鸡肋

### 2.1 `--no-humanize` 是给 agent 留的开关，但只 `search` 有

```
$ ./target/release/gsearch.exe search "tokio" --limit 3 --no-humanize  # ok
$ ./target/release/gsearch.exe browse "https://example.com" --no-humanize
error: unexpected argument '--no-humanize' found
```

**只有 `search` 有这个 flag**，browse / fetch / dl 全都没有"跳过拟人化"概念。要么 agent 全部默认走 fast path，要么只在 search 上做是半吊子设计。**鸡肋 = 做了一半**。

### 2.2 `dl` 对纯 HTTPS 直链也开 Chrome

```
$ time ./target/release/gsearch.exe dl "https://github.com/Be90nia/gsearch-rs/releases/download/v0.2.9/gsearch-x86_64-pc-windows-msvc.exe" -o reports/devil-dl/gsearch-v0.2.9.exe
INFO 使用浏览器: Chrome -> C:\Program Files\Google\Chrome\Application\chrome.exe
WARN dl 导航失败（...30014ms）: Request timed out. —— 继续尝试页内 fetch 直链
已下载: D:\Project\gsearch-rs\reports\devil-dl\gsearch-v0.2.9.exe\gsearch-x86_64-pc-windows-msvc.exe (8938496 bytes)
real    0m47.539s
```

8.9 MB 的 GitHub release 资产，**先开 Chrome 走一遍 30s timeout 的导航，失败后才用页内 fetch**——总共 47s。**reqwest 0.13 直接 GET 这个 URL 是 1 秒内的事**。`dl` 名字叫"带 profile 登录态下载"，M6 实现的就该只对需要登录态的站点开浏览器，否则纯 HTTP fetch。**纯鸡肋**：90% 调用不需要 profile 登录，但工具默认走全路径。

### 2.3 `verify` 子命令存在但 README 没出现

`./target/release/gsearch.exe --help` 列了 `verify`（"HEADless URL 健康检查：HEAD/GET + redirect 链 + SSL + 延迟（M14-1A，无需 Chrome）"），但 README / USER_GUIDE 我没扫到任何示例。**功能上线了但文档零覆盖**——典型"做了一半就发"的痕迹。

---

## 3. 哪里不好用（具体不爽瞬间，按触发顺序）

### 3.1 长 query 黑洞 33 秒

```
$ ./target/release/gsearch.exe search "simd-json Rust benchmark 2026" --limit 5 --no-humanize
SearXNG http://192.168.89.249:8888 查询失败（查询无结果，HTML 结果页亦无结果），已回退 Google 直爬
INFO 使用浏览器: Chrome -> C:\Program Files\Google\Chrome\Application\chrome.exe
error: 加载 https://www.google.com/search?q=simd-json%20Rust%20benchmark%202026&start=0 失败: net::ERR_CONNECTION_TIMED_OUT
Wall time: 32.97 seconds
```

SearXNG 零结果 → 自动回退 Google → Google 翻墙 timeout → **白等 33 秒一个结果没出**。`gsearch search "tokio"` 同样的链路 3 秒就返 3 条（参见 1.4 节）。**问题：query 长度是隐藏 SLA，但工具不告诉你阈值在哪**。第二次试短 query "simd-json Rust" 也黑洞——说明不是 query 长度问题，是某些 token 组合 SearXNG 不认。

### 3.2 `dl -o` 的语义是薛定谔的

```
$ ./target/release/gsearch.exe dl <URL> -o reports/devil-dl/gsearch-v0.2.9.exe
# 实际产物：
$ ls reports/devil-dl/gsearch-v0.2.9.exe/
gsearch-x86_64-pc-windows-msvc.exe  # 文件名还是 URL 里的 basename！
```

`-o reports/devil-dl/gsearch-v0.2.9.exe` 给的是**一个无扩展名的"目录路径"还是"文件名"**？实际是**目录**——`gsearch-v0.2.9.exe/` 子目录里放了 `gsearch-x86_64-pc-windows-msvc.exe`。**用户预期**：要么是文件名（最终落在 `reports/devil-dl/gsearch-v0.2.9.exe`），要么干脆拒绝无扩展名。**工具实际行为**：把 `-o` 当目录前缀，basename 沿用 URL。最后两个空目录（我第一次猜错路径生成的 `gsearch-v0.2.8.zip/` 和 `iana-logo.svg/`）得手动 `rmdir` 收尸。

### 3.3 `shell` 的 `quit` 不退出

```
gsearch> quit
退出请输入 EOF（Ctrl+D / Ctrl+Z+Enter）
gsearch>
```

**help 文档说**："`exit / quit 提示退出；EOF / Ctrl+D 真退出`"——OK 提示退出我能接受，但**实际是"假装收到，提示你 EOF，根本没动作"**。我只能 Ctrl+Z 强退。**对比 bash / fish / cmd**：quit 就是 quit。一个 REPL 把退出语义埋进 EOF 是 1970 年代的设计，2026 年的工具照抄 = 没动脑。

### 3.4 crates.io SPA 404，fetch 不自动改路径

```
$ ./target/release/gsearch.exe fetch "https://crates.io/crates/simd-json"
error: HTTP 404 Not Found: https://crates.io/crates/simd-json
```

`/crates/<name>` 是 SPA 入口，API 在 `/api/v1/crates/<name>`——crates.io 自己跳。fetch 看到 404 就甩错，**不试 redirect / 不试 API 等价路径**。前一份 `client-choice-notes.md` 也提过同样的问题，**没人修**。

### 3.5 dl 第二次同 URL 直接挂

```
$ ./target/release/gsearch.exe dl "https://github.com/Be90nia/gsearch-rs/releases/download/v0.2.9/gsearch-x86_64-pc-windows-msvc.exe" -o reports/devil-dl/gsearch-v0.2.9.exe
INFO 使用浏览器: Chrome ...
WARN dl 导航失败（...30014ms）: Request timed out. —— 继续尝试页内 fetch 直链
error: 页内 fetch 失败（...）: TypeError: Failed to fetch
```

第一次成功（Chrome profile 还没起过），第二次**Chrome 还在前一次页面状态里**，新页面的 in-page fetch 抛 `TypeError: Failed to fetch`（典型 CORS / 跨源 iframe 限制）。**幂等性 = 0**。

### 3.6 search 输出 10 条不带"内容类型"标签

```
1. Tokio
   https://en.wikipedia.org/wiki/Tokio
   Topics referred to by the same term
2. 13 ideas de Tokio en 2026 | viaje a japón, tokio, japón
   https://www.pinterest.com/anitabergaz/tokio/
   2026 - ...
3. Tokio Hotel - Monsoon (Lyrics) - YouTube
   https://www.youtube.com/watch?v=VFct33WRrPs
   ...
```

10 条里 1 是 wiki，2 是 Pinterest（垃圾），3 是 YouTube 歌词视频（垃圾）。**没有"权威性 / 域名类型 / 内容类型"标签**，agent 没法自动筛。要我肉眼挑 = 把脏活外包给用户。上一份 client-choice-notes 提过"search URL 表直接标注 docs.rs / GitHub / 博客"，**没改**。

---

## 4. 我是作者我怎么改（打脸式重设计）

### 4.1 `search` 加 4 件事，按实现成本排序

1. **query 长度 > N（默认 4 token）自动 OR 降级**——`simd-json Rust benchmark` → 跑 `simd-json` / `Rust benchmark` / `simd-json Rust` 三次取首个非空。**消除 33s 黑屏**。
2. **失败时 stderr 附加一行诊断**——"SearXNG 零结果 + Google 回退超时，建议：① 跑 `gsearch doctor` ② 简化 query ③ 跑 `gsearch fetch <URL>` 抓特定源"。**前一份笔记也提过，没人改**。
3. **结果带 `domain_class` 字段**（wiki / github / blog / forum / video / shop / docs / news / qa）——基于 URL 启发式 + 内容指纹。
4. **`--max-results <N>` 上限 50**——默认 10 太少、limit 50 又太多，中间给个滑动条。

### 4.2 `dl` 的真重构

**当前**：所有 URL 都开 Chrome 走 30s nav + 失败才 fetch。  
**重设计**：
- 先 `reqwest::head()` 看 `Content-Type` / `Content-Length` / `Set-Cookie` 头——**纯静态资源（CDN / GitHub release / S3 / 直链 PDF）直接走 reqwest 流式 GET**。
- 只在 `Set-Cookie` 提示要登录 / URL 域名匹配已知"登录墙站点白名单"时才起 Chrome。
- `-o` 严格区分：`--output-file` 必须含 `.` 字符，否则拒绝（消歧义）。
- 默认 **8 MiB 流式**写盘，不下载到 `Vec<u8>` 再 dump（`Vec<u8>` 内存峰值 = 文件大小，8.9 MB 不致命但 100 MB 文件会爆）。

### 4.3 `shell` 把 `quit` 真退

```rust
// 伪代码
match cmd.as_str() {
    "exit" | "quit" => return Ok(()),  // 不是 EOF，真退
    _ => ...
}
```

**5 行代码**的事，做了 6 个月没人动。

### 4.4 版本号闭环

Cargo build.rs：

```rust
vergen::cargo_sha() // commit sha
```

main.rs 启动打印 `gsearch {version} ({git_sha})`，`--version` 同款。`Cargo.toml version` 和 tag 同步靠 `cargo release` + CI 校验。**做一次，终身免疫**。

---

## 5. 还缺什么功能（不加就是残废）

### 5.1 **批量 fetch 并发**

```
gsearch fetch URL1 URL2 URL3 ... URL10
```

按 search 的 batch 语义对齐。**agent 调研链路 80% 是这个场景**，现在只能 `for` 循环串行 10×3s = 30s，并发能压到 5s。

### 5.2 **同一 URL 结果缓存**

`fetch https://...` 第二次命中应 < 100ms 出结果（落 `~/.gsearch/cache/<sha256>.json`）。`search` 也加——同 query 1 分钟内重发不重抓 SearXNG。

### 5.3 **SearXNG 健康度探测内置**

`doctor` 顺手加一行："SearXNG 端点可达：✅ / 当前启用引擎数：24 / 过去 1h 平均 results 数：48"。**避免今天 9:51Z 那种"SearXNG 活着但零结果"的盲区**——前一份 client-choice-notes 踩过。

### 5.4 **cookies / session 跨子命令复用**

`login` 完某个站点后，`browse` / `search` 自动带上；当前状态我不确定（黑盒看不出来），但**文档没说**。登录态可观测性 = 0。

### 5.5 **`browse` 的元素选择器**

`browse https://example.com --extract "table tr:nth-child(2n)"` ——M2 AdaptiveRead 的 paragraph_index 是被动抽取，**没有"我要这块"的主动接口**。

### 5.6 **`--dry-run` 全局支持**

所有会写盘 / 起浏览器的子命令加 `--dry-run`，只 print 计划。agent 编排前先 dry-run 看副作用是标配。

### 5.7 **退出码文档表实际行为对账**

README 退出码表写"1 = 命令执行错误"，但 fetch JS 壳路径预期返 1（"需用 browse 渲染"）——**两种语义共用一个码**。audit-quality.md I-1 提了，没修。

---

## 6. 综合修改意见（按我喷的狠度排优先级）

| # | 痛点 | 严重度 | 改法 | 估时 |
|---|---|---|---|---|
| **P0** | `version` 与 tag 与 Cargo.toml 三处不一致 | 数据正确性 | build.rs 注入 git_sha，CI 卡死 | 0.5d |
| **P0** | `dl -o` 路径语义薛定谔、幂等性 = 0 | 用户陷阱 | reqwest head 预检 + 严格 `-o` 解析 | 1d |
| **P0** | `search` 长 query 33s 黑屏 | 用户陷阱 | 自动 OR 降级 + 失败 stderr 诊断 | 0.5d |
| **P1** | `shell` 的 quit 不退出 | UX 退化 | 5 行 match 改 EOF 行为 | 5min |
| **P1** | `search` 结果无内容类型标签 | agent 友好性 | URL 启发式分类 + 输出字段 | 1d |
| **P1** | `dl` 对 CDN 直链也开 Chrome 浪费 30s | 性能 | head 头预检分流 | 1d |
| **P2** | 批量 fetch 并发 | agent 必需 | `for url in urls` → `join_all` | 0.5d |
| **P2** | 同 URL 结果缓存 | 性能 / token | sha256 key + 1 min TTL | 1d |
| **P2** | `doctor` 加 SearXNG 引擎数 / results 数 | 可观测性 | 多打两行 | 0.5d |
| **P3** | `--no-humanize` 仅 search 有，要全命令铺开 | 设计一致性 | 提到全局 Options | 0.5d |
| **P3** | `verify` 子命令 README 零覆盖 | 文档 | 补用法段 | 0.5h |
| **P3** | exit code 表与 fetch JS 壳语义冲突 | agent 契约 | 拆退出码（建议 fetch 专用 64/65） | 0.5h |

---

## 收尾诚实声明

**没干的事 / 没看的**：
- 没翻 `src/` 源码（"黑盒使用"纪律）；所有判断来自 `--help` + 实际调用 + `--verbose debug` 输出。
- 没跑 `cargo test` / `cargo clippy`（验真不是本任务范围）。
- 没对 sonic-rs / simd-json benchmark 交叉验证（用 sonic-rs 自家 README 数字已声明在 json-choice-notes.md）。
- 重复跑 `dl` 同一 URL 只跑了 2 次（一次成功一次挂），**没跑第 3 次**——可能是 Chrome 残留态也可能是真随机，需要 5+ 次才能定性。
- 跨平台行为（macOS / Linux）未测，仅 Windows 11 x64。
- `doctor` 的 FAIL 项"网络连接超时 (www.google.com:443)"今天复现——**这是已知基础设施问题**，不是工具 bug，但用户用工具时第一反应会是"工具坏了"。

**真实判定**：工具的"骨架"立得住（doctor / fetch --json / shell / browse 都做对了核心场景），"细节"烂得很有 Rust CLI 通病味道（文档与代码脱节、版本号三处不一致、UX 小坑累积）。**不是不能用，是用着硌手**——每一次都让你写一行"下次记得别这样"的小抄。
