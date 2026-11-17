# CLAUDE.md — gsearch-rs 项目规范

本文件是本仓（gsearch-rs）的项目级纪律，供任何 agent / 人在本仓工作时遵循。
**优先级**：用户当次指令 > 本文件 > 通用 skill 习惯。

本仓是 Rust CLI：单 exe「搜索 + 通用浏览器代理」。行为真相源永远是**源码 + 测试**，
文档是派生品；文档与实测冲突时以实测为准并顺手修文档。

---

## 1. 构建与测试

```bash
cargo test --lib --bins --tests -- --skip live_tests   # 全量（CI 同款；跳过需真浏览器的 live 用例）
cargo clippy --all-targets -- -D warnings               # CI 同款 lint 门
```

- `cargo test`（不带参数）会连 `#[ignore]` 之外的 live 用例一起编进来；**live 用例按惯例不跑**
  （需真 Chrome + profile，`cargo test --ignored` 才跑）。
- **`tests/cli_golden_bytes.rs` 是逐字节 stdout 契约**：跑真实 CLI 二进制 + 本地回环假 SearXNG，
  stdout 归一后与 `tests/fixtures/golden_search_*.txt` 逐字节比对，并锁死正常态 meta 键集与键序。
  **改了输出形态（键序 / 键集 / md 排版 / 信封结构）必须同步该文件与 golden fixture**，
  否则 CI 必红。golden 来源与归一规则见 `tests/fixtures/cli_golden_provenance.md`。
- `tests/fixtures/golden_search_*.txt` 必须保持 **LF**（`.gitattributes` 已声明 `eol=lf`），
  Windows 本机 `core.autocrlf=true` 会把 LF 转成 CRLF 导致本地假红——不要去"修" fixture 行尾。

## 2. 产物落盘路由

| 产物类型 | 落位 | 入库 |
|---|---|---|
| 调研 / 审计 / 验证报告 | `<repo>/reports/`（日期前缀，如 `reports/2026-11-17-xxx.md`） | 是 |
| 深度调研 / 竞品分析 / 上游 issue 草稿 | `<repo>/docs/research/` | 是 |
| 临时脚本 / 探针输出 / 抓取样本 | **系统临时目录**（如 `%TEMP%\gsearch-doc\`） | 否 |

- **临时文件禁落 repo 根或工作区根**。本仓已发生过 `.probe/` 在 repo 根堆出 60+ 临时文件、
  以及 `tmp_fulltest/` 这类目录出现在根下——收尾前必须清零（`rm -rf` 该目录并确认
  `git status --short` 里没有它们）。
- 临时目录若必须放仓库内（如需要被测试夹具引用），用 `.gitignore` 已覆盖的模式，
  且**收尾必须删**。

## 3. 行尾纪律（Windows 工作树）

- 本机 `git config core.autocrlf=true`，Windows 工作树里 `.rs` 是 **CRLF**；仓库内容规范是 LF。
  `git status` 里的 `LF will be replaced by CRLF` 警告**无害**，不要去改。
- **行级 commit 工具（逐行写回型编辑）会做全文件行尾归一**：一次只改几行的实质修改，
  落盘后整个文件被重写。实证：`0c9b8ae` 提交 `src/main.rs` 显示 `2161` 增 `2159` 删、
  `src/skeleton.rs` `813` 增 `764` 删——实质语义差异只有数行。
- **提交前必须逐文件审 diff**，确认没有全文件污染：

```bash
git diff --stat            # 增删行数与语义改动量是否匹配
git diff -- src/foo.rs     # 逐文件看
```

  出现「改 3 行却显示几百行增删」= 行尾归一污染，**修回**（恢复 CRLF 或用 `git checkout --` 重做
  该文件的最小改动），不要把污染带进提交。

## 4. 浏览器测试环境

```bash
"D:/Program Files/Python 3.14/python.exe" -m http.server 8931 --bind 127.0.0.1
```

- Windows 上 `python` 在 PATH 为 `D:\Program Files\Python 3.14`（本机实测 Python 3.14.6）。
- 本地伺服只起 `127.0.0.1`（`--bind 127.0.0.1`），**不要**绑 `0.0.0.0`。
- 读私网页面必须显式放行：`gsearch browse http://127.0.0.1:8931/x --allow-private`；
  `fetch` 私网同理（另有 `GSEARCH_FETCH_ALLOW_PRIVATE=1`）。默认私网门是拒绝（SSRF 防护）。
