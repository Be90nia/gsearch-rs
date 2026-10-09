# 盲测六修复回执（FixB6：n76 / isatty / 9gb / 54c / h90 / 9as）

VERDICT: DONE — 5 单 + isatty 拍板全部落地，clippy --all-targets -D warnings 0 error，cargo test --all-targets 171 passed / 0 failed，三场景自查实测通过（两条如实备注见文末）。禁 commit 已遵守。

改动：src/main.rs（isatty 双 flag + resolve_humanize 纯函数 + humanize_effective 接线 5 处 + 9gb 两行 hint + fetch/browse --max-chars CLI）；src/fetch.rs（FetchOpts.max_chars + fetch_one 注入 + 截断单测）；src/general.rs（BrowseOpts.max_chars + cmd_browse 三处 cap + provider 置空 + goto_timeout_error）；src/postproc.rs（read_full_text 增加 limit 参数）；src/browser.rs（lock_failure_msg 提取 + fetch 出口建议）；src/parse.rs（unwrap_google_redirect + 接线 + 单测表）；src/skeleton.rs（code_examples 字段 + <pre> 收集预算 + format_adaptive [示例代码] 节 + 3 单测）；src/types.rs（provider 空=键缺席 + 单测）；src/duckduckgo.rs（percent_decode pub(crate) 一行）；README.md / USER_GUIDE.md 同步。

## n76(P2)：fetch/browse --max-chars

- 实现：`--max-chars <N>`（默认 50000，1..=10_000_000）；fetch 作用于 `text` 字段、browse 作用于 HTML/innerText/markdown 上限；截断后 `meta.truncated=true` + `meta.omitted` 如实。默认值=原 READ_BODY_MAX_CHARS，未传参行为不变。
- 单测：`fetch::tests::max_chars_caps_text_and_flags_truncated`（未超限不截断 / 截 3 字符 omitted 如实 / JSON meta.truncated 同步）。
- e2e：`gsearch fetch https://github.com/tokio-rs/tokio/releases --max-chars 8000` → rc=0，stdout=8554B（≤约9000B），`truncated:true, omitted:8246, text_chars:8000`。（注：Be90nia/gsearch-rs/releases 页正文仅 6388 字符未超 8000 预算 → truncated=false 输出 7308B，同为契约内行为；故换更大的 tokio releases 页验证截断路径。）

## isatty(PM 拍板)：humanize 默认运行时检测

- 实现：SearchArgs 拆 `--humanize`/`--no-humanize` 双 SetTrue flag（overrides_with 双向，后传覆盖先传）；`resolve_humanize(explicit_on, explicit_off, stdout_is_tty)` 纯函数；`humanize_effective()` 接线 warmup/meta/batch/两个 emit fn 共 5 处。
- 单测：`humanize_explicit_flags_parse_both_directions`（双 flag 解析）+ `resolve_humanize_three_states`（显式覆盖优先 ×2 / 默认跟随 TTY ×2）。
- e2e：管道跑 `--config bare-config.json search "rust async runtime" --limit 3`（本 bash stdout 非 TTY）→ `meta.humanize:false`；干净路径 9.67s（meta.elapsed_ms=9443，<10s）。README/USER_GUIDE/help 已写明「管道自动快档」。
- 如实备注：首次 bare run 撞 CAPTCHA 有头人解（盲测六 C 同款 89s 风险面），本次人解快耗时 13.4s>10s——快档收益结构存在（humanize=false 省 warmup），CAPTCHA 等人是独立风险非本单回归。

## 9gb(P2)：回退提示行

- 实现：SearXNG NotConfigured 且最终 provider=google → stderr 一行 `[hint] SearXNG 未配置，已回退 Google 直爬（可配 GSEARCH_SEARXNG_URL 提速；agent 高频建议 --no-humanize）`（PM 原文）；显式 --humanize 生效且非 TTY → warmup 前 stderr 一行耗时代价提示。
- e2e：bare run stderr 实测含 hint 行（见上方 e2e_bare.err 摘录）；humanize 提示行因需「显式 --humanize + 非 TTY」组合，未单独跑真实浏览器路径（代码路径与 hint 同型 eprintln，逻辑一行 if，回归风险低）——**此提示行仅单测未覆盖，端到端未验证**。

## 54c(P2)：Google 直爬 goto 解包

