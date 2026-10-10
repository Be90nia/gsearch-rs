# 修复批三：审计遗留五条 issue（w2f / 0vk / 2se / cgp / 4bq）

基线 b49fa70（main），单代理闭环，未 commit。总计 +331 / -192 行（10 文件，含新增测试）。

## 1. w2f — lean 清理包（六项，-53 行净删除）

| 项 | 改动 | 落点 |
|---|---|---|
| D1 | 删 `find_chrome`（全树唯一命中即定义，调用方已迁 find_browser/find_specific） | browser.rs |
| D2 | 删 `pub use types::SearchResult` re-export（全树 0 处 `gsearch::SearchResult` 消费） | lib.rs |
| Y1 | 删 `FetchOpts.anchor_pad_lines` 死旋钮：字段/Default/测试断言/main.rs 构造点四处摘除，锚点裁剪读取点定值化传 0（`crop_text_lines` 签名与其 pad 行为测试不动） | fetch.rs、main.rs |
| R1 | 删 shell.rs `browser_alive` 私有副本，改 `use crate::general::browser_alive` | shell.rs |
| R2 | 删 search.rs / shell.rs 的 `swap_to_headed` 透传 wrapper，各 1 调用点直调 `browser::swap_to_headed`（search.rs:157 错误文案原文保留） | search.rs、shell.rs |
| R3 | 删 `launch_with_kind` 中间层：env 读取内联进 `launch`（`launch_with_kind_proxy` 有 bin 侧 5 处消费，保留） | browser.rs |

【验证命令+关键输出】`cargo check --all-targets` → `Finished ... 0 warnings`；`cargo test --bin gsearch shell::` → `3 passed`；`cargo test --bin gsearch fetch::`（覆盖 Y1 的 FetchOpts Default 测试与锚点裁剪）→ `68 passed`。

## 2. 0vk — 内存小颗粒（M-1~M-3）

- **M-1 collapse 链三连拷**：`collapse_blank` 改收 `&str` + trim 前置（首尾空白只置 pending 永不 push，输出与旧实现逐字节一致——`s.trim()` 与 `out.trim()` 同空白集，首字符 push 由 `out.is_empty()` 门控保证等价）；调用侧 `html.to_string()` / `seg.to_string()` 免拷。每次提取从 ~3×T+1×B 降到 1×T。
- **M-2 fetched_json move 化**：签名 `&Fetched` → `mut f: Fetched`，text/title 走 `mem::take`；单条调用传 owned，batch 改双分支各自循环（json 分支消费式迭代拿 owned，非 json 分支保持借用——ok_count 两分支各自累计，统计语义不变）。
- **M-3 投影零深拷贝**：`project_json_paths` 收 owned Value，`eval_json_path`/`step_json_path_mut` 改 `&mut` 链，路径末端节点 `mem::take` 摘取（通配分支整组数组摘出后逐元素 move）。副作用是原树被掏成 Null——调用方 `project_json_body` 提前记 `is_top_array`（Err 路径只用原始 body 串，不读原树）。返回类型 `Value` 不变，5 个既有投影测试断言零改动（仅调用形态 `&payload`→`payload.clone()` 适配编译）。

【验证命令+关键输出】`cargo test --bin gsearch fetch::` → `68 passed; 0 failed`——含盲测敏感回归网 `collapse_preserving_code_matches_collapse_blank_without_sentinel`（逐字节等价单测，绿）、`project_json_paths_wildcard_*` 3 例、`fetched_json_*` 9 例、`fetched_json_includes_status_ok`。

## 3. 2se — 先截后提取的尾部窗口补救

- 新增 `postproc::extract_adaptive_capped`：第一片 `cap_extract_source` 提取后，若 `truncated && omitted > 10_000 && summary 空`，从源 HTML 尾部取同预算字符段（起点 = total-limit = omitted，`char_to_byte_offset` 换算不劈 UTF-8）重跑 `extract_adaptive`；仍空维持第一片（hint 链照旧）。内存有界（第二片 ≤ limit 字符），勿全量建树。
- `read`（postproc）与 `cmd_browse`（general）两条路径接入；`truncated/omitted/truncated_at_offset` 恒反映第一片预算截断（meta 语义诚实）。
- hint 去重：`cmd_browse` 的 `needs_empty_body_hint` 调用改传 `omitted=0`——截断吃光形态的 hint 由 `render_read`（postproc.rs:326 条件 omitted>0）负责，browse 侧只补「content 拿空」形态，同一页不再双行重复；`needs_empty_body_hint` 本体与其测试不动。

【验证命令+关键输出】`cargo test --bin gsearch postproc::` → `16 passed; 0 failed; 2 ignored`——含新用例 `extract_adaptive_capped_recovers_body_from_tail_window`（构造 60KB head + 尾部正文，断言 summary 非空且含 marker）与对照 `extract_adaptive_capped_matches_plain_path_when_body_present`（正文在预算内不进补救分支）。

## 4. cgp — 低危五项

