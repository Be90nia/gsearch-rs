# T7 收尾回执：xt2 focus emulation + kda 提取管线统一

**结论：两 issue 全部落地并三重验收通过。VERDICT: PASS**

- gsearch-rs-xt2 PASS：`Emulation.setFocusEmulationEnabled` 已挂全部建页入口，隐藏 tab 节流实测 3→125 ticks/2s
- gsearch-rs-kda PASS：fetch 正文提取统一 scraper 树内路径（手写状态机/字符串扫描 title/手写实体解码全删），二进制 MIME 门全量前置拒绝

---

## xt2：focus emulation 防后台 tab 节流

**改动**：
- `src/browser.rs:669-680`：新增 `pub async fn open_page(&Browser) -> Result<Page>` —— new_page("about:blank") 后立即 `SetFocusEmulationEnabledParams::new(true)`（`chromiumoxide::cdp::browser_protocol::emulation`，0.9.1 绑定确认存在）。emulation 失败 warn 不 abort（性能加固非正确性路径，对齐 chaser-stealth 补丁风格）。
- 接线 14 个 new_page 调用点（全部 `browser.new_page("about:blank")` 收口到 `browser::open_page`）：main.rs:414（search 顶层）、search.rs:112/157/198（shell 会话搜索、撞码切有头、解码切回无头重建）、shell.rs:69/182/191/437/485（会话初始、captcha 超时/换 browser/有头登录/切回重建）、general.rs:62/170/279（browse/login/dl）、postproc.rs:407/537（goto_page→read/browse 登录墙重抓、dl）——shell 多 tab 与 swap_to_headed 场景全覆盖。

**观测实证**（headless、3 tab、setInterval 16ms × 2s 窗口，探针跑后已删）：
```
baseline_hidden_tab_ticks=3     # 隐藏 tab 被 Chrome 钳到 ~1s/次
focus_emulation_ticks=125       # 16ms 满速率（2s/16ms≈125 理论值精确命中）
verdict=THROTTLE_OBSERVED_EMULATION_HELPED
```

**附带项（settle/humanize sleep 收紧）处置**：
- settle：已是 marker 判稳、无固定 sleep（postproc `wait_content_stable` 两轮快照 200ms 间隔即过，shell.rs 注释同款）——附带项意图已满足。
- humanize：`stealth::warmup` 的 180-420ms/次滚动 + 1-3s 收尾是反检测拟人节奏；代码库无「输入后等联想」路径（无 typing/autocomplete 流），jev 的 200ms/50ms 收紧无映射对象，收紧反而破坏反检测目的——不改，留此记录。

## kda：fetch 提取管线统一 + 二进制门

**改动**：
- `src/fetch.rs`：`extract_text` 重写为 scraper 树内 DFS（显式栈 + children 逆序入栈保文档序；script/style/noscript/template 子树整跳；块级 `\n`/行内空格约定不变；实体由 html5ever 解析期解码）。scraper 0.20 不 re-export ego_tree，遍历全程类型推断，零新增依赖。
- `process_html`：HTML 单次解析，title 与正文同出树内（原字符串扫描版 `extract_title` 对 `title="a>b"` 同类泄漏）。
- 删除手写路径三件套：`extract_text` 状态机、`tag_name`、`extract_title`、`decode_entities`（及其测试 `decode_entities_cases`——函数已删，行为由解析器承担）。既有提取契约测试（`extract_text_strips_and_keeps_body`/`extract_text_comments_and_case`/`js_shell_detection` 等）断言零改动全绿。
- 二进制门：`is_binary_content_type`（image/audio/video/font/model 前缀 + zip/gzip/tar/7z/rar/iso/octet-stream/wasm/java-archive/elf/msdownload/Office 等）+ fetch_one 在 PDF 门后接线（body 下载前拒绝）。文本类（text/*、JSON、+xml、javascript）与无 CT 头按既有契约放行。
- 新增测试：`extract_text_attr_gt_no_leak`（毒性 HTML：`title="a>b"`/`data-x="p>q"` 不漏 `b">`/`q">` 碎片——fetch 拒 file://，按验收指示用单测锁行为）、`binary_content_type_detection`（21 真 7 假样本）。

**契约回归**：--markdown（htmd 独立转换，text 换源逻辑未触碰）、--include（scraper 解析 + inner_html 重提取路径不变）、JS 壳双条件判定（现有测试锁死）、10MB cap（accumulate_chunk 未动）、非 HTML 源文保真（`process_html_non_html_preserves_entities` 绿）——全部原样通过。

## 验收证据

**code-level**：`cargo clippy --all-targets -- -D warnings` 0 error；`cargo test --all-targets` 94 lib + 57 bin 全过（2 ignored 为既有 live 测试）。

**end-to-end（新编译 debug 二进制，08:34:14 后无再编）**：
- a. `fetch https://docs.rs/serde/latest/serde/ --json` → `len: 4708, leak_frag: False, meta.truncated=false`
- a+. 毒性页全管线（本地 http.server 18742 + `fetch http://127.0.0.1:18742/p.html --allow-private --json`）→ `title: 'poison page title'`、`text: 'poison page title\nbody text one body text two'`、零属性尾巴 PASS（旧二进制同输入复现泄漏 `b">poison page title`，对照成立）
- b. `fetch https://github.com/rust-lang/rust/archive/refs/tags/1.0.0.zip` → `error: 二进制内容（application/zip），fetch 不做文本提取；用 gsearch dl … 落盘后处理`，exit=1，无乱码
- c. focus emulation 前后对比见上节探针输出

## Side-effects（三态：无）

本次改动未引入预期外行为变化/残留进程/临时文件（探针与本地 server 已清理）。**工作树存在非本任务改动，未触碰**：`D AGENTS.md`、`D CLAUDE.md`、`D .claude/settings.json`、`D .agents/skills/beads/*`、`M .beads/interactions.jsonl`、untracked `gsearch.json`、`tk_*.json/txt`（接手前已在）。图谱索引按指令未动（索引落后于本次改动，待上级统一触发）。

## 文档

README.md：fetch 节补「二进制内容拒抓」「正文提取走 scraper 树内解析」两条；shell 节补 focus emulation 一行（含实测数字）。

## 未做 / 非目标确认

- 未 commit（按指令归上级）；未关 bd issue（建议验收后 close，xt2 附带项处置见上节记录）
- 未触碰 search/searxng/duckduckgo/update 输出路径
- `application/*` 未知子类型不在二进制清单内的（如自定义 `application/x-foo` 二进制）仍按文本透传——与改动前一致，实测投诉出现再扩清单（is_binary_content_type 单点扩展）
