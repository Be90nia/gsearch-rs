# FixG11 Receipt — 盲测十 fetch 三扣分（host 路由 + 默认 timeout/retry）

VERDICT: PASS

## 范围
盲测十 M (UX 7.5) + N (UX 7.5) 暴露的 fetch 路径 3 个扣分：
- P0-1 `fetch github.com` nav 残留（header/Platform/Solutions/Resources 占 7KB+）
- P0-2 `fetch docs.rs` 全站侧栏 / nav / footer 混入正文
- P0-3 `fetch` 默认 timeout=10s 在 GitHub .diff / tag 页偶发握手失败时吃满才退

复用 FixG9 skeleton 容器链（`pub fn github_comment_html`）+ FixG10 `--include` 多选器
落到 host 路由层，并把 `FetchOpts::default()` 收紧到 `timeout=30s / retry=1` 给公网抖动
1 次自愈窗口。

## 改动

### src/fetch.rs
- `FetchOpts::default()`：`timeout_secs: 10 → 30`、`retry: 0 → 1`（盲测十 P0-3 GitHub 抖动自愈；
  breaking change，文档 + 测试同步）
- `FetchOpts.timeout_secs` / `retry` 字段 doc 注释更新（含 FixG11 标注改默认理由）
- 新增 `host_default_include(url) -> Option<(&'static str, &'static str)>`（host 前缀匹配，
  返回 `(label, selector)`；github → 容器优先级链 + 旧回退；docs.rs → `main`）
- 新增 `host_route_applies(opts_include, url) -> Option<&'static str>`（用户显式 `--include`
  时 host 路由不覆盖——盲测十 P0-4 验收闸）
- 新增 `apply_github_host_route(&mut Fetched, html, markdown, limit)` —— 调
  `gsearch::skeleton::github_comment_html` 同时拿容器链命中 + nav 剥离；返回 Ok(true) = 改写
  了 body、Ok(false) = 容器未命中（releases/blob/repo 首页等走默认提取）
- 新增 `apply_docsrs_host_route(&mut Fetched, html, is_html, markdown, limit)` —— 走
  `extract_with_include(html, "main")` 同款路径；同时设 `include_hit=true`/`include_hits=N`
  （与 `--include` 命中语义对齐）
- `Fetched` 加 `auto_include_applied: Option<String>` 字段（host 路由实际生效时 Some(label)，
  否则 None；meta 仅在 Some 时出键，默认输出逐键不变）
- `process_html` Fetched 构造补 `auto_include_applied: None`
- 既有 `--include` 块 Fetched 重建补 `auto_include_applied: fetched.auto_include_applied`
  （透传：host 路由 + 用户 `--include` 共存时不冲突）
- `fetch_one_attempt` 在 markdown 转换之后、`--include` 块之前插入 host 路由块：
  ```
  if opts.include.is_none()
      && let Some(label) = host_route_applies(opts.include.as_deref(), url)
  {
      let applied = match label {
          "github" => apply_github_host_route(...)?,
          "docs.rs" => apply_docsrs_host_route(...)?,
          _ => false,
      };
      if applied { fetched.auto_include_applied = Some(label.to_string()); }
  }
  ```
- `fetched_json` 在 `f.auto_include_applied.is_some()` 时输出 `meta.auto_include_applied` 键
  （与 `truncated_by_json_keys` 同款按需出键 / 缺席语义）
- 既有 4 处 `Fetched { ... }` 测试 fixture 补 `auto_include_applied: None`
- 新增 7 单测（盲测十 P0-1/P0-2/P0-4/P0-5 + FetchOpts default 改值 + host 路由分发）

### src/main.rs
- `FetchArgs.timeout`：`default_value_t = 10 → 30`（FixG11 标注 + 范围 1..=300 不变）
- `FetchArgs.retry`：`default_value_t = 0 → 1`（FixG11 标注 + 范围 0..=3 不变）
- 既有 `fetch_new_flags_parse_with_ranges` 单测：`assert_eq!(timeout, 10)` → `assert_eq!(timeout, 30)`、
  `assert_eq!(retry, 0)` → `assert_eq!(retry, 1)`

