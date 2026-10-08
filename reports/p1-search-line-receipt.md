# P1 Search·Doctor·Browse 线回执（zc6 / 6dp / ptb / 2i1 / fve / nw4）

**结论：六条全部完成，双验收通过。VERDICT: PASS**

- code-level：`cargo check --all-targets` 零 error；`cargo test --lib -- search:: searxng:: util:: types:: skeleton::` 46 passed / 0 failed；`cargo test --bin gsearch -- compact_meta browse_full doctor_json ...` 9 passed / 0 failed（domain_class 分类器带 22 断言单测）。
- end-to-end：六条 verify 行逐条真实跑，输出见下。
- 回归闸：三项全过（见 §七）。

---

## 1. zc6 — SearXNG 零结果熔断快速失败 ✅

改动：`src/search.rs`（try_searxng 返回 `SearxngAttempt{NotConfigured|Results|FallbackGoogle|CircuitBroken}` 四态 + `google_fallback_precheck()` TCP google:443 1.5s + `SEARXNG_CIRCUIT_MSG` 常量；run_search 熔断转 Err）+ `src/types.rs`（RunStatus::SearxngDegraded）+ `src/main.rs`（cmd_search 熔断早退 exit 2 + emit_searxng_degraded_json）。bd comment 三硬约束全带：status=searxng_degraded（非 error）✓ / provider=searxng ✓ / stderr 一行诊断豁免静默 ✓。

**验证 A（熔断路径）**：mock SearXNG（200 + `{"results":[]}`）+ 当前 google:443 不可达（doctor FAIL 实证）：
```
$ gsearch search "simd-json Rust benchmark 2026" --json --no-humanize --config <mock>
SearXNG 零结果已熔断（基础设施降级，非查询无资料）；建议换短 query/跑 doctor/直接 fetch 已知源
{ "meta": { ..., "provider": "searxng", "results_count": 0 },
  "run": { "status": "searxng_degraded", "message": "SearXNG 零结果已熔断（…）" }, "results": [] }
real 0m1.534s  ---exit=2
```
（旧版同场景空耗 33s；现 1.5s 秒级返回。人读模式同命令：仅 stderr 一行、stdout 零污染、exit 2。）

**验证 B（健康路径 = 老行为不变）**：真实例恢复后同查询：
```
status: ok | provider: searxng | results_count: 10 | real 0m3.041s | exit 0
```
未测 arm：FallbackGoogle（SearXNG 空但 google 可达）——本机当前 google:443 不可达无法构造；该臂代码与旧回退链逐行相同（仅前置预检门），健康路径已证明预检通过时链路完整。

## 2. 6dp — --compact-meta ✅

改动：`src/types.rs`（COMPACT_META AtomicBool + 8 字段 skip_serializing_if，保留 query/results_count/truncated/provider/elapsed_ms/recency）+ `src/main.rs`（SearchArgs/Browse 加 flag；main() 派发前算 `flag && !debug` 一次性设置）。

```
$ gsearch search "tokio" --json --limit 2 --compact-meta --no-humanize
meta keys: [elapsed_ms, provider, query, recency, results_count, truncated]   ← 恰 6 字段
$ gsearch search "tokio" --json --limit 3 --no-humanize
meta field count: 14                                                          ← 默认全量不变
$ … --compact-meta --verbose debug  → 14
$ GSEARCH_LOG=debug … --compact-meta → 14                                      ← debug 强制全量硬约束双向验证
```

## 3. ptb — doctor SearXNG 健康度探测 ✅

改动：`src/searxng.rs`（`probe()`：GET /search?q=probe&format=json，UA gsearch/0.2.9，3s 超时，报 HTTP 状态 + results 数 + unresponsive_engines 数）+ `src/main.rs` cmd_doctor 第 7 项。三分支实测：
```
[ OK ] SearXNG: HTTP 200, results=47, unresponsive_engines=3 (http://192.168.89.249:8888)
[WARN] SearXNG 可达但零结果（引擎降级/IP 信誉嫌疑）：HTTP 200, results=0, unresponsive_engines=0 (mock)
[WARN] SearXNG 探测失败: error sending request …（实例挂）  /  [SKIP] SearXNG: 未配置…跳过（无配置 CWD）
```
排障增强：200-非 JSON 分支报错带响应头 120 字符（实测抓到真因：unresponsive_engines 元素是 [engine,error] 数组对，Vec<String> 反序列化必炸——已修为 Vec<Value>）。

## 4. 2i1 — doctor --json ✅

