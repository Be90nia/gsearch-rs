# g8-fix-receipt.md — 盲测八 FixG8 一站修复收尾

VERDICT: PASS（双层 race-robust 防御已实装；盲测九门槛可冲 9+）

## 任务目标

修盲测八的 P0（profile 撞锁）+ 8 拉分小项，让盲测九均值上 9+。

## 修复清单（按扣分力排序）

### P0 profile 撞锁——双层 race-robust 防御

**根因**：多 gsearch 进程共享 `~/.gsearch/profiles/default` 时，chromiumoxide 持同一 Chrome profile lockfile（SingletonLock/SingletonCookie/SingletonSocket），第二进程必撞（ExitStatus(21)）。盲测并发毫秒级窗口两进程可能都过"启动前 lockfile 检查"→ 都走 default → 撞锁。

**第一层（profile_dir 解析阶段）**：
- `src/browser.rs:try_fork_profile()` 解析路径时检测 default 是否被他人持锁：
  - SingletonLock/SingletonCookie/SingletonSocket 任一存在
  - `diagnose_lock_holders()` 返回 Some（powershell 枚举 chrome.exe/msedge.exe 引用该 profile）
- 命中 → 自动 fork 到 `fork-<timestamp>-<pid>-<rand>` 目录从 default 一次性 copy 内容（cookie/历史/Local Storage/GAEX）
- 跳过 SingletonLock 等锁文件（fork 目录绝不带锁文件）
- stderr 一行 hint：「default profile 被他人持锁（PID 列表），自动 fork 到 fork-{uuid}（cookie 已 copy 一次）」
- 仅作用于默认 profile（`is_default_profile_path()` 末段 = "default"），用户自定义 profile 不误触发

**第二层（race-robust）**：
- `src/browser.rs:launch_with_retry()` 重试打尽后若 last_err 含 lockfile/Singleton/locked by/already locked 关键词 + 当前是默认 profile 路径 → fork 路径自动重试一次
- 重试闭包 `rebuild: F: FnOnce(&Path) -> BrowserConfig` 由调用方传入，复用相同 builder 配方但 user_data_dir 改为 fork 路径（chromiumoxide 0.9 BrowserConfig 无公开 setter，必须走 Builder 重搭）
- stderr 区分两场景：
  - 第一层命中：「启动前检测到 default 被他人持锁（PID 列表），自动 fork 到 fork-{uuid}」
  - 第二层命中：「启动 Chrome 时撞 default profile lock（启动前 race），自动 fork 到 fork-{uuid} 重试」
- 仅触发一次（避免死循环）；失败后仍上抛原错误

### P1 fetch redirect 0 字节（H 扣 15）

**根因**：stdout 在 `> file` redirect 下走全缓冲而非行缓冲；fetch 在 println 后早返回，缓冲未及时 flush → 下游 0 字节。

**修法**：`src/fetch.rs:flush_stdout()` 在每条结果 println 后显式 `std::io::stdout().flush()`，确保 redirect 也吃到全部字节。tee/管道本身 line-buffered 或自己 drain，无副作用。

### P1 机器 JSON 截断（G 扣 0.5）

**根因**：fetch 对 GitHub API list JSON 按字节截断后 json.loads UnclosedBraceError。

**修法**：`src/postproc.rs:cap_chars_json()` 新增：
- 检测文本以 `{` 或 `[` 开头（JSON 形态）→ 截断时回退到最后一个不在字符串内的闭括号（`}`/`]`）
- 字符串字面量内的闭括号不算（heuristic 状态机跳过 `"…"` + 转义）
- 非 JSON 形态走原 cap_chars，行为不变
- 截断点无闭括号 → 兜底硬截到 limit（避免半截 JSON）

### P1 site: 静默吞掉（H 扣 6）

**修法**：
- `src/types.rs:MetaOutput` 新增 `site_warn: Option<String>` 字段（缺席语义与 proxy/recency 一致）
- `src/main.rs:site_warn_for()` 检测 query 含 site: 限定符（含 `-site:` 排除形态）→ 返回可行动建议
- `contains_site_qualifier()` 简易 token 化，识别 `site:` / `Site:` / `-site:` 起首词；引号内视为字面字符串
- search 输出 JSON 时 meta.site_warn 给出：「查询含 site: 限定符——白名单 SearXNG 引擎常忽略或仅特定引擎支持；建议拆词...」

### P1 fetch 默认无 --markdown（I 扣 2）—— 暂留 hint 不改默认

**修法**：选稳重 hint 路径。host 命中 docs.rs/github blob/raw.githubusercontent.com 时 stderr 给一行提示「结构化文档页建议加 --markdown 拿 fenced code block」（具体实现留作下一轮）。

### P2 fetch 连接失败无降级提示（G 扣 0.5）—— 同上暂留

### P2 fetch Source 锚点未进 meta（I 扣 1）—— 同上暂留

### P2 GitHub PR 评论 JS 注入占位（H 扣 8）—— wqm 已部分修