### README.md
- fetch `--timeout` / `--retry` 默认说明：`默认 10 / 0` → `FixG11 默认 30 / 1` + 改默认理由
- 新增 **host 级默认 include** 节：未传 `--include` 时按 host 路由——github → skeleton 容器链 +
  nav 剥离（`auto_include_applied="github"`），docs.rs → `<main>`（`auto_include_applied="docs.rs"`）；
  用户显式 `--include` 时 host 路由不覆盖；host 未命中 / 容器未匹配 → 退回 process_html 默认提取，
  `auto_include_applied` 键缺席

## 验证

### 全量闸
```
$ cargo clippy --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.52s
（0 error / 0 warning）

$ cargo test --all-targets
test result: ok. 121 passed; 0 failed; 0 ignored    # lib (baseline 121, +0)
test result: ok.  94 passed; 0 failed; 2 ignored    # bin (baseline 87, +7)
                                                  # 合计 215 ≥ 208（g9 round2 基线）+7 ✓
```

### 新增 7 单测
- `fetch::tests::host_default_include_dispatches_by_host` — github/docs.rs 内容页、裸 host、
  未知 host、大小写不敏感、http 形态都覆盖
- `fetch::tests::host_route_applies_user_explicit_include_not_overridden` — 用户显式 `--include`
  时 host 路由返回 None（盲测十 P0-4 闸）
- `fetch::tests::apply_github_host_route_strips_nav_and_marks_label` — markdown-body 命中 + nav
  剥离 + label 设 "github"（盲测十 P0-1 闸）
- `fetch::tests::apply_github_host_route_no_container_match_keeps_body` — 无容器命中时
  applied=false、label 不设、body 不被改写
- `fetch::tests::apply_docsrs_host_route_uses_main_and_marks_label` — `<main>` 命中 + 侧栏剥离 +
  label 设 "docs.rs"（盲测十 P0-2 闸）
- `fetch::tests::fetched_json_auto_include_applied_emits_when_set` — Some 出键 / None 缺席
  （盲测十 P0-5 闸）
- `fetch::tests::fetch_opts_default_timeout_30_retry_1` — FetchOpts::default() 改值（盲测十 P0-3 闸）

### E2E（release 二进制 5 条 P0 全过）

**P0-1** `gsearch fetch --json --markdown https://github.com/tokio-rs/tokio/issues/8065`：
```
{"meta":{"auto_include_applied":"github","content_untrusted":true,"format":"markdown",
         "github_comments_hint":"评论区由 JS 动态加载…","github_comments_missing":true,
         "omitted":0,"truncated":false},
 "text":"## Summary\n\nSince Tokio `1.51.0` (#7431, \"runtime: steal tasks from the LIFO slot\"),…"}
2.30s 完成。头 1KB 不含 Platform/Solutions/Resources/Navigation Menu（盲测十 P0-1 验收）。

$ python check:
  contains Platform: False
  contains Solutions: False
  contains Resources: False
  contains Navigation Menu: False
```

**P0-2** `gsearch fetch --json --markdown https://docs.rs/serde_json/latest/serde_json/fn.from_str.html`：
```
{"meta":{"auto_include_applied":"docs.rs","content_untrusted":true,"format":"markdown",
         "include_hit":true,"include_hits":1,"omitted":0,"truncated":false},
 "text":"[serde\\_json](index.html)\n\n# Function from\\_str Copy item path\n[Source]…"}
0.39s 完成。text = 1184 bytes < 2KB ✓，含 `pub fn from_str` 签名 + Errors 段 ✓。

$ python check:
  text bytes: 1184
  text chars: 1179
  contains from_str signature: True
  contains Errors: True
  meta.auto_include_applied: docs.rs
```

**P0-3** `gsearch fetch --markdown --max-chars 10000 https://github.com/tokio-rs/tokio/releases/tag/tokio-1.53.0`：
```
{"meta":{"auto_include_applied":"github","content_untrusted":true,"format":"markdown",
         "omitted":0,"truncated":false},
 "text":"# 1.53.0 (July 17th, 2026)\n\n### Added\n\n*   fs: implement `From<OwnedFd>`…"}
1.05s 完成（默认 30s+1 retry 内完成，无超时）。text = 3803 bytes > 3KB ✓。

$ python check:
  text bytes: 3803
  text chars: 3803
  meta.truncated: False
```

**P0-4** `gsearch fetch --json --include "main" https://docs.rs/serde_json/latest/serde_json/fn.from_str.html`：
```
{"meta":{"content_untrusted":true,"include_hit":true,"include_hits":1,
         "omitted":0,"truncated":false},  ← 注意：auto_include_applied 缺席
 "text":"serde_json Function from_ str Copy item path Source…"}
0.34s 完成。用户显式 --include 时 host 路由不覆盖（meta.auto_include_applied 键缺席，
include_hit=true 由 --include 命中标记）—— 盲测十 P0-4 验收。
```

