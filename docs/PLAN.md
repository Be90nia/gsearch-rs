# gsearch-rs 开发规划

> 把 plsearch（Python/Playwright/MCP）的核心能力移植为纯 Rust 单文件 exe CLI。
> 项目根：`D:/Project/gsearch-rs`

## 0. 背景与目标

**现状**：Google 搜索工具链里，唯一能绕过反爬的是 plsearch（真 Chrome + 持久 profile 养号 + CAPTCHA 弹真窗人工解）。但它是 Python + 常驻 MCP 服务的形态，依赖 uv/venv，不是"用完即走的 exe"。

> **参考源码（Python 原版，移植时对照用）**：`D:/Sdk/plsearch`（git clone 自 https://github.com/IIshikiII/Playwright-search-MCP，完整历史）
> - 核心文件：`src/plsearch/main.py`（搜索状态机 236-343 行 / AppContext 101-160 行）、`src/plsearch/config.py`（常量+CAPTCHA 判定 112-139 行）、`src/plsearch/parse_page.py`（SERP 解析全文件 61 行）
> - 我们加的一次性 CLI 壳：`gsearch.py` + `gsearch.cmd`（未 commit，仅本机）

**目标**：单 exe、零运行时依赖（机器上有 Chrome 即可）、用完即关。定位 = **Google 搜索（唯一能绕反爬的场景）+ 通用浏览器代理（浏览/登录态/下载，无扩展版 panerelay）**。命令形态（clap 子命令）：

```
gsearch search "query" [--limit N] [--json] [--read N] [--dl N] [--open N]   # Google 搜索
gsearch browse <url>                                                          # 任意 URL → 正文文本（渲染后）
gsearch login  <url>                                                          # 有头窗人工登录，cookie 落 profile
gsearch dl    <url> [-o PATH]                                                 # 带 profile 登录态下载
```

### 与 panerelay（https://github.com/F-loat/panerelay）的关系（2026-08 决策）

- panerelay 能复用日常浏览器的真身份（扩展 + Native Host 桥），但**做不了 Google 搜索**（无养号、无 UA 遮蔽、无 CAPTCHA 策略）；plsearch/gsearch 反之，搜索能活但周边功能少
- 本项目 = 两者功能结合：Google 搜索走养号 profile；通用浏览/下载走**同一个专用 profile**（首站登录一次，cookie 永久留存），不起扩展、不 attach 日常浏览器
- 不做 panerelay 的纯 HTTP Fetch（无渲染 API 调用）：browse/dl 已覆盖需求，YAGNI

**不可妥协的三个移植点**（这是 plsearch 能活、别人全死的原因）：
1. **持久 profile**：`D:/Sdk/plsearch-profile`（沿用 Python 版养出的号，Chrome 磁盘格式与驱动语言无关，直接继承）
2. **UA 覆写 + AutomationControlled 关闭**：HeadlessChrome UA 和 `navigator.webdriver=true` 是 Google 的顶级 bot 信号，必须遮掉
3. **CAPTCHA 双模式**：headless 撞验证码（首页且零结果时）→ 关掉 → 同 profile 起有头窗 → 轮询等人解（≤120s）→ 回 headless 继续翻页；**翻到后面页才遇验证码则返回已有部分结果，不打扰人**

## 1. 依赖选型（已核实 crates.io，2026-08）

| crate | 版本 | 用途 | 备注 |
|---|---|---|---|
| `chromiumoxide` | 0.9（features=["tokio-runtime","_fetcher"]不需要） | CDP 驱动 Chrome | 360万下载，2026-02 仍更新 |
| `tokio` | 1（full） | 异步运行时 | |
| `scraper` | 0.20 | HTML 解析（≈BeautifulSoup） | html5ever + CSS 选择器 |
| `clap` | 4（derive） | CLI 参数 | |
| `serde_json` | 1 | `--json` 输出 | |
| `anyhow` | 1 | 错误处理 | |

> 备选：`headless_chrome`（1.0.22，2026-06 更新）API 更同步直觉但弱类型；chromiumoxide 更主流，先用它，卡住再换。

Chrome 定位顺序：`GSEARCH_CHROME` env → `C:/Program Files/Google/Chrome/Application/chrome.exe` → PATH 里 `chrome` → 报错退出。

## 2. 架构（单 crate，模块划分）

```
src/
├── main.rs        clap 定义 + 流程编排（main 全在這，~150 行）
├── browser.rs     启动/关闭/有头无头切换/陈旧锁清理
├── search.rs      翻页搜索 + 去重 + CAPTCHA 判定与处理（核心状态机）
├── parse.rs       SERP HTML → Vec<SearchResult>（scraper）
└── output.rs      文本/JSON/read/dl/open 五种输出
```

### SearchResult（全链路统一结构，serde 序列化）

```rust
#[derive(Serialize, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,   // 对应 python 的 page_content
}
```

## 3. 核心逻辑移植对照（Python → Rust 行为规格）

### 3.1 browser.rs