- 测完**停掉本地伺服**（释放端口与句柄），否则下次起服务会撞端口。
- Windows 下 `python` 与 Git Bash 的 `/tmp` **不互通**：需要临时文件时用真实 Windows 路径。

## 5. 零 token 纪律

- OMP `bash` 工具用 `timeout: 0` **一次调用跑完**（长任务也不分段等）。
- **禁两段式**：`bash` 起任务 → `wait`/`hub` 等 → 再 `bash` 取结果。改成一条 bash 命令里
  自己等到结束再打印结果。
- 需要多段输出时用文件重定向落在临时目录，再一次性读回——不要靠多次轮询。

## 6. commit 规范

- 消息格式：`<type>: <description>`（`<type>` 取 `feat` / `fix` / `docs` / `test` / `chore` / `refactor`）。
  description 用中文，一行说清影响面；相关 bd 单号写在括号里，如
  `fix: browse --from 接线与 AdaptiveRead 滑动窗口（bd p8r 续）`。
- **逐文件 `git add <file>`**；**禁 `git add -A` / `git add .`**（会带上别的 agent 的改动）。
- **commit 归 PM**：子代理只改文件、只跑验证、只写报告，**一律不 commit / 不 push**。
- 改动落地前先看 `git status --short`，确认工作树里的其它改动不是自己的。

## 7. bd（beads）用法

- **持久 issue 一律用 `bd`**（本仓 issue 都挂在 bd 上）。
- **禁 `bd edit`**（它开 `$EDITOR` 交互，agent 不可用）；改字段用 `bd update` / `bd note` / `bd comment` 等
  非交互子命令。
- 子单挂父单：`bd create "<标题>" --parent gsearch-rs-<父单号>`。
- **完工要核销**：`bd close gsearch-rs-<单号> --reason "<一句话结论>"`。
  没 close 的单子等于没做完。

## 8. 文档同步纪律

- **改了行为，同批次同步文档**，禁事后补——漂移的起点就是「先改码、文档下个版本再说」。
- 同一批次内做：
  1. `CHANGELOG.md` 顶部的 `## [Unreleased]` 段（Keep a Changelog 格式；该段已存在于文件最前，
     其下是最新发布段 `## [0.4.1]`）——改动条目写进 `Unreleased`，**不要**直接改已发布段。
  2. 对应文档：`README.md`（人读手册与输出契约）、`USER_GUIDE.md`（场景指南）、
     `docs/PLAN.md`（设计变更时）、`llms.txt`（agent 入口）、`CLAUDE.md`（纪律变更时）。
  3. `docs/PLAN.md` 是**权威设计文档**——架构/形态变了必须改它。
- **文档不写参数默认值全表**：`llms.txt` 与 skill 类文档只写「怎么用 / 怎么判 / 坑」，
  参数真相指向 `gsearch --help` / `gsearch <cmd> --help`（运行时取当版）。
  静态抄参数表必随版本腐坏——本仓已实证过（`--from` 语义修完后旧描述全作废）。
- 文档里的每个事实性断言都要能追到**实测命令与输出摘录**；查不到就标「未验证」或删掉，
  不写「可能 / 大概」。

## 9. 不得替用户改环境

- **禁擅自 `setx`**（写用户级环境变量）、**禁擅自改 `~/.gsearch/config.json`**、
  禁改系统代理设置、禁动注册表环境变量值——除非 PM 明确授权并指名要改的值。
- 需要换代理/换 profile 时用**单次命令 flag**（`--proxy` / `--config`），不落环境变量、不落配置文件。
- `GSEARCH_PROXY` 等环境变量**只有 gsearch 读**，不改系统代理——保持这个边界。
- 查当前环境用只读命令（`reg query "HKCU\Environment" /v GSEARCH_PROXY`、
  `echo "[$GSEARCH_PROXY]"`），别用会写入的探测。
