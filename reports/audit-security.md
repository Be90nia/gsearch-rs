# 安全审查报告 — gsearch-rs diff 0daea09..acc7b61

**VERDICT: CONDITIONAL**

引入一个**新增 SSRF 攻击面**（fetch 子命令无 host/内网地址过滤），其余 7 个 commit 安全面平移或改善。多路 `content_untrusted` 标注、`rustls` 启用、recency 字符面为静态枚举，无 Critical。修复须聚焦 fetch 的网络边界。

---

## 范围与方法

- **范围**：`git diff 0daea09..acc7b61`（8 commit：batch+输出契约、退出码文档、浏览器探测用户级、beads 状态×2、fetch、JS 壳判定修复、recency、clippy）
- **审计维度**：OWASP Top 10 / NIST SSRF.SSRF01 / 提示注入 / 依赖与 TLS / 配置注入
- **威胁模型**：本地单用户 CLI（agent / 用户驱动）、GitHub Releases 二进制分发

## 漏洞发现

| 级别 | 类型 | 位置 | 描述 | 修复建议 |
|---|---|---|---|---|
| **Important** | SSRF（内网/云 metadata） | `src/fetch.rs:30-45` `cmd_fetch` | URL 只过 reqwest `IntoUrl`（拒 `file://`/`blob:`，但放过一切 `http://`/`https://`）。**无 host 白名单/黑名单**：可直击云 metadata `http://169.254.169.254/latest/meta-data/`，或扫内网 `http://127.0.0.1/admin`、`http://[::1]:port/`、`http://10.x/192.168/172.16`。攻击链：用户被诱导 `gsearch fetch http://169.254.169.254/...` → 二进制把 IAM role 凭据正文打进 stdout/JSON，agent 进一步处理可致凭据外泄或云控制面命令注入 | (1) 解析 URL 后取 host，拒 RFC1918 / link-local (169.254.0.0/16, 169.254.169.254) / loopback (127.0.0.0/8, ::1) / multicast / `0.0.0.0`；或 (2) 默认仅放白名单公开域，添加 `--allow-private` opt-in；或 (3) 与 searxng.rs 的"局域网实例"先例对齐，在 fetch 文档显式声明「内网可达由用户承担」。**至少**需要在 `cmd_fetch` 起点拒绝 loopback / link-local。 |
| **Important** | SSRF（scheme 跳变 / 跨 scheme 重定向） | `src/fetch.rs:35` `redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))` | `Policy::limited(10)` **不设 https_only**（reqwest `redirect.rs:288-291`，`with_https_only` 默认 false）。允许 `https://x.com` → `http://y.com` 跨 scheme 重定向；也允许同 scheme 重定向到内网。攻击链：用户 fetch 一个公网攻击者 URL，服务端 `302 Location: http://127.0.0.1:8080/admin` → 抓到 admin 页面正文带 cookie | `builder.https_only(true)` 强制 https→https，或在每跳 `reqwest::redirect::Policy::custom` 后置回调逐 URL 校验 host。前者一行修复且与 fetch 子命令"互联网只读"定位吻合。 |
| **Minor** | 响应体无上限（OOM） | `src/fetch.rs:57` `resp.text().await` | reqwest `text()` 调用 `bytes()` 全量入内存（`response.rs:163-177`），body 限由 `Timeout(Duration::from_secs(10))` 兜底但**不受字节上限约束**。`--full`/50_000 字符截断在 `process_html` 之后——之前的 HTTP body 已读完。一个恶意 `Content-Length: 5_000_000_000` 流会被 `text()` 全收。攻击链：被诱导 fetch 一个 `Transfer-Encoding: chunked` 永不结束的页 + `Content-Encoding: identity` → 客户端读到 10s 超时为止，O(n) 解码 64-bit 累加（实测字节堆积可能数百 MB）。攻击者为本地用户→本地二进制，比 SSRF 严重度低 | 改 `.bytes_stream()` 边读边累加，超过 50MB 提前断开 + `truncated` 字段；或在 `cap_chars` 之前做字节预算上限。 |
| **Minor** | 输出截断可能换语义 | `src/postproc.rs:185-195` `cap_chars` + `src/fetch.rs:108` | `cap_chars` 按字符数截断，截断点可能落在注入字符串中间（如 `<system>` 后半段被截，指令整段仍可命中模型的截断前 token）。**这是已知 tradeoff**——`postproc::READ_BODY_MAX_CHARS=50_000` 已下推到 fetch；`meta.truncated=true` 标注已给消费方。属于契约而非漏洞 | 文档明示"截断点不保证语义边界"。无需代码修复，**已满足契约**。 |
| **Minor** | 浏览器路径本地配置注入 | `src/browser.rs:146-160` `find_browser`：`GSEARCH_CHROME` env 或 `gsearch.json "chrome"` 键指任意路径，`is_file()` 通过即被 `BrowserConfig::builder().chrome_executable()` 加载 | gsearch.json 本就是用户控制文件，与 `.bashrc`/`PATH` 注入威胁模型同质——攻击者能改 gsearch.json 也能改 PATH。本用户单用户 CLI 无多租户提升 | 不算 finding。已说明在 Contract 节。 |
| **Minor** | rustls 主版本膨胀（637 行新传递） | `Cargo.lock +638 / reqwest-0.13.4`、`hyper-rustls 0.27.10`、`rustls 0.23.45` | rustls 引入为树内 TLS 后端切换非新 HTTP 栈（Main 已批准）。**所有路径走 `rustls-platform-verifier` 默认证书校验**（无 `danger_accept_invalid_certs`，无 `.tls_built_in_root_certs(false)`，无 `tls_certs_only` 覆写）。审查 `Cargo.lock`/`client.rs:284-298` 默认 `hostname_verification=true, certs_verification=true` 已确保不绕过校验 | 不算 finding。已开 audit 显式确认证书校验链未关。 |
| **Minor** | `urlencode` 中文/特殊字符完备 | `src/search.rs:449-460` `urlencode` 全字节扫描 + `s/[^A-Za-z0-9-_.~]/<%XX>/g` | `serp_url` 与 `searxng::build_url` 共用，全 URL 字节 percent-encode。`urlencode_handles_fuzz_inputs` 测试覆盖 `& ? = 中 🦀`。**无 URL 拼接注入面**。`fetch_in_page` 用 `serde_json::to_string(url)` JS 注入，URL 任意字节安全 | 不算 finding（属正面）。 |
| **Minor** | shell/postproc 改动无新 RCE 面 | `src/shell.rs` `cmd_click` `settle_after_click` → `postproc::wait_content_stable`；`postproc.rs::read` html 截断前置 | 仅替换判稳语义/截断点位，**所有命令仍然走 `is_http_url` + `explorer.exe/open/xdg-open` 单 token 参数**（postproc.rs:51-72）。JS 注入点仅有 `shell_snap.rs` 的 `format!("querySelectorAll({sel})", serde_json::to_string(sel))` 和 `format!("els[{i}]", i)`——`i`/`sel` 都走程序内字符串、无用户控制 | 不算 finding。已确认回滚路径仍受 `is_http_url` 保护。 |

