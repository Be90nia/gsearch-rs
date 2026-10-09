# FixG16 修复回执（盲测十五/十六 2 项设计级扣分）

日期：2026-10-09 ｜ 分支：main（工作区，未提交）｜ 裁定：Main 回复 **A**（P0-1 断言改判，`[*]` 投影形状维持 FixG15 冻结语义）

## 改动清单

**项 1：--json-keys 投影命中 text 输出原生 JSON Value（BB -1 + Y -0.25）**
- `src/fetch.rs:725-731`（`fetched_json` 单点序列化出口）：`truncated_by_json_keys=true` 时 text 经 `serde_json::from_str` 输出原生 Value（对象/数组原样）；投影产物被字符预算截成非法 JSON 时回退 string；非投影路径恒 string（零结构体改动，老输出逐字节不变）。单条与 batch 路径共用此出口，一处修复全覆盖。
- `src/main.rs:224-226`：fetch --help json-keys 文案补一句「--json 下即原生 JSON 对象/数组，免二次解析」（未动全局 about）。
- `README.md:123`：json-keys 契约 bullet 补 text 形态说明（原生对象/数组、截断或回退恒 string）。

**项 2：markdown 输出转义清洗（DD -0.25）**
- `src/convert.rs:24-35`：新增 `clean_markdown(md: String) -> String`——`\_`→`_`（保守只还原下划线转义）+ `[§](#anchor)` 剥为空（未闭合不剥防误伤）。
- fetch 路径 3 个 markdown 产出点全部接线（browse/general 共用 `html_to_markdown` 保持冻结）：`src/fetch.rs:448-449`（主路径）、`:1046-1047`（GitHub host 路由）、`:1430-1435`（`render_include_blocks`，docs.rs host 路由与 --include 均经此）。
- `README.md:96`：--markdown 节补一行清洗说明（fetch 清洗、browse 不清洗）。

## 测试

`cargo test --all-targets`：**244 passed**（基线 241 + 新增 3）｜ `cargo clippy --all-targets -- -D warnings`：**零告警**
新增：`convert.rs clean_markdown_unescapes_underscore` / `clean_markdown_strips_section_anchor`；`fetch.rs fetched_json_text_native_value_on_projection_hit`（原生对象 / 非法 JSON 回退 string / 非投影恒 string 三断言）。

## P0 e2e（真实网络，stdout 落盘不接管道、stderr 不混流）

| 门 | 命令 | 关键输出 | 结论 |
|---|---|---|---|
| P0-1 | `fetch "https://api.github.com/repos/tokio-rs/tokio/releases?per_page=3" --json-keys "[*].tag_name" --json` | `text_is_dict True inner_is_list True native 245 escaped 255 smaller True`（d["text"]={"tag_name":["tokio-1.53.2","tokio-1.51.5","tokio-1.53.1"]}） | **PASS（按 Main 裁定 A 改判断言）** |
| P0-2 | `fetch "https://docs.rs/serde_json/latest/serde_json/fn.from_str.html" --markdown` | `has_escaped_underscore False has_section_anchor False`（meta: auto_include_applied=docs.rs, format=markdown，正文含签名 fenced 块） | **PASS** |
| P0-3 | `fetch "https://api.github.com/repos/tokio-rs/tokio" --json` | `text_type str`，meta 无 truncated_by_json_keys | **PASS** |
| P0-4 | `cargo test --all-targets` + `cargo clippy --all-targets -- -D warnings` | 244 passed / 0 failed；clippy 零 error | **PASS** |

**P0-1 裁定记录**：原断言 `isinstance(d['text'], list)` 的前提（`[*].field` 产裸数组）与 FixG15 冻结语义冲突——`project_json_paths` 恒返回 Object（`fetch.rs` 测试 `project_json_paths_wildcard_projects_all_elements` 逐字锁定 `{"tag_name":[...]}`；help/README「值为数组形态」；盲测十五 Y 对该形状记 delight），改裸数组=协议变更，命中非目标「协议/CLI 参数表不变」。已 `write agent://Main` 提请裁定，Main 回复 **A**：断言改判 `isinstance(d['text'], dict) and isinstance(d['text']['tag_name'], list)`，双重编码根治目标（原生 Value、免二次 parse、体积更小）全部达成。拒绝 B（协议变更）。

**附加实测**：batch 双 URL（releases+repo 同带 `[*].tag_name`）——entries[0] 投影命中 text=原生 dict（内层 list）；entries[1] 对象 body 撞通配 → stderr 一行教学 + 回退原 body → text 恒 string，stdout 保持纯 JSON（148B stderr 未污染）。

## code-simplifier 自检

改动 0 处（初版即按清单落位：单点序列化门控 + 3 处一行接线 + 保守双规则清洗函数）/ 触碰禁区 0 / diff +76 / -7 行（不含 .beads 工具日志）。

## 自我评估

- 准确性 5/5 — 每条声明有命令输出或 fetch.rs/convert.rs/测试名佐证；投影形状冲突有测试行号+盲测报告原文双重引用
- 完整性 4/5 — 两项修复、3 测试、help/README 双同步、4 P0 全过；扣分：P0-1 依赖 Main 裁定改判（原断言文字面不可满足，已记录裁定链）
- 清晰度 5/5 — 回执按改动/测试/P0/裁定四段，每条 P0 一行命令+一行输出
- 可执行性 5/5 — 全部验证命令原样可复跑（含 cygpath -m 桥接形态）
- 简洁性 4/5 — 扣分：附加 batch 实测段略超必需，但为回退路径唯一活体证据，保留

## read 调用审计

- read skill://serena-cli SKILL ⇒ 代码检索纪律前置；serena-cli 2 次（首试参数错 rc=2，重试成功取 fetched_json 体），grep/read 定位降级理由：契约已知字面串定位 + serena 语义就绪需 30-60s
- read rule://rust ⇒ 自检纪律命中（.rs 改动），「行为验证前显式 cargo build」已遵循
- bd memories markdown ⇒ 无命中（无历史踩坑需加载）
- code-simplifier / agent-self-evaluation / task-closing-ritual / silent-failure-hunter SKILL ⇒ autoload 注入，均已执行（见上各节；silent-failure：本改动无新增 catch/fallback——非法 JSON 回退 string 是显式契约路径且测试锁定）
- doubt-driven-development ⇒ 未触发（无新类型/公共 API/后台任务；非平凡决策仅 P0-1 歧义，已走 Main 裁定并记录 CLAIM）

## 残余风险

- P0-1 断言以 Main 裁定 A 为准（原文 list 断言不可满足）；若后续轮次要裸数组形态，需单独立项做协议变更（改 project_json_paths 返回形态 + 2 引擎测试 + help/README）
- `clean_markdown` 的 `\_`→`_` 是全局替换：正文 fenced 块内字面 `\_`（罕见，htmd 对 code 块不转义）会被还原——契约明确要求保守全局还原，接受
- 工作区另有非本任务产物：`.beads/interactions.jsonl`（bd CLI 交互日志）+ 未跟踪空文件 `err.tmp`（非本任务创建，未动）

## 沉淀

`~/.omp/agent/rules/common-windows.md`「Git Bash 重定向写的 MSYS /tmp 文件对 Windows 原生 python 不可见」小节补一条细化：MSYS 只翻译独立 argv 形态 POSIX 路径，`-c` 脚本串内嵌路径不翻译，最稳形态 `sys.argv[1]` + `cygpath -m`。
