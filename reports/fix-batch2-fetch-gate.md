# Fix Batch 2 — fetch.rs 安全门绕过 + Client 复用（qmg / uvi）

执行者：FixBatchB（所有权：fetch.rs / general.rs / update.rs）

## qmg（安全 P1）：SSRF 门 host 提取 userinfo 绕过 + IPv4-mapped V6 伪装

### 改动摘要
- **qmg-a** `src/fetch.rs:163-175` classify_url：手工切片（`find("://")` + `/?#:` 截断 + `]` 查找）整体替换为 `reqwest::Url::parse`（reqwest 对 url crate 的再导出，与连接层同一 parser，**零 Cargo.toml 改动**）。host 从 `parsed.host_str()` 取，IPv6 字面量成对剥方括号后走既有 `resolve_host`（字面量直读 / 域名 DNS 不变）。reqwest 实连 host 与门判 host 由同一 parser 保证零分歧；userinfo（`http://a.com:80@192.168.1.1/`）、反斜杠归一化（url crate 对 special scheme 按 `/` 处理）整类消灭。返回签名 `(String, IpAddr, bool)` 不变，`gate_check`/general.rs `ensure_browsable_url` 调用方零改动。
- **qmg-b** `src/fetch.rs:132-136` is_private_ip：V6 分支 `to_ipv4_mapped()` 先转 V4 递归判定（`::ffff:192.168.x.x` 类伪装不得绕门；mapped 公网照常放行）。doc 注释同步。
- **qmg-c** 新增 2 测试（`src/fetch.rs:1858-1889`）：
  - `ssrf_gate_rejects_userinfo_obfuscation`：5 用例（a.com:80@192.168.1.1 / evil.example@10.0.0.5 / user:pass@169.254.169.254 / x@[::1] 带 userinfo / `192.168.1.1\@public.example` 反斜杠归一）全断言错误含「拒绝」
  - `ssrf_gate_rejects_ipv4_mapped_v6`：4 URL 用例（mapped 192.168/10/169.254 拒、mapped 8.8.8.8 放行）+ 2 直判断言（`::ffff:172.16.0.1` 私、`::ffff:1.1.1.1` 公）
- **resolve_host 未动**：build_client 重定向 Policy 与 general.rs `is_gate_dns_error`（匹配 "host 解析失败/为空"）的错误文案契约保留。

### 验证证据
```
cargo test --bin gsearch fetch::tests::
test fetch::tests::ssrf_gate_rejects_ipv4_mapped_v6 ... ok      ← 新增
test fetch::tests::ssrf_gate_rejects_userinfo_obfuscation ... ok ← 新增
test fetch::tests::ssrf_gate_rejects_private_addresses ... ok   ← 既有全段表
test fetch::tests::ssrf_gate_allow_private_bypasses ... ok      ← 既有
test fetch::tests::is_private_ip_cases ... ok                   ← 既有
test result: ok. 67 passed; 0 failed; 0 ignored (74 filtered out)
```

## uvi（perf P1）：Client 每 URL×attempt 重建 → 单命令单 Client

### 改动摘要
- `src/fetch.rs:243-250` 新增 `command_client(opts)`：allow 判定 + timeout clamp（FETCH_TIMEOUT_MAX_SECS，原 fetch_one 内的 clamp 收敛至此唯一处）+ build_client，单命令构建一次。
- `src/fetch.rs:590` cmd_fetch 入口唯一构建点；`&Client` 下传 `fetch_one(url, opts, &client)`（:592 单条）与 `cmd_fetch_batch(urls, opts, &client)`（:614）→ batch 闭包内全部 URL 共用（:660）。
- `src/fetch.rs:365-370` fetch_one_attempt 签名 `(url, opts, client)`：删 build_client 行与 timeout_secs/allow 死参（allow 仅门判定消费，留在 fetch_one）；重试 loop 不再逐次重建。
- `src/general.rs:489` dl_direct：:493/:502 两处互斥分支的 build_client 收敛到函数顶一处，DNS 复核与主路径共用。
- `src/update.rs:40`：**核查即合规**——cmd_update 本就单命令单构建（复用 build_client），无 diff。
- SSRF 重定向门是 builder 级 `Policy::custom`，复用同一 client 不受影响；stdout 输出契约逐字节未动（仅 client 构建点移动）。

### 验证证据（单命令单 Client 构建点）
```
src/fetch.rs:590   let client = command_client(opts)?;            ← cmd_fetch 唯一构建点（单条+batch 共用）
src/fetch.rs:249   build_client(opts.proxy.as_deref(), ...)       ← command_client 内部，全仓唯一 opts 侧调用
src/general.rs:489 let client = crate::fetch::build_client(...)   ← dl_direct 唯一构建点
src/update.rs:40   let client = crate::fetch::build_client(...)   ← cmd_update 唯一构建点（原状合规）
cargo check → Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.39s（零 error 零 warning）
```

## 纪律遵守
- 只跑 cargo check + `cargo test --bin gsearch fetch::tests::`（fetch 模块定向）；未跑全量 build/test/clippy；未 commit。
- 未触碰 browser.rs / postproc.rs / shell.rs / search.rs；classify_url/gate_check/build_client 签名不变。
- stdout 契约不变；classify_url 畸形 URL 的错误文案变化（"URL 无 scheme/IPv6 host 未闭合" → "URL 解析失败: {url}"）仅影响失败路径 stderr/batch 错误 message，成功输出零变化。

## 残余风险
- clippy 归 PM 合并态统一跑（纪律禁全量）；新代码可能触发 lint 的面：`strip_prefix/strip_suffix` 链、V6 递归 early-return——模式常规，风险低。
- gate_check 拒绝文案中 host 串来自 url crate 归一化（域名转小写），含大写域名的私网拒绝 message 与旧文案有大小写差异（ip/「拒绝」语义不变，非 stdout 成功契约）。
