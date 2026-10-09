VERDICT: PASS

# gsearch-rs v0.2.9 独立第三方杠精盲测回执（BlindTest5）

- 日期：2026-10-09；对象：`target/debug/gsearch.exe`（git:v0.2.9-14-gb86cdf4）
- 方法：黑盒（`--help` + 实跑；未读 src/）。公开契约 = README.md + USER_GUIDE.md + clap help。
- 环境：`GSEARCH_SEARXNG_URL=http://192.168.89.249:8888`（实测健康，doctor probe results=32）；代理 `http://127.0.0.1:10808`（按场景开关）；CWD=D:/Project/gsearch-rs（自带 gsearch.json: searxng_url 同实例）。
- 评分（AI 消费者视角）：**7 / 10**

## 分级统计

| 级别 | 数量 |
|---|---|
| P1（真卡点） | 1 |
| P2 | 6 |
| P3 / 观察 | 7 |
| 通过项 | 14 |

---

## P1

### P1-1 browse 无 scheme/私网门：`file://` 本地文件内容完整读入 stdout；私网照渲染（与 fetch 的 SSRF 门不对称）
- 复现：
  - `printf '<html>...TOPSECRET-BT5...marker-12345...' > D:/gsbt5/secret.html`
  - `gsearch browse "file:///D:/gsbt5/secret.html"` → **rc=0**，stdout JSON `summary_paragraphs` 含 `TOPSECRET-BT5` 与 marker 全文（实测实锤）。
  - `gsearch browse "file:///C:/Windows/win.ini"` → rc=0（系统文件照样导航）。
  - `gsearch browse "http://192.168.89.249:8888/"` → rc=0，SearXNG 内网页面完整渲染（标题 "SearXNG"）。
- 对照：`fetch` 对同一私网地址 rc=1 拒绝（私网门 + https 门 + metadata 门齐全）。
- 杠精裁定：**P1**。工具自身威胁模型（`meta.content_untrusted`、agent 消费 LLM 搜索结果里的 URL）成立于 fetch，却在能力更强、带 profile cookie 的 browse 上完全不设防。二次注入链完整：恶意页面 → agent 被诱导 `browse file:///C:/Users/<u>/.ssh/...` → 本地文件内容进上下文外泄。建议 browse 加 scheme 白名单（http/https）+ 私网/loopback 门（复用 fetch 的 `--allow-private` 语义），或 README 显式声明风险边界。

## P2

### P2-1 README 自带示例 `--humanize=false` 硬错，错误 tip 还指错方向
- 复现：`gsearch search "rust async" --humanize=false` → rc=2 `error: unexpected argument '--humanize' found` + `tip: a similar argument exists: '--human'`。
- 杠精裁定：**P2**。README「用法」节原文示例零上下文可复制，抄即翻车；且 tip 指向的 `--human` 是"切人读文本"，语义与意图（跳过 warmup）完全不同，实际 flag 是 `--no-humanize`。agent 顺着 tip 会把输出格式改掉而 warmup 照跑。

### P2-2 USER_GUIDE.md 全面过时，多处与 README/实况直接矛盾
- 证据（对照实跑）：
  - USER_GUIDE 2.2「`--json` 给下游」+ 缩进 JSON 示例 ↔ 实况：默认已单行紧凑 JSON，`--json` 是隐藏 no-op。
  - USER_GUIDE 6.2 `gsearch --humanize search ...` + 「`--humanize` 默认 off」↔ 实况：默认 on，flag 不存在（同 P2-1 必 rc=2），README 说默认启用。
  - USER_GUIDE 8 doctor 默认人读表 ↔ 实况：默认结构化 JSON（`--human` 才是人读表）。
  - USER_GUIDE 10 退出码 3 行 ↔ README 6 行（缺 3/4/5 与 batch 语义）。
  - USER_GUIDE 2.1 默认输出人读文本 ↔ 实况默认 JSON。
- 杠精裁定：**P2**。任务书明言两份文档都是 agent 会读的对外契约；读 USER_GUIDE 学到的是三处必错用法。

### P2-3 单查询空串/纯空白 query：无前置校验 → 6–11s 白烧回退链 + 误诊"基础设施降级"（batch 模式暴露真相是 SearXNG 400）
- 复现：
  - `gsearch search "" --limit 3` → 6560ms，rc=2，`run.status="searxng_degraded"`，message「SearXNG 零结果已熔断（基础设施降级，非查询无资料）」；`search "   "` 同样（11142ms）。
  - 同一空串进 batch：`gsearch search "rust async" "" --limit 2` → 该条 `status="error"`，message 实锤 **「SearXNG 返回错误状态 … q=&…: HTTP 400 Bad Request」**，rc=1。
