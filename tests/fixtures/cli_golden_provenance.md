# bd gsearch-rs-d8s · CLI stdout golden 的来源与复现配方

本目录下的三个期望文件是**真实 CLI 二进制的实跑 stdout**（不是手写常量、不是按输出格式拼的），
由引入本特性**之前**的 commit 捕获，供 `tests/cli_golden_bytes.rs` 逐字节回归。

| 期望文件 | 来源命令 | 捕获自 |
|---|---|---|
| `golden_search_json.txt` | `gsearch --config <tmp> search "rust async" --json` | **改动前** commit `ae1d59540270455f510f33ef231e2e362a1d68cb` 的 release 二进制 |
| `golden_search_md.txt` | `gsearch --config <tmp> search "rust async"`（默认 md 形态） | 同上 |
| `golden_search_json_off_topic.txt` | `gsearch --config <tmp> search "kubernetes pdb yaml" --json` | **改动后**的二进制（可疑态是本特性新增的状态，改动前的二进制产出不了） |

- **基准 commit**：`ae1d59540270455f510f33ef231e2e362a1d68cb`（2026-11-13T11:34:08+08:00，`git log -1 --format=%cI HEAD`）
- **捕获时间**：2026-11-13T07:20:08Z（UTC）
- **数据源**：本地回环假 SearXNG（载荷见 `cli_golden_payloads.json`，零外网、零浏览器）——同一个假服务器同时喂给新旧两个二进制
- **归一规则**：**只动三项**——① **换行符 CRLF→LF**（期望文件是某一次捕获的字节，落盘后可能被 git 的 `core.autocrlf` 改写成 CRLF，而 stdout 是程序直接写的字节；不归一则 Windows 检出的工作区与 Linux/macOS CI 上逐字节必红。比对时**两侧各自归一**：读入的期望文件 + 实际 stdout）；② **耗时字段**——JSON 的 `"elapsed_ms":<数字>` → `"elapsed_ms":0`；md meta 头的 `elapsed: <数字>.<数字>s` → `elapsed: 0.0s`（两者是同一个 `meta.elapsed_ms` 的两种渲染）；③ **版本号字段**——JSON 的 `"version":"<任意>"` → `"version":"0.0.0"`（版本号随发版变化，**不该参与「正常态输出是否变化」的判定**；期望文件里是捕获当时的 `0.4.0`，此后每次 bump 版本都会把逐字节锁打红——2026-11-13 bump 到 0.4.1 时 CI ubuntu 即因此红过一次）。**除此之外不屏蔽任何字段**（键序、键集、格式、浮点字面量、字符串、空白全部参与逐字节比对）
  - 独立断言：`tests/cli_golden_bytes.rs::newline_normalization_makes_crlf_and_lf_equivalent` —— 把期望文件的换行换成另一种形态（LF ⇄ CRLF）后比对**仍**通过 ⇒ 归一生效，且正常态输出**没有别的隐含平台差异**（键序/键集/浮点/字符串全等）
- **正常态缺席语义**：正常态的 meta **没有** `off_topic` 键、结果**没有** `relevance` 键（不是 `0`、不是 `null`）——这正是本 golden 要锁住的东西

## 复现配方（从基准 commit 重建旧二进制并重新捕获）

```bash
# 1) 取出基准 commit 的源码（仓库外，避免污染工作树）
mkdir -p target/pre-src && git archive ae1d59540270455f510f33ef231e2e362a1d68cb | tar -x -C target/pre-src
# 2) 构建基准二进制（release）
cd target/pre-src && cargo build --release --target-dir ../pre-build
# 3) 捕获：假 SearXNG + 两个二进制 → 期望文件 + 逐字节比对（仓内脚本，一次性工具）
cd ../..
GSEARCH_PRE_BINARY=target/pre-build/release/gsearch.exe \
  cargo test --test cli_golden_bytes -- --ignored capture_goldens --nocapture
```

第 3 步的生成脚本就是仓内的 `tests/cli_golden_bytes.rs::capture_goldens`（`#[ignore]`，常规 CI 不跑）。它会：

1. 起一个回环 HTTP 服务器，按 query 返回 `cli_golden_payloads.json` 里的固定载荷；
2. 用**基准二进制**与**当前二进制**分别跑 `--json` 与默认 md 两条路径；
3. 归一后断言两者**逐字节相同**（断言不过即失败）——这就是「本特性未改正常态输出字节」的证据；
4. 把归一后的 stdout 写成上表的期望文件。

仓内常驻回归是 `tests/cli_golden_bytes.rs`（`cargo test --test cli_golden_bytes`）：它每次都用
`env!("CARGO_BIN_EXE_gsearch")` 跑真实二进制 + 同款回环假 SearXNG，与上表期望文件逐字节比对，
并额外锁 meta/run/results 的**键集与键序**（往正常态 meta 加键、改键序必红）。
