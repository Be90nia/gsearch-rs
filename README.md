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
- `GSEARCH_CHROME`：覆盖默认 Chrome 路径
- `RUST_LOG=debug`：查详细信息

### 安装与构建

```
cargo build --release
./target/release/gsearch.exe --help
```

单 exe + Chrome 即可运行。不装 Python/venv/Node。

## 设计

项目根 PLAN.md 为权威设计文档。