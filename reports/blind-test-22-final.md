# 盲测二十二报告（2026-10-10）——FixG21 终验

## 实验设计
- 目的：验证 FixG21（2e2f5e0：--include selector 语义边界文档化——rustdoc 平铺结构 #errors 为 h2 id 需用 #errors ~ p 或父容器）
- 受试者：SubjectTT / SubjectUU / SubjectVV（三个全新 spawn worker）
- 工具：main=2e2f5e0

## 成绩

| 受试者 | UX | Token | vs 盲测二十一 | FixG21 专项验证 |
|---|---|---|---|---|
| TT (tokio 版本) | 8.5 | 9 | QQ -0.8/-0.5 | 投影配方"接近 9.5 惊喜线"记 delight；新发现 batch×投影文档缺口（已补，7474c9f） |
| UU (GitHub issue) | **9.5** | **9.5** | RR **+0.5/+0.5** | **双满分**（史上第三次，AA 之后）；投影两次首试命中零试错 |
| VV (serde_json 文档) | 9 | 9 | SS 0/+0.5 | **同场景零扣分记 delight**："--include 帮助直接写明 docs.rs DOM 结构……最难一环零试错"+ "工具把 rustdoc DOM 语义写进帮助文档，属未预期收获" |
| **均值** | **9.0** | **9.17** | -0.1/+0.17 | — |

## FixG21 验证
SS（二十一轮）-0.5 坑（--include "#errors" 只返回标题）→ VV（二十二轮）同场景零扣分 + delight。文档消歧根治确认：
- VV 实测 `#errors ~ p` "Errors 段唯一 <p> 完整取回(include_hits=1 = 无第二段遗漏)"
- VV friction 反向印证："若猜错将回退全文提取(50000B cap)烧大量 token"——文档 Precisely 教会了正确姿势

## 立即收口
`7474c9f`：TT 新发现的 batch×--json-keys 交互文档缺口（两模式投影形态不一致未文档化）PM 亲修两句话（fetch about 补"batch 元素 text=逐元素对象，与单 URL [*] 并列数组形态区分"）。259 tests 全绿。

## 17 轮总轨迹
六 7.0/7.33 → 十二 9.0/9.17 → 十五 9.0/9.0 → 十九 9.17/9.33 → 二十 9.13/9.50 → 二十一 9.1/9.0 → **二十二 9.0/9.17**

## 单点纪录终态
- UX：9.5 双满分（UU/AA）> 9.3（QQ）> 9.2×2（NN/OO）
- Token：9.5 × 9 人次（UU/AA/TT外的Y/HH/KK/MM/NN/OO）

## 剩余扣分结构（本轮 7 条）
- read 双语义认知税：3 条（-0.25~-0.33 级，命名固有，help 已自辩）
- 全局选项 help 重复（VV -0.33，clap global args 恒显示，框架行为）
- 上游噪声/措辞残留：2 条
- 上游格式（CRLF）：1 条
- **工具硬缺陷：0 条**

## 终态判定
17 轮 51 名受试者，FixG8-G21 十四轮修复/立项 45+ 项。工具硬缺陷连续 4 轮零新增（含本轮 batch 文档缺口系文档级且即时修）。均值 9.0-9.33 强平台，双 9.5 × 3 人次。剩余全部为：命名认知税（read）、框架行为（clap global）、上游噪声——无工具侧可修项。
