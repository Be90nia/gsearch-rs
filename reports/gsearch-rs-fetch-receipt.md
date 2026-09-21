# gsearch-rs fetch 子命令回执（issue gsearch-rs-fetch）

**VERDICT: PASS**

`gsearch fetch <url>` 纯 reqwest GET 取正文已落地，零浏览器，三条 DoD verify 本机全过。

## 改动清单

| 文件 | 改动 |
|---|---|
| `src/fetch.rs` | 新建（334 行含测试）：cmd_fetch + 手写轻量正文提取（script/style/noscript/template 整块删、块级标签转行、实体解码、空白规整）+ JS 壳判定 + `--json`（url/title/text + meta{truncated,omitted,content_untrusted}，与 read/browse 同契约） |
| `src/main.rs` | mod fetch; 注册（bin-crate 本地，与 general.rs 同层）+ Command::Fetch 变体（url 位置参数 + --json flag）+ match 派发 |
| `Cargo.toml` | reqwest features `["json"]` → `["json", "rustls"]`（见下方裁量节） |
| `Cargo.lock` | 随 feature +638 行（新增 65 个 rustls 系传递 crate） |

**模块落位说明**：任务清单列了 src/lib.rs，但 postproc 是 bin-crate 模块（main.rs `mod postproc`），lib crate 看不见其 pub(crate) 项——fetch.rs 挂 main.rs 才能只读复用 cap_chars/read_max_chars，与 general.rs（cmd_browse 所在）同层，符合项目既有惯例。lib.rs 未动，postproc.rs 零改动。

## 约束裁量（Main 已批准，单列）

**reqwest 无 TLS 问题**：在树 reqwest 0.13 是 M16 刻意的 `default-features=false` 编译（Cargo.toml 原注释"局域网 http 直连，无需 TLS"），https 实测报 `invalid URL, scheme is not http`，DoD 三条 verify 全 https，不补 TLS 一条都过不了。补开 `rustls` feature（reqwest 0.13 起 default-tls 本体即 rustls，查 registry 源码确认），纯 Rust、三平台 release CI 交叉编译友好。Main 批准原文要点：「禁新依赖」本意是禁新 HTTP 栈；rustls 是树内 reqwest 的 TLS 后端开关非新客户端；verify.rs 注释记录的「依赖树无 TLS」缺口当年挂账，这次是还账。

## 验证证据

### code-level
- `cargo test`：bin **64 passed / 0 failed**（含 fetch 5 个新测试：提取/实体/壳判定/截断/纯文本保护）+ lib **32 passed** + 2 ignored
- `cargo check`：零警告（Finished dev profile）

### end-to-end（本机 Windows 实跑）

**verify1** `fetch https://raw.githubusercontent.com/rust-lang/rust/master/README.md`：
```
real 0m1.371s（<3s ✓）  exit=0  stderr bytes: 0（无浏览器痕迹 ✓）
stdout: === https://raw.githubusercontent.com/... |  ===
        <div align="center"> ... （README.md 源码原样输出，含 HTML 属文档内容本身）
```

**verify2** `fetch https://github.com`（样例前提已过时，见下节）：
```
exit=0，stdout 7777 字符 —— github.com 对任意 HTTP UA 返回 SSR 营销首页（真实服务端正文）
```
壳判定改用真纯 CSR 站复测 **`fetch https://excalidraw.com`**：
```
real 0m3.325s  exit=1 ✓
stderr: 该页无服务端正文（JS 壳），需渲染：用 gsearch browse https://excalidraw.com
```

**verify3** `fetch https://httpbin.org/status/404`：
```
real 0m2.994s  exit=1（非零 ✓）
stderr: error: HTTP 404 Not Found: https://httpbin.org/status/404（含状态码 ✓）
```

**--json 契约**：
```
keys: ['meta', 'text', 'title', 'url']
meta: {'content_untrusted': True, 'omitted': 0, 'truncated': False}
```

## verify2 样例说明（上游知悉）

验收样例选 github.com 作"JS 壳"代表的前提已过时：GitHub 首页对无 JS 客户端返回完整 SSR 正文（7777 字符营销文案），fetch 正常输出正文、壳判定不触发是**正确行为**（此时判壳反而错杀）。壳判定能力由 excalidraw.com（纯 CSR，剥 script 后正文近空）真实复测通过 + 5 个离线单测覆盖（<500 阈值、SPA 挂载点、正常页、边界 499/500）。

## side-effects

- **Cargo.lock**：+65 传递 crate（rustls/hyper-rustls/tokio-rustls/aws-lc-rs 系）。release CI 首编会多编译这些 crate；运行时行为对 searxng/搜索路径零影响（feature 只增不改）。
- **postproc.rs / src/lib.rs**：零改动（见模块落位说明）。
- **调试残留**：已清理（target/fetch2_debug.txt 已删）。

## 实现要点（供 review）

- HTTP 层与 searxng.rs 同款 Client::builder：10s 超时、UA `gsearch/CARGO_PKG_VERSION`、redirect limited(10)、显式代理透传（无代理时跟环境，与 searxng 的 no_proxy 相反——fetch 面向公网）
- 非 HTML content-type（text/plain 等）不剥标签不判壳——保护 markdown 源码里的 `Vec<u8>` 类字面量
- 壳判定收敛为单条件 `正文 < 500 字符`：SPA 挂载点页（id=root/app）剥净 script 后必然剩不了 500 字，任务原文的"含 id=root 且无正文段"分支被逻辑蕴含，不写死代码
- 测试期间抓到并修复 `collapse_blank` pending 泄漏 bug（前导空白泄漏进正文中间），留回归断言
