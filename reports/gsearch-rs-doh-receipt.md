# gsearch-rs-doh 回执：batch 多查询搜索

**结论**：实现完成，代码级验收在本人改动范围内全绿；三条 end-to-end verify 因并行派单工作树持续变动，按 PM 指示转为待 PM 在合并态统一执行（命令已备好，见文末）。

**VERDICT: PASS**（代码级 PASS；e2e 三条 DEFERRED → PM 合并态执行）

## 改动清单（独占四文件，+255 / -48）

### src/search.rs（+97/-48 核心重构）
- `try_searxng` 改薄壳：未配置 → `?` 早退 None；收集内核抽为 `searxng_collect(base, cfg) -> Result<Vec<SearchResult>, String>`（原翻页循环原样迁移，语义不变）
- `degrade_html` 从 `Option<Vec>` 改 `Result<Vec, String>`：失败原因（json 层 + html 层）上抛，回退 warn 从函数内移到 `try_searxng`（单查询 stderr 措辞与改前逐字节一致："SearXNG {base} 查询失败（{err}），已回退 Google 直爬" + 403 提示）
- 新增 `batch_one`：SearXNG-only 单条查询，禁浏览器回退；未配置给出明确错误文本
- 新增 `pub run_batch(queries, limit)`：`futures::future::join_all` 并发（futures 0.3 已有依赖，未新增），单条失败兜为 `Err(原因)`，返回顺序与输入一致，条目间互不阻塞

### src/types.rs（+47）
- 新增 `BatchEntry { query, status: RunStatus, message, meta: MetaOutput, results }`：复用现有 RunStatus / MetaOutput 序列化结构（符合跨任务契约）
- 新增契约测试 `batch_entry_serializes_contract_keys`（五键齐备 + status snake_case + error 条目形状）

### src/output.rs（+9）
- 新增 `print_batch_json(&[BatchEntry])`：裸 JSON 数组输出（元素自带 meta，无外层信封）

### src/main.rs（+150/-27）
- `SearchArgs.query: String` → `Vec<String>`，显式 `#[arg(required = true, num_args = 1..)]`（clap 4 对 Vec 位置参数默认**不**强制至少一个值，实测踩坑后显式补上）
- `cmd_search` 头部：`len > 1` 分流 `cmd_search_batch`；单查询路径行为不变（提取 `query` 变量替换原 `args.query` 引用）
- 浏览器路径解析抽为 `resolve_browser_meta`（原逻辑原样搬移，batch 与单查询共用；只探测 exe 不启动）
- 新增 `cmd_search_batch`：拒绝 `--read/--dl/--open`（batch 无浏览器参与）；并发跑 `run_batch` 后组 `BatchEntry`（meta 与单查询同源；proxy 恒 None——searxng.rs 固定 no_proxy 纯 HTTP）；人读模式逐条 `=== [i/N] query ===` 分隔标题，条目内沿用现有 `print_text` 格式；退出码 全成功 0 / 部分失败 1 / 全部失败 2
- `emit_captcha_timeout_json` 增加 `query: &str` 参数（单查询路径内部签名适配，行为不变）
- 新增 clap 解析测试 `search_accepts_one_or_more_queries`（多查询收集 / 单查询兼容 / 零查询拒绝）

## 关键设计边界（与派单一致）
- batch 禁浏览器回退：`batch_one` 只走 SearXNG，None/失败/空结果一律 error 条目——浏览器单例不可并发，刻意边界
- 单查询模式零行为变化：searxng → Google 回退链、CAPTCHA 状态机、warmup、`--read` 全部原路径
- M17 语义保持：searxng 命中全程零浏览器；batch 即使 searxng 全挂也不起浏览器（每条 error 收口）
- 并发无 semaphore：ponytail 备注——共享 reqwest 客户端 + 局域网实例，查询量到几十条再加分批

## stderr 边缘行为变化（如实申报，共 2 处，均为改前措辞失真处）
1. 部分结果 + 后续页双失败场景：改前会打"已回退 Google 直爬"（实际返回部分结果、并无回退，措辞是谎话）；改后该场景不打此 warn。总失败路径的 warn 文本逐字节不变。
2. 各页结果全被去重致空：改前静默回退 Google；改后多打一行 warn 再回退（补充了原因，更可诊断）。

## 验证证据（本人改动范围，真实输出）

1. `cargo check --all-targets`：
```
    Checking gsearch v0.2.8 (D:\Project\gsearch-rs)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.20s
```
2. 修复 clap required 前的 `cargo test --bin gsearch` 抓到真实边界：`assertion failed: Cli::try_parse_from(["gsearch", "search"]).is_err()`（零查询被放行）→ 显式 `required = true` 修复。
3. 修复后 `cargo test --bin gsearch`：`test result: ok. 25 passed; 0 failed; 2 ignored`（含新增 `search_accepts_one_or_more_queries`）
4. 全量 `cargo test`（bg_1 完成快照，本人不再重跑）：
```
running 63 tests  → test result: ok. 63 passed; 0 failed   （lib）
running 29 tests  → test result: ok. 27 passed; 0 failed; 2 ignored （bin）
running 0 tests   → test result: ok. 0 passed; 0 failed    （doc）
```
5. 期间出现的 bin test E0308 编译错经 PM 判定为 TaskPostproc 在途编辑中间态，本人文件无关（本人 --bin 测试在其前后均绿）。

## 未做 / 待 PM 合并态执行

- 三条 e2e verify（debug 构建已产出 `./target/debug/gsearch.exe`，release 由 PM 统一构建；命令中的 release 路径按 PM 构建产物替换）：
  1. `./target/release/gsearch search "rust async runtime" "tokio tutorial" --json --limit 3` → 期望 JSON 数组 len=2，每条 results 非空且 meta.provider="searxng"，stderr 无浏览器痕迹
  2. `./target/release/gsearch search "武汉大学" --limit 3` → 单查询回归，行为与改前一致
  3. `GSEARCH_SEARXNG_URL=http://127.0.0.1:9 ./target/release/gsearch search "rust async runtime" "tokio tutorial" --json --limit 3` → 期望两条 status="error" 条目 + stderr "batch 完成：0/2 条成功" + 退出码 2（非 panic）
- 合并态全量 `cargo test` 复绿确认 + `cargo build --release`
- 备注：live 跑 verify 时 searxng 路径零浏览器，无需 GSEARCH_PROFILE 隔离；若 searxng 恰好宕机，verify 2 会走 Google 回退起浏览器，建议加 `GSEARCH_PROFILE=doh-verify` 隔离

## 沉淀

- 已沉淀: `~/.omp/agent/rules/rust.md`（rule://rust 常见陷阱段）← clap 4 derive 的 `Vec<String>` 位置参数默认不强制至少一个值（零值放行、空查询静默流入业务），须显式 `#[arg(required = true, num_args = 1..)]` 并用零值用例锁死契约