## 影响范围

**fetch 子命令 SSRF（Important×2）** 的 blast radius：

- 调用方：`coding agent`（LLM 驱动）直接执行 user/网页注入诱导的命令；用户在 shell 里被 social-engineering 诱导
- 数据外泄：本机探测（loopback 服务、路由器 admin、`localhost:11434` ollama、未打补丁的服务返 debug 栈）；云 metadata IAM 凭据（169.254.169.254）；内网 SMB/RDP/HTTP 服务指纹
- 二次利用：fetch 的输出直接进 agent 上下文（含 `content_untrusted:true` JSON 标注——但 agent prompt 注入绕过 meta 标签的能力已实证），凭据可被 agent 重组进下一轮工具调用

**本机 CVE 触发**：本审查期间**未发现** `danger_accept_invalid_certs`、`danger_accept_invalid_hostnames`、`no_proxy`滥用证书绕过——所有 TLS 路径默认拒绝无效证书（reqwest `client.rs:284-298` 默认值）。

## 架构建议（可选）

1. **fetch 默认拒绝内网**：`fn gate_private(url) -> Result<()>` 解析 host 后对 RFC1918/loopback/link-local/169.254/fe80::/::1 显式拒绝；error 提示"如需内网加 --allow-private"。
2. **`https_only(true)`**：fetch 默认 https-only；http 公网站启用 `--allow-http` opt-in。
3. **bytes stream 而非 text()**：在 fetch.rs 把 `resp.text()` 换成带字节上限的 stream，OOM/zip-bomb 同治。
4. **凭证清理**：fetch 输出的 JSON/text 不应在截断 80 字符 URL 段打印前脱敏——若未来扩展到 header 转发（当前不转发），注意 Authorization/Cookie 默认剥（reqwest 已自动剥，见 `redirect.rs::remove_sensitive_headers` 路径）。

## 验收建议（next batch）

- [ ] 加 fetch SSRF 门（`is_http_url` 同款 host 校验）
- [ ] 加 `https_only(true)` 或 redirect 前置 host 校验
- [ ] 把 `resp.text()` 换 `bytes_stream()` + 50MB 字节预算

---

审计追溯：8 commit 全部对照，已对照 reqwest 0.13.4 源码（`into_url.rs`/`redirect.rs`/`client.rs`/`response.rs`）、rustls 0.23.45 默认证书校验路径、`chromiumoxide` 0.9 + `BrowserConfig::chrome_executable` 路径。
