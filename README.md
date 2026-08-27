# gsearch-rs

Google 搜索 + 通用浏览器代理 CLI：单 exe、零扩展、零运行时依赖（有 Chrome 即可），移植自 plsearch（Python/Playwright）的核心能力。

## 用法

### search（Google 搜索）

```
gsearch search "python asyncio" --limit 10
gsearch search "fastapi tutorial" --json
gsearch search "..." --humanize=false   # 跳过搜索前 warmup
gsearch search "..." --read 1
gsearch search "..." --dl 1
gsearch search "..." --open 1
```

`--humanize` 默认启用：Google 搜索前随机访问 Wikipedia/GitHub/HN，滚动并短暂停留；指纹补丁仅用于 search，不改变 browse/login。

### browse / login / dl（通用代理）

```
gsearch browse https://example.com          # 渲染后正文（innerText 前 5000 字）+ URL/标题
gsearch login  https://github.com           # 弹有头窗人工登录；关窗 = 完成，cookie 落 profile
gsearch dl    https://.../file.pdf          # 带 profile 登录态真下载（Chrome 原生下载流）
gsearch dl    https://.../file.pdf -o DIR   # 下载到指定目录（不存在则创建）
```

- **browse**：headless 渲染取正文；遇 CAPTCHA 报错退出并提示用 `login` 手工验证后重试
- **login**：有头窗 + 不限时轮询，人关窗（或关页签）即认为登录完成，cookie 随 profile 落盘；不判 CAPTCHA
- **dl**：CDP `Browser.setDownloadBehavior` 走 Chrome 原生下载（登录态、重定向、大文件均支持）；
  渲染型 URL（普通网页不触发下载）自动回退页内 fetch 落盘（同源 cookie），默认存当前目录

### shell（交互模式，可选）

`gsearch shell` 起一次 Chrome 后台会话，prompt `gsearch> ` 持续读 stdin，cookie / 页面状态跨命令延续。
单 exe 「用完即走」原则不破：shell 是可选模式，顶层一次性命令全部保留。

```
$ gsearch shell
进入 gsearch shell（输入 help 查命令，exit / quit / Ctrl+D 退出）
gsearch> search python asyncio --limit 2
1. asyncio — Asynchronous I/O
   https://docs.python.org/3/library/asyncio.html
   ...
2. Python's asyncio: A Hands-On Walkthrough
   https://realpython.com/async-io-python/
   ...
gsearch> click 1
已跳转到: https://docs.python.org/3/library/asyncio.html
gsearch> read
=== https://docs.python.org/3/library/asyncio.html | asyncio — Asynchronous I/O === ...
gsearch> status
current_url : https://docs.python.org/3/library/asyncio.html
title       : asyncio — Asynchronous I/O
results     : 2
profile     : C:\Users\Begonia\AppData\Local\gsearch\profile
gsearch> dl                # 下载 current_url 到当前目录（按 URL 末段自动命名）
已下载: asyncio.html (25375 bytes)
gsearch> <Ctrl+D>          # EOF 优雅退出，Chrome 自动关
```

可用命令：`search <query> [--limit N]` / `click <N>`（或 `open <N>`）/ `read` / `dl [N]` / `browse <url>` /
`login <url>` / `back` / `status` / `help` / `exit` / `quit`。
EOF（Ctrl+D / Ctrl+Z+Enter）才真正退出；单条命令出错只打印 `error:` 不退出 shell。

### Profile

- 默认命名 profile：`~/.gsearch/profiles/default/`
- `GSEARCH_PROFILE=work`：使用 `~/.gsearch/profiles/work/`；`GSEARCH_PROFILE=D:/foo/bar/` 使用末段 `bar`
- 空的末段、`..` 或根路径会报错，不回退覆盖已有 profile
- 首次冷启动养号，可能遇 CAPTCHA，人工解一次后养熟
- Profile 可整目录 zip 携走，换机只需放同位置

### 环境变量

- `GSEARCH_PROFILE`：profile 名或任意输入路径（统一取末段名）
- `RUST_LOG=debug`：查详细信息

### 配置文件（gsearch.json，可选）

