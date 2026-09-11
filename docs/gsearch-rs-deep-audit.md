# gsearch-rs 深度审计 + GitHub 差距对比报告

- **生成日期**：2026-08-26
- **HEAD**：`92611d0 polish(product): C 路线 5 项产品级产出`（M12）
- **基线**：356 nodes / 1094 edges 知识图谱（本次会话重新索引）
- **方法**：
  1. Self-Audit（read-only，codebase-memory-mcp + grep + read）— 见 `gsearch-rs-self-audit.md` (36.7 KB)
  2. GitHub Competitor Scan（web search only, no clones）— 见 `gsearch-rs-competitor-scan.md` (8.6 KB)
  3. 本报告 = 两份合并 + 差距清单 + 路线图 + ADR 提案

---

## 0. TL;DR — 三句话结论

1. **代码本身在生产可用档位**：0 unwrap / 0 panic 在 production、graceful_close 全员覆盖、模块 DAG 干净。3 个 Critical 都在 10 行内可修。
2. **赛道独占**：real Chrome + persistent portable profile + Google + login + dl + shell + multi-browser fallback 这条线，5 个最强 GitHub 竞品没有一个占齐。我们赢这片。
3. **真正缺的不是别人已经做对的事**：是 anti-bot 升级路径（M10 隐身强度不够时）、MCP-server 入口（agent 友好）、search-cli 多 provider 互补（offload AI 走 API 时）。三件事都有标杆，**不抄即落后**。

---

## 1. 自家审计结果（自评 — Critical/Important 优先）

### 1.1 Critical（3 项，必须修）

| # | 标题 | 位置 | 修复大小 |
|---|------|------|----------|
| C1 | `cmd_login` 用户跳转后无限轮询 | `general.rs:104-133` + `shell.rs:353-370` | ~3 行 × 2（检测 URL 变化） |
| C2 | `--proxy` 凭据泄漏到 stderr | `browser.rs:220` | ~1 行（redact `user:pass@`） |
| C3 | `postproc::dl` + `dl_in_page` 静默忽略 `--output` | `postproc.rs:151-152` + `shell.rs:319-320` | ~10 行（thread `output`） |

**安全/正确性影响**：C2 是密码泄漏到日志（CI / terminal scrollback / 屏幕录制全暴露）；C1 让 `login` 命令必须 Ctrl+C 强杀；C3 是 `search --dl 1 -o ./dir/` 静默写到 CWD。

### 1.2 Important（9 项，技术债 + 边界）

- **I1** `cleanup_stale_locks` 在 Unix 上吞 PermissionDenied（profile 权限 bug 被遮蔽） — `#[cfg(windows)]` 3 行
- **I2** `is_captcha` 子串匹配误报（任何提到 `recaptcha` 的博客触发）— DOM 域检测 10 行
- **I3** `poll_until_solved` 解码后返回的是 Google 欢迎页（0 结果误导用户）— 重新导航原 SERP 5 行
- **I4** 3 个 `dl` 路径吞 goto timeout — `let _ =` 换 `?` 3 行
- **I5** `dl_in_page` 用 base64 而非 JSON bytes — 与 `fetch_in_page` 不一致，50MB+ 文件 3x 内存浪费
- **I6** 死代码：`human_type` / `human_click` + `Jitter`（~190 行）— M10 没用上，要么接进来要么删
- **I7** 重复：`graceful_close` 两份、`browser_alive` 两份 — 删一份
- **I8** `cmd_login` 同 C1
- **I9** `cmd_dl -o` 允许任意绝对路径 — 无 cwd/home 限制

### 1.3 测试覆盖

- **当前**：~36 个单测，集中在 skeleton.rs（14 个 extract_adaptive）+ parse.rs（SERP 配对）
- **缺口（5 MUST-add）**：
  1. `filename_from_url` + `dl` 写盘路径回归测试
  2. `is_captcha` 误报 fixture（5-10 个真 Google 摘要含 "recaptcha"）
  3. `profile_name` Windows 保留名（CON/NUL/PRN/AUX/COM1-9/LPT1-9）
  4. `cmd_login` URL 变化退出（mock page）
  5. `cleanup_stale_locks` Unix PermissionDenied 传播