- 杠精裁定：**P2**。两个错误：(a) 客户端可知的非法输入（空 query）不前置拒绝，白烧 6–11s 网络；(b) 单查模式把 SearXNG 400（我们发了个坏请求）吞成"零结果→基础设施降级"，agent 按语义会去跑 doctor/换源，而真相是"你传了空串"。batch 自己都把真因报出来了，单查却在撒谎。

### P2-4 recency 过滤零结果被误诊为基础设施降级（直连 API 实证非故障）
- 复现：
  - `gsearch search "rust release notes" --recency week --limit 3` → rc=2，`run.status="searxng_degraded"`「基础设施降级，非查询无资料」。
  - 直连对照：`curl 'http://192.168.89.249:8888/search?q=rust+release+notes&format=json'` → 18 条；同 URL 加 `&time_range=week` → **0 条**（实例正常，真没新鲜结果）。
- 杠精裁定：**P2**。「过滤后无结果」与「源挂了」是两种决策（前者该去掉过滤/换词，后者才该 doctor/换源）；现语义把前者硬标成后者，agent 被误导进排障死胡同。

### P2-5 similar 快乐路径信封自相矛盾：rc=0 + 有结果 + `run.status="error"`（3/3 复现）
- 复现：`gsearch similar "https://docs.rs/serde" --limit 3` → rc=0，3 条结果（similarity/score 齐全，`meta.query="serde"` 派生正确），但 `run:{"status":"error"}`。换 `https://tokio.rs` 同样（query=`site:tokio.rs`，status=error）。三连测 3/3。
- 杠精裁定：**P2**。同一信封三个信号互相打架：信 status → 假失败；信 rc → 假成功；信 results → 才是对的。任何 `if status=="ok"` 的 agent 判据必翻车。

### P2-6 dl / update 违反「所有顶层命令默认输出单行紧凑 JSON」总契约
- 复现：
  - `gsearch dl <raw.githubusercontent 直链> -o out.bin` → stdout = `mode: direct\n已下载: D:\\... (23126 bytes)\n` 两行**人读文本**，非 JSON。
  - `gsearch update` → stdout = `查询 GitHub latest release…\n已是最新（本地 v0.2.9，远端 v0.2.9）` 人读中文。
- 杠精裁定：**P2**。README 3gw 总纲写"所有顶层命令默认输出单行紧凑 JSON"，`json.loads(stdout)` 是 agent 对该承诺的自然消费方式，对 dl/update 直接抛异常。要么补 JSON 信封，要么 README 把这两条显式豁免。

## P3 / 观察

### P3-1 meta 缺席语义违约：`"recency":null` / `"proxy":null` 键常驻
- README：「`meta.recency` 回显本次过滤值（未传时键缺席）」；实测未传时输出 `"recency":null`（单查与 batch 均是）。`proxy:null`、`humanize:true`、`tool/version/profile` 同为对 agent 零决策价值的常驻载荷（~60B/响应）。`--compact-meta` 键清单在 help（含 truncated 不含 limit）与 README（含 limit 不含 truncated）间还互相对不上。建议：缺席语义按自家总纲执行到底。

### P3-2 DDG 等价分公式与文档不符
- README：「DDG html 用返回序等价分（首条 = n 递减到 1）」；实测（死 searxng + 代理 → provider=duckduckgo，limit 3）score = `[10.0, 9.0, 8.0]`，非 `[3.0,2.0,1.0]`——是 SERP 页位置分（10-k）。功能无害，文档精确承诺是错的。

### P3-3 `--read N` 越界：契约行为对，但白起一次 Chrome 才报错
- `gsearch search "rust async" --limit 2 --read 5` → rc=1 + 顶层 `read_error`（✓契约），但 stderr 先出现 `INFO 使用浏览器: Chrome -> ...` 再 `postproc 失败: --read 5 越界（结果数 2）`。结果数在搜索完成时已知，越界是静态可判的，检查应前移到浏览器 launch 之前。

### P3-4 `browse javascript:alert(1)` 启动完整 Chrome 等 CDP 超时
- rc=1，报「goto javascript:alert(1) 失败: Request timed out.」——非 scheme 快失败，报错文案还误导（像是网络问题）。无安全洞（无法导航），纯浪费 + 文案 nit。

### P3-5 fetch 公网 http 拒绝文案推荐一条死路
- `fetch http://example.com` → rc=1 文案「如确需内网 http 页面，请传 --allow-private…」。实测 `fetch http://example.com --allow-private` → **照样 rc=1 同文案**。把公网 host 误标成"内网场景"并推荐一个实测无效的 flag，agent 会白绕一圈。（https 门本身工作正常，是文案缺陷。）

