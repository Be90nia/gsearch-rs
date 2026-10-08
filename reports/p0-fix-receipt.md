# P0 修复回执（gsearch-rs-0mf / 9re / b95）

三条 P0 串行修复完成，全部验收通过、全量闸与回归闸全绿。VERDICT: PASS

- 执行者：P0FixRun（子代理）· 2026-10-08 · 构建 target/release/gsearch.exe（含全部改动）
- 汇总：src/main.rs +89/-~60、src/postproc.rs +42、src/fetch.rs +76/-~40、README.md +6/-6；未 commit（PM 负责）

---

## 1. gsearch-rs-0mf（search --read --full --json 拼接破坏 JSON 契约）— PASS

改动：
- src/main.rs:336-369 — envelope 构建上提；`read_solo_json`（--json+--read）时 envelope 延后装配，stdout 只出一份 JSON
- src/main.rs:384-400 — post 块 read 产物装配：--full → `content_text` 字段；其余 → `read` 字段
- src/main.rs:413-419 — read 失败时兜底补打 envelope（保住「SERP JSON 已出 stdout」旧行为）
- src/postproc.rs:265-271 — read() --json 模式静默返回串（不再直接 println）
- src/postproc.rs:274-304 — read_full() 接 ReadOpts，--json 静默；拆 read_full_text（无打印核心），read_full_inner 保留给 browse --full

【验证命令+关键输出】（终版二进制）
```
$ export GSEARCH_SEARXNG_URL=http://192.168.89.249:8888
$ ./target/release/gsearch.exe search "rust tokio" --read 1 --full --json --no-humanize 2>/dev/null \
  | python -c "import json,sys; d=json.load(sys.stdin); print('JSON_OK', list(d.keys()), 'content_text_len:', len(d.get('content_text','')))"
JSON_OK ['content_text', 'meta', 'results', 'run'] content_text_len: 2518
```
完整解析成功，无 raw text 残留。read 失败路径：envelope 兜底补打，stderr 报 postproc 失败。

## 2. gsearch-rs-9re（fetch SSRF 防护 + --allow-private）— PASS

改动：
- src/fetch.rs:90-129 — `allow_private_requested(flag)`：env 改值语义（仅 "1"/"true"，防 =0/空值静默开洞）；`gate_check` 核心（返回 host/ip/是否私网，放行时仍解析供 http 判定用）
- src/fetch.rs:150-192 — cmd_fetch：scheme 检查 → SSRF 门先行（私网 URL 无论 scheme 默认在此拒，报错含「私网」）→ 公网明文 http 拒、**私网 http 仅显式放行时允许**（内网端点常见 http-only；原实现 https_only 全局开关会把放行路径也拒掉）
- src/fetch.rs:168-189 — 重定向 `Policy::custom` 每跳过门（9re(c) 防重定向绕过）：每跳 host 解析→私网判定（解析失败 fail-closed）+ scheme 规则；替代 https_only
- src/main.rs:142-143 — Fetch flag 文档同步
- README.md:48,49,52 — https-only 收窄为公网、私网放行语义、重定向每跳过门、env 值语义

【验证命令+关键输出】
```
$ ./target/release/gsearch.exe fetch http://127.0.0.1
error: fetch 拒绝私网地址 127.0.0.1（host=127.0.0.1）。如确需内网，请传 --allow-private 或设置 GSEARCH_FETCH_ALLOW_PRIVATE=1
exit=1                                    # stderr 含「私网」✓
$ GSEARCH_FETCH_ALLOW_PRIVATE=1 ./target/release/gsearch.exe fetch "http://192.168.89.249:8888/search?q=test&format=json"
=== http://192.168.89.249:8888/search?q=test&format=json |  ===
{"query": "test", "results": [], ...}     # 到 HTTP 层，SearXNG 200 响应 ✓ exit=0
$ ./target/release/gsearch.exe fetch "http://192.168.89.249:8888/..." --allow-private   # flag 路径同过 ✓ exit=0
$ ./target/release/gsearch.exe fetch https://example.com --json
JSON_OK ['meta', 'text', 'title', 'url'] len: 171, exit=0   # 行为不变 ✓
```
code-level：fetch 单测 70 passed（含 ssrf_gate_rejects_private_addresses 12 段全谱 + allow_private bypass + is_private_ip 边界）。

