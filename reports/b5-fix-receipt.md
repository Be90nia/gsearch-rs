VERDICT: PASS——6 单（pkp/o1p/1az/ago/d3u/7z0）全部修复，单测锁 + 真实命令 e2e 双验收；clippy --all-targets -- -D warnings 0 error；cargo test --all-targets 162 passed / 0 failed（101 lib + 61 bin）。

# 盲测五修复回执（FixB5，2026-10-09）

基线：0d1e98e（v0.2.9-16）；禁 commit，改动全部在工作树。

## pkp（P1 安全）：browse scheme 白名单 + 私网门

改动：src/fetch.rs（拆 `classify_url` 无文案判定层，`gate_check` 保留 fetch 文案包装）；src/general.rs（`browsable_scheme_ok` 四分支：空拒 / http(s):// 过 / 含 `://` 或危险词头拒 / 裸 host 补 https://；`ensure_browsable_url` = scheme + 私网门；BrowseOpts.allow_private；cmd_browse/cmd_login/cmd_dl 三入口接线 rc=2 快失败）；src/main.rs（Browse 子命令 `--allow-private` flag）。README/USER_GUIDE 写明门语义与 I9 rationale。

单测锁（general.rs）：`browsable_scheme_whitelist_table`（8 个拒绝样本 + 补全 + host:port 放行）、`browsable_private_gate_table`（6 私网 IP 默认拒 + `--allow-private` 放行 + 公网过 + file:// 无 flag 可绕 + 文案带 --allow-private）。

e2e：
- `browse file:///D:/gsbt5/secret.html` → rc=2 一行报错（0.17s 三场景合计，无 Chrome）
- `browse javascript:alert(1)` → rc=2；`browse data:text/html,x` → rc=2
- `browse http://192.168.89.249:8888/` → rc=2「browse 拒绝私网地址 192.168.89.249…请传 --allow-private」
- `browse http://192.168.89.249:8888/ --allow-private --full` → rc=0，Chrome 启动（stderr INFO），SearXNG 页正文渲染成功（366B JSON）

## o1p（P2）：错误语义三态

改动：src/search.rs（`SearxFail{HealthyEmpty, SourceError}` 分类；`searxng_collect`/`degrade_html`/`batch_one`/`run_batch` 分类化；`SearxngAttempt::FallbackGoogle(Option<真因>)/CircuitBroken(真因)/HealthyEmpty`；`empty_status(recency)` 纯函数；`FILTERED_EMPTY_MSG`/`NO_RESULTS_MSG`/`circuit_diag`）；src/types.rs（RunStatus +`filtered_empty`/`no_results`）；src/main.rs（空 query 前置拒 `first_blank_query` 两入口同拦 rc=2；run.message 与 stderr 诊断行同源同步；SearXNG HTTP 真因透传进诊断行）。

单测锁（search.rs/types.rs/main.rs）：`searx_fail_empty_status_maps_recency`（Some→filtered_empty / None→no_results / 与 degraded 互斥 / circuit_diag 真因透传）、`searx_fail_describe_keeps_reason_chain`、`blank_query_gate_detects_whitespace_only`、RunStatus 序列化表扩 2 变体。

e2e：
- `search ""` → rc=2 **27ms**（原 6560ms 白烧回退链）；`search "   "` → rc=2；batch 含空串 → rc=2 整批前置拒
- filtered_empty（真实实例 + `site:zzqxkcd089.invalid` + `--recency day`）→ rc=2，`run.status=filtered_empty`，message「recency 过滤后零结果（SearXNG 源健康，非基础设施故障）…」与 stderr 同步；走了 SearXNG→DDG(超时)→Google 直爬全链后正确定态，不再误标 degraded
- degraded（`GSEARCH_SEARXNG_URL=http://127.0.0.1:59999`）→ rc=2，`run.status=searxng_degraded`，message+stderr 同带真因「请求 SearXNG 失败: …: error sending request」
- batch：ok 条目 + `filtered_empty` 条目（带诊断 message，不再一律 error）✓；死端口 batch 全条目 error 带真因、格式「SearXNG 查询失败（…）…」不变 ✓
- 环境边界（如实记录）：`no_results`（无 recency + 源健康空）在本 SearXNG 实例无法端到端触发——该实例引擎对 gibberish/引号短语均做模糊兜底不愿返回空集；其定态逻辑与 filtered_empty 共享同一 Google 回退链（已实证）+ `empty_status(None)` 分支单测锁定

## 1az（P2）：similar run 信封

改动：src/main.rs cmd_similar——`run.status=Ok` + `run.message="searxng 派生查询命中 N 条（查询: …）"`（原 `RunStatusInfo::default()` = error，与 rc=0/results 非空打架）。