- 实现：parse.rs `unwrap_google_redirect`——仅对 google.com 壳生效；`/url?q=`（q 优先 url 兜底）与 `/goto?url=` 解包 + percent-decode，目标须 http/https；解不出保留原样不报错。SearXNG 路径零影响（host 门）。
- 单测：`unwrap_google_redirect_table`（直链不动 ×2 / 解包 ×3 / 解不出保留 ×2）+ `parse_serp_unwraps_goto_shell_urls`（SERP 端到端 + domain_class 按真实 URL 判定）。
- e2e（bare Google 回退路径实测）：本次真机 SERP 返回的是 `google.com/goto?url=CAESfg...` **不透明 base64 blob** 形态——urlsafe_b64decode 后为 protobuf 加密二进制（printable 全乱码，无明文 URL），客户端不可解包 → 保留原样分支真实命中（符合「解不出保留原样」契约）。`/url?q=` 形态由单测锁。**结论：该形态 URL 无法客户端还原，README 消费指南已注明对 google.com 域 URL 用 --read 兜底。**

## h90(P3)：browse 错误出口引导

- ① general.rs `goto_timeout_error`：browse goto 超时文案补 `若目标站点需代理可达，试 --proxy http://127.0.0.1:7890（GSEARCH_PROXY 同效）`；单测 `goto_timeout_error_contains_proxy_hint`。
- ② browser.rs `lock_failure_msg`：profile 锁 5 轮重试打尽后的失败文案补 `无需渲染的场景可用 gsearch fetch <url> 替代（纯 HTTP，不经 Chrome 无 profile 锁）`；单测 `lock_failure_msg_contains_fetch_exit`。
- ③ 拍板：**browse meta.provider 删该键**（provider 置空串 + `skip_serializing_if = "String::is_empty"`）。理由：provider 值域是三个搜索后端名（searxng/duckduckgo/google），browse 无搜索来源；塞目标 host 与值域冲突、伪装 "google" 误导分流（盲测五审计 + B 受试者双实锤）；键缺席是本仓库既定「缺席=正常」语义（8lp），搜索路径恒非空、搜索输出键存在性零变化。单测 `provider_empty_key_absent_search_keeps_key` 双向锁。

## 9as(P3)：AdaptiveRead code/example 加权

- 实现：extract_adaptive 收集 `<pre>` 文本（覆盖 docs.rs rustdoc 签名/示例与 GitHub md 围栏块），新增 `AdaptiveRead.code_examples: Vec<String>`（skip 空 Vec——无代码页输出逐键不变）；预算：前 2 块 × 单块 1500 字符封顶（超长尾部标注截断）；format_adaptive 增 [示例代码] 节（摘要→示例→段落索引）。**拍板说明：单独字段而非并入 summary_paragraphs**——并入会污染 drop_summarized_pi 的「摘要段数=pi 剔除数」计数（多剔 2 个未展示段的索引），单独字段零计数副作用。
- 单测 ×3：`code_examples_collected_from_pre_blocks`（签名+Example 各一块 / 无 pre 页键缺席）/ `code_examples_bounded_by_budget`（前 2 块 + 1500 截断标注）/ `format_adaptive_renders_code_section`（节序 + 内容）。
- e2e：`search "serde_json from_str" --read 1`（SearXNG 命中 → docs.rs）→ 信封 `read.code_examples` 恰含 `pub fn from_str<'a, T>(s: &'a str) -> Result<T>...` 签名 + 完整 `serde_json::from_str(j).unwrap()` 示例——一次调用拿齐，无 --full 二跑。

## 文档

- README：humanize isatty 自动档段重写；fetch/browse `--max-chars` 示例+说明；新增「单行 JSON 消费姿势（agent/管道必读）」段（重定向+切片 / python -m json.tool / 预算控制 / 禁 2>&1 / provider 键缺席说明）；read/browse 契约补 code_examples；SearXNG 段补 [hint] 行。
- USER_GUIDE：6.2 重写为「humanize 自动档（isatty）」。
- help：clap doc 注释同步（--humanize/--no-humanize/--max-chars ×2）。

## 全量闸

```
cargo clippy --all-targets -- -D warnings   → Finished，0 error 0 warning
cargo test --all-targets                    → 107 passed (lib) + 64 passed (bin) / 0 failed / 2 ignored(live)
```