| 项 | 修复 |
|---|---|
| L1 Windows 保留设备名 | `filename_from_url` 的 sanitize 链加 `crate::browser::is_windows_reserved(&safe)`（browser.rs 既有 fn 提升 `pub(crate)` 复用），命中降级 `download.bin`（`fs::write("NUL")` 在 Windows 成功但零文件的静默数据丢失封死） |
| L2 100.64.0.0/10 | `is_private_ip` v4 arm 加 `(o[0]==100 && (64..=127).contains(&o[1]))`（CGNAT/Tailscale） |
| L5 fe80::/10 | `v6.segments()[0] == 0xfe80` → `(segments()[0] & 0xffc0) == 0xfe80`（原只匹配 /64，漏 fe81-febf） |
| L3 重定向 IPv6 字面量误拒 | `resolve_host` 剥 `[...]` 后走字面量 parse——redirect policy 的 `host_str()` 带括号不再解析失败 fail-closed；私网字面量仍被 is_private_ip 拒 |
| L4 verify curl 注入面 | `curl_args` 加 `--globoff`（禁 URL globbing）+ URL 前 `--`（`-` 开头不可信 URL 不再被当选项） |

【验证命令+关键输出】`cargo test --lib util::` → `12 passed`（含新 `filename_from_url_downgrades_windows_reserved_names`：NUL/con/NUL.txt/COM1/lpt9.bin/AUX.tar.gz 全降级，nullify.txt 不误伤）；`cargo test --bin gsearch fetch::` → 68 passed（含扩充的 `is_private_ip_cases`：100.64.0.1/100.127.255.254 私网、100.63/100.128 公网、fe80/fe9f/febf 私网、fec0/fe7f 公网；新 `resolve_host_parses_bracketed_ipv6_literals`）；`cargo test --lib verify::` → `18 passed`（含新 `curl_args_ends_with_double_dash_before_url`：`-K/etc/passwd` 形态 URL 被 `--` 隔离）。

## 5. 4bq — M 四项（PM 预拍板）

| 项 | 修复 |
|---|---|
| M2 meta.proxy 凭据脱敏（必修） | `redact_proxy` 从 browser.rs 私有 fn 迁 `util.rs` 提为 `pub`（browser.rs 内 2 处 tracing 调用点与其 2 个单测随迁，断言原文不动）；meta 装配 4 处套用（main.rs search 主信封 / captcha_timeout 信封 / searxng_degraded 信封 + general.rs browse 信封），键不变值脱敏 `user:pass@host` → `user:***@host`；`proxy: None` 的两处（similar/batch）无需处理 |
| M3 结果集导航绕门（必修） | `postproc::open_page` 入口过 `ensure_browsable_url(url, allow_private)`（read/read_full/登录墙重抓同覆盖）；`ReadOpts` 加 `allow_private` 字段，`SearchArgs` 新增 `--allow-private` flag 透传（与 browse/fetch 同款写法）；browse 门错误文案对 `search --browse N` 用户同样可行动 |
| M4 fetch_in_page Rust 侧长度复核（必修） | b64 解码前复核 `b64.len() > FETCH_IN_PAGE_MAX_BYTES / 3 * 4 + 4` 即 bail——JS 侧 32MB 阈值被页面 JS patch 时 Rust 侧兜底，报错非静默 |
| M1 DNS rebinding TOCTOU（允许降级） | 按 PM 拍板仅注释登记已知限制（`build_client` doc：门校验与实际连接两次解析可换 IP；完整修复需自定义 resolver 钉扎，reqwest API 仅能钉首跳），不做代码级修复 |

【验证命令+关键输出】`cargo check --all-targets` → 0 errors / 0 warnings；`cargo test --lib browser::` → `13 passed`（redact 测试随迁后 profile_name/保留名回归绿）；`cargo test --lib util::` → 12 passed（redact_proxy 双用例随迁原样通过）；M3 的门复用 `ensure_browsable_url` 既有测试网（general.rs 13 passed 含门语义 7 断言）；`search --allow-private` 的 clap 透传由 `cargo check --all-targets`（含 bin 编译）+ 既有 ArgGroup 测试保证。

## 汇总验证

```
cargo check --all-targets          → Finished，0 errors / 0 warnings
cargo test --bin gsearch fetch::   → 68 passed / 0 failed
cargo test --bin gsearch postproc::→ 16 passed / 0 failed / 2 ignored
cargo test --bin gsearch general:: → 13 passed / 0 failed
cargo test --bin gsearch shell::   → 3 passed / 0 failed
cargo test --lib util::            → 12 passed / 0 failed
cargo test --lib verify::          → 18 passed / 0 failed
cargo test --lib browser::         → 13 passed / 0 failed
cargo test --lib types::           → 12 passed / 0 failed
```

stdout 成功契约：0vk 改动经逐字节等价单测 + 全部 fetched_json 键级断言锁死；2se 只对「原本输出空」的页面产生差异（空→有正文，修复目标）；cgp/4bq 的 stdout 差异仅 meta.proxy 值脱敏（修复目标）与新增错误路径（超限 bail/导航门拒）。

## 残留风险（如实登记）

1. **shell 会话 `read` 不走 postproc::read**（shell 自组装 render_read）——结果集 URL 无门。不在本批 context 清单（清单只点名 search --read N / open_page），同类威胁未覆盖。
2. **`search --dl N`** 不经 open_page，URL 门未加（审计 M3 (a) 提及 read/dl，PM 清单未列 dl；dl 内网放行另有 general.rs:74 文档化决策交叉，留 PM 裁量）。
3. **M-3 原树摘取副作用**：`project_json_paths` 收 owned 后原 Value 被掏成 Null——当前唯一调用方已适配，未来新调用方需知晓（fn doc 已注明）。
4. 全量 build/test/clippy 按纪律未跑，归 PM 合并态统一执行。