- `launch(headless: bool) -> Browser`：
  - `BrowserConfig::builder()`
    - `.user_data_dir(PROFILE)` ← `D:/Sdk/plsearch-profile`（env `GSEARCH_PROFILE` 可覆盖）
    - `.chrome_executable(找到的 chrome 路径)`
    - `.arg("--disable-blink-features=AutomationControlled")`
    - `.user_agent(UA)` ← 常量：标准 Chrome UA（无 HeadlessChrome 字样；**注意每半年更新一次版本号**，UA 过老本身也是信号）
    - headless 由 chromiumoxide 的 `.headless(true/false)` 或 `--headless=new` arg 控制
- `cleanup_stale_locks(profile)`：启动前删 profile 目录下的 `SingletonLock`/`SingletonCookie`/`SingletonSocket`（上次被 kill 的残留；ignore 不存在）
- 有头⇄无头切换 = `browser.close()` 后用**相同 user_data_dir** 重新 `launch`（cookie 落盘保留，Playwright 也做不到热切换，同款方案）

### 3.1.1 profile 管理（自动初始化 + 与 Python 版共用）

- **自动创建**：exe 启动时检查 profile 目录——不存在则 `create_dir_all` 并打日志"新 profile，首次搜索可能弹 CAPTCHA，解一次后养熟"（新机器第一次跑 = 冷启动养号，行为等同 plsearch 首跑）
- **默认路径**：`%LOCALAPPDATA%/gsearch/profile`（exe 自治，不依赖 D 盘存在）
- **覆盖链**：env `GSEARCH_PROFILE` > 内置默认。CLI 不加 `--profile` 参数（避免误指向空目录）
- **本机迁移**：本机已有 `D:/Sdk/plsearch-profile`（Python 版养的号）→ 设置 env `GSEARCH_PROFILE=D:/Sdk/plsearch-profile` 直接继承，或把目录整个拷到新默认路径
- **跨机器**：profile 是纯 Chrome 用户数据目录，整目录 zip 拷到新电脑同样位置即可带走在别处养出的信誉（注意 Chrome 版本差异过大时可能自动升级 schema）
- **互斥**：同一 user_data_dir 只允许一个 Chrome 实例；启动前 cleanup_stale_locks 只清"进程已死"的残留锁，活进程的锁报错退出并提示
- **备份（运维层）**：养稳后手工 zip 存档；损坏可回滚，换机可直接落位

### 3.2 search.rs（状态机，直接照抄 plsearch main.py:236-343 语义）

```
常量：RESULTS_PER_PAGE=10, MAX_PAGES=10, CAPTCHA_TIMEOUT=120s, DEFAULT_LIMIT=10
URL 模板：https://www.google.com/search?q={urlencoded}&start={page_idx*10}

search(browser, query, limit):
  collected=[], seen=HashSet<url>
  for page_idx in 0..MAX_PAGES:
    if collected.len() >= limit: break
    goto URL; content = get_content()
    if is_captcha(content):            # contains("captcha-form") || contains("recaptcha")
      if collected 非空: return collected[..limit]   # 后页撞墙→给部分结果
      # 首页撞墙→ 切有头
      browser = relaunch(headless=false)
      goto URL; content = get_content()
      if is_captcha(content):
        轮询每 1s 取 content，直到非 captcha 或 120s 超时(→Err)
      browser = relaunch(headless=true)   # 解完永远切回
    results = parse(content)
    if results.is_empty(): break        # 页面无结果→自然终止
    for r in results: if seen.insert(r.url): collected.push(r)
  return collected[..limit]
```

### 3.3 parse.rs（照抄 parse_page.py 语义）

- 遍历所有 `a[href]`
- `a` 内含 `h3` → title=h3 文本、url=href
- snippet：该 `a` 之后的第一个 `div.VwiC3b` 的文本（scraper 的 sibling 遍历；拿不到就空串）
- `replace("\u{a0}", " ")` 清理 nbsp
- **注意**：Google 改版只动 HTML 结构，这套 `<a><h3>` + `.VwiC3b` 选择器是 plsearch 2026 年还在工作的形态，解析为空时要打日志（说明 Google 改版了，第一个排查点）

### 3.4 output.rs

- 默认：`1. 标题\n   url\n   snippet前160字`
- `--json`：serde_json 全量
- `--open N`：`std::process::Command::new("cmd").args(["/c","start","",url])`
- `--read N`：goto 该 url → 取 body inner_text（chromiumoxide 执行 JS `document.body.innerText` 截 5000 字）
- `--dl N`：reqwest GET（或 `page.evaluate(fetch→base64)` 走浏览器 cookie；MVP 用 reqwest 直下，遇需 cookie 站点再升级）→ 按 content-disposition/url 后缀/mime 定文件名 → 写当前目录

### 3.5 通用代理子命令（M6，共享 browser.rs/profile/CAPTCHA 链路）

