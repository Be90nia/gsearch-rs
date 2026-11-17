# --from 在 --full 形态的消费统一（bd gsearch-rs-0z8 遗留矛盾收尾）

日期: 2026-11-17 ｜ 执行: fullstack-engineer (SkillFromHintFix) ｜ 派单: PM

## 结论

VERDICT: PASS（含 1 处契约偏差需 PM 复核，见「契约偏差」节）

统一逻辑落地：browse 命令与 search --browse N 两条路径的 `--full` 形态都经
`md_page_section` 统一输出点按空行切块消费 `--from`；「不消费」名单收缩为
`--json` / `--headings-only`（+ search 侧 `--read N` snippet 形态与 batch）。

## 改动清单

| # | 位置 | 内容 |
|---|---|---|
| 1 | src/general.rs:30-34 | 新增 `browse_from_not_consumed()`：browse 侧不消费名单 = `json \|\| headings_only` |
| 2 | src/general.rs:133-138 | hint 条件 `(opts.json \|\| opts.full \|\| opts.headings_only)` → `browse_from_not_consumed(opts)`；文案「仅默认 md 形态」→「仅默认 md 与 --full 形态」 |
| 3 | src/general.rs:36-37 | `BrowseOpts.from` 字段 doc 同步 |
| 4 | src/general.rs:838-855 | 新增单测 `browse_from_not_consumed_only_json_and_headings`（4 断言：默认 md/--full 消费，json/headings-only 不消费） |
| 5 | src/postproc.rs:573-584 | `read_full` md 分支 `md_page_section(&title, &txt, 0)` → `opts.from`（硬编码 0 是 search 侧矛盾根源） |
| 6 | src/postproc.rs:1167-1182 | 新增单测 `md_page_section_consumes_from_for_full_innertext_shape`（innerText 五段 PARA 形态，from=2 跳前两块） |
| 7 | src/main.rs:487-499 | `from_is_consumed` 改为 `browse && !json && (!headings_only \|\| full)`；warn 文案去掉 `--full`，改「仅 --browse N（默认 md / --full）生效」 |
| 8 | src/main.rs:545-547 | cmd_search 调用点注释同步 |
| 9 | src/main.rs:117-118 | browse 子命令 `--from` CLI help 同步 |
| 10 | src/main.rs:1815-1836 | `from_is_consumed` 契约测试更新为 8 断言矩阵（含 `--read N --full` 仍不消费、browse 下 full 优先于 headings_only） |

`from_is_consumed` 谓词说明：`--full` 只在 `--browse N` 下有消费路径（read_full），
且 dispatch 中 full 优先于 headings_only（main.rs:784 `if opts.full` 先判），故
`browse && !json && (!headings_only || full)`；裸 `--read N --full` 不消费（snippet-only），
保留 hint（0z8 禁静默原则）。

## 契约偏差（需 PM 复核）

派单(2)/验收 e2e 写的 `search "<query>" --read 1 --full --from 2`「hint 消失、行为消费」——
**该 flag 组合根本不进 read_full**：

- 证据1（baseline exe 实测）：`search "example domain" --read 1 --full --from 2` →
  stdout 仅 291 字节 snippet 列表，无内容段（%TEMP%/gs_e2e 留档前已复核）。
  FixG10 L-1 起 `--read N` = snippet-only 不启浏览器（main.rs:661-662 `read_n`/`browse_n`
  分立；read_full 仅在 browse_n 分支 main.rs:784 调用）。
- 证据2：修复后同命令 → hint 仍发（新文案）。若让 `--read N --full` 真消费需让 --read
  启浏览器 = 语义重设计，超出本票范围，未做。
- read_full 的真实调用方 = `--browse N --full`；统一消费落在该路径，e2e 已证（下表）。
- 顺带发现：search 子命令 `--from` 的 clap help（main.rs:323）仍写「--browse N --from K
  滑动窗口」旧语义（v0.4.0 默认 md 路径实为 md_page_section 切块），未在本票范围，交 PM 文档收尾。

## 验证

### code-level