### 1.4 Ponytail 视角的「故意保留」

- `Jitter` 单步 xorshift + modulo bias → 隐身够用，不需 CSPRNG ✅
- `format_adaptive` 不转义 markdown → stdout 给人看，pipe 到 md 渲染器才需要 ✅
- `--verbose info` 打印 query/IP → doctor 和 search 都需要这条给用户看 ✅
- `dl_in_page` 50MB 阈值 warn 而非 reject → 沿用现行 ✅
- `0.unwrap` 在 production + 0 panic ✅（M12 polish 修干净了）
- Chromiumoxide 0.9 close+wait 模式全员覆盖 ✅

### 1.5 「什么是好的」清单（保留）

1. 模块 DAG 单向无环：`main → general/postproc/shell/search → browser/skeleton/stealth/parse → util/types/output`
2. 所有 release 命令都 `close + wait`
3. `--open/--read/--dl` 已经在 clap ArgGroup 互斥（M12）
4. `profile_name` 路径穿越防护 + 3 个测试用例
5. 中文 SERP 友好的标点处理（first_sentence 双语）
6. humanize 默认 false（保护老 profile 不被污染）

---

## 2. GitHub 竞品对比 — 5 个最强相邻项目

> 完整版：`gsearch-rs-competitor-scan.md`

| 项目 | Stars | 解决什么 | 他们的强 | 我们的强 | 是否移植 |
|------|-------|----------|----------|----------|----------|
| paperfoot/search-cli | ~500 | 多 provider rank-fusion + JSON for AI | 13 个 SERP provider（Serper/Exa/Tavily/Brave/Jina/...）| 零 API key + 真 Chrome + login + dl + shell + 多浏览器 fallback | **NO**（破零配置属性）|
| vercel-labs/agent-browser | ~23.5k | Agent 友好浏览器（CDP CLI）| a11y-tree `@eN` refs + MCP + Electron 技能 + 视频录制 + auth vault | Google-specific + profile 便携 + multi-browser + doctor + native dl | **NO**（受众不同；可学 a11y-ref）|
| chaser-oxide | ~270 | chromiumoxide fork，协议级隐身 | CDP 传输层 stealth + Turnstile/GeeTest solver | 我们是成品 CLI、他们是库 | **YES（feature flag 接入）** |
| BB-fat/browser-use-rs | ~42 | 浏览器 MCP server | MCP-server mode（Claude Code 直接驱动）| Google 解析 + login + shell + 多浏览器 + doctor | **NO**（低活跃；MCP 自包即可）|
| us/crw | ~590 | Firecrawl 替代 + /search + /scrape API | MCP server + 6MB RAM + 2.3x Tavily 速度 | 真 Chrome + 持久 profile + login + native dl | **NO**（API-key 服务路线）|

### 2.1 我们没解决的问题 → 别人解决了吗？

| 痛点 | 别人怎么做的 | 我们要不要学 |
|------|--------------|--------------|
| **Google CAPTCHA 撞码**（裸搜首跑）| chaser-oxide 集成 Turnstile/GeeTest solver（付费 API）；2Captcha/AntiCaptcha 服务 | **不抄**——保留 "warmup → fail-loud → 人工 login" 零成本路径。仅在用户投诉撞码率高时升级。 |
| **AI agent 友好入口**（Claude Code 想直接调）| vercel-labs/agent-browser 走 MCP；BB-fat/browser-use-rs 走 MCP；us/crw 走 MCP | **应该考虑 M13**——gsearch shell 已经是 interactive agent 友好，但 MCP 1 天能加（~100 LOC thin wrapper）|
| **多 provider 互补**（Google 被封时切 Brave/Serper）| search-cli 一站聚合 | **不抄**——保持零 API key。但可以在 README 推荐 search-cli 作为「付费 agent-day-to-day」伴侣 |
| **协议级隐身**（WebDriver/CDP 指纹被识别）| chaser-oxide transport patch | **应该作为 M13 升级路径**（feature flag 接入，约 150 LOC + Doctor 更新）|
| **Accessibility ref 方案**（`click @e3`）| agent-browser 首创 | **值得借用**（~1 天）让 shell `click N` 更直观 |
| **HTTP-only SERP**（不依赖浏览器）| **没人**做到生产级；Python reqwest + curl-impersonate 是私有 fork 模式 | **greenfield**，不抄 |
| **HTTP-only 隐身抓 Google**（curl-impersonate + ja3 + http2）| 私有 fork，无开源产品 | 不抄 |

