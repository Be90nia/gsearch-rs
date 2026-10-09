# gsearch-rs 盲测五·杠精轮——独立评审终版

**流程**：应用户要求，本轮改独立第三方盲测（e2e-runner 黑盒，18m57s，14 场景组实跑），PM 不参与裁定，只做抽验（P1-1、P2-5 双实锤）与交叉对照。盲测者=修复者=裁定者的利益冲突已纠正。

## 总分：7/10（独立评审裁定，PM 采信）

核心引擎扎实（退出码 0/1/2/4/5 实测全对、JSON 契约、batch、DDG 回退、fetch/verify 护栏、配置优先级）；失分集中在**文档契约漂移**与**错误语义误导**——README 是 agent 唯一契约源，漂移=契约破。

> 对照：盲测四 PM 自评 9.2 —— 独立杠精视角 7.0。差距即"自测自裁"的乐观偏差。

## 交叉对照矩阵

| 发现 | PM 侧 | 独立评审 | 裁定 |
|---|---|---|---|
| browse file:// 本地文件进 stdout | ✓ 起了 Chrome（未验内容） | **P1**：HTML canary TOPSECRET 全文进 stdout，PM 按其姿势重放 grep=1 实锤 | **P1 成立**（pkp） |
| 空 query 白烧回退链 | ✓ 5s 链（初判 rc 坑已复测 rc=2） | P2：+ batch 实锤真因 SearXNG 400 被吞 | P2 成立（o1p） |
| score 跨源量纲 | ✓ [10,9,8] vs [1.0,0.5] | P3-2：+ README 公式"首条=n"与实测矛盾 | 并入文档组（ago） |
| browse javascript: 白起 Chrome | ✓ INFO 实录 | P3-4：等 CDP 超时才报错 | 并入 P1 scheme 白名单 |
| dl -o 路径穿越 | ✓ 目录真建 | P3-6 观测一致 | P3 成立（7z0） |
| env 优先级 | ✓ env 覆盖 json（DDG 兜底 ok） | 通过项 | 不立案 |

独立评审独有高价值发现：P2-5 similar 状态机矛盾（PM 抽验 3/3 实锤）、P2-1 README 示例硬错（与盲测四 clap 亲测交叉）、P2-4 recency 零结果误诊（直连 API 实证）、P3-5 fetch 文案死路、P3-3 --read 越界白起浏览器。

## 建档（7 条）

| bd | 级 | 主题 |
|---|---|---|
| `pkp` | **P1** | browse scheme 白名单+私网门（file:// 外泄/私网渲染/javascript: 白起，与 fetch SSRF 门不对称） |
| `o1p` | P2 | 错误语义误导三连（空 query/零结果与 recency 空误诊 degraded/真因被吞） |
| `1az` | P2 | similar 快乐路径 run.status=error 自相矛盾（run 信封空洞） |
| `ago` | P2 | 文档契约漂移五连（README 示例错/USER_GUIDE 过时/meta 缺席违约/score 公式/dl·update 违反 JSON 总契约） |
| `d3u` | P3 | --read 越界检查前移（免白起 Chrome） |
| `7z0` | P3 | fetch 公网 http 文案死路 + dl -o 穿越门 |
| — | 观察 | doctor 强制代理环境恒 FAIL；batch 默认不省字节（省延迟，v2 才 -15%）——进报告不建档 |

## 通过项（14 检查组）

退出码表 0/1/2/4/5 全对、--json stdout 纯净（诊断全在 stderr）、batch 裸数组单文档、DDG 第二源实战可用、fetch/verify 护栏密度高、GSEARCH_SEARXNG_URL env 优先级正确、emoji/1KB 超长 query 不崩。

## 未覆盖（诚实边界）

exit 3（CAPTCHA 超时，SearXNG+养熟 profile 未触发）、CAPTCHA 亲解/login 人工流程、shell 交互、Google 直爬 SERP 解析（出口直连 google:443 超时，仅验了 TCP 预检熔断）。

## 修复优先级建议

1. `pkp` P1（安全，与 I9 威胁模型升级一脉相承）
2. `ago` P2 文档组（agent 契约源，修复成本最低收益最大）
3. `o1p` + `1az` P2 错误语义组
4. `d3u` + `7z0` P3 打磨