- `cargo test` → rc=0，全二进制 0 failed：lib 131 + bin 153(4 ignored live) + 集成
  2/6/10/10/10/3/2/11/4 + doc 0（谓词修正前后两轮全绿；第二轮 18:11 完成于 e2e 之前）
- `cargo clippy --all-targets` → rc=0（两轮）
- 新增/更新 3 条测试全过：
  - `general::tests::browse_from_not_consumed_only_json_and_headings ... ok`
  - `postproc::tests::md_page_section_consumes_from_for_full_innertext_shape ... ok`
  - `tests::from_is_consumed_in_full_and_browse_default_md ... ok`

### end-to-end（python http.server 127.0.0.1:18777 五段 PARA fixture + rust-lang.org 真搜）

| 命令 | 结果 |
|---|---|
| `browse <fixture> --allow-private --full --from 2` | stdout = `# FromUnify Fixture` + PARA-02..05（块1=h1、块2=PARA-01 被跳，与 PM 实测「PARA-01 被跳」同型）；stderr 仅 chrome INFO 行，**无 [hint]** |
| `browse <fixture> --allow-private --full`（无 from；新 exe vs baseline exe） | 归一 `elapsed: X.Xs` 后 cmp **逐字节一致**（MD_NOFROM_IDENTICAL=yes；249B=249B） |
| `browse <fixture> --allow-private --json --full --from 2` | stderr 有 `[hint]`（文案：仅默认 md 与 --full 生效）；stdout 归一 `elapsed_ms` 后与无 --from **逐字节一致**（JSON_FROM_NOFROM_IDENTICAL=yes；356B=356B） |
| `search "rust programming language" --browse 1 --full --from 2` | 内容段自 "Why Rust?" 起（rust-lang.org innerText 前 2 个 nav 块被跳；同 URL 无 from 版自 nav "Install/Learn/..." 起，168 行 vs 142 行）；stderr 无 [hint] |
| `search "rust programming language" --read 1 --full --from 2` | stderr 仍发 `[hint]`（新文案「仅 --browse N（默认 md / --full）生效…--read N snippet…不消费」）——0z8 禁静默保持，见契约偏差 |

## side-effects 三态

- **预期外副作用：无**。tests/cli_golden_bytes.rs 逐字节契约全绿（stdout 未动）；禁区文件
  src/search.rs / src/duckduckgo.go tests/fallback_matrix.rs 未触碰；README/CHANGELOG/SKILL.md 未触碰；
  ~/.gsearch/config.json 未动、无新增环境变量。
- **已知伴随变化（契约变更本体）**：stderr hint 文案两处（general.rs / main.rs）、browse
  `--from` CLI help 文案、main.rs `from_is_consumed` 单测矩阵语义。
- **未验证项**：单块（无空行）页面 `--full --from K>0` → 正文为空——与默认 md 形态既有语义
  一致（md_page_section 单测 from 越界行为已覆盖），未单独 e2e。

## 工作树异常发现（交 PM 分流，非本票改动）

HEAD 的 src/postproc.rs blob 已损坏：`fn md_page_section_from_skips_blocks` 在文件头部
（45/57/69/81 行，mod tests 外）重复 4 份——HEAD 状态该文件无法编译。工作树中存在**先于本
会话**的去重修复（未提交；本会话 3 次编辑均唯一匹配，证明去重先在）。本会话编辑将文件 EOL
统一为 CRLF（与 general.rs/main.rs 全 CRLF 的仓库主流一致；HEAD 该文件本为 48CRLF+1306LF
混合）。建议：由 PM 确认后尽早提交该去重修复，避免后续 agent 在损坏 HEAD 上叠 diff。

## 临时产物清理确认

- python http.server（bg_63）→ 已 kill
- %TEMP%/gsearch_baseline_pre_fullfrom.exe、%TEMP%/gs_from_fixture/、%TEMP%/gs_e2e/、
  %TEMP%/gs_test*.txt、%TEMP%/postproc_head.rs → 已删（见回执执行记录）
- repo 内零临时文件落盘；本报告为唯一落盘产物

## 沉淀

无新增经验（契约偏差与 HEAD 损坏均为项目事实，上文交 PM 分流项目记忆；无跨项目语言/工具坑）