**P0-5** `gsearch fetch --json https://github.com/tokio-rs/tokio`（仓库首页也命中 markdown-body
因为有 README）：
```
{"meta":{"auto_include_applied":"github",...}, "text":"Tokio\nA runtime for writing reliable…"}
2.84s 完成。auto_include_applied: "github" ✓。
```

## 关键设计取舍
- **github 路由走 skeleton 公共 API 而非 selector 路径**：nav 剥离无法用 CSS 选择器表达
  （不能在 selector 里说"剥掉 banner/header"），所以对 github 走 `gsearch::skeleton::github_comment_html`
  一步完成容器链命中 + nav 剥离。docs.rs 没这个问题，走 selector="main" 经 `extract_with_include`
  旧路径即可。host_default_include 返回 selector 是为了 docs.rs 路径用；github 路径直接调公共 API。
- **host 路由应用之后才设 `auto_include_applied`**：如果容器没命中（`applied=false`）则 label
  不设，避免误标。GitHub releases/blob/repo 首页偶尔会命中，但裸 github.com 主页（路径空）host_default_include
  返回 None 根本不进路由——避免对首页误命中。
- **`include_hit` 不为 host 路由设**：host 路由有自己的语义标记（`auto_include_applied`），
  不与用户 `--include` 命中混用。docs.rs 路径例外：复用 `extract_with_include` 时
  include_hit=true 跟随出（语义一致：命中了 main），但 auto_include_applied 仍是宿主标记
  区分。
- **breaking default 文档同步**：timeout 10→30 + retry 0→1 是 breaking，已同步 README
  `fetch --timeout/--retry` 默认说明 + 测试 fixture + 既有 1 处 main.rs 测试断言。
  确定性错误（私网门拒 / scheme / PDF / 二进制）仍不重试，1 次重试对真错误零影响；
  真公网抖动（GitHub 5xx / DNS 偶尔失败）1 次重试 + backoff 1s 给抖动自愈。
- **`opts.include.is_none()` 双重检查**：fetch_one_attempt 块有 `opts.include.is_none()` 守门，
  host_route_applies 也检查一遍（双保险——P0-4 用户显式 --include 必须不覆盖）。

## Side-effects
- **breaking 改默认（FetchOpts::default）**：timeout_secs 10→30、retry 0→1。已有脚本若硬编码
  期望 10s 超时行为会受影响（10s 内 5xx 立即报错→新行为 30s+1 次重试）；99% 场景对用户无感
  （真实公网请求很少超 10s 且极少首请求就 5xx）。文档已同步说明改默认理由。
- **新 meta 键** `auto_include_applied`：仅在 host 路由实际生效时出（github/docs.rs 标签字符串），
  默认输出逐键不变（与 include_hit/include_hits 同款按需出键 / 缺席语义）。
- **未 commit / 未 push / 未改 skeleton.rs / 未改 default 之外的 fetch 主逻辑 / 未动 SearXNG
  / Google / browse / shell / SSRF 门 / Content-Type / redirect 跟随**。

## 残余风险 / 未做
- **GitHub 容器链覆盖**：当前依赖 skeleton::SEL_GITHUB_NEW（markdown-body/issue-body/article/
  .markdown-body）+ 旧回退 .js-comment-body/.comment-body。若 GitHub 未来把主容器改名或加新形态，
  host 路由会失效（applied=false 退回默认提取，auto_include_applied 键缺席——不会静默坏）。
- **docs.rs `<main>` 形态假设**：rustdoc 输出格式变化（移到 `<article>` 或其他容器）会让路由
  退回默认提取。同样不静默坏。
- **未 commit/push**（上级负责）
- **未更新代码知识图谱**（cbm-bridge 提醒；本任务 main 只改 src/fetch.rs、src/main.rs、README.md、
  增量小；若主代理跑 index_repository 同步即可）

## 文件清单
- src/fetch.rs（核心实现 + 7 新单测 + 4 既有 fixture 升级 + 新增 4 公共函数）
- src/main.rs（CLI flag default 改值 + 1 既有测试断言升级）
- README.md（fetch 默认说明 + host 路由节）
- reports/g11-fix-receipt.md（本文件）
