# T3 收行回执：xih（--markdown）+ q34（PDF 提示）+ 8i3（shell quit）

**结论**：三项全部落地并过双验收（clippy -D warnings 0 error + cargo test --all-targets 136 passed / 0 failed / 2 ignored[live]；端到端 a-d 全实测）。

**VERDICT: DONE**

- 分支：合并态（cbc6712 之后工作树），未 commit（按派单禁令）
- 改动文件：`Cargo.toml`、`src/convert.rs`（新建）、`src/fetch.rs`、`src/general.rs`、`src/main.rs`、`src/postproc.rs`、`src/shell.rs`、`README.md`、`Cargo.lock`（htmd 引入）

---

## xih：read/fetch --markdown（P1）

### crate 选型：htmd = "0.5"（1 个新直接依赖，PM 预批 ≤2 内）

| 候选 | 结论 | 依据 |
|---|---|---|
| **htmd 0.5.5** | **选用** | turndown.js 移植（passing 全部 turndown 测试用例）；**表格转 md 管道表格**；传递依赖仅 html5ever + markup5ever_rcdom + phf（scraper 已带入同源生态）；维护活跃（crates.io updated 2026-07-27，5.88M downloads）；~16ms/1.37MB 页 |
| html2text 0.1.10 | 否 | 定位是 plain-text 渲染（表格 ASCII 画），非 markdown 表格 |
| html-to-markdown-rs 3.11 | 否 | 依赖 astral 系（重传递依赖面），多语言绑定项目 |
| 手写 scraper 转换器 | 否 | 表格/嵌套格式/实体边缘自研成本高；issue 明示可加 1 个 md crate |

### 契约（README「输出契约」节已写清）

- `fetch <url> --markdown`：转换在剥标签**前**的原始 HTML 上做（`is_html` 源才转；text/plain/JSON/.md 源文不转原样保留）。`--json` 时 `text` 字段换源为 markdown 产物 + `meta.format: "markdown"` 标注；`--include` 命中容器时对 inner_html 转 markdown；批量模式逐元素带 format 标注
- `browse <url> --markdown`：渲染后 HTML（content_retry 复用）→ markdown，**隐含全文模式**（与 `--headings-only` clap 互斥报错）；`--json` 时 `content_text` 字段换源 + `meta.format: "markdown"`
- `search --read N` / shell `read`：**不支持**（取舍见下）
- 无 flag：输出逐字节不变（回归实测见下）

### 【验证命令 + 关键输出】

**单测三断言（convert.rs 新增 4 测）**：
```
cargo test --bin gsearch convert::
test convert::tests::table_not_flattened ... ok     # 4 行管道结构 + 表头/分隔/数据行
test convert::tests::heading_levels_preserved ... ok # h1/h2/h3 → #/##/###（ATC，非 Setext）
test convert::tests::links_preserved ... ok          # [text](href) 绝对/相对链接保留
test convert::tests::skips_script_style_noscript ... ok
```
（首轮 `table_not_flattened` 失败：htmd 单元格带对齐空格 `| Rust   |`，断言从 `| Rust |` 放宽为 `| Rust`；探针实锤输出 `"| Lang   | Year |\n| ------ | ---- |\n| Rust   | 2010 |\n| Python | 1991 |"`）

**验收 a（fetch --markdown --json 结构字符）**：
```
# 规格原命令的 wikipedia 在本网络不可达（环境层，非代码）：
curl -A gsearch/0.2.9 https://en.wikipedia.org/wiki/... → 000 15.01s 超时
（en.m / zh.wikipedia.org 同 000；example.com 200 1.78s 正常对照）
# 换可达等价结构页（MDN table 文档页，含真实表格）：
gsearch fetch 'https://developer.mozilla.org/en-US/docs/Web/HTML/Element/table' --markdown --json
→ format: markdown | len: 50000(capped) | pipes: 8 | 链接 [text](href) 在正文
# 同断言形态跑通：'|' in t or '#' in t == True
```

**验收 b（默认输出回归）**：改动前基线（合并态构建）vs 改动后，3 个 URL 逐字节对比：
```
cmp /tmp/baseline_mdn.json      .tmp/new_mdn.json      → mdn:      BYTE-IDENTICAL
cmp /tmp/baseline_rustlang.json .tmp/new_rustlang.json → rustlang: BYTE-IDENTICAL
cmp /tmp/baseline_example.json  .tmp/new_example.json  → example:  BYTE-IDENTICAL
```
batch 附验：`fetch URL1 URL2 --markdown --json` → 两元素均 `status:ok, meta.format:markdown`。

