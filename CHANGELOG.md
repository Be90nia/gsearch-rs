# Changelog

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)；版本遵循语义化版本。

## [0.3.0] - 2026-10-10

安全审计硬化里程碑：全库三轮深度审计（安全 / 静默失败 / 测试缺口 / 内存 / 性能 / lean）后的集中修复批次。

### Security

- **修复 SSRF 门绕过 ×2**：`classify_url` 手工 host 切片不认 userinfo（`http://a.com:80@192.168.1.1/` 门判公网、实连私网）——改走 `reqwest::Url` 与连接层同一解析器；`is_private_ip` 补 IPv4-mapped IPv6（`[::ffff:192.168.x.x]`）与 CGNAT（100.64.0.0/10）判定
- **结果集导航门**：`search --browse N` 与 shell `click N` 的目标 URL 来自搜索结果 = 不可信输入，读前过 scheme 白名单 + 私网门；`search` 新增 `--allow-private`（语义与 fetch/browse 对齐）
- **shell 会话接门**：shell `browse` 过完整门（scheme + 私网），`login`/`dl` 过 scheme 白名单——此前 shell 会话完全绕过顶层门
- `meta.proxy` 输出信封凭据脱敏（`scheme://user:***@host`）
- `fetch_in_page` 页内 fetch base64 通道 Rust 侧长度复核（此前仅 JS 侧）
- `verify` curl 参数加 `--` 分隔与 `--globoff`（封死 URL 以 `-` 开头的注入面）
- `dl` 文件名命中 Windows 保留设备名（CON/PRN/AUX/NUL/COM1-9/LPT1-9）降级 `download.bin`
- `resolve_host` 支持 IPv6 字面量重定向（公网不再误拒，私网仍拒）

### Fixed

- **shell `read` 吞 CDP -32000 为空正文**（顶层路径已修、shell 路径漏网的 j44 同款事故模式）——改走 `content_retry` + 空正文 stderr `[hint]`
- **CDP evaluate 链无超时上界**：`content_retry`/`eval_string_retry`/`page_snapshot`/`fetch_in_page` 四处统一 30s 超时——恶意页同步死循环 JS 楔死 renderer 不再永久挂起 CLI
- **`search --browse N` / `browse` 对 head/nav 超重页面（>50KB）正文全空**：截断吃光正文时自动从源 HTML 尾部窗口重提取（内存有界）
- `swap_to_headed/headless` 失败路径旧 handler 未 abort 导致会话状态悬挂；shell REPL 异常早退跳过 `graceful_close` 残留 chrome.exe 持 profile 锁
- 重定向每跳 SSRF 门在 `fetch`/`dl`/`update` 间 Client 复用后语义不变（每 URL 独立过门）

### Performance

- `fetch`（含 batch）/`dl`/`update` 单命令单 `reqwest::Client`（此前每 URL×每 attempt 重建，丢同 host 连接复用）
- `search --read N` 免二次全页 DOM 序列化（captcha 检查正文带回复用）
- profile fork 跳过 Cache/Code Cache/GPUCache/Service Worker 等目录（fork 关键路径 I/O 降 1-2 个数量级）
- collapse 链三连拷与 `--json-keys` 路径深拷贝收敛（输出逐字节不变）

### Removed

- 死代码清理（-32 行）：`find_chrome`、未消费的 `SearchResult` re-export、恒为 0 的 `anchor_pad_lines` 旋钮、`browser_alive`/`swap_to_headed` 重复定义与透传层

### 文档

- README 增补：结果集导航门、`meta.proxy` 脱敏语义
- 测试补强：SSRF 门 userinfo/伪装用例、fork 跳过、CDP 超时注入、保留名降级、CGNAT/fe80 边界、尾部窗口补救、SearXNG collect 归因与回退链决策矩阵等定向测试