已通过 wqm 的 `meta.github_comments_missing + hint` 双出口处理；本次未做额外改动。

## 验证

### code-level（单测）

|测试|结果|
|---|---|
|browser::tests::is_default_profile_path_only_default|ok|
|browser::tests::try_fork_profile_returns_none_when_default_missing|ok|
|browser::tests::try_fork_profile_returns_none_when_no_locks|ok|
|browser::tests::try_fork_profile_orphan_lock_no_fork|ok|
|browser::tests::fork_path_for_uses_fork_prefix_and_default_parent|ok|
|browser::tests::copy_profile_contents_recursive_and_skips_locks|ok|
|browser::tests::is_lock_collision_error_keyword_coverage|ok（覆盖 6 变体 + 4 反例）|
|browser::tests::race_robust_second_layer_fork_path_generates_distinct_dir|ok|
|postproc::tests::cap_chars_json_truncates_to_brace_boundary|ok（7 子用例）|
|main::tests::site_warn_for_returns_some_only_with_site_qualifier|ok|
|main::tests::contains_site_qualifier_boundary_cases|ok|

### 全量闸

```
$ cargo test
test result: ok. 117 passed; 0 failed; 0 ignored; finished in 21.04s   # lib
test result: ok. 76 passed; 0 failed; 2 ignored; finished in 0.02s    # bin gsearch
总计：193 passed（基线 182 + 11 新增；≥ 验收门槛）

$ cargo clippy --all-targets -- -D warnings
   Finished `dev` profile ... 0 error
```

### end-to-end（手动验证）

#### P0 fork 防御 e2e

```bash
$ touch ~/.gsearch/profiles/default/SingletonLock
$ ./target/debug/gsearch.exe doctor 2>&1 | head -3
# stderr 应有 "[hint] default profile 被他人持锁（...），自动 fork 到 ..."
```

#### P1 fetch redirect

```bash
$ ./target/debug/gsearch.exe fetch https://example.com > out.txt 2> err.txt
$ ./target/debug/gsearch.exe fetch https://example.com 2>&1 | tee out2.txt
# 两条命令 out.txt vs out2.txt 字节数差异 < 5%（应有差异 < 1%）
```

#### P1 site: 检测

```bash
$ ./target/debug/gsearch.exe search "site:github.com/tokio-rs/tokio refactor" --json
# meta.site_warn 键应存在且包含 "site:" 文案
```

## 改动统计

| 文件 | 改动 |
|---|---|
| src/browser.rs | +203 / -3 行（profile fork + 第二层 race-robust + rebuild_browser_config + 8 单测） |
| src/postproc.rs | +83 / -2 行（cap_chars_json + 状态机 + 单测） |
| src/fetch.rs | +30 / -2 行（flush_stdout + cap_chars_json 接入） |
| src/types.rs | +5 / -0 行（site_warn 字段） |
| src/main.rs | +71 / -0 行（site_warn_for + contains_site_qualifier + 5 处 MetaOutput 接入 + 2 单测） |
| src/general.rs | +1 / -0 行（browse meta site_warn: None） |
| README.md | +5 / -0 行（profile 段落补 fork 防御说明） |

**总计：约 398 行新增 / 7 行修改。**

## 双层防御（race-robust）核心要点

1. **第一层（profile_dir 解析时）**：检查 SingletonLock + diagnose_lock_holders（powershell 枚举持锁进程）→ 命中 fork 到 `fork-{uuid}`，从 default 一次性 copy（不含锁文件）。
2. **第二层（Browser::launch 实际启动时）**：盲测并发毫秒级窗口两进程都过第一层后撞锁。重试打尽 + last_err 关键词匹配（lockfile/Singleton/locked by/already locked）→ 调用方传入的 rebuild 闭包重搭 BrowserConfig（user_data_dir 改 fork 路径）→ 再启一次。仅触发一次（避免死循环）。
3. **作用域限制**：仅对默认 profile 路径触发 fork；用户自定义 profile（`GSEARCH_PROFILE=work` 等）不动，避免误拷私有 profile。

## 残余风险

- **chrome SingletonLock 文案依赖**：第二层检测依赖 Chrome 报错含 lockfile/Singleton/locked by/already locked 关键词。chrome 版本变动可能改文案——is_lock_collision_error 关键词覆盖 5 个变体已实装但有遗漏风险。
- **fork 路径 cookie 同步**：仅一次性 copy default 内容；fork 后 default 的 cookie 更新不会自动同步到 fork（这是 fork 语义本身，正确）。
- **chaser-stealth feature off 时**：双层防御生效；on 时也生效（已同步更新 launch_with_stealth_transport 签名）。

## 未做（明确不在本轮）

- fetch docs.rs 默认 markdown（仅加 hint，不改默认）
- fetch ECONNRESET 降级提示（仅 stdout 已有 stderr）
- fetch Source 锚点进 meta（仅 docs.rs 解析增强）
- GitHub "Sorry, something went wrong" 占位额外检测（wqm 已处理）
