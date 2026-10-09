# T5b 回执：DDG html 第二源插层（af9）+ similar 子命令（e7c）

**VERDICT: 部分达成——af9 代码/插层/契约全落地且全绿；e7c 全落地全绿；验收 b 的 `provider=duckduckgo` 真实命中被外部 blocker 挡住（reqwest TLS/头指纹被 DDG anomaly 风控识别），插层与失败语义已按真实链路验证。**

## 交付物

| 项 | 内容 |
|---|---|
| `src/duckduckgo.rs`（新） | DDG html 第二源：`POST https://html.duckduckgo.com/html/` 表单（q [+ df]），第一页免 vqd 不翻页；`div.result` 容器 + walk 兜底双解析路径；uddg 跳转链解码（手写 percent_decode，零新依赖）；广告链（/y.js）过滤；anomaly 风控页显式报错（防静默算零结果） |
| `src/search.rs` | `try_searxng` Err 分支插层 SearXNG → **DDG** → Google（DDG 在 Google 预检/熔断之前；stderr 一行接管/失败提示，与现有降级诊断同风格）；`similar()` 内核 + split_site_keys/host_of/tokenize/title_overlap |
| `src/types.rs` | `SimilarHit { #[serde(flatten)] hit, similarity }` |
| `src/main.rs` | `Similar { url, --limit 3, --human, --json 占位 }` 子命令 + `cmd_similar`（信封 to_value + 顶层 `similar_of`/`note`，0mf 先例手法）+ clap 解析测试。**未触碰 update/browse/dl/doctor/browser.rs** |
| `Cargo.toml` | reqwest features + `"native-tls"`（仅 DDG client `use_native_tls()`+`http1_only()` 用；指纹对抗尝试，见 blocker） |
| `README.md` | search/provider 节：三级回退链、DDG df、score 键缺席来源、similar 子节、GSEARCH_SEARXNG_URL、Companion tools 过时句更新 |

## 验收

### code-level ✅
- `cargo check`：0 error
- 过滤测试全绿（契约内范围）：search:: 11、duckduckgo 6、searxng 12、parse 4、types 9、bin similar 1 —— **43 passed / 0 failed**
- batch 只走 SearXNG 未动（batch_one 不经 DDG）✓

### e2e（真实跑）
- **a. 活端口 → provider=searxng ✅**
  `GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 gsearch search "rust async" --limit 3 --no-humanize`
  → `{"meta":{...,"elapsed_ms":2855,"provider":"searxng"},"run":{"status":"ok"},"results":[...score:7.5...]}` exit 0
  复跑（最终二进制）与 `--recency week` 变体（provider=searxng, recency:"week"）均过——回退链顶部未动
- **b. 死端口 → DDG 接管 ⚠️ 插层链路验证 ✅ / provider=duckduckgo 命中 ❌（外部 blocker）**
  `GSEARCH_SEARXNG_URL=http://127.0.0.1:1 gsearch search "rust async" --limit 3 --no-humanize`
  → stderr：`DDG html 直连失败（DDG 风控 challenge 页（anomaly），出口 IP/TLS 指纹被识别），继续 Google 回退链` → `SearXNG 零结果已熔断...` → exit 2（本机 Google:443 同刻不通，熔断正确）
  **诊断链（本机出口对 google/DDG 全断，passwall 透明代理失效场景）**：
  1. 路由器侧（192.168.89.249）curl POST 同形态 → **HTTP 200 / 29KB / 10× result__a**——请求形态、UA、解析器选择器与真实 DDG 页逐项吻合（真实样本已核）
  2. paramiko 隧道（路由器 xray socks/http 127.0.0.1:1070 → 本机 11070）后：curl 带 gsearch UA → **200 28.9KB**（无 UA 202——UA 敏感，已内置）
  3. 同隧道同刻同 UA：curl(schannel) 200 / .NET HttpClient 200 / **reqwest rustls 202 / reqwest native-tls+http1_only 202**
  → 结论：**reqwest 的 TLS hello/头指纹被 DDG anomaly 风控识别**，与 IP/UA/协议版本无关。产品级出路 = rquest（浏览器指纹模拟）或指纹不敏感出口，超本任务边界
- **c. similar ✅**
  `gsearch similar "https://docs.rs/serde" --limit 3`
  → `{"meta":{...,"query":"serde","provider":"searxng"},"note":"启发式派生查询...非 exa 神经 findSimilar","results":[{"title":"serde - Rust - Docs.rs","similarity":"title=serde; site=docs.rs","score":0.5,"domain_class":"docs",...},...]}` exit 0
  出口恢复后复验 b：`export GSEARCH_SEARXNG_URL=http://127.0.0.1:1 && gsearch search "rust async" --limit 3 --no-humanize` 期望 `provider":"duckduckgo"`

## 设计决策（派单授权自定项）

1. **DDG 不进 NotConfigured 路径**：任务明文限定"SearXNG 零结果熔断/连接失败时先试 DDG"，未配 SearXNG 保持直落 Google（老行为零变）
2. **--recency：DDG 支持**（`df=d/w/m/y`，复用 `qdr_letter()` 同字母）——回执说明：已支持，请求形态经路由器旁证，端到端命中因 blocker 未验
3. **similar 用子命令**（非 --similar flag）：独立语义/独立输出结构（similarity 字段），flag 挂 search 会污染 SearchArgs 与输出契约
4. **similar 查询构造**：单查询 `<keywords>`（title 词），同域作为**重排 bonus + similarity 标注**而非 site: 过滤——贴 exa findSimilar 跨站语义；纯域名退化 `site:<host>`；`similarity` 标注启发来源（`title=serde; site=docs.rs` / `none（仅派生查询命中...）`）
5. **202 anomaly 显式报错**：原实现 202 静默归零结果会误导 agent（知乎限流页同款教训），已改 bail

## Side-effects（三态）

- **明确无**：batch 路径（SearXNG-only 不变）、searxng.rs（零改动）、fetch/postproc/browse/dl/doctor/update/browser.rs（未触碰）、SearXNG 活端口时行为逐字节不变（a/b 双验收回归）
- **已确认有**：① SearXNG 失败路径新增 1 行 stderr（DDG 接管/失败提示）+ DDG 尝试耗时（≤5s 超时封顶，死端口场景 e2e 总耗时 6.1s vs 原 1.5s 预检+熔断）② Cargo.lock 新增 native-tls→schannel 树（Windows 无系统依赖）③ `run.status` 熔断判定在 DDG 之后（DDG 命中时不再熔断——语义增强）④ main.rs/README.md 为与 T2b 共享文件（均为追加式，分区无重叠：我只写 search/Similar 区，已核对其 update 区改动共存编译过）
- **未知**：reqwest 指纹在非共享代理出口（家宽直出/海外 VPS）下是否仍被 DDG 拦——需出口恢复后复验 b

## 踩坑（通用）

- reqwest 0.13 `default-features=false` 下 `.form()` 不可用（serde_urlencoded 不在树）→ 手写 urlencoded body（`search::urlencode` 复用）
- reqwest 0.13 `default-tls` feature 实际映射 rustls；真 schannel 需 `native-tls` + `use_native_tls()`
- Windows 排除端口段导致 bind WinError 10013（1070）→ 换端口；bash 工具的 service/`set`/`$` 语义在本机不可靠

工具：serena-cli 0 次（owned 文件整读改写 + 并行 T2b 同库实时编辑，骨架 read+git diff 更适配）；降级原因：符号级检索需求少且需整文件语义。