### P3-6 dl `-o` 相对路径可静默穿越出 CWD【观察】
- `cd D:/gsbt5 && gsearch dl <URL> -o ../upone.bin` → rc=0，文件落 `D:/upone.bin`（CWD 之外），零警告。黑盒未见对 `-o` 的 `..`/分隔符 sanitize（与 v0.2.8 曾宣称的文件名安全门印象不符——黑盒无法核实内部实现，仅报观测事实）。消费方是 agent 时，`-o` 值可能来自上下文污染，建议同 fetch 私网门思路加路径门或至少 stderr 提示落点。

### P3-7 doctor 网络检查直连 TCP google:443 不吃 GSEARCH_PROXY【观察】
- 强制代理环境下 `doctor` 的 network 检查恒 FAIL → exit 1（本次实测 fail_count=1 即此因），削弱「撞码先跑 doctor」的诊断价值：agent 分不清"环境真断网"和"只是没代理"。行为与 README 描述一致（文档即如此设计），属设计层观察。

## 通过项（杠精也挑不出毛病的）

1. 单查默认单行紧凑 JSON，stdout 纯净（searxng 命中 stderr=0 字节）✓
2. `--json` 隐藏 no-op，结构级与默认输出一致 ✓
3. `--human` 人读文本可切回 ✓
4. 退出码表实测对上：0（成功/update）/1（read_error、batch 部分、fetch 门、doctor FAIL）/2（参数错、degraded、similar 非 URL、verify 404）/4（DNS）/5（超时）；3（captcha_timeout）本次环境未触发，未验证 ✓（4/6 实测）
5. batch：3 查 3.0s 并发、rc=0/1/2 正确、单条失败不阻塞、空串条目 error 携带真因（400）、stderr 一行汇总 ✓
6. `--envelope v2`：顶层批统计 `{n_total,n_ok,n_fail,elapsed_ms}`、元素不带 meta，字节 -15%（3135 vs 3679）✓
7. DDG 第二源实战可用：死 searxng + 代理 → `provider=duckduckgo`、stderr 一行接管提示、fallback 不再 degraded ✓
8. 配置优先级 env > gsearch.json 实锤：死端口 env 压过健康 config（degraded）；空 env 视为未设回退 config（ok）✓
9. fetch 私网门（RFC1918/loopback 拒 + 可行动提示）、公网 https 强制（`--allow-private` 不可绕）、`--markdown`（meta.format 标注）✓
10. verify 单/批 rc 语义 + verdict 分类（ok/http_error/dns_error/timeout）全对，`final_url` 可回溯 ✓
11. similar 非 URL 输入**发起搜索前**拒绝（stderr 明确 + rc=2），不产垃圾派生查询 ✓
12. 参数护栏：`--limit 0/101` 拒（1..=100）、`--read 0` 拒、`--snippet-len 0` 拒（1..=100000）✓
13. 极端输入健壮：emoji query rc=0、1KB query rc=0、101 上限拒绝 ✓
14. update：rc=0、版本比对正确、不做自替换 ✓；doctor 7 项检查含 SearXNG probe（results=32）✓

## Token 账

- batch(3) 默认 3679B vs 3×单查 3650B：**默认 batch 不省字节**（每条目重复 11 键 meta），省的是延迟（3.0s vs ~6s+）。省字节要显式 `--envelope v2`（-15%）。README 未承诺字节节省 → 观察，建议文档明示或让 v2/compact-meta 组合更可发现。
- meta 全量键中 `tool/version/profile/proxy/humanize` 对 agent 决策价值≈0（详见 P3-1）。

## 未覆盖 / 环境不可触发

- exit 3（captcha_timeout）：本环境 SearXNG + 养熟 profile 未触发 CAPTCHA。
- CAPTCHA 亲解链路（swap_to_headed）、login 人工流程、shell 交互模式：需真人/交互，不在本轮黑盒脚本范围。
- Google 直爬回退路径（SERP 解析正确性）：出口对 google:443 直连超时，仅验证了 TCP 预检熔断路径。

## 结论

核心引擎（search JSON 契约、退出码语义、batch、DDG 回退、fetch/verify 护栏、配置优先级）扎实可靠，护栏密度在同类 CLI 里属高水准。失分集中在：**文档契约层漂移**（README 示例错、USER_GUIDE 系统性过时、meta 缺席语义违约、score 公式错）与**错误语义误导**（空 query / recency 零结果被标"基础设施降级"、similar 快乐路径 status=error），外加 browse 对 file:// 完全不设防的一处 P1。修复 P1 + 两处状态误诊后，AI 消费者视角可上 8.5+。
