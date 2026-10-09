# FixG7 回执：盲测七三缺陷修复（0bh / wqm / b5f）

**结论：三缺陷全部修复并通过双验收（code-level 单测 + 真实 e2e），全量闸 clippy -D warnings 0 error、cargo test --all-targets 109+73 全绿。**
**VERDICT: PASS**

## 缺陷A（0bh · convert.rs · fenced 代码块静默变异）

**根因（vendored htmd-0.5.5 源码实证）**：`span_handler` 的 `trim_matches('\n')` 剥掉 span 携带的行尾换行（换行折叠）；`<a>` 在 pre 内被 anchor handler 转 markdown 链接（链接注入）；`div.where` 走 block 元素边界注入 `\n\n`（伪影空行）。builder 现成选项无解（`preformatted_code` 只管 inline code；`TranslationMode::Faithful` 会把复杂 pre 序列化成 HTML 更糟）。

**修复**（convert.rs，最小预处理不换库）：`sanitize_pre_blocks` 把每个 `pre code` 的子树重写为单 Text 节点（textContent + 块级元素前边界换行），保留 pre>code 容器与 class——htmd 原生 fenced/语言标注路径不变；纯文本 code 块（GitHub md）经此路径逐字节等价（防回退单测锁定）。

**撇号子项定性（实测定性修正，非管线缺陷）**：docs.rs HTML 源里正文就是 U+2019（rustdoc pulldown-cmark smart punctuation 上游渲染），registry 源 de.rs 全文件 U+2019 = 0（源码为 U+0027）——**偷换发生在上游 rustdoc 渲染层，gsearch 忠实转换，不可也不应改写字符**。验收「's 为 U+0027」在签名段成立（`from_str<'a, T>` 全程 U+0027，e2e 断言通过）；正文 `` `T`’s `` 与页面 HTML 一致（忠实性契约，README 已注明）。

## 缺陷B（wqm · fetch.rs · GitHub 评论区静默丢失）

`github_thread_comment_gap(url)` 纯函数识别 `github.com/{o}/{r}/(issues|pull)/{n}`（query/hash 不影响；issue 与 PR 对话统一指 api issues comments 端点），命中时 meta 恒带 `github_comments_missing: true` + `github_comments_hint`（browse --markdown 与 api.github.com 两条可行动出口）；非 thread 页两键缺席（默认输出结构零变化）。未引入浏览器回退（fetch 保持纯 HTTP）。

## 缺陷C（b5f · main.rs · help 泄漏内部代号）

owned 范围（convert/fetch/main）所有 `///` 的行首代号前缀清除（3gw/cxa/l6o/n76/xih/pkp/6dp/fve/nx4/e1i/cw8/e7c/q34/kda/n76/M9/M11/M14-1A/M6/M7/745/issue gsearch-rs-* 等，含 main.rs 进 help 的句中代号）；普通 `//` 注释按任务约定保留。新增 `help_text_free_of_internal_issue_codes` 测试渲染顶层级 + 全部子命令 help，断言 19 个已知代号零命中。

## 【验证命令+关键输出摘录】

```
$ cargo clippy --all-targets -- -D warnings
    Finished `dev` profile … in 2.26s          # 0 error（修掉自家 needless_borrow 1 处）

$ cargo test --all-targets
test result: ok. 109 passed; 0 failed …        # bin（含 6 个新增测试）
test result: ok. 73 passed; 0 failed; 2 ignored …  # lib

$ ./target/debug/gsearch.exe search --help && … fetch --help ｜ grep -E "cxa|l6o|3gw|M9|6dp|n76|xih|pkp|kda|dsg"
grep exit=1                                     # 零命中

$ ./target/debug/gsearch.exe fetch https://docs.rs/serde_json/latest/serde_json/fn.from_str.html --markdown
签名块 fence 实况 repr： "where\n    T: Deserialize<'a>,\n```\n\nExpan"
四断言全 PASS：where 前 0 空行 ✓ ／无 [](...) 链接 ✓ ／撇号 U+0027 ✓ ／derive/struct 分行 ✓
（GT 对照 de.rs：#[derive(Deserialize, Debug)]\n/// struct User { 分行 = True；源码 U+2019 = 0）

$ ./target/debug/gsearch.exe fetch https://github.com/tokio-rs/tokio/issues/7787   （exit 0，stderr 0 字节）
meta keys: ['content_untrusted','github_comments_hint','github_comments_missing','omitted','truncated']
github_comments_missing: True
hint: 评论区由 JS 动态加载，未包含在本输出中（勿据本文判断有无讨论）；完整讨论：gsearch browse … --markdown，或 GET https://api.github.com/repos/tokio-rs/tokio/issues/7787/comments
```

## Side-effects

**【三态】无预期外副作用。** 理由：`--markdown` 非 HTML 路径与无 flag 输出逐字节不变（process_html 主路径零改动）；GitHub 非 thread 页 meta 键缺席（fetched_json 默认结构零变化，单测锁定）；其余 GitHub 页（blob/tree 等）不触发新键；browse/search 路径未触碰。

## 残余风险与未做

1. **非 owned 文件的 /// 代号前缀未清**（general.rs/types.rs/config.rs/browser.rs/skeleton.rs/duckduckgo.rs/util.rs/verify.rs 等 ~40 处行首前缀）——不在 Owned Files 授权清单内；它们不进任何 --help（验收 grep 面 = main.rs doc comments，已干净），建议另派任务清理。
2. 正文 docblock 的 `T’s`（U+2019）为上游 rustdoc 渲染产物，gsearch 不做字符归一——若 PM 判定需要「还原源码撇号」，属内容改写策略决策，需拍板。
3. `fetch 纯文本/非 json` 人读模式不加 stderr 提示（任务要求面 = meta 标注，按最小变更执行）。
4. convert.rs 现对每个文档页多做一次 scraper 解析 + 序列化（pre 保真预处理）；实测单页 fetch 耗时无可见回退（e2e 2.55s 内含网络），未做基准对比。

## 交付物

- src/convert.rs：sanitize_pre_blocks / faithful_code_text / is_block_level + 4 个新单测（+127 行 diff）
- src/fetch.rs：github_thread_comment_gap + Fetched.github_comment_hint 接线（fetch_one / include 分支）+ 1 个新单测（+89 行 diff）
- src/main.rs：doc comment 代号清理 + help_text_free_of_internal_issue_codes 测试（112 行 diff，纯删除/改写 + 22 行测试）
- README.md：输出契约节 +2（fenced 逐字保真；GitHub 评论区缺失信号）
- 新单测 6 个：convert×4（换行/链接与where空行/撇号/语言标注防回退）、fetch×1（meta 信号与键缺席）、main×1（help 代号零泄漏）