e2e：`similar https://tokio.rs --limit 2` → rc=0，`run.status=ok`，`run.message="searxng 派生查询命中 2 条（查询: site:tokio.rs）"`，2 条结果 + similarity 标注齐全。

## ago（P2）：文档契约 + meta 缺席语义

改动：src/types.rs（proxy/recency `skip_serializing_if = "skip_compact_absent_opt"`——未传键真缺席，compact 时仍全跳）；README 6 处；USER_GUIDE 全量重写过时段。

实测先行（禁臆造逐项）：`--version`→"gsearch 0.2.9 (git:v0.2.9-16-g0d1e98e)"；doctor 默认 JSON（checks/elapsed_ms/fail_count/warn_count）；browse 输出 366B 实证 proxy/recency 键缺席；score 公式按 duckduckgo.rs `(n-i)`（全量去重后打分，SERP 第 1 位=10.0 递减，limit 截断不改分——盲测实测 [10,9,8]）+ 注明跨 provider 量纲不可直比。

README：①`--humanize=false` → `--no-humanize` ②总契约补**人读豁免**（dl/update/login 人读文本，doctor 默认 JSON 除外，json.loads 不适用）③meta proxy/recency「未传时键真缺席，不输出 null」④score 公式改真实描述 ⑤run.status 枚举 + 三态语义段重写。
USER_GUIDE：①--version 0.2.9 ②2.1 默认 JSON（--human 人读）③2.2 --json 兼容占位 ④2.3 --read 默认 JSON + N≤limit ⑤6.2 `--no-humanize`（--humanize 不存在 rc=2）⑥8 doctor 默认 JSON ⑦10 退出码表 0/1/2/3/4/5 全量 ⑧3.1 browse scheme 门。
测试翻转（拍板契约变更）：`meta_proxy_none_serializes_as_null`（断言 `"proxy":null` 常驻）→ `meta_proxy_recency_none_keys_absent`（断言键缺席 + 传值出现）。

## d3u（P3）：--read 越界检查前移

改动：src/main.rs cmd_search 两道——①搜索前静态闸（`--read N > --limit` 必越界，rc=2 零网络零浏览器）②post 块内 ensure_search_browser 之前的运行时校验（N≤limit 但返回不足，保持 w9y read_error rc=1 契约，错误信息与 postproc 内层同款）。

e2e：`search "rust async" --limit 2 --read 999` → rc=2 **27ms**（盲测场景为白起 Chrome ~10s+）；运行时分支 `--read 999`（静态闸前版本）实测 rc=1 + 顶层 read_error + Chrome launched: False（w9y 契约保持）。

## 7z0（P3）：fetch 文案 + dl -o 穿越门

改动：src/fetch.rs（公网 http 拒绝文案去掉误导的 `--allow-private` hint，写真因「--allow-private 仅放行内网 http，对公网地址无效」）；src/general.rs（`has_parent_traversal`：-o/--output-file 相对路径含 `..` 段 → rc=2 拒绝；绝对路径显式放行）。

单测锁：`dl_output_parent_traversal_table`（../upone / a/../../b / .. 拒；sub/x、绝对路径、绝对含 .. 放行）。

e2e：
- `fetch http://example.com` → rc=1 新文案（无 --allow-private hint）；加 `--allow-private` 仍 rc=1 同文案（文案已说明对公网无效，不再是死路指引）
- `dl <URL> -o ../upone.bin`（CWD=D:/gsbt5）→ rc=2 拒绝，`D:/upone.bin` 确认未落盘
- `dl <URL> -o D:/gsbt5/abs_ok.md` → rc=0 mode: direct 落盘 9380B

## 全量闸

- `cargo clippy --all-targets -- -D warnings` → 0 error（修掉一处 collapsible_if）
- `cargo test --all-targets` → **101 passed + 61 passed / 0 failed**（2 ignored 为既有网络跳过项）

## side-effects 三态

- **预期契约变化**（拍板授权，文档同步）：①meta.proxy/recency 未传键缺席（原 null 常驻，消费方若断言键存在需改）②batch 源健康零结果条目 status=filtered_empty/no_results 而非 error ③空 query batch 整批前置拒绝（原跑完出 error 条目）④裸 host URL 经三入口被规范化为 https://（Chrome 原语义等价）⑤shell 路径 SearXNG 健康空 + Google 预检不通时报可读诊断而非空耗浏览器
- **无副作用**：fetch（仅文案）、verify、doctor、update、login 主行为零变化；dl 内网下载（私网 browser 回退）路径保留——dl/login 只加 scheme 白名单不设私网门
- **回归风险**：`no_results` 端到端未在本实例触发（环境限制见 o1p 节）；browse 补全分支对非常规裸输入（含中文等）按 https:// 补全，goto 失败会走既有 30s 超时路径，未逐一验证