改动：`src/main.rs`（Doctor{json} + DoctorStatus{ok,warn,fail,skip} + DoctorCheck/DoctorOutput + record_check 单点计数；exit 规则不变）。
```
$ gsearch doctor --json | jq 形态
{ "checks": [{name,status,message}×7], "elapsed_ms": 5073, "fail_count": 1, "warn_count": 2 }
  [  ok] chrome / [warn] edge / [  ok] profile_writable / [  ok] exit_ip / [fail] network / [  ok] profile_source / [warn] searxng
```
人读模式 1-6 项文本逐字节不变（收集制重构，仅新增 SearXNG 项 + [SKIP] 标签族）；exit 规则不变（FAIL→1 实测）。

## 5. fve — browse --full ✅

改动：`src/general.rs` cmd_browse（full 路径：--json 走 0mf 信封+content_text，meta.truncated 照标；文本模式 + 截断 stderr 提醒）+ `src/main.rs`（Browse full⊥headings_only clap group="browse_mode"）+ `src/postproc.rs`（read_full_text cap 5000→READ_BODY_MAX_CHARS=50000，返回 (text,truncated,omitted)；删 read_full_inner/READ_FULL_MAX_CHARS——唯一调用方已迁移，干净替换）。
```
$ gsearch browse https://example.com --full        → 纯 innerText 全文输出, exit 0
$ gsearch browse https://example.com --full --json → {meta,run,results,content_text} 单文档可解析, truncated:false, exit 0
$ gsearch browse https://example.com --full --headings-only → clap 拒绝, exit 2
```

## 6. nw4 — domain_class ✅

改动：`src/util.rs`（纯函数 `domain_class()`：host 归一 + 子域后缀匹配，值域 docs/github/wikipedia/blog/forum/video/news/qa/other，未命中 other；22 断言单测含近似域名不误标 notgithub.io/docker.com）+ `src/types.rs`（SearchResult 追加末字段，单测锁末键位）+ 装配处 `src/parse.rs`/`src/searxng.rs`（Google JSON/HTML、SearXNG JSON/HTML 四路）+ `src/output.rs` print_text 标题行尾 `[class]`。
```
$ gsearch search "simd-json Rust benchmark 2026" --json --no-humanize
  docs.rs→docs · github.com→github · en.wikipedia.org→wikipedia · substack/medium/github.io→blog · phoronix.com→other
$ gsearch search "tokio async" --json --limit 5 --no-humanize
  github.com/tokio-rs/tokio→github · docs.rs/tokio→docs · tokio.rs→other（不误标）
$ 人读模式：`3. GitHub - tokio-rs/tokio: … [github]` 行尾标注同源
```

## 七、回归闸

| 闸 | 结果 |
|---|---|
| `search "tokio" --json --limit 3`（无新 flag） | meta 14 字段 + run.status ok，信封结构不变 ✅ |
| `browse https://example.com --json` | AdaptiveRead 结构不变（headings/paragraph_index/summary_paragraphs/title/url/meta）✅ |
| `doctor` 人读模式 | 1-6 项文本逐字节不变，仅新增 SearXNG 项 ✅ |

## Side-effects（三态）

1. **跨 owned 文件的必要触碰**（验收标准强制，编译器导向）：
   - `src/postproc.rs`：read_full_text 签名/常量改动（fve）→ **search --read --full 的 innerText cap 5000→50000**（对称性即 fve issue 意图，方向性增强）；read_full_inner 删除。
   - `src/output.rs`：print_text 行尾 `[class]`（nw4 验收明文要求）→ shell/batch 人读输出同获标注（一致增强）。
   - `src/parse.rs`：SearchResult 构造补 domain_class（nw4 编译强制）。
2. **行为变化**：shell 会话内 SearXNG 空+Google 不通时，从「浏览器回退空耗 30s 超时」变为「快速 Err + 诊断文案」（shell.rs 零改动，编译兼容）。
3. **无静默破坏**：所有 JSON 新字段追加末尾；默认路径（无新 flag）信封/结构逐字段不变。

## 未做 / 残留

- FallbackGoogle arm 未能端到端实测（本机 google:443 当前不可达，无法构造预检通过场景）——代码为旧链路原样 + 预检门，健康路径已验链路完整。
- batch 模式 SearXNG 零结果仍为 status=error（batch 无 Google 回退链，zc6 熔断语义不适用；spec 改动点未含 batch）。
- cmd_search `_ =>` 臂内继承旧缩进（差 4 空格）——编译/语义无影响，rustfmt 全量重排会撞并行线在途编辑，留给 PM 统一格式化。
- 每 query 熔断（连续 N 次统计）未做——按 spec 用预检方案（单次即判），bd comment (1) 的「连续 N 次」形态被 (c) 预检方案取代。

已沉淀: bd remember ← SearXNG JSON API unresponsive_engines 元素为 [engine,error] 数组对（Vec<String> 反序列化必炸）+ 探测报错须带 HTTP 状态与响应头片段。
