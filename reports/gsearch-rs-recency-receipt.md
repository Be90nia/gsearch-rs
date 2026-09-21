# 回执：search 子命令 --recency 时间过滤（双 provider）

日期：2026-09-21 ｜ 执行：TaskRecency ｜ 状态：**VERDICT: PASS**

## 改动面

| 文件 | 改动 |
|---|---|
| src/search.rs | `Recency` 枚举（as_str/qdr_letter）+ `SearchConfig.recency` + `serp_url()`（Google tbs 唯一构造点）+ `run_batch()` 签名加 recency + 3 个单测 |
| src/searxng.rs | `build_url()`（json/html 共用，time_range 仅 Some 时拼）+ search/search_html 加参 + 2 个单测 |
| src/main.rs | `RecencyArg`（clap ValueEnum，镜像 lib 惯例）+ SearchArgs `--recency` + 单查询/batch/captcha 三处 meta 回显 + run_batch 透传 + 1 个单测 |
| src/types.rs | `MetaOutput.recency: Option<String>`（末尾追加，None→null 同 proxy 风格）+ fixture 1 行 |
| src/shell.rs | SearchConfig 构造补 `recency: None`（1 行，TaskFetch 确认不碰） |
| README.md | search 节用法 1 行 + 说明 1 段（含 site: 透传） |

禁改文件（fetch.rs/postproc.rs/config.rs）零触碰；无新依赖；未 commit。

## code-level 验证

- `cargo check --all-targets`：Finished，零警告
- `cargo test --lib`：69 passed / 0 failed（新增 serp_url×2、build_url×2、recency_as_str）
- `cargo test --bin gsearch`：33 passed / 0 failed / 2 ignored（新增 recency_flag_parses_enum_values：四值枚举 + 缺省 None + 非法值拒绝）

## end-to-end 验证（真实输出摘录）

1. **searxng 路径**：`./target/debug/gsearch search "rust release" --recency week --limit 5 --json` → EXIT=0，`meta.provider="searxng"`、`meta.recency="week"`，5 条结果。
2. **Google 路径**：`GSEARCH_SEARXNG_URL=http://127.0.0.1:9`（死端口强制回退）+ `--recency week --json --no-humanize` → EXIT=0，`meta.provider="google"`、`meta.recency="week"`；**结果全部落在窗口内**（snippet 相对时间：9小时前 / 2天前 / 4天前 / 4天前）→ tbs=qdr:w 真实收窄的直接观察证据。期间撞一次 CAPTCHA 走既有状态机人解（captcha_solved=true，未改动状态机）。附赠证据：回退 warn 原样打印了 SearXNG 请求 URL，可见 `&time_range=week` 已拼入。
   - **tbs 核实方法**：Google SERP URL 全库唯一构造点 = `search.rs` `serp_url()`（grep `www.google.com/search` 仅此一处 format!），单测字节锁：`serp_url("rust", 10, Some(Week)) == "https://www.google.com/search?q=rust&tbs=qdr:w&start=10"`。运行时页面级 URL 观察未做（结果页 URL 会重定向且无日志点，单测字节锁 + 唯一构造点 + 结果窗口收窄三者互证）。
3. **None 逐字节一致**：单测锁死——`serp_url(_, _, None) == "https://www.google.com/search?q=rust%20async&start=0"`、`build_url(..., None) == ".../search?q=rust&format=json&pageno=1"`（断言串 = 改动前 format! 的逐字节拷贝）；实跑不传 --recency → `{"provider":"searxng","recency":null,"count":3,"status":"ok"}` EXIT=0。
4. **SearXNG 实例探针**（UA: gsearch/0.2.8）：`time_range=day` → 28 条 vs 无过滤 → 46 条，实例侧收窄真实生效。

## side-effects

- batch 多查询：`--recency` 对 batch 同样生效（run_batch 透传，SearXNG time_range）；meta.recency 随条目回显。此为签名变更 `run_batch(queries, limit, recency)`，调用方仅 cmd_search_batch（已改）。
- shell 交互式 search：固定 recency=None（shell 无该 flag，行为不变）。
- CAPTCHA 超时 JSON 信封（status=captcha_timeout）同样携带 meta.recency。
- browse/dl/login/fetch：不受影响（MetaOutput.recency 为 None→null）。

## 残余风险 / 未做

- 非目标未做：site:/domain 专属参数、语言参数、自定义天数（README 已说明 site: 透传）。
- tbs 的页面级运行时观察（浏览器地址栏/抓包）未做，核实方法见上（单测字节锁 + 唯一构造点 + 结果窗口收窄互证）。
- codebase-memory 图谱未重建：项目记忆明令「子代理禁 reindex」，且并行批次（TaskFetch/fetch 子命令）在途，建议 PM 合并态统一 index_repository。
