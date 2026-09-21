# 通用质量+架构审计回执（0daea09..acc7b61）

VERDICT: CONDITIONAL

审计范围：8 commits 全量 diff（src/ 11 文件 +2066/-173、Cargo.toml、README），关键语义段（fetch.rs、postproc::wait_content_stable/cap_chars/render_read、search.rs searxng_collect/run_batch、main.rs cmd_search_batch、browser.rs 探测）已对照实现全文核验，不只看 hunk。只读审计，未改任何源文件。

## Findings

### Important

**I-1 README 零覆盖 fetch 子命令；退出码表与 fetch 实际退出语义冲突** — `README.md:39-50`（退出码表）、`README.md:9-19`（用法示例）、对照 `src/fetch.rs:32-35`
README 自称权威契约（"退出码（agent 消费必读）"），但本次新增的 `fetch` 子命令（351 行模块 + clap 变体）在 README 中无任何出现：无用法示例、无输出契约说明、退出码表无条目。fetch 的 JS 壳路径以**预期结果**退出 1（stderr "该页无服务端正文（JS 壳），需渲染：用 gsearch browse {url}"，`fetch.rs:71-74`），而表格中退出码 1 的含义是"命令执行错误（error 链）"——按表消费的 agent 会把"需换 browse 渲染"误判为可重试错误。触发条件：agent 按 README 契约调 `gsearch fetch <SPA URL>`。修复：README 补 fetch 段（用法 + json meta 契约）+ 退出码表 row 1 追加 "fetch JS 壳（需渲染，stderr 有指引）"。纯文档提交。

### Minor

**M-1 fetch 回执"实现要点"已过时：壳判定仍写单条件，代码已是双条件** — `reports/gsearch-rs-fetch-receipt.md:52-55` vs `src/fetch.rs:112-120`
回执（8cc36f8 时点）声称"壳判定收敛为单条件 正文<500 字符，挂载点分支被逻辑蕴含，不写死代码"；b62b43a 恰恰推翻了该推断（压测抓到小静态页假阳性），改为双条件（<500 **且** SPA 挂载点）。回执自称"供 review"却描述已被后续修复推翻的语义，后人按回执理解壳判定会得出错误结论。测试数（64 bin）同样基于旧版。建议回执补一行 b62b43a 修订注记。

**M-2 skeleton::format_json 生产调用点清零，沦为死公共 API** — `src/skeleton.rs:237`
general/postproc/shell 三处调用全部迁往 postproc::render_read 后，format_json 仅剩自身单测引用。因是 lib pub 项 clippy 不报。按 clean cutover 应删除或降为 #[cfg(test)]；留着即第二份 JSON 序列化路径（render_read 走 to_value+注入 meta，语义已分叉）。

**M-3 meta.truncated/omitted 度量的是 HTML 源字符，README 契约写的是"正文"上限** — `src/postproc.rs:257-258`、`src/general.rs:80-81`、`README.md:35`
cap_chars 施加在 `content_retry` 返回的原始 HTML 上（限解析成本，实现刻意），因此 meta.omitted 计的是被截掉的**标记字符**而非正文字符，truncated=true 也不代表输出正文达到 50000。触发：60k HTML/20k 正文的页面 → omitted≈40k（含标签），agent 按 omitted 推算正文预算或按"上限 50000"预期输出长度都会错。README 措辞应改为"HTML 源码硬截断，omitted 为源码字符数"。

**M-4 --full 模式固定 5000 字上限，read_max_chars 配置对其不生效，README 契约未排除** — `src/postproc.rs:21,281-284`、`README.md:35`
read --full / browse --full / shell read --full 走 READ_FULL_MAX_CHARS=5000，`gsearch.json read_max_chars` 只影响默认 AdaptiveRead 路径。触发：agent 配 read_max_chars=80000 后调 `--read N --full` 期望 80k，实得 5k。方向安全（更严），但"默认 50000 可配"对该模式为假，契约段应注明 --full 例外。

