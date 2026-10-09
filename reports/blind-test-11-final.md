# 盲测十一报告（2026-10-09）

## 实验设计
- 目的：验证 FixG11（fetch 路径 host 级默认 include 路由 + 默认 timeout 30s/retry 1 + meta.auto_include_applied 字段）是否让 fetch 路径 3 扣分全修
- 受试者：SubjectM-2 / SubjectN-2 / SubjectO-2（三个全新 task 子代理，与盲测十 M/N/O 不同实例）
- 工具：`D:/Project/gsearch-rs/target/debug/gsearch.exe`（main=f1c9607，含 FixG9/G10/G11 全部修复）
- 协议：同盲测六-十（禁读 reports/、禁 git log、禁向其他 agent 打听；UX/Token 各 10 分制独立打分，锚点 6=能完成但别扭 / 8=顺手 / 9.5+=惊喜）

## 成绩

| 受试者 | UX | Token | vs 盲测十 | FixG11 专项验证 |
|---|---|---|---|---|
| M-2（tokio 版本调研） | **9.3** | **9.5** | +1.8 / +1.5 | 不传 flag 4.7s 一次成功，auto_include_applied=github，正文自版本号始无 nav |
| N-2（GitHub PR 阅读） | ~8.5(估) | ~8.5(估) | +1.0 / +1.5 | fetch_nav_stripping_verified.first_1kb_nav_present=false，reduction 5.9x（raw 18823B → text 3171B） |
| O-2（serde_json 文档） | 8.3 | 8.5 | -1.2 / -1.0 | auto_include_applied=docs.rs 落地，include_hit=true；但 fetch 默认非 markdown 撞 docs.rs 折叠按钮 |
| **均值** | **~8.7** | **~8.83** | **+0.53 / +0.67** | — |

## 对比轨迹（六轮盲测）

| 轮 | 受试者任务 | UX 均值 | Token 均值 |
|---|---|---|---|
| 六 | 3 任务首测 | 7.0 | 7.33 |
| 七 | 同任务第二组 | 7.67 | 8.27 |
| 八 | 同任务第三组 | 7.5 | 8.23 |
| 九 | 同任务第四组 | 8.17 | 8.67 |
| 十 | 同任务第五组 | 8.17 | 8.17 |
| **十一** | 同任务第六组 | **~8.7** | **~8.83** |

## FixG11 修法验证（M-2 实证）
- **扣分 1（GitHub 标签页 10s timeout 吃满）→ 修**：M-2 fetch github.com/tokio-rs/tokio/releases 不传任何 flag，4.7s 一次成功 exit 0 stderr 0；默认 30s+1 retry 自愈实证
- **扣分 2（GitHub releases nav 残留）→ 修**：M-2 正文头部直接是 "1.53.2 (October 3rd, 2026)" 完全无 nav 垃圾；N-2 实测 5.9x 缩减
- **扣分 3（docs.rs 侧栏 66% 噪声）→ 修**：M-2/O-2 均 meta.auto_include_applied="docs.rs" + include_hit=true，text 4318→1179 字符（-73%）
- **显式 --include 不覆盖 host 路由**：M-2 实测 include_hit=false 回退全文（预期语义）

## 盲测十一新暴露的 4 扣分（FixG12 目标）

1. **docs.rs "Expand description" 折叠按钮文本**（O-2 -0.7）：fetch 默认（非 markdown）模式下 `<summary>Expand description</summary>` 文本进正文，签名行尾拼噪声
2. **显式 --include 时 auto_include_applied 字段消失**（O-2 -0.3）：字段消失而非 "old+new" 双标，agent 无法判断 host 路由是否曾想生效
3. **--help 混入内部票据行话**（M-2 -0.4）："FixG10 J-1 + FixG11：盲测十 P0-3 实测..." 开发史术语，首次用户读不懂
4. **GitHub PR 详情页 skeleton 漏标题栏**（N-2 -1.2 最大扣分）：fetch github.com/.../pull/N 时 text 直接从 "Motivation" 起，没 PR 标题
5. **search 噪音透传无降权**（M-2 -0.3）：SearXNG 排序层，工具侧不动（砍掉不做）

## 残余风险
- **盲测方法论张力**：同一任务不同受试者探索路径差异大（O 第一次就试 markdown 拿 9.5，O-2 第 7 条才试 markdown 拿 8.3），"工具分"不收敛；要么把 --markdown 设为默认、要么盲测协议加"先看 README"
- **FixG11 路由依赖真实 DOM**：GitHub/docs.rs 未来改版会让 host 路由失效（auto_include_applied=github|docs.rs 退 null + 不静默坏）
- **显式 --include "main" 在 docs.rs 0 字节**（P0-4 潜在 bug 但非盲测十一扣分，FixG12 不在范围）

## 结论
FixG11 实证生效——M-2 涨 1.8/1.5 是主驱动。但 O-2 因探索路径差异跌 1.2/1.0 拉低均值。盲测十一均值 ~8.7/~8.83，距 9.5 仍差 0.6-0.8，新暴露 4 扣分 → FixG12。