- `browse <url>`：`launch(headless=true)` → `goto` → `evaluate("document.body.innerText")` → 截 5000 字打印；遇 CAPTCHA 自动走 M3 双模式（任何站点都可能撞码，Google 是已知高发）
- `login <url>`：`launch(headless=false)` → `goto url` → 轮询每 1s 取 content 至用户主动关闭窗口或按 Enter → **不判 CAPTCHA**（让用户登录页面就是真人登录页）；cookie 随 profile 落盘
- `dl <url> [-o PATH]`：**必须走浏览器 cookie**（这是用户要这个工具的核心动机）；`launch(headless=true)` → `goto` → `Browser.setDownloadBehavior{behavior:"allow", downloadPath:PATH}` → goto 触发下载 → 轮询 `downloadProgress` 至 done → 默认 PATH=`.`，`-o` 指定绝对/相对目录；非 Chrome 直接下载（如 raw 文件）走 `page.evaluate(fetch + arrayBuffer + base64)` 落盘（同源 cookie）

## 4. 里程碑

| M | 内容 | 验收 |
|---|---|---|
| **M1 骨架** | clap + browser.rs 启动/关闭 + 打印页面 title | exe 跑起来，能看到 google.com 的 title |
| **M2 核心搜索** | search.rs + parse.rs + 默认输出 | `gsearch search "python asyncio" --limit 3` 出 ≥3 条真结果，耗时 <15s |
| **M3 CAPTCHA** | 双模式切换 + 轮询等待 | 人工把 profile 弄脏触发验证码（或新空 profile 首跑），弹窗可解，解后继续 |
| **M4 搜索结果后处理** | search 子命令的 `--json` / `--open N` / `--read N` / `--dl N` | 论文场景：搜 → read 摘要 → dl PDF |
| **M6 通用代理（无扩展版 panerelay）** | `browse <url>` / `login <url>` / `dl <url>` 三个独立子命令，共享 search 的 browser.rs/profile/CAPTCHA 链路 | `gsearch login https://github.com` 弹有头窗登录关掉后，私有页面 `browse` 能读到；`gsearch dl <pdf-url>` 带登录态落盘非空 |
| **M7 交互式 shell** | `gsearch shell` 起一次 Chrome 会话复用，cookie/页面状态在 prompt 间延续；命令集 search/click/read/dl/browse/login/back/status/help/exit | `gsearch> search python asyncio --limit 2` → `click 1` → `read` → `status` 一气呵成不重启 Chrome；EOF 干净退出 |

## 5. 风险与预案

| 风险 | 预案 |
|---|---|
| chromiumoxide API 与 Playwright 概念差异（Page vs BrowserContext） | plsearch 只用到 goto/content/new_page 四个原语，chromiumoxide 全覆盖；卡住看官方 examples |
| headless=new 模式参数 | chromiumoxide 0.9 的 headless 已是新 headless；UA 遮蔽别忘 |
| Google 改版（parse 空） | 日志警告 + 选择器集中常量，一处改 |
| 老 profile 被 Chrome 新版升级 | 与 Python 版共用同一 Chrome 二进制，无版本漂移问题 |
| CAPTCHA 轮询期间 Chrome 被用户手关 | 轮询里捕获连接断开 → 报错退出（不静默） |
| 出口 IP 被 Google 临时风控（8.219.85.68 实际撞过） | M3 双模式弹有头窗人工解；IP 级封锁不靠 profile 养号解决 |
| chromiumoxide 0.9 `is_likely_js_function` 不认 async 箭头 | 用 `async function ()` 而非 `async () =>`；postproc.rs:75 / general.rs:159 / shell.rs:273 注释 |
| chromiumoxide 0.9 默认 args + Windows Chrome 触发 `ExitStatus(21)` | `disable_default_args()` + 显式安全子集 `{--headless=new, --disable-gpu, --no-sandbox, --disable-dev-shm-usage}`；browser.rs:113-117 |
| cleanup_stale_locks 容错不足 → `os error 32`（活进程持锁）| 容忍 `ErrorKind::NotFound` + `PermissionDenied` + `cfg(windows) raw_os_error()==32`；browser.rs:89-95 |
| 相对路径 `--user-data-dir` Chrome 按 CWD 解析失败 | `profile_dir()` 返回前 `std::path::absolute`；browser.rs:60 |
| chromiumoxide `close()` 不等进程退出 → 下次 launch 撞锁 + 进程残留 | `browser.wait()` 同步等子进程死透（main.rs:155 已加，shell.rs:graceful_close 已用） |
## 6. 验收基准（对照 Python 版实测数据）

- `gsearch search "fastapi tortoise orm tutorial" --limit 3` ≈ 7-10s（Python 版 7.7s）
- `gsearch shell` stdin 全链路实测：`search → click 1 → read` 连续不重启 Chrome，profile cookie 延续；EOF 退出 exit 0
- 5 次连搜背靠背 cmd_search 实测：Chrome 进程残留从修复前 ~5 累计 → 修复后 0 累计（修复 = `browser.wait()`）
- exe 体积 ~8-12MB，`gsearch.exe --help` 无任何外部依赖输出
