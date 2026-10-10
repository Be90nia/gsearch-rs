# gsearch-rs 第二轮安全专项审计（2026-10-10）

VERDICT: FAIL

范围：`src/` 全部 21 个文件中与网络边界 / 子进程 / 文件写 / 输出契约 / 凭据 / profile fork 相关的面。只读审计（未构建、未改码）。
检索方式说明：serena-cli rust 可用（overview/symbol 定位），但审计证据需要精确 file:line + 上下文，故主体降级为 `read` 定向区间 + `grep` 定位；已知项（fdacc5a sanitize_filename、每跳重定向门、searxng no_proxy、I5 32MB JS 阈值、b64378f open 修复）只验不重报。

## 结论

第一轮 CONDITIONAL 的 fetch SSRF 缺口已被补上（`gate_check` + `Policy::custom` 每跳门，方向正确、fail-closed 意识好）。但本轮发现**门的判定函数本身可被绕过**：`classify_url` 手工切片提取 host 与 WHATWG URL 解析存在分歧（userinfo 冒充），且 `is_private_ip` 漏判 IPv4-mapped IPv6——两条独立的 SSRF 绕过路径，均可用一条 URL 直接触发。另发现 shell 会话子命令完全没挂门（file:// 内容可直接进 agent 上下文）。依赖均新，无硬编码凭据。

---

## 漏洞发现（按威胁等级）

| # | 级别 | 类型 | 位置 | 可利用性 |
|---|------|------|------|----------|
| H1 | HIGH | SSRF 门绕过（URL 解析分歧） | fetch.rs:160-176 | 远程·直接 |
| H2 | HIGH | SSRF 门绕过（IPv4-mapped IPv6） | fetch.rs:126-138 | 远程·直接 |
| H3 | HIGH | 门缺失（shell 子命令） | shell.rs:435-448,556 | 本地·agent 会话 |
| M1 | MEDIUM | SSRF TOCTOU（DNS rebinding） | fetch.rs:108-114,216-240 | 远程·需攻击者 DNS |
| M2 | MEDIUM | 凭据泄漏进输出契约 | types.rs:36-38, main.rs:706 | 本地/日志层 |
| M3 | MEDIUM | 浏览器路径私网可达（初始门外） | postproc.rs:414+, general.rs:361-364 | 远程·需 agent 动作 |
| M4 | MEDIUM | 内存 DoS（无 Rust 侧长度复核） | postproc.rs:735-748 | 远程·需 agent dl |
| L1 | LOW | Windows 保留设备名绕过 sanitize | util.rs:36-67 | 远程触发·Win |
| L2 | LOW | 私网漏判 100.64.0.0/10 | fetch.rs:126-138 | 远程·特定网络 |
| L3 | LOW | 重定向 IPv6 字面量误拒（fail-closed） | fetch.rs:220-224 | 可用性 |
| L4 | LOW | curl argv 注入面/URL globbing | verify.rs:326-353 | 低影响 |
| L5 | LOW | IPv6 link-local 段漏判 | fetch.rs:141-145 | 边缘 |

### H1 — `classify_url` 手工 host 提取与 WHATWG 分歧（userinfo 冒充）

**位置**：`src/fetch.rs:160-176`（`classify_url`：`after_scheme.find(['/', '?', '#', ':'])` 截取 host）。同源调用面：`general.rs:75-81`（browse 门）、`general.rs:488`（dl_direct 门）、`fetch.rs:323`（fetch 门）。

**攻击场景**：URL `http://a.com:80@192.168.89.1/`
1. `classify_url` 在第一个 `':'` 处截断 → host=`a.com` → 解析为公网 IP → 门放行；
2. Chrome / reqwest / url crate 按 WHATWG 解析：`a.com:80` 是 userinfo，**真实 host = 192.168.89.1**；
3. `gsearch browse` 直接把内网（或 link-local / metadata）页面渲染后全文打进 agent stdout —— 内网内容外泄进模型上下文。fetch 侧 https 变体（`https://a.com:443@169.254.169.254/…`）绕过门直连目标；`general::cmd_dl` 的 `dl_direct`（general.rs:488 门 + 513 GET）会被 reqwest 拉取**内网文件落盘**，`gate_check(url,false)` 完全失效。

