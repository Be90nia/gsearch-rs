# gsearch-rs

Google 搜索 + 通用浏览器代理 CLI：单 exe、零扩展、零运行时依赖（有 Chrome 即可），移植自 plsearch（Python/Playwright）的核心能力。

## 用法

### search（Google 搜索）

```
gsearch search "python asyncio" --limit 10
gsearch search "fastapi tutorial" --json
gsearch search "..." --read 1   # 读第 1 条正文
gsearch search "..." --dl 1     # 下载第 1 个文件（走同源 cookie）
gsearch search "..." --open 1   # 弹默认浏览器打开第 1 条
```

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

- 路径优先级：env `GSEARCH_PROFILE` > `%LOCALAPPDATA%/gsearch/profile`
- 首次冷启动养号，可能遇 CAPTCHA，人工解一次后养熟
- Profile 可整目录 zip 携走，换机只需放同位置
- 本机已有 `D:/Sdk/plsearch-profile` 是 Python 版养的号，设 env 即可继承

### 环境变量

- `GSEARCH_PROFILE`：覆盖默认 profile 路径
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