## 3. gsearch-rs-b95（headings-only 比 --full 还贵 70%）— PASS

改动：
- src/postproc.rs:207-221 — render_read json 分支补 headings_only 判定（根因：旧 json 分支从未看该 flag，全量序列化 AdaptiveRead→char_count/summary 全泄漏）；headings-only JSON 只出 {url,title,headings}+meta
- src/main.rs:341-342,366-368 — text 模式 headings-only 跳过 print_text（SERP 全集不进输出）
- src/main.rs:396-399 — 装配时 headings-only 置 `results: []`

【验证命令+关键输出】（终版二进制）
```
$ ./target/release/gsearch.exe search "tokio docs" --read 1 --headings-only --no-humanize 2>/dev/null | wc -c 等效
bytes: 371            # 期望 <2000，原 11067B，-97%；输出头 = === https://docs.rs/tokio/latest/tokio/ | tokio - Rust ===\n本文 13 个标题:
SERP 残留编号行: 0    # 不含 10 条 SERP 列表 ✓
$ ... --headings-only --json ...
JSON_OK ['meta', 'read', 'results', 'run'] results: [] read_keys: ['headings', 'meta', 'title', 'url'] n_headings: 13
```
无 char_count 列表、无 summary/paragraph_index。

---

## 4. 全量闸（CI 同款）

```
$ cargo clippy --all-targets -- -D warnings
    Finished `dev` profile ... in 0.39s        # 0 error 0 warning
$ cargo test --all-targets
test result: ok. 70 passed; 0 failed; 0 ignored
test result: ok. 39 passed; 0 failed; 2 ignored
test result: ok. 0 passed;  0 failed
```

## 5. 回归闸

```
$ ./target/release/gsearch.exe search "tokio" --json --limit 3 --no-humanize
JSON_OK ['meta', 'run', 'results'] meta_keys_n: 14 results_n: 3 provider: searxng   # 信封结构不变 ✓
$ ./target/release/gsearch.exe browse https://example.com --json
JSON_OK ['headings', 'meta', 'paragraph_index', 'summary_paragraphs', 'title', 'url']  # 可解析 ✓
```

## 环境现象（照实记录，与改动无关）

- 验收中途 SearXNG 出现约 10 分钟零结果窗口：JSON 端点 HTTP 200 但 results=0（tokio/rust/github/python 全 0），Google 直爬回退报 ERR_CONNECTION_TIMED_OUT——与既往「共享出口 IP 信誉 / passwall 节点死亡，实例配置健康」同款；等待后自行恢复（41/46 结果）。窗口期 0mf/b95 曾在含相应改动的中间构建先行验证通过，恢复后在终版构建复验全过。
- 首轮回归 B 失败为验证命令误带 `--no-humanize`（browse 无此 flag，clap 拒绝 → 空 stdout），按验收原文重跑即过——非代码问题。
- 工作树存在非本次改动的预存变更（CLAUDE.md/AGENTS.md/.agents/skills/beads/.claude/settings.json 删除、Cargo.lock 变更），未触碰未回退，提请 PM 知悉。

## Side-effects 三态

- commit/push：none（未执行，PM 负责）
- 已执行：README.md fetch 节文档同步；~/.omp/agent/rules/rust-pitfalls.md 沉淀一条 reqwest https_only/Policy 陷阱
- 需 PM 执行：bd close gsearch-rs-0mf / gsearch-rs-9re / gsearch-rs-b95（附本回执）；codebase-memory 图谱重索引（项目规约子代理不重索引）

## 残余风险 / 行为变化说明

- SSRF 门存在 DNS rebinding TOCTOU：gate 解析后 reqwest 重新解析、未 pin IP；重定向每跳判定同基于当次解析。工单范围未要求 IP pin，未做。
- `search --read N --json`（adaptive 默认模式）信封新增 `read` 字段——原行为 envelope+read JSON 两段拼接同样破坏 json.loads，属 0mf 同根因，一并修复。
- `browse --json --headings-only` 因 render_read 根因修复同步变瘦（全量 AdaptiveRead JSON → 只 {url,title,headings,meta}）——共享同一 bug 分支，语义与 b95 一致。
- fetch 公网 http 拒绝报错文案微调（「明文 http 已禁用」→「公网明文 http 已禁用」+ 放行提示），因语义实际变化（私网可放行）。