**最小修复**：门改用树内已有 `url::Url` 解析（url 2.5.8 已在依赖树，零新增依赖）取 `host_str()`；一行版补丁为在切片前剥 userinfo：`let after = after_scheme.rsplit_once('@').map(|(_, h)| h).unwrap_or(after_scheme);`（WHATWG 语义 = 最后一个 `@` 之后才是 host）。单测补 `http://a.com:80@127.0.0.1/` 拒绝用例。

### H2 — `is_private_ip` 漏判 IPv4-mapped IPv6

**位置**：`src/fetch.rs:126-138`。V6 arm 只查 unspecified/loopback/ULA/`segments()[0]==0xfe80`/multicast；`::ffff:a.b.c.d` 首段为 0，全部漏过。**129-130 行注释声称「含 IPv4-mapped 形态」与实现不符**——文档性防线不存在。

**攻击场景**：`gsearch browse "http://[::ffff:192.168.89.1]:8888/"` → Rust `IpAddr::parse` 得 V6 → 判公网 → 放行；Chrome 双栈把 `::ffff:` 连接到真实 IPv4 → 内网 http 页面正文进 agent 上下文（browse 无 scheme 限制，http 可用）。fetch 侧 `https://[::ffff:10.0.0.5]/` 打内网 TLS 服务。

**最小修复**：V6 arm 首行加 `if let Some(v4) = v6.to_ipv4_mapped() { return is_private_ip(IpAddr::V4(v4)); }`；fetch.rs:1844 放行测试组补 mapped 用例。可选顺带：`64:ff9b::/96`（NAT64）同法映射判定。

### H3 — shell 会话子命令零门：`browse file://` 本地文件外泄进 agent 上下文

**位置**：`src/shell.rs:435-448`（`cmd_browse` 裸 `goto`）、`shell.rs:556-560`（`goto` = 裸 `page.goto`，无 scheme/私网检查）、`shell.rs:385-408`（`cmd_dl` 直下任意 URL）、`cmd_login` 同裸。

**攻击场景**：agent 在 shell 会话中执行 `browse file:///C:/Users/x/.ssh/id_rsa`（URL 来自 LLM 上下文/搜索结果/页面链接），Chrome 渲染本地文件后 `read --full` 把内容打进 stdout —— 本地敏感文件外泄进模型上下文。顶层 `browse` 有完整 `ensure_browsable_url`（scheme 白名单 + 私网门，general.rs:75-81，测试锁 general.rs:710-714），shell 版**一概没有**；`javascript:`/`data:` 亦不被拦（Page.navigate 对 javascript: 一般拒绝，file: 则完全可达）。

**最小修复**：shell `cmd_browse`/`cmd_login`/`cmd_dl` 入口先过 `general::browsable_scheme_ok` + `fetch::classify_url` 私网门（与顶层对齐；`Page.navigate` 不走的路也先在 CLI 层拒掉），复用顶层同款测试。

### M1 — SSRF 门 TOCTOU（DNS rebinding）

**位置**：`src/fetch.rs:108-114`（`resolve_host` 用 `ToSocketAddrs` 独立解析）vs reqwest 连接期自有解析；重定向每跳 policy（fetch.rs:220-224）同样解析两次。

**攻击场景**：攻击者权威 DNS 对自己的域名 TTL≈0，门检查时返回公网 IP、连接时返回 192.168.x.x → 门放行、连接打内网。前提是攻击者控制 DNS（钓鱼域名/结果页指向的短 TTL 域名），与「URL 来自不可信内容」威胁模型一致。

**最小修复**：`build_client` 增加钉扎参数，`ClientBuilder::resolve(host, addr)` 把门解析结果钉死给首跳；重定向每跳受 reqwest API 限制无法全钉，文档声明残余。多 A 记录只取首解析（`addrs.next()`）的窗口一并收敛。

### M2 — `meta.proxy` 原样回显带凭据代理 URL

**位置**：`src/types.rs:36-38`（`MetaOutput.proxy` 无脱敏序列化）、`main.rs:706`（search 信封）、`general.rs:182`（browse 信封）。对照 `browser.rs:562-573` 的 `redact_proxy` 只用于 tracing（606/670）——bfa3efe 的脱敏没有覆盖输出契约。