## Side-effects 三态

1. **预期内行为变更**：① search 默认 humanize 由「恒开」变 isatty 自动（TTY 人不变；管道 agent 变快档）——PM 拍板本体；② fetch 正文字符上限由「gsearch.json read_max_chars 可覆盖」变为「CLI --max-chars（默认 50000 常量）」——gsearch.json read_max_chars 对 fetch 失效、对 search --read/browse read 路径仍生效；③ browse --json --full/--markdown 不再输出 meta.provider 键。
2. **预期外**：无（fetch 对 github.com 间歇 10s 超时为本机网络抖动，curl 同期 200，与本次改动无关——改动未触网络层）。
3. **非本代理的工作树变更（未触碰，交上级裁定）**：`AGENTS.md`/`CLAUDE.md`/`.claude/settings.json`/`.agents/skills/beads/*` 删除，`reports/b5-fix-receipt.md` 修改，`.beads/interactions.jsonl` 追加，未跟踪 `tk_*.json`/`tk_s.txt`/`gsearch.json`。本代理改动仅限上列 11 文件 + 本回执。

## 备注

- 超出 Owned 清单的文件：skeleton.rs / postproc.rs / general.rs / fetch.rs / duckduckgo.rs——任务项（9as/9gb/n76/h90）实现落点在这些文件（Owned 清单中「src/search.rs（AdaptiveRead 权重）」与实际代码位置 skeleton.rs 不符）；合并态独占无并行冲突。
- 9gb humanize 非 TTY 提示行未跑真实浏览器端到端（见该节注）。

## 复测追加（FixB6 修复者自复测 + 残口修复）

Main 复测邀请后按记账纪律重跑（任务：读 GitHub issue 正文+讨论；tokio#2537）。**抓到一个 h90① 残口并当场修复**：真机超时走的是 chromiumoxide 内部 `goto … 失败: Request timed out.` 分支（先于外层 30s 包装触发），该分支原无 --proxy 提示——已加 `goto_nav_error` helper 统一两层出口，实测复跑 `browse https://192.0.2.1/` → `error: goto https://192.0.2.1/ 失败: Request timed out.；若目标站点需代理可达，试 --proxy …`。单测 `goto_timeout_error_contains_proxy_hint` 扩展锁两分支。gates 复跑全绿（clippy 0 error / 107+64 passed）。

复测记账：① fetch issue 页 4321B 单行（预算内，rc=0 3.2s）；② browse AdaptiveRead `--max-chars 6000` 247B（truncated 标注）；③ browse `--full` 信封 meta 键 `[elapsed_ms, humanize, limit, profile, query, tool, version]`——**provider 键缺席实锤**；④ 超时路径两层文案均带 --proxy 出口（残口修复后）。

## 打回轮 1（Main 复测裁定 UX 8.17 / Token 8.83）四条残留修复

1. **AdaptiveRead GitHub issues/pull 空正文（B -1，根因修复）**：根因是正文容器在 DOM 尾部，先 cap（50000 字符）后抽 → head/nav/SVG sprite 吃光预算，summary 空 + omitted=589331。修：skeleton 新增 `github_comment_html()`（.js-comment-body/.comment-body 容器 inner_html 拼接）+ postproc `cap_extract_source()`（仅 `github.com` 的 `/issues/`|`/pull/` URL 启用，先抽容器再过同一字符预算；其余 URL 行为逐字节不变）；read 与 browse 双路径接线。**通用保底**：render_read 对「summary 空且 omitted>0 且非 headings-only」stderr 一行 `[hint] 正文提取为空（omitted N 字符），试 --markdown 或 --full`。单测：skeleton `github_comment_html_extracts_thread_containers`、postproc `cap_extract_source_prefers_github_containers`（GitHub 线程命中 / 非 GitHub URL 行为不变 / GitHub 非线程页不启用，三向锁）。
2. **code_examples 签名空白保真（C -1）**：根因是 rustdoc 签名 token 间距靠 CSS margin（span 相邻无空白文本节点），`text()` 直拼把 `Result<T>where` 粘成非法 Rust。修：pre 提取换 `code_block_text()` 树内遍历——内部空白原样保留（禁空白折叠），仅跨元素边界且两侧为词字符（或左侧 `>`/`)` 收尾）时补一个空格；`from_str(`、`&'a` 天然相邻不拆。单测：`code_block_text_preserves_word_boundaries`（pub fn from_str / Result<T> where / 换行缩进保留 / 非词边界不插）。
3. **profile 锁 WARN 阶段 fetch 出口（B 部分复现）**：launch_with_retry 每轮 warn 追加 `或用 \`gsearch fetch <url>\`（纯 HTTP 无需 Chrome）`（LOCK_WARN_FETCH_EXIT 常量）；终态 error 的 fetch 出口维持原样。单测 `lock_failure_msg_contains_fetch_exit` 扩展锁常量两关键词。
4. **[hint] 行时机（C nit）**：SearXNG 未配置 hint 从运行结束（match 后）前移到回退决策点（Google 臂入口，Chrome 启动等待期即可见）。