**M-5 fetch 非 HTML 路径仍做实体解码，plain text/JSON 内容被静默改写** — `src/fetch.rs:100-107`
`process_html` 非 HTML 分支 = `collapse_blank(decode_entities(html))`。注释宣称非 HTML 保护源码字面量（"markdown 源码里的 `Vec<u8>` 不是标签"），但实体解码同样改写内容：`fetch` 一个含字面 `&amp;`/`&#20013;` 的 text/plain 或 JSON 资源，输出变成 `&`/`中`。对"取源文件原文"这一 fetch 核心场景是保真度缺口。建议非 HTML 跳过 decode_entities。

**M-6 fetch 手写提取管线与树内 scraper 解析器并存，且注释辩护不准确** — `src/fetch.rs:139-155`、`Cargo.toml:21`
轻量状态机对属性值含 `>` 的标签会漏片段进正文：`<a data-x="a>b" href="#">x</a>` 输出混入 `b" href="#"`（`after.find('>')` 在属性内截断）。模块注释以"不引新 crate"辩护，但 scraper 已在依赖树（skeleton::extract_adaptive 在用），复用零新增依赖——实际取舍是"输出形态"（流式纯文本 vs AdaptiveRead 结构）而非依赖成本。第二套 HTML→文本路径与 read/browse 提取语义会渐进分叉。轻量提取可接受，但注释理由应改准确，并给 M-5/M-6 两类保真度边界留测试锚。

**M-7 持续动态页上 wait_content_stable 必烧满判稳窗口（read/browse +10s、click +4s）** — `src/postproc.rs:150-178`
marker = title+visibleText 需连续两次相同；complete 后正文持续变化的页面（行情条、每分钟刷新的相对时间戳）永不相等 → 50/20 轮全烧。旧行为（readyState complete 即返回）≈0-200ms。结果正确性无损（窗口耗尽返回最后一次快照），纯延迟回退且有界，属判稳设计的已知代价——注释只覆盖了"无导航两轮即过"的静态场景，建议补记此上限场景。

## 已验证无问题的关键面（对照审查维度）

- **正确性**：batch `join_all` 错误隔离成立——每条兜为 `Err(String)`，futures 内无 unwrap/panic 传播面，`join_all` 保序与 README"顺序一致"承诺一致；`searxng_collect` 的 Err/部分结果/全去重三分支与旧 try_searxng 行为逐点对齐（doh 回执申报的 2 处 stderr 变化属实且单向改善）；`serp_url`/`build_url` recency=None 双 provider 逐字节兼容均有测试锚定；壳判定 499/500 边界有回归测试；fetch 超时 10s 含 redirect 链（reqwest client timeout 语义）、`Policy::limited(10)` 与默认一致；`wait_content_stable` 对 -32000/晚跳转的 marker 重置逻辑覆盖 shell cmd_click 旧 ponytail 记账项；快照 None 时登录墙判定退化为 URL 特征与旧 evaluate 全败行为一致；`wait_dom_complete` 未成死代码（content_retry/eval_string_retry 的 -32000 退避仍调用）。
- **架构**：fetch.rs 挂 bin-crate 复用 postproc pub(crate) 项（cap_chars/read_max_chars），与 general.rs 同层，模块落位有据；`resolve_browser_meta` 抽取消除了 cmd_search 内联重复；degrade_html Option→Result 上抛是正确的错误传播重构；公共 API 面增量（Recency/SearchConfig.recency/searxng 第 4 参/BatchEntry/print_batch_json/run_batch）均有消费方，无投机接口。
- **可维护性**：run_batch 是新增而非改签名；SearchConfig/MetaOutput/searxng::search 的字段/参数变更调用点全覆盖（CI 绿 + grep 证实）；测试测行为（URL 字节、五键契约、退出码 clap 解析、边界 499/500、example.com 假阳性回归、pending 泄漏回归）非实现；run_batch 无并发上限有 ponytail 记账。
- **文档一致性**：batch 语义（裸数组/五键/退出码 0/1/2、禁回退）、recency 语义（双 provider/不传字节一致/meta.recency null）、read 输出契约（content_untrusted）与实现+测试一致；差异仅上述 I-1/M-3/M-4。

## 裁决依据

无 Critical/Important 级代码缺陷，正确性面干净；但 I-1（README 契约对新增子命令零覆盖 + 退出码语义冲突）直接违背本 patch 自己宣称的"agent 消费必读"文档定位，属合并前应补的纯文档缺口。补 README fetch 段后即可转 PASS；M-* 均可择机处理。