## 教训沉淀

- `bd remember`：URL scheme 门「带 `:` 无 `://`」输入穿补全分支的坑（pkp 单测实锤抓出 javascript: 穿门）
- `~/.omp/agent/rules/common.md` 新增「URL scheme 白名单门（跨语言）」：四分支结构 + file:///x 与 file:x 两类形态测试表

## 评分（AI 消费者视角，修复后版本复测口径）

**总分：8 / 10**（10 - sum(cost)）

### deductions

```json
[
  {
    "point": "browse 不接受 --no-humanize，从 search 迁移来的调用习惯直接 rc=2，clap tip 也不指向真语义（browse 本就无 warmup，无等价 flag）",
    "cost": 1,
    "why": "pkp 验收第 5 步真实撞上：agent 在 search 学到『高频调用加 --no-humanize』，同一会话内对 browse 复用该习惯必吃 rc=2，多花一轮重跑并需要翻 --help 才能确认 browse 本来就不做 warmup",
    "cmd": "target/debug/gsearch.exe browse \"http://192.168.89.249:8888/\" --allow-private --no-humanize  → error: unexpected argument '--no-humanize' found"
  },
  {
    "point": "browse 输出 meta.provider 硬编码 \"google\"，与实际渲染目标无关，provider 字段对 browse 无信息量且误导分流",
    "cost": 1,
    "why": "browse 渲染的是自建 SearXNG 内网页（非任何搜索引擎），stdout 却标 provider=google——AI 若按 provider 做策略分流（如『google 来源需防撞码/结果按 google 量纲理解』）会拿到错误信号；实测 366B 输出中该键与 facts 相悖",
    "cmd": "target/debug/gsearch.exe browse \"http://192.168.89.249:8888/\" --allow-private --full → {\"meta\":{...\"provider\":\"google\"...},\"content_text\":\"…SearXNG…\"}"
  }
]
```

### highlights（爽点，均有命令证据）

```json
[
  {
    "point": "空 query 前置拒绝：27ms 一行报错，替代原 6.5s 的 6 条 stderr 白烧回退链",
    "cmd": "target/debug/gsearch.exe search \"\" → rc=2, elapsed_ms=27, error: query 不能为空或纯空白"
  },
  {
    "point": "browse 私网门放行路径干净：rc=0，stdout 366B 纯净 JSON（stderr 仅 1 行 Chrome INFO），渲染正文完整",
    "cmd": "browse \"http://192.168.89.249:8888/\" --allow-private --full → rc=0, stdout=366B, stderr=115B"
  },
  {
    "point": "dl 直链快路径 + 绝对路径落盘：0.63s 完成，输出三行（mode/落盘路径/字节数）无噪声；穿越拒绝时指引可操作（『确需外部路径请用绝对路径』）",
    "cmd": "dl https://raw.githubusercontent.com/.../README.md -o D:/gsbt5/abs_ok.md → rc=0, mode: direct, 9380 bytes"
  },
  {
    "point": "similar 信封三信号一致：rc=0 + status=ok + message 摘要同帧到达，AI 无需交叉验证",
    "cmd": "similar https://tokio.rs --limit 2 → rc=0, run.status=ok, run.message=\"searxng 派生查询命中 2 条（查询: site:tokio.rs）\""
  },
  {
    "point": "filtered_empty 定态的 message 直接给行动建议（去掉 --recency/换窗/换词），AI 无需二次推断决策路径",
    "cmd": "search \"site:zzqxkcd089.invalid\" --recency day → rc=2, run.status=filtered_empty, message=「recency 过滤后零结果（SearXNG 源健康…）；建议去掉 --recency…」"
  }
]
```

### notes（不扣分的观察）

- doctor 默认 rc=1（network 检查直连 google:443 不吃 GSEARCH_PROXY，fail_count=1）——盲测五已裁定「观察不立案」，本轮实测 `gsearch doctor` 复现同形态；AI 需解析 JSON checks 才能区分「环境断网」与「工具故障」
- 本 SearXNG 实例对 gibberish/引号短语做模糊兜底（`search "qxkcd089zzq731" --recency day` → 3 条 VIN decoder/优惠券级无关结果，status=ok）——搜索引擎侧行为，gsearch 忠实透传无相关性信号可用，造「真零结果」测试场景需 site: 语法