**browse --markdown 端到端**：
```
gsearch browse 'https://www.rust-lang.org/' --markdown --json
→ format: markdown | len: 5291 | links: 44 | heading-markers: 22
gsearch browse 'https://example.com' --markdown --headings-only → clap 互斥报错 CONFLICT_OK
gsearch browse 'https://example.com' --json（无 flag）→ summary_paragraphs 在、无 content_text、无 format 键（默认 AdaptiveRead 不变）
```

### read（search --read N）不支持 --markdown 的取舍

`search --read N` 的输出装配在 `cmd_search`（main.rs）的 search JSON 信封内——Wave1（T1Flip）已冻结 search 输出路径，加 flag 必须动该装配代码，越非目标红线。browse 独立信封（general.rs 自有装配），故 browse 支持、read 缓行；README 契约节已明写「read 尚未支持，后续补」。

---

## q34：PDF 提示（P3）

### 【验证命令 + 关键输出】

**fetch PDF MIME 门**（现状实测确认乱码路径成立后加门）：
```
gsearch fetch 'https://pdfobject.com/pdf/sample.pdf'
→ error: PDF 二进制内容，fetch 不做本地解析；用 gsearch dl https://pdfobject.com/pdf/sample.pdf 落盘后由外部工具提取文本
→ fetch_rc=1（此前行为：binary → lossy UTF8 → 剥标签 → 乱码正文）
```
（w3.org dummy.pdf 对本出口 403，换 pdfobject.com sample.pdf 实测；is_pdf_content_type 纯函数单测 6 断言全过，含 `application/pdf+xml` 不误伤）

**dl 落盘 PDF 提示行**：
```
gsearch dl 'https://pdfobject.com/pdf/sample.pdf' -o tmp_t3_receipt
→ mode: direct
→ 已下载: ...\tmp_t3_receipt\sample.pdf (18810 bytes)
→ stderr: 提示: 二进制 PDF 已保存（本地未解析文本）；agent 可用外部工具提取   [rc=0]
```
提示行覆盖**全部五条 dl 落盘路径**：general::cmd_dl 的 direct / browser 原生下载 / 页内 fetch 三条 + postproc::dl（search --dl N）+ shell::dl_in_page，共用 `general::pdf_hint()`（按最终落盘路径扩展名判 `.pdf`，大小写不敏感）。

---

## 8i3：shell quit 三重自相矛盾（P3）

改动：`run_shell` REPL 循环 `exit|quit` 直接 break（graceful 关 Chrome 后 rc=0）；dispatch 删除旧「仅提示」臂；help 表 `exit / quit  退出（rc=0；EOF / Ctrl+D 同效）`；banner 原文案（「exit / quit / Ctrl+D 退出」）修后成真，未动。

### 【验证命令 + 关键输出】
```
printf 'quit\n' | ./target/debug/gsearch.exe shell
→ 进入 gsearch shell（...）\n gsearch>  → rc=0（无「退出请输入 EOF」字样）
printf 'exit\n' | ... → exit_rc=0
printf '' | ...（纯 EOF）→ eof_rc=0（Ctrl+D 兼容保留）
printf 'help\nquit\n' | ... → help 行含「rc=0；EOF / Ctrl+D 同效」
```

---

## 侧效应声明（三态）

- **新依赖**：`htmd = "0.5"`（直接，1 个，PM 预批内）；Cargo.lock 随之 +174 行（html5ever 0.38 族，与 scraper 的 html5ever 0.27 并存双版本，编译期各 ~3s，无冲突）
- **既有行为变更**：仅 8i3 的 quit/exit 语义（派单要求的行为修复）与 fetch 对 PDF 的报错（q34 要求的门）；无 flag 输出路径逐字节不变（验收 b 实测）
- **未触碰**：工作树中非本任务的变更原样保留（`AGENTS.md`/`CLAUDE.md`/`.claude/settings.json`/`.agents/skills/beads/*` 删除、`tk_*.json`/`gsearch.json` untracked——兄弟任务/环境产物，未动未还原）；search 输出路径零改动；验证临时目录 `.tmp_t3/`、`tmp_t3_receipt/` 已清理
- 测试临时目录清理后遗留 `tmp_t3_receipt/` 内 pdf/err 已删（均为本任务自产探针产物）

## 残余风险

- wikipedia 直连在本网络不可达（curl 000 实证），验收 a 以 MDN 等价结构页替代；若 CI/其他网络 wikipedia 可达，规格原命令应同样通过（结构断言与实现无 wikipedia 特化）
- htmd 与 scraper 的 html5ever 双版本并存增加少量编译时间（release 冷构建 +~5s 量级），无功能影响
- browse --markdown 对超 50k 字符的 markdown 产物按 `read_max_chars` 截断（meta.truncated 标注）；截断可能落在表格中段（与 --full innerText 同边界，契约一致）
