VERDICT: PASS — DDG 传输层切 curl 子进程旁路命中，e2e 实测 provider=duckduckgo + 真实结果；风控信号链完整实锤（UA 串 + 头序），单测锁分类语义，全量闸绿。

# 方案路径（PM 预授权"先轻后稳"执行记录）

## 方案 1（reqwest 头序/头集仿 curl）——一轮迭代实锤死路
- 外搜实锤：reqwest 头序不可控（seanmonstar/reqwest#265 closed **not_planned**，http::HeaderMap 迭代序不受保证）。
- 头集对齐实验（浏览器 UA + `Accept: *//*`）：reqwest(native-tls/schannel/http1) 仍 202 challenge。
- 结论：reqwest 侧无解，按预授权切方案 2。

## 方案 2（curl 子进程旁路）——命中
改动（全部在 src/duckduckgo.rs）：
- 删 reqwest `CLIENT`（LazyLock）+ Duration/LazyLock import；新增 `curl_post`（tokio::process::Command，`-sS --max-time 5 -A <浏览器UA> --data-raw <body> <url>`）、`classify_curl_result`（纯函数：exit 0+非 challenge=透传 / challenge=显式报错 / 28=超时 / 其余=传输失败 / None=信号终止，Err 一律走既有回退链）、`is_challenge`（原文案判定抽出）、`curl_args`（参数构造纯函数）。
- `DDG_UA` 浏览器 UA 常量（亮明 UA 教训只适用于自托管 SearXNG）。
- collect 签名/返回/Ok(零条) 语义不变；调用方 search.rs:279 未动。
- Cargo.toml：仅注释更新（native-tls 现无代码消费者，feature 组合按红线不动）。

# 风控信号链（本轮实证，比 bd 记录更进一步）

同刻同隧道（127.0.0.1:10808）curl(schannel 8.21.0) 对照：
| 变量 | 结果 |
|---|---|
| UA=浏览器串 | 200（28KB 真实 SERP） |
| UA=curl 默认 / UA=gsearch/0.2.9 | 202 |
| UA=浏览器串 + **显式 `-H "Content-Type: ..."`** | **202** |
| UA=浏览器串 + `--data-raw` 自动 Content-Type | 200 |

**根因：DDG anomaly 风控按头序识别**——curl 自动生成 Content-Type 时排在 Content-Length 之后（原生指纹）；`-H` 显式提供会把它提到 Content-Length 之前（非原生 = 爬虫特征）直接 202。UA 串是第一道信号（非浏览器 UA 必拦），头序是第二道。reqwest 无论怎么配头集都输在头序（库层不可控）。

# 验证

【e2e（验收命令，代理开着）】
```
$ export HTTPS_PROXY=http://127.0.0.1:10808 HTTP_PROXY=http://127.0.0.1:10808 GSEARCH_SEARXNG_URL=http://127.0.0.1:1
$ ./target/debug/gsearch.exe search "rust async runtime" --limit 3 --no-humanize
SearXNG ... tcp connect error: ...(os error 10061)...，已回退 DuckDuckGo html 直连
{"meta":{...,"elapsed_ms":5309,"provider":"duckduckgo",...},"run":{"status":"ok"},
 "results":[{"title":"The Async Ecosystem - Asynchronous Programming in Rust","url":"https://rust-lang.github.io/async-book/..."},
            {"title":"Async in depth | Tokio - An asynchronous Rust runtime","url":"https://tokio.rs/tokio/tutorial/async",...}]}
exit=0
```
provider=duckduckgo ✓ + n>0 ✓。

【SearXNG 正常路径不受影响】unset 死端口后真实单查 → provider=searxng + status=ok。

【单测锁】`classify_curl_result_locks_fallback_semantics`（成功透传/challenge 显式报错/28 超时/其他 exit/信号终止五分类）+ `curl_args_carry_ua_timeout_body_url`（UA/超时/body/URL 就位 + **负向锁：不得显式传 Content-Type**）。既有 form_body/parse/real_url 等测试原样全绿。

【全量闸】`cargo clippy --all-targets -- -D warnings`：0 error；`cargo test --all-targets`：**98 passed(lib, 含新增 2) + 57 passed(bin) = 155 passed / 0 failed / 2 ignored**。

# Side-effects（三态）

- 预期内：DDG 请求从 reqwest 变 curl 子进程（Windows 10 1803+ 自带，verify 已有同款依赖先例）；native-tls feature 失去唯一消费者（Cargo.toml 注释已标注，feature 本体未动）。
- 预期内：`-A` 浏览器 UA 仅 DDG 出口使用，SearXNG（no_proxy + 无 UA 伪装）与 Google 直爬（浏览器路径）不受影响。
- 未观察项：`df=d/w/m/y` recency 参数走 curl 路径未单独 e2e（同 bd khn 既有 blocker，参数在 form_body 层与传输无关，单测覆盖拼接）。

---

# 打回轮 1（PM 亲验：limit 未生效 + score 缺席，af9 遗留缺口）

改动（仍在 src/duckduckgo.rs）：
- **limit 生效**：新增 `parse_limited(html, limit)`（= parse + truncate），collect 改走它——截断在解析后、返回前。DDG 首页一次抓全，`--limit N` 不再多给。
- **score 同键**：parse 尾部去重后按最终序统一打分——**返回序等价分**（首条 = n，末条 = 1，递减 f64）。DDG html 返回序即相关性序，与 SearXNG score 同消费语义（高分更相关、agent 可按分筛序），不另造打分体系（对齐 PM"禁发明第二套语义"约束）。
- README 契约行同步：`score` 条目改为「SearXNG 透传；DDG html 用返回序等价分（首条 = n 递减到 1）；Google 等无分来源此键缺席」。
- 既有测试 `parse_container_decodes_uddg_and_snippet` 的 score=None 断言随行为变更同步为新语义断言（Some(2.0)/Some(1.0)）。

## 契约顺手核对（PM 点名三项）
- `domain_class` ✓ 每条在（push_result 装配 `util::domain_class`）
- `snippet cap` ✓ 装配层统一 160 cap（e2e 实测 141/160 字符）
- `compact-meta` ✓ meta 层开关与 provider 无关，DDG 路径同样生效

## 验证

【e2e（验收命令 + 加验 limit）】
```
$ export HTTPS_PROXY=http://127.0.0.1:10808 HTTP_PROXY=http://127.0.0.1:10808 GSEARCH_SEARXNG_URL=http://127.0.0.1:1
$ ./target/debug/gsearch.exe search "rust async runtime" --limit 3 --no-humanize
provider: duckduckgo
n: 3
  score=10.0 domain_class=blog  snippet_len=160 url=https://rust-lang.github.io/async-book/...
  score=9.0  domain_class=other snippet_len=141 url=https://tokio.rs/tokio/tutorial/async
  score=8.0  domain_class=other snippet_len=160 url=https://corrode.dev/blog/async/
```
n≤3 ✓（修复前实测 n=10）；每条 score/domain_class 在 ✓；snippet ≤160 ✓；provider=duckduckgo ✓。

【单测锁】`parse_limited_caps_results`（3 条 fixture：limit=2 → len=2、保前缀首条 score=3.0；limit=100 → len=3）+ parse 容器测试 score 递减断言。

【全量闸重跑】clippy --all-targets -D warnings：0 error；cargo test --all-targets：**99 passed(lib) + 57 passed(bin) = 156 passed / 0 failed**（中间一轮 parse_limited 断言笔误——3 条 fixture 首条应为 3.0 写成 2.0——已修，语义无涉）。