### 2.2 我们独占的护城河（5 项竞品都没做对）

1. **Profile 目录可压缩 zip 整机迁移**（agent-browser 有 known bug）
2. **`doctor` 自检 + `dl` 走原生 CDP `Browser.setDownloadBehavior` + 多浏览器 Chrome↔Edge 兑底** —— **三条同时存在 = 0 个竞品**
3. **Interactive shell 复用单次 Chrome session**（竞品都是一次性命令）
4. **M11 multi-browser fallback**：Chrome 挂了自动兑底到 Edge（竞品要么 Chrome 要么硬编码）
5. **零 API key + 零 daemon + 单 exe**：gsearch 的本体定位

---

## 3. 差距清单 = 改进路线图（推荐优先级）

### P0 — 立即修（CRITICAL，~30 行净增）

| 行动 | 文件 | 影响 |
|------|------|------|
| 修 C1：cmd_login URL 变化检测 | general.rs + shell.rs | 解决 `login` 必须 Ctrl+C 死锁 |
| 修 C2：proxy log redact | browser.rs:220 | 避免密码泄漏 |
| 修 C3：dl --output plumbing | postproc.rs + shell.rs + SearchArgs | CLI 行为一致 |

**预估**：1 个里程碑（M13-Security）+ 3 个 commit，~30 行

### P1 — 借机会搭便车（IMPORTANT，~80 行净删）

| 行动 | 收益 | 行数 |
|------|------|------|
| 修 I4：dl 三处 `let _ =` → `?` | 大文件下载超时能感知 | -3 / +3 |
| 修 I5：dl_in_page 改 JSON bytes | 50MB+ 下载 3x 内存节省 | ~10 |
| 修 I6：删 human_type + human_click（M10 没接）| 死代码清理 | -38 |
| 修 I7：删 duplicate graceful_close + browser_alive | -14 行 | -14 |
| 修 I1：`#[cfg(windows)]` gate PermissionDenied | Unix 真 bug 显形 | ~3 |
| 修 I2：is_captcha strip `<script>`/`json-ld` | 误报降 | ~10 |
| 修 I3：poll_until_solved 重新导航原 SERP | 用户不再收到「0 结果」 | ~5 |
| 修 I9：cmd_dl -o 限制 cwd/home | 防御越界写入 | ~5 |

**预估**：1 个里程碑（M13-DebtCleanup），~80 行净删

### P2 — 测试覆盖补齐（5 MUST-add，~100 行）

见 §1.3 的 5 项 MUST 测试清单。

### P3 — 借 M10/M11/M12 的外部标杆

| 行动 | 来源 | 优先级 | 理由 |
|------|------|--------|------|
| 调研 chaser-oxide 接入（feature flag）| chaser-oxide | M13/14 | 当用户撞码率超 warmup-only 容忍度时启用。~150 LOC。**不要默认接** |
| 加 MCP-server thin wrapper（~1 天）| agent-browser + BB-fat + us/crw 都做 | M13 agent-friendly | 现有 shell 已能服务 agent，但 MCP 协议让 Claude Code/Cursor 直接调，门槛更低 |
| 借 a11y-ref 方案（`click @e3`）| vercel-labs/agent-browser | M14 | shell UX 微优化，~1 天 |
| README 末尾加「Companion tools」段落推荐 search-cli | paperfoot/search-cli | M13 文档 | 互补而非竞争，agent 用户需要知道 |

