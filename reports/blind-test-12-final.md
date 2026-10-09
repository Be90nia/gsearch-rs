# 盲测十二报告（2026-10-09）

## 实验设计
- 目的：验证 FixG12（docs.rs summary 剥除 + auto_include_applied 恒在+overridden 字段 + --help 行话清理+守门测试升级 + GitHub PR 标题栏前缀）是否让盲测十一 4 扣分全修
- 受试者：SubjectP / SubjectQ / SubjectR（三个全新 spawn worker，与盲测十一 M-2/N-2/O-2 不同实例）
- 工具：`D:/Project/gsearch-rs/target/debug/gsearch.exe`（main=29862a4，含 FixG9/G10/G11/G12 全部修复）
- 协议：同盲测六-十一（禁读 reports/、禁 git log、禁向其他 agent 打听；UX/Token 各 10 分制独立打分）

## 成绩

| 受试者 | UX | Token | vs 盲测十一 | FixG12 专项验证 |
|---|---|---|---|---|
| P (tokio 版本调研) | 9 | 9.5 | M-2: -0.3 UX / 持平 | 不传 flag 1.9s 一次成功，标题前缀 ✓，13 个 nav 串 0 命中 ✓，help grep 零代号 ✓ |
| Q (GitHub PR 阅读) | 9 | 9 | N-2: +0.5 / +0.5 | PR 7170 1.7KB 含标题前缀+正文+review，auto_include_applied 恒在 ✓，github_comments_hint 诚实 ✓ |
| R (serde_json 文档) | 9 | 9 | O-2: +0.7 / +0.5 | docs.rs 966 字符纯正文，无 "Expand description" ✓，侧栏全剥 ✓，include_overridden_by_user 语义 ✓ |
| **均值** | **9.0** | **9.17** | **+0.30 / +0.34** | — |

## 对比轨迹（七轮盲测）

| 轮 | 受试者 | UX 均值 | Token 均值 |
|---|---|---|---|
| 六 | M/N/O | 7.0 | 7.33 |
| 七 | M/N/O 第二组 | 7.67 | 8.27 |
| 八 | M/N/O 第三组 | 7.5 | 8.23 |
| 九 | M/N/O 第四组 | 8.17 | 8.67 |
| 十 | M/N/O 第五组 | 8.17 | 8.17 |
| 十一 | M-2/N-2/O-2 | ~8.7 | ~8.83 |
| **十二** | **P/Q/R** | **9.0** | **9.17** |

## FixG12 修法验证（4 项实证）
1. **summary 剥除**：R 实测 docs.rs text 无 "Expand description"，签名干净
2. **auto_include_applied 恒在 + include_overridden_by_user**：P/Q/R 三受试者均验 overridden=true 时 label 恒在
3. **--help 行话清理**：P/Q/R 三受试者 grep FixG\d/盲测/[JKLM]-\d/P0-\d 零命中
4. **GitHub PR 标题栏前缀**：Q 实测 fetch PR 7170 text 头部含标题，无 "Motivation 起手无标题"

## 盲测十二新暴露的 3 扣分（FixG13 目标）
1. **markdown/text 转换器剥代码缩进**（R -1 Token 最大）：docs.rs `fn.from_str` where 行 4 空格缩进剥掉 + "from_ str" / "println! (" 游离空格；签名逐字符要求不过关
2. **用户 --include 覆盖路径无 nav 剥离**（P -0.5）：覆盖时跳过 host 路由，回退全文含 "Skip to content/Navigation Menu"
3. **--json-keys 不支持数组索引**（Q -0.5）：GitHub comments API 数组响应 `--json-keys "0.user.login"` 失败，4.4KB 重发

## 残余风险
- **markdown 折损是结构性限制**：R 扣的 -1 Token 是 scraper tree_text + collapse_blank 对 `<pre>`/`<code>` 的压缩行为，FixG13 修法是 pre/code 内文本保真，但要防止普通文本回归
- **search 噪音是 SearXNG 上游**：Q 遇 Zalo 广告、R 遇 CMMS-Singapore——非工具层
- **9.5 均值可达性**：修完 3 扣分理论均值 9.4-9.5，剩余缺口大半是 SearXNG 上游噪音 + markdown 结构性折损

## 结论
FixG12 实证生效——P/Q/R 均涨至 9+/9+，均值 9.0/9.17 比盲测十一涨 0.30/0.34，比盲测十涨 0.83/1.0。距 9.5 均值仍差 0.5/0.33，新暴露 3 扣分 → FixG13。