**攻击场景**：`GSEARCH_PROXY=http://user:pass@corp:8080 gsearch search q --json` → stdout JSON `meta.proxy` 含明文凭据 → agent 转写、CI 日志、会话记录扩散。

**最小修复**：信封构造处套 `redact_proxy`（提到 `util.rs` pub(crate)），与 log 脱敏同源。

### M3 — 浏览器路径在初始门之外私网可达

**位置**：`postproc.rs:414+`（`--read N` 直接 goto 结果 URL，无任何门）、`general.rs:361-364`（`cmd_dl` 仅 scheme 门——**内网下载放行是 general.rs:74 文档化决策，不重复报**）。本条报两点：(a) 结果集驱动（SearXNG/Google 结果里的 SEO 恶意页）的 read/dl 无门；(b) 所有浏览器路径的门只管**初始 URL**——页内 meta-refresh/JS 可把 Chrome 导航到任意内网地址（CDP 无拦截）。

**攻击场景**：恶意页进结果集 → `gsearch search x --read 1` → 页面 JS `location='http://192.168.89.249:8888/'` → 内网响应被当正文抽进 agent 上下文；页面 JS 亦可对内网端点发跨站请求（浏览器行为，非 CLI 网络栈）。

**最小修复**：`postproc::read`/`goto_page` 前对结果 URL 套 `ensure_browsable_url`（与 cmd_browse 对齐）；页内导航残余需 CDP Network 拦截或在威胁模型文档显式声明接受。

### M4 — `fetch_in_page` base64 返回无 Rust 侧长度复核

**位置**：`src/postproc.rs:735-748`（`evaluate → into_value::<String> → b64_decode`）+ `util.rs:75-79`（`Vec::with_capacity(s.len()*3/4)` 无上限）。postproc.rs:750-751 的 32MB 阈值在 **JS 侧**——同上下文页面 JS 可先 patch `btoa`/返回值，让阈值形同虚设。

**攻击场景**：agent 对恶意页执行 `dl` → 页内 JS 让 evaluate 返回 1GB+ 字符串 → CDP 物化 + Rust 预分配 0.75× → 本进程 OOM（agent 宿主连带受损）。

**最小修复**：`b64_decode` 前加一行 `if b64.len() > FETCH_IN_PAGE_MAX_BYTES / 3 * 4 + 4 { bail!(…) }`。

### L1 — `sanitize_filename` 未拦 Windows 保留设备名

**位置**：`src/util.rs:36-67`（只滤字符，不滤 `CON/NUL/PRN/AUX/COM1-9/LPT1-9` stem）。`browser.rs:457-460` 已有 `is_windows_reserved` 未复用。

**攻击场景**：`gsearch dl "https://x.com/NUL" -o D:\dl`（Win）→ `fs::write("D:\dl\NUL")` 写进 NUL 设备 → **返回成功、零文件**，agent 误以为已保存（静默数据丢失）；COM1 形态可能挂在串口驱动上。

**最小修复**：sanitize 尾部对 stem（`split('.')` 首段大写）跑 `is_windows_reserved` → 命中落 `download.bin`。

### L2 — 私网漏判 100.64.0.0/10

**位置**：`fetch.rs:126-138` v4 arm。CGNAT/Tailscale（100.x）网段不在 `is_private()` 内 → `gsearch fetch http://100.64.x.x/`（配 --allow-private 才该可达的段）按公网放行（http 仍被 fetch 拒，但 browse/https 可达）。Tailscale 组网在目标用户群（homelab）常见。

**最小修复**：手写 `octets[0]==100 && (64..=127).contains(&octets[1])` 判定并入 v4 arm。

### L3 — 重定向 policy 对 IPv6 字面量恒判私网（fail-closed 误拒）

**位置**：`fetch.rs:220-224`：`u.host_str()` 返回带括号 `"[::1]"` 形态 → `resolve_host` 解析失败 → `unwrap_or(true)` → 判私网拒；而初始 URL 走 `classify_url`（161-170 手工剥括号）可正常放行。安全方向安全，但重定向到公网 IPv6 字面量被系统性误拒。采用 H1 的 `Url::parse` 方案后自然消失（`host_str()` 语义统一）。