### P4 — 不抄（保留 YAGNI）

- ❌ 多 provider rank-fusion（破零 API key）
- ❌ HTTP-only SERP 模式（greenfield，无标杆）
- ❌ 视频录制、auth vault（与我们定位无关）
- ❌ 自动 CAPTCHA solver（Google reCAPTCHA v3 还是要付费 API）

---

## 4. ADR 提案

### ADR-001: gsearch-rs 定位 = "零 API key 单 exe 真 Chrome CLI"

**背景**：5 个 GitHub 竞品走「API-key + 服务端」路线，3 个走「agent 通用浏览器」路线。我们是这之间唯一占「零配置 + 真 Chrome + Google-specific vertical」的位置。

**决策**：
- 不接 API-key（不抄 search-cli / us/crw）
- 不做服务端 API（保持单 exe）
- 不脱 Chrome（HTTP-only SERP 是 greenfield，无验证模式）
- 接 MCP server 时**包一层而不是改 base**

**后果**：
- 正面：独占市场，无直接竞争
- 负面：用户撞码时只能 `login` 人工（除非升级到 chaser-oxide feature flag）

### ADR-002: M13 = Security & Debt Cleanup（不是新功能）

**背景**：审计发现 3 个 Critical 都是「容易修 + 影响大」+ 9 个 Important 大多是「死代码 / 重复」。M14 才有外部标杆可学。

**决策**：M13 不开新功能，只做：
- C1+C2+C3 三个 Critical 修
- I1+I4+I5+I6+I7 五个 Important 修
- 5 MUST-add 测试

**后果**：~110 行净删（M12 polish 已是 C 路线 5 项产品级产出，M13 仍能保持产品级输出）

### ADR-003: M14 = chaser-oxide 升级 + MCP-server（按需）

**触发条件**：M13 合并后看用户反馈：
- 撞码率 > 用户可接受 → 接 chaser-oxide feature flag
- agent 用户（MCP 客户端）有需求 → 加 MCP thin wrapper
- 两者都没需求 → M14 跳过，看 M15

**决策前置条件**：
- chaser-oxide 必须测试覆盖率 ≥ 现 chromiumoxide 0.9 路径（不能为隐身换稳定）
- MCP wrapper 不改内部 API，仅暴露 `cmd_search/cmd_browse/cmd_login/cmd_dl/cmd_status` 5 个工具

---

## 5. 下一步（PM 拍板需要你决策）

| 决策点 | 选项 | 推荐 |
|--------|------|------|
| M13 范围 | A. 只修 3 Critical / B. 3 Critical + 5 Important + 5 测试 / C. 全部 | **B**（最高 ROI） |
| M14 是否接 chaser-oxide | A. 接 / B. 不接 / C. 等 M13 用户反馈再定 | **C**（feature flag 准备好但默认不开） |
| M14 是否加 MCP server | A. 加 / B. 不加 / C. 等需求 | **C**（设计留位，不写代码） |
| 死代码 human_type/human_click | A. 删 / B. 留 M10 接 / C. 移到独立 feature branch | **A**（YAGNI；M10 真要时 git revert） |
| `cmd_dl -o` 路径限制 | A. 限 cwd / B. 限 home / C. 不限制 | **B**（与 `dl` 默认行为一致） |
| profile_name 拒绝 Windows 保留名 | A. 加 / B. 不加 | **B**（仅 Chrome 行为怪异，不崩；6 行换 0 收益） |

---

## 6. 文件清单

| 文件 | 大小 | 用途 |
|------|------|------|
| `gsearch-rs-self-audit.md` | 36.7 KB | 完整自家审计（含每项的修复建议 + 测试用例） |
| `gsearch-rs-competitor-scan.md` | 8.6 KB | 5 个 GitHub 竞品 + gap map |
| `gsearch-rs-deep-audit.md` | 本文件 | 汇总 + 路线图 + ADR + PM 决策点 |
