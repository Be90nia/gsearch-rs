# gsearch-rs 用户指南

> 把 Google 搜索 + 通用浏览器代理封进一个 Rust 单文件 exe CLI。
> 真 Chrome + 持久 profile + 自动 CAPTCHA 双模式（headless ↔ 有头窗）。

---

## 1. 安装

### 系统要求

- Windows 10/11（64-bit）/ macOS / Linux 64-bit
- Chrome 或 Edge（已在系统中安装）
- ~10 MB 磁盘空间

### 编译（开发者）

```bash
git clone https://github.com/your-org/gsearch-rs
cd gsearch-rs
cargo build --release
# 产物在 target/release/gsearch(.exe)，单文件，~10.7 MB
```

### 直接下载 release exe（用户）

[Releases 页面](https://github.com/your-org/gsearch-rs/releases) 下载对应平台单文件，扔进 PATH 即可。

```bash
gsearch --version
# gsearch 0.1.0
```

---

## 2. 五个常见场景

### 2.1 基础搜索

```bash
gsearch search "fastapi tortoise orm tutorial" --limit 5
```

输出形如：

```
1. Tortoise ORM FastAPI Tutorial
   https://tortoise.github.io/examples/fastapi.html
   FastAPI 是一个基于 Starlette 的 Python web 框架。...

2. FastAPI Tortoise ORM 入门指南
   https://zhuanlan.zhihu.com/p/...
   本文将介绍如何在 FastAPI 中集成 Tortoise ORM...

...
```

### 2.2 `--json` 给下游程序

```bash
gsearch search "rust tokio" --limit 3 --json
```

```json
[
  {
    "title": "Tokio - An asynchronous runtime for Rust",
    "url": "https://tokio.rs/",
    "snippet": "Tokio is an asynchronous runtime..."
  },
  ...
]
```

### 2.3 `--read N` 直读结果正文（不必点开链接）

```bash
gsearch search "rust tokio tutorial" --limit 3 --read 1
```

输出：目录 + 摘要 + 段落索引（M9 AdaptiveRead 智能选择短/中/长文策略）。

```
=== https://tokio.rs/ | Tokio ===
[目录]
  # Tokio - An asynchronous runtime for Rust

[摘要]
本文介绍 Tokio 异步运行时...

[段落索引]
1. Tokio 是 Rust 生态最流行的异步运行时...（300 字）
2. Tokio 提供异步 I/O、定时器、信道...
```

加 `--full` 拿纯 innerText，`--headings-only` 只输出目录，`--json` 拿结构化 JSON。

### 2.4 `--dl N` 下载链接带 profile 登录态

```bash
gsearch search "arxiv 2503.12345" --limit 3 --dl 1
# 已下载: ./download.pdf (1.2 MB)
```

复用你 profile 里的 cookie 登录态——免登录下载学术 PDF。

### 2.5 `--open N` 系统默认浏览器开

```bash
gsearch search "github" --limit 1 --open 1
```

---

## 3. 通用浏览器代理：browse / login / dl

### 3.1 `browse <url>` ——任意 URL 拿正文

```bash
gsearch browse https://example.com
```

### 3.2 `login <url>` ——有头窗人工登录

```bash
gsearch login https://github.com
```

打开有头 Chrome → 你输入账号密码 → 关窗 → cookie 自动落 profile。

下次 `browse <github私有URL>` 就能读到。

### 3.3 `dl <url>` ——带 profile 登录态下载

```bash
gsearch dl https://example.com/file.pdf -o ~/Downloads
# 已下载: C:\Users\You\Downloads\file.pdf (1.2 MB)
```

---

## 4. 交互式 Shell（高级用户）

```bash
gsearch shell
gsearch> search python asyncio --limit 3
gsearch> click 1       # 在搜索结果里点第 1 条，等同 browse 第 1 个
gsearch> read          # 当前页 read
gsearch> status        # 当前 URL / profile
gsearch> back          # 后退
gsearch> dl 1          # 下载第 1 个搜索结果
gsearch> exit          # EOF 也行（Ctrl+D / Ctrl+C）
```

---

## 5. Profile 管理

### 5.1 默认 profile

`~/.gsearch/profiles/default/`——自动创建、首次启动会打 INFO 日志。

### 5.2 命名 profile（多账号/工作-生活分流）

```bash
GSEARCH_PROFILE=work gsearch search "github rust client" --limit 3
GSEARCH_PROFILE=personal gsearch search "..." --limit 3
```

会自动创建 `~/.gsearch/profiles/work/` 与 `~/.gsearch/profiles/personal/`。

### 5.3 继承 Python 版 plsearch profile

Python 版 `plsearch` 养的号可以直接继承（Chrome 磁盘格式与驱动语言无关）：

```bash
GSEARCH_PROFILE=D:/Sdk/plsearch-profile gsearch search "..."
```

### 5.4 跨机器迁移

profile 是纯 Chrome 用户数据目录——`zip` 拷过去，落位即可：

```bash
# 老机器
zip -r profile.zip ~/.gsearch/profiles/work/

# 新机器（假设先安装 gsearch）
unzip profile.zip -d ~/.gsearch/profiles/work/
```

---

## 6. CAPTCHA 与 IP 风控

Google 2026 反爬严苛，本机出口 IP 偶尔被临时封禁。gsearch 应对：

| 场景 | 行为 |
|---|---|
| 首次搜索撞 CAPTCHA（首页）| 自动切有头窗弹真 Chrome → 每 15s 心跳进度 → 等你人工解 → 自动回 headless 继续 |
| 翻页撞 CAPTCHA | 不打扰人，立即返回已有部分结果 |
| IP 被封全局撞码 | 走 `gsearch doctor` 看出口 IP；考虑换 IP / 用 `--proxy http://127.0.0.1:7890` |

### 6.1 `--proxy` / `GSEARCH_PROXY` 环境

```bash
gsearch --proxy http://127.0.0.1:7890 search "..." --limit 5
# 或
GSEARCH_PROXY=socks5://127.0.0.1:1080 gsearch search "..." --limit 5
```

支持 HTTP / SOCKS5（Chrome 协议）。

### 6.2 `--humanize`（opt-in）

新 profile / 裸搜可能撞码时启用：装 10 个 fingerprint 补丁 + 访问 Wikipedia/GitHub warmup。

```bash
gsearch --humanize search "..." --limit 5
```

**注意**：`--humanize` 默认 off——保护已有 profile 用户不被污染。

---

## 7. 浏览器选择

```bash
gsearch --browser auto search "..." --limit 5   # 默认：Chrome → Edge 兑底
gsearch --browser chrome search "..." --limit 5  # 强制 Chrome
gsearch --browser edge search "..." --limit 5    # 强制 Edge
```

Chrome 突然挂、版本冲突时用 Edge 兑底。

### 7.1 自定义 Chrome 路径

```bash
GSEARCH_CHROME=/path/to/chrome.exe gsearch search "..."
```

---

## 8. doctor 子命令

```bash
gsearch doctor
```

输出 6 项检查：

```
gsearch doctor
[ OK ] Chrome: C:\Program Files\Google\Chrome\Application\chrome.exe
[ OK ] Edge:   C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe
[ OK ] profile 可写: C:\Users\You\.gsearch\profiles\default
[ OK ] 出口 IP: 61.144.188.80
[ OK ] 网络连通 (www.google.com:443)
[ OK ] GSEARCH_PROFILE 未设置（默认 ~/.gsearch/profiles/default/）

所有检查通过 ✓（耗时 385ms）
```

撞码时第一跑 `doctor` 看出口 IP——被 Google 封了换 IP / proxy 即可。

---

## 9. 日志与调试

```bash
# 默认：仅 INFO/WARN
gsearch search "..."

# debug：完整 CDP / WS 通信日志（撞码诊断用）
gsearch --verbose debug search "..."

# 单模块 verbose
gsearch --verbose gsearch::search=debug search "..."
```

日志写到 stderr（结构化 `INFO 搜索...`），stdout 留给结果输出。

---

## 10. 退出码

| 退出码 | 含义 |
|---|---|
| 0 | 成功 / 仅有 WARN |
| 1 | 出错（启动失败 / 配置错误）|
| 2 | 搜索无结果 |

---

## 11. 已知限制

- **真 Google 搜索撞码率**：依赖出口 IP 信誉，长期养 profile 可降低；新 profile 首跑可能撞码
- **CAPTCHA 不自动解**：撞码需人工介入（有头窗弹出）——这是 Google 政策红线，自动解绕不过
- **proxy 协议**：仅 HTTP / SOCKS5（Chrome 原生支持）
- **Windows-only 几个细节**：  
  - `--open` 走 `cmd /c start`（Linux/macOS 用户可改 `xdg-open` / `open`）  
  - 控制台 UTF-8 用 `SetConsoleOutputCP(65001)`（仅 Windows）  
  - profile lock 文件路径在 Windows 下是 `lockfile`（POSIX 是 `SingletonLock`）

---

## 12. FAQ

**Q：和 Python 版 `plsearch` 有什么区别？**
A：plsearch 是常驻 MCP 服务（依赖 uv/venv），gsearch-rs 是单 exe 用完即走（~10MB，零运行时依赖）。核心算法一致（CAPTCHA 双模式、UA 遮蔽、profile 持久化）。

**Q：能挂代理吗？**
A：能。`--proxy http://127.0.0.1:7890` 或 `GSEARCH_PROXY` 环境变量。

**Q：profile 能跨机器？**
A：能。profile 是纯 Chrome 目录，`zip` 拷过去即可。

**Q：撞码怎么破？**
A：`gsearch doctor` 看出口 IP；临时换 IP/VPN；长期养熟 profile。

**Q：为什么单 search 没结果但医生说 Chrome OK？**
A：Google 服务端判断。撞码率与 IP 信誉有关——`--humanize` + `--proxy` 临时救场，长期靠养 profile。

---

## 13. 命令速查

| 命令 | 用途 |
|---|---|
| `gsearch search "query" --limit N` | 搜 Google |
| `gsearch search "query" --json` | JSON 输出 |
| `gsearch search "query" --read N` | 直读第 N 条正文 |
| `gsearch search "query" --dl N` | 下载第 N 条 |
| `gsearch search "query" --open N` | 系统浏览器开第 N 条 |
| `gsearch browse <url>` | 任意 URL 拿正文 |
| `gsearch login <url>` | 有头窗人工登录 |
| `gsearch dl <url> -o DIR` | 带 profile 下载 |
| `gsearch shell` | 交互式 REPL |
| `gsearch doctor` | 自检 |
| `gsearch --verbose debug ...` | debug 日志 |
| `gsearch --proxy URL ...` | 走代理 |
| `gsearch --browser chrome\|edge\|auto ...` | 选浏览器 |
| `GSEARCH_PROFILE=name gsearch ...` | 命名 profile |
| `GSEARCH_PROXY=URL gsearch ...` | env 代理 |
| `GSEARCH_CHROME=PATH gsearch ...` | 自定义 Chrome |