验收复测（修复后真机）：

- **① B 场景**：`browse https://github.com/tokio-rs/tokio/issues/2782`（默认 AdaptiveRead）→ rc=0，3266B，**summary 10 段真实正文**（首段 "This PR is the first in a series of PRs to solve #2720..."）+ code_examples 2 块——不再是 297B 空壳+omitted 58 万。
- **② C 场景**：`search "serde_json from_str" --read 1`（docs.rs 真实 DOM）→ `code_examples[0]` = `pub fn from_str<'a, T>(s: &'a str) -> Result<T> where\n    T: Deserialize<'a>,`——`Result<T> where` 空格还原、无 ` \n` 尾随空格、换行缩进原样，合法 Rust 字面。
- **③ 锁 WARN**：复测运行真实撞锁竞态（3 轮 WARN 实录），每轮均带 `；或用 \`gsearch fetch <url>\`（纯 HTTP 无需 Chrome）`；单测常量断言双关键词锁。
- **④ hint 时序**：bare 管道 search stderr 第 0 行即 `[hint] SearXNG 未配置…`，先于第 1 行「使用浏览器」INFO——等待期可见。meta.humanize=false 照常。

修复后追加单测 3 个（skeleton×2 + postproc×1）+ browser 断言扩展；全量闸复跑：clippy --all-targets -D warnings 0 error，cargo test --all-targets **109 lib + 65 bin passed / 0 failed**。54c 维持 PM 裁定 RULED（可解形态已解 + README 声明 --read 兜底，blob 形态物理不可解已如实记录）。禁 commit 已遵守。

附带修复（FixB6b 静态盘点 FYI 采纳）：code_block_text 对自带前导空白的文本节点跳过补空格（避免 `where \n` 尾随空格），回归断言 `!code.contains(" \n")` 入单测。

## 打回轮 2（browse 302 竞态 + 保底条件洞，终轮）

PM 实测 browse issues/2782 连续 3 轮全空（summary/title/url 全空串、meta 仅 content_untrusted）——与 B 上轮 omitted=58 万是**两种失败形态**：本轮是 /issues/N → /pull/N 的 302 竞态（goto 只覆盖首个响应，落定前 content() 拿空；wait_content_stable 对空串 marker 恒稳兜不住），原保底条件 `summary 空 && omitted>0` 在 content==0 时 omitted=0 永不触发 → 静默空输出。修（general.rs，最小域）：

1. **302 落定重试**：`content_needs_settle_retry()`（空/极短 <200 字符判未落定）→ 重新 goto（直达落定页）+ wait_content_stable + content_retry 一次。
2. **保底条件放宽为 PM 公式**：`needs_empty_body_hint(summary, headings_only, omitted, raw_content_empty)` = `summary 空 && !headings-only && (omitted>0 || content 空)`——两种失败形态（截断吃光 / content 拿空）都出 stderr `[hint] 正文提取为空，试 --markdown 或 --full`。

单测 +2：`content_needs_settle_retry_threshold`（空/极短/阈值三态）、`empty_body_hint_gate`（截断形态 / content 空形态 / headings-only 不适用 / 有正文不适用 / 真无段落小页不适用，五向锁）。

**验收（修复后 3 连跑）**：browse issues/2782 → 3/3 rc=0、3264B、summary_paragraphs=10 段真实正文——302 竞态由重试兜住，hint 按设计静默（正文在）。全量闸复跑：clippy --all-targets -D warnings 0 error；cargo test --all-targets 109 lib + 67 bin passed / 0 failed。禁 commit 已遵守。