不想用环境变量时，写 JSON 配置文件：

```json
{
  "profile": "work",
  "chrome": "D:/Sdk/Chrome/chrome.exe"
}
```

查找顺序：`--config <path>` 显式指定 → `./gsearch.json`（当前目录）→ `~/.gsearch/config.json`。
只读已存在的文件，不主动创建——exe 和 gsearch.json 放同一目录即“绿色软件”，清理零残留。

优先级（各键独立）：环境变量 > 配置文件 > 默认值。`profile` 值语义同 `GSEARCH_PROFILE`
（取末段名放进 `~/.gsearch/profiles/`，保留名/非法路径照拦）。

### `--browser <chrome|edge|auto>`（M11 多浏览器兑底）

所有顶层子命令（`search` / `browse` / `login` / `dl`）接受 `--browser`：

```
gsearch search "rust" --browser edge               # 强制走 Edge
gsearch search "rust" --browser chrome             # 强制走 Chrome
gsearch search "rust"             # 默认 auto：优先 Chrome，缺则兑底 Edge
```

检测顺序：
1. `GSEARCH_CHROME` env（指向 chrome.exe / msedge.exe 都行，含 `msedge` 自动判 Edge）
2. Chrome 默认安装路径（`C:/Program Files/Google/Chrome/Application/chrome.exe`）
3. Edge 默认安装路径（`C:/Program Files/Microsoft/Edge/Application/msedge.exe` + x86 路径）
4. `where chrome.exe` / `where msedge.exe`

显式指定不可用时仍兑底到第一个可用浏览器，不报错。Edge 是 Chromium 内核，与 Chrome 参数完全兼容。

### `gsearch doctor`（M11 健康检查）

不启动浏览器；3 秒内完成 6 项自检，每项标 `[OK]` / `[WARN]` / `[FAIL]`：

```
$ gsearch doctor
gsearch doctor
[ OK ] Chrome: C:\Program Files\Google\Chrome\Application\chrome.exe
[ OK ] Edge:   C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe
[ OK ] profile 可写: C:\Users\Begonia\.gsearch\profiles\default
[ OK ] 出口 IP: 61.144.188.80
[ OK ] 网络连通 (www.google.com:443)
[ OK ] GSEARCH_PROFILE 未设置（默认 ~/.gsearch/profiles/default/）

所有检查通过 ✓
```

- **Chrome / Edge**：路径是否找到；Edge 缺仅给 WARN（仍可跑）
- **profile 可写**：在默认 / 自定义 profile 目录建一个临时探针文件做读写验证
- **出口 IP**：明文 HTTP GET `http://ipv4.icanhazip.com/` 取公网 IP。**撞码时可以这里查 IP 被封状况**（出口 IP 异常/变了都提示 VPN/代理需切换）
- **网络连通**：TCP connect `www.google.com:443`，2 秒超时
- **GSEARCH_PROFILE**：环境变量检查，缺/空用默认；路径不存在仅 WARN（首次启动会建）

任意 FAIL 退出码 1；WARN 整体可用；都 OK 退出 0。CI 或首次安装后跑一次可快速定位是浏览器路径、profile 权限、网络出口哪一类故障。
### 安装与构建

三种方式任选：

```
# 1. Release 页下载单二进制（Windows / Linux / macOS）
#    打 tag v* 自动构建并附加到 GitHub Releases

# 2. 源码安装（需 Rust 工具链）
cargo install --path .

# 3. 源码构建
git clone <repo> && cd gsearch-rs && cargo build --release
./target/release/gsearch --help
```

单 exe + Chrome 即可运行，不装 Python/venv/Node。Linux/macOS 同样只需本地有 Chrome 或 Edge。

## 设计

项目根 [`docs/PLAN.md`](docs/PLAN.md) 为权威设计文档。

## License

MIT，见 [LICENSE](LICENSE)。

## Companion tools

需要多搜索引擎 provider（Bing / DuckDuckGo / Brave 等）互补时，推荐搭配 paperfoot 或 search-cli；gsearch 专注 Google 搜索 + 通用浏览器代理这一条单刀路径。