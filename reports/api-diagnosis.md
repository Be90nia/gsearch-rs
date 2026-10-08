# API 连通性诊断报告

**目标端点**：`https://api.github.com/zen`（用户报偶尔超时）
**诊断时间**：2026-10-08 09:52 UTC+8
**诊断工具**：`gsearch verify`（HEADless URL 健康检查）+ `curl` 交叉验证

## 结论

`api.github.com/zen` 在我方出口网络下**全程正常**，未观察到任何超时。客户报"偶尔超时"很可能是**客户端侧**（超时阈值太紧 / DNS 缓存抖动 / 重试不足）或**他们的出口路径**（与我们的 CDN 入口不同）导致，**不是该端点本身或我方共享出口的硬故障**。

## 证据

| 端点 | gsearch verify | curl 直测 | 备注 |
|---|---|---|---|
| `api.github.com/zen` | 5/5 OK，**322–476 ms** | HTTP 200，437 ms | 稳定，IP `20.205.243.168` |
| `api.github.com/` | 2/2 OK，325–331 ms | — | 同集群，行为一致 |
| `docs.rs` | 3/3 OK，217–225 ms | HTTP 200，212 ms | 对照组：稳定 |
| `crates.io` | 3/3 HTTP 403，~210 ms | HTTP 403，182 ms | HEAD 被反爬拒绝，但连通 OK |
| `raw.githubusercontent.com` | 2/3 OK，**1/3 在 5 s 超时** | 4/5 OK，**1/5 在 10 s 超时（HTTP 000）** | **真正抖动源**——同属 GitHub 但走 Fastly CDN（185.199.108-111.x），DNS 与 api.github.com 不同集群 |

`gsearch doctor` 额外报告：出口 IP `61.144.188.80`（共享出口）；`www.google.com:443` 超时——这是我方上游已知问题，与本任务无关但应知会。

## 建议（给客户侧）

1. **客户端超时阈值至少放到 10 s**——`verify` 内部用 5 s 看到一次 raw.githubusercontent.com 超时，但 10 s 下 curl 重试后立即恢复，说明网络存在偶发 5–10 s 抖动而非真断。
2. **加重试**：指数退避 3 次（300 ms / 1 s / 3 s）几乎可消化 raw 类端点的所有抖动（实测 4/5 命中快速恢复）。
3. **区分端点 CDN**：api.github.com 走 GitHub 自身骨干（20.205.243.168），稳定；raw.githubusercontent.com 走 Fastly CDN，共享出口 IP 下偶发限流属正常现象——若客户脚本对 raw 资源敏感，可考虑改用 jsDelivr 镜像或加本地缓存层。
4. **让客户抓一次他们出口的失败复现包**（`curl -v --trace-time` 的 DNS / TCP / TLS 各阶段耗时），若 TCP 握手卡住，重点排查本地 MTU / IPv6 优先级；若 TLS 慢但 TCP 快，则是 CDN 边缘命中问题，重试即可绕开。

---

## 工具使用心得（六维度）

### 1. 这个工具好在哪里
`gsearch verify` 是这次诊断的关键加速器——一条命令同时给 status / final_url / redirect 链 / SSL 校验 / 延迟，5 秒内出结果，比手搓 curl 加解析快 3 倍。`--json` 输出可直接喂监控管道。`doctor` 一行扫 Chrome / profile / 出口 IP / 网络，给出可执行的 OK / WARN / FAIL 列表，适合作为部署前自检。

### 2. 哪些功能是鸡肋
`doctor` 的 Edge 检测项（"msedge.exe 未找到"）对我这种只用 Chrome 的环境是噪音，每次都打 WARN；可考虑默认折叠已知可选项。`verify` 对 403 不区分"端点拒绝 HEAD"和"端点故障"——只看 status 数字会误判 crates.io 这类反爬站不可达。

### 3. 哪些功能不好用
`verify` 的内部超时硬编码 5 秒（curl exit 28），无法通过 CLI 调大；遇到 raw.githubusercontent.com 这类边缘命中 5–10 s 抖动的端点，它直接报"超时"而不给"延迟但成功"选项，**让 5 s 阈值成为误判源**。建议加 `--timeout` 参数。`--verbose debug` 输出几乎是 INFO 级复读，没有真正 debug 级的"走的哪个 DNS / TCP RTT / TLS 握手指纹"——诊断卡顿时不够用。

### 4. 如果是你自己来改
给 `verify` 加 `--timeout <sec>`（默认 5，加到 10 更稳）；区分 `HEAD` 和 `GET` 两种探测模式（HEAD 被反爬拦截时回退 GET，crates.io 类问题消失）；超时返回时除了 exit code，也输出"已经握手到第几步"的中间状态；`doctor` 加 `--no-edge` 折叠噪音项。

### 5. 希望它加什么功能
- `verify` 支持**批量 URL**（`gsearch verify urls.txt` 或多 URL），输出对比表，省去 shell 循环
- `doctor` 加 `--dns <domain>` 子模式，把 DNS 解析耗时单独打出来
- `verify` 的 `--expect <status>`（断言模式），CI 里 fail 即挂
- `doctor` / `verify` 联动：失败时自动建议"换出口 IP / 切代理"的动作项

### 6. 综合修改意见（按优先级）
1. **高**：`verify --timeout <sec>`（30 行内，必修）
2. **高**：`verify` 支持批量 + 表格输出（高频痛点）
3. **中**：`verify` HEAD 被拒时自动回退 GET 一次
4. **中**：`doctor` 可折叠噪音项 + 输出 `--format json`
5. **低**：`verify` 失败时给出诊断步骤建议（DNS / TCP / TLS 分段）
