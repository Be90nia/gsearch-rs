# 盲测十三报告（2026-10-09）

## 实验设计
- 目的：验证 FixG13（pre/code 空白保真 + include 覆盖回退 nav 剥离 + --json-keys 数组索引）
- 受试者：SubjectS / SubjectT / SubjectU（三个全新 spawn worker）
- 工具：`D:/Project/gsearch-rs/target/debug/gsearch.exe`（main=c84f7cb）
- 协议：同盲测六-十二

## 成绩

| 受试者 | UX | Token | vs 盲测十二 | 关键发现 |
|---|---|---|---|---|
| S (tokio 版本调研) | 8.5 | 8.5 | P 9/9.5: -0.5/-1.0 | **行内 code 邻接空格被吞**（mpscPermitbefore/validateMAX_PERMITSin 错词）；crates.io 响应 auto_include_applied 缺席与"恒在"契约歧义 |
| T (GitHub PR 阅读) | 8.5 | 9.5 | Q 9/9: -0.5/+0.5 | **--json-keys 多索引同名字段静默 last-write-wins 覆盖**（items.0.title + items.1.title 只剩最后一条，数据丢失型误导）；github_comments_hint/标题前缀/数组投影全 PASS |
| U (serde_json 文档) | 7.5 | 8.5 | R 9/9: -1.5/-0.5 | **字节级 diff 最严苛受试者**：pre 块本体逐字节一致 ✓ 但签名 `Result<T>`→`where` 间 1 换行丢 + 行内 code 吞空格 4 处 + UI chrome 20B 粘入 |
| **均值** | **8.17** | **8.83** | **-0.83 / -0.34** | — |

## 轨迹（八轮盲测）

| 轮 | UX 均值 | Token 均值 |
|---|---|---|
| 六 | 7.0 | 7.33 |
| 七 | 7.67 | 8.27 |
| 八 | 7.5 | 8.23 |
| 九 | 8.17 | 8.67 |
| 十 | 8.17 | 8.17 |
| 十一 | ~8.7 | ~8.83 |
| 十二 | 9.0 | 9.17 |
| **十三** | **8.17** | **8.83** |

## 根因分析（FixG13 边界回归）
FixG13 修 pre/code 保真时，sep 注入落点在哨兵段外的 prose 段——collapse_blank 去首尾把它们 trim 掉：
1. **行内 code**：前置 ' ' 是前 prose 段尾→trim；后置 ' ' 是后 prose 段首→trim → "byT" 错词
2. **pre 块**：后置 '\n' 是后 prose 段首→trim → "Result<T>where" 换行丢
3. **--json-keys 冲突**：project_json_paths 的 Map 同名 key last-write-wins（既有 bug，FixG13 加数组索引后暴露频率上升）

修复中（FixG14）：块级 pre 边界 '\n' 移进哨兵段内；行内 code 检查 out 末尾已有空白则不注入、后置依赖下一节点自带边界；--json-keys 冲突 key 全路径化。

## 关键教训
- **FixG13 的 P0 e2e 漏测了行内 code 场景**——P0-1 只验了 pre 块内缩进，没验 prose 里行内 code 的边界。边界 case 单测要覆盖哨兵段与 prose 段的**接缝**，不只段内。
- **受试者严苛度差异大**：U 做了 curl 字节级 diff 对照（发现 2 个 S 只发现一半的 bug），R 十二轮没做。"签名逐字符精确"验收硬标准下，受试者会自带 ground-truth 验证——工具必须在无对照情况下也正确。
- **均值回落不代表工具退步**：FixG13 修的 pre 块内保真被 U 确认"逐字节一致"；跌分全部来自新暴露的边界 bug（部分是 FixG13 引入，部分既有但更易撞）。

## 结论
FixG13 主体生效（pre 块保真实证）但引入 2 个边界回归 + 1 个既有 bug 暴露 → FixG14（已派单，根因+修法已定位）。
