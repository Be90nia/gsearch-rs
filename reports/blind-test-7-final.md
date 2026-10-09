# 盲测七——全新 AI 受试者、零修复上下文（用户要求：不要原班复测）

**方法**：3 个全新 task 子代理（SubjectD/E/F），任务与盲测六 A/B/C 一一同构（tokio 版本调研 / GitHub issue 阅读 / serde_json 文档提取）。上下文只给 exe + README/USER_GUIDE + help；**禁读 reports/、禁 git log、禁打听**——不知道任何修复历史。逐命令记账 + 结构化评分。PM 不参与打分。

## 成绩单

| 受试者 | 任务 | 完成 | UX | Token | 消耗口径 |
|---|---|---|---|---|---|
| D | tokio 最新版本+变更要点 | ✅ 1.53.2 双源确认 | **8.5** | **9** | --max-chars 全程自控，有效输出 ≈36KB（盲测六 A 同任务 116KB） |
| E | GitHub issue 正文+讨论 | ✅ #7787 | **7** | **7.3** | 9 命令全 exit 0，最终走 browse --markdown 12.4KB |
| F | serde_json from_str 签名/Errors/示例 | ✅（外接 registry 逐字节核对后交付） | **7.5** | **8.5** | --include main 后 1713B 纯正文 |

**均分：UX 7.67 / Token 8.27**（盲测六修复前 7.0/7.33；原班复测 9.5/9.5）
任务成功率 3/3；零 CAPTCHA 零弹窗；三条路径全程纯 HTTP+searxng（1.3-3.0s），Chrome 仅 browse 按需启动。

## 为什么比原班复测低 1.8 分

原班复测知道修了什么、定点验证；干净眼踩的全是**修复没覆盖的面**。这 1.8 分的差值=修复真实生效 + 新暴露面的差。

## 修复被无上下文受试者当"原生能力"消费（生效验证）

- `--max-chars`：D 全程自控预算、E/F 主动使用，D 点名"精确控制 token 预算且截断诚实"——无一人踩盲测六的头号坑（93.6KB 放血）
- isatty 快档：三人全裸环境（独立 shell），**无一人遭遇 89s 慢档**，无人提及 humanize——盲测六 C 的 -2 场景消失
- 空正文 hint：E 真实用上——browse 新版 GitHub 页空正文 → stderr 指引 → 一次重试恢复，只轻扣 0.2（"恢复成本低"）
- meta 诚实标注（provider/elapsed/truncated/content_untrusted）：E/F 各自点名 delight

## 新发现（干净眼独立命中，按严重度）

### P1 内容保真（fetch --markdown 管线，F 逐字节实锤）
F 用本地 cargo registry 源码做 ground truth，发现 4 处**静默变异**，truncated 标志完全不覆盖：
1. fenced 代码块内换行被折叠（`#[derive(...)]\nstruct User {` 合并成一行）——对"签名逐字符"类任务是致命失真（-1.5）
2. 签名代码块注入 markdown 链接语法（`&'a [str](https://...)`）+ 伪影空行（-1）
3. ASCII 撇号 U+0027 偷换弯引号 U+2019（-0.5）
4. 结论原话："这些静默变异迫使我外接第二来源核对，工具单独输出不可直接交付"

### P1 fetch GitHub issue 页评论区静默丢失（E）
`fetch <issue URL> --markdown` 正文完整但**评论区被丢**，meta 标 omitted=0/truncated=false（自称完整）；E 靠 api.github.com 交叉验证才识破——"agent 极易得出'该 issue 无讨论'的错误结论"（-1，正确性风险）。

### P2 已知残留
- GitHub 新版 React DOM 页（7787，2025-12）browse 默认档仍空正文（容器选择器只覆盖老 SSR 形态）——hint 兜底生效但根因在
- search 噪声：英文查询混 4/10 中文站（D）或游戏词条（E）——SearXNG 白名单含 baidu 的副作用
- fetch 默认 65%（F docs.rs）/34%（E GitHub）导航样板税——--include 可救但默认未启用
- browse --markdown 输出 ~20% UI 噪音（头像 URL/Reactions 词）；单行巨串 JSON（E 0.3）

### P3 打磨
- fetch 无 --timeout knob（D 撞 github.com 10s 固定超时被迫换源，-0.7）
- meta.truncated 语义歧义（snippet 截断 vs 结果数截断，D 0.5）
- **help 文本混内部代号 cxa/l6o/3gw/M9/6dp**（E）——issue 追踪代号泄漏进用户文档，必修

## 对比轨迹

| 轮 | 方法 | UX | Token |
|---|---|---|---|
| 六 | 真 AI 受试者（首测） | 7.0 | 7.33 |
| 六复测 | 原班受试者（知道修了什么） | 9.5 | 9.5 |
| **七** | **全新受试者（零上下文）** | **7.67** | **8.27** |

结论：修复真实生效（头号 token 坑消失、慢档消失、hint 兜底可用），但干净眼暴露出下一层：**内容保真 > 输出字节数**。下一轮优先级建议：fetch --markdown 保真化（代码块逐字+链接剥离开关+变异标注）> GitHub 评论完整性信号 > React 页容器。

[INFERENCE] 样本量 3，单人 ±0.5 分噪声；三受试者独立命中的共性发现（保真/噪声/样板税）可信度高于任何单条扣分。