### L4 — verify.rs curl 无 `--` 分隔、无 `--globoff`

**位置**：`verify.rs:326-353`（URL 作为最后 argv 原样拼接）。来自不可信内容的 URL 以 `-` 开头可被 curl 当选项（注入面有限：`-K/--config` 需本地文件配合）；URL 含 `[a-z]`/`{a,b}` 触发 curl globbing 改变请求语义。**最小修复**：push URL 前加 `"--"`，并加 `"--globoff"`。

### L5 — IPv6 link-local 只匹配 `fe80::/64` 精确段

**位置**：`fetch.rs:141-145`（`segments()[0] == 0xfe80` 漏 fe81-febf）。**最小修复**：`(seg0 & 0xffc0) == 0xfe80` 或 `Ipv6Addr::is_unicast_link_local()`（Rust 1.83+ stable）。

---

## 验证为 PASS 的面（无发现）

- **硬编码凭据**：`src/` 全量模式扫描（api_key/secret/password/token/私钥 PEM）零命中。
- **依赖新鲜度**：reqwest 0.13.5、url 2.5.8、rustls 0.23.45、chromiumoxide 0.9.1、serde_json 1.0.151（Cargo.lock 实测）。未跑 cargo-audit（需联网 advisory DB，遵守只读纪律）。
- **postproc::open**（postproc.rs:48-73）：`is_http_url` 锚定 http(s)、explorer.exe/open/xdg-open 单 argv、不经 cmd —— b64378f 修复完好，cmd 元字符注入不可达。
- **duckduckgo.rs**（24-60）：URL 为常量端点；query 经 `urlencode`（search.rs:688，fuzz 单测锁）进 `--data-raw` argv；无 shell 解释层。
- **PowerShell 锁诊断**（browser.rs:797-806）：profile 路径单引号双写转义正确，PS 单引号字面量不插值，无注入路径。
- **searxng.rs**：SHARED_CLIENT `no_proxy()`（19-28）+ q urlencode（57-63）+ recency 静态枚举。
- **update.rs**：无自替换（纯版本比对），常量 API URL + 门客户端（40）。
- **browsable_scheme_ok**（general.rs:56-72）：file:/javascript:/data:/blob:/vbscript:/view-source: 显式阻断，裸 host 补 https —— 顶层入口契约完好。
- **has_parent_traversal**（general.rs:86-89）：相对 `-o` 禁 `..` 穿越。
- **content_untrusted**：fetch JSON 恒在（fetch.rs:730）、read JSON 恒在（postproc.rs:357）、browse 同（general.rs:120）。
- **profile fork**：锁文件跳过（359-363）、符号链接逐项跳过（384-388）、fork 失败回落不阻断；fork 名 16-bit 随机可预测但处于 HOME ACL 保护下，无实际越权路径。`fs::copy` 保留原 cookie 权限位（Unix 0600 随拷）。

## 影响范围

- H1/H2 均落在 `classify_url`/`is_private_ip` 两个共享判定函数 → **一次修复全入口受益**：fetch（fetch.rs:323）、browse（general.rs:77）、dl_direct（general.rs:488）、shell（修复 H3 后）。重定向 policy（fetch.rs:216-240）基于 url crate 解析，不受 H1 手工切片影响，但受 H2（`resolve_host → is_private_ip`）影响。
- M3 修复点在 postproc::read 前置，不触碰已验证的 open/dl_direct 路径。

## 架构建议

1. **单一 URL 解析基准**：把「URL → host/私网分类」收敛为唯一基于 `url::Url` 的函数，门、重定向 policy、shell 门共用——手工字符串切片是 H1/L3 两类分歧的根因。
2. **门分层显式化**：初始 URL 门（现有，cheap）与「加载后页内导航」（需 CDP 拦截）是两条独立防线；在 README/威胁模型文档里写明接受边界，避免"有门=全程有门"的错觉。
3. **输出契约**：文本模式（shell `read --full`、顶层文本输出）可考虑在标题行附 `content_untrusted` 标注；JSON 路径已全覆盖。
