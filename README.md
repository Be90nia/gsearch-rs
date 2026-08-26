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
