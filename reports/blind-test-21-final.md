# 盲测二十一报告（2026-10-10）

## 实验设计
- 目的：验证 FixG20（eae18ff：fetch 单URL status 键 / dup_paragraphs 重复标注 / truncated 双语义文档）
- 受试者：SubjectQQ / SubjectRR / SubjectSS（三个全新 spawn worker）
- 工具：main=eae18ff

## 成绩

| 受试者 | UX | Token | vs 盲测二十 | FixG20 专项验证 |
|---|---|---|---|---|
| QQ (tokio 版本) | **9.3** | 9.5 | KK +0.3/0 | help "单/多 URL status 语义一眼可懂"；NN 的 KeyError 坑未复现，残留仅"形状分支" -0.2（从 -0.5 降级） |
| RR (GitHub issue) | 9 | 9 | OO -0.2/-0.5 | status 键确认生效；dup 场景未触发（读的 issue 引用少）|
| SS (serde_json 文档) | 9 | 8.5 | MM 0/-1.0 | 三源交叉验证（--raw 23128 字符 + vendored 源码逐字比对 + 签名粘贴编译）——正确性六连零 bug |
| **均值** | **9.1** | **9.0** | -0.03/-0.5 | — |

## FixG20 三项验证
1. **status 键统一**：QQ 9.3 历史新高 + "一眼可懂"；KeyError 坑降级为形状分支（-0.5→-0.2）
2. **dup_paragraphs**：本轮未复现触发场景（标注逻辑有独立单测兜底）
3. **truncated 文档消歧**：SS 残留 -0.25 心智成本（文档消歧的天花板）

## 新发现（工具侧可修）
- **`--include "#errors"` 容器命中只返回标题文本，段落正文不可达**（SS -0.5）：include selector 对容器元素只取直接文本不含后代段落，SS 被迫 --raw 24.5KB 兜底只为 400 字符 Errors 正文（30 倍字节差）——include 提取器真实语义边界，候选 FixG21

## 16 轮总轨迹
六 7.0/7.33 → … → 十五 9.0/9.0 → 十六 8.67/9.0 → 十七 8.5/8.83 → 十八 8.67/9.0 → 十九 9.17/9.33 → 二十 9.13/**9.50** → **二十一 9.1/9.0**

单点纪录：UX 9.3（QQ，历史新高）> 9.2×2（NN/OO）；Token 9.5 × 8 人次。

## 扣分结构（本轮 10 条）
- 上游 SearXNG 噪声/措辞残留：4 条（-0.2~-0.25 级）
- 冻结设计语义：3 条（[*] 并列数组 / batch 单 keys / read 双语义，均 ≤0.3）
- 新工具缺陷：1 条（include 容器语义，-0.5）
- 设计边界：2 条（per-URL 投影 / 逐元素截断，feature 请求级）
