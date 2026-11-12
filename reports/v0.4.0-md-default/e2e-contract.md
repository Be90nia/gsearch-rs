# v0.4.0 默认 md 输出 —— 独立端到端验证契约（PM 派 e2e-runner 用）

## 背景

gsearch v0.4.0 把默认输出从 JSON 反转为 md 排版（`--json` 变显式开关）。
PM 已用同源对照实测：JSON 436 tok vs md 308 tok（−29%），且独立 agent 盲测（JSON 组 5/5、md 组 5/5，答案逐字一致）证明 md 形态**不损 AI 提取准确率**。

**验证目标**：证明新二进制**默认输出就是 md**、**AI 能从 md 正确提取**、**`--json` 仍给出与 v0.3.0 键集一致的 JSON**、**退出码与错误路径未回归**。

## 被验对象

- 新二进制：`D:/Project/gsearch-rs/target/release/gsearch.exe`
- 对照基线（v0.3.0）：`D:/Project/gsearch-rs/target/gsearch-v0.3.0-baseline.exe`

## 验证项（逐项跑，贴真实输出）

### V1 默认输出不是 JSON
```bash
cd D:/Project/gsearch-rs
GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 ./target/release/gsearch.exe search "rust async" --limit 3 2>/dev/null | python -c "import sys,json;json.loads(sys.stdin.read())"; echo "exit=$?"
```
期望：`exit` 非 0（默认输出解析失败 = 已是 md）。若 exit=0 → **FAIL**。

### V2 默认输出是 md 结构
```bash
GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 ./target/release/gsearch.exe search "rust async" --limit 3 2>/dev/null
```
期望：首行是 meta 头（`provider: ... | results: ... | limit: ... | ...`），随后是 `1. 标题 [domain_class]` + 缩进 URL + 缩进摘要的列表。**贴全文**。

### V3 --json 仍是合法 JSON 且键集与 v0.3.0 一致
```bash
GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 ./target/gsearch-v0.3.0-baseline.exe search "rust async" --limit 3 --json > target/e2e_a.json 2>/dev/null
GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 ./target/release/gsearch.exe search "rust async" --limit 3 --json > target/e2e_b.json 2>/dev/null
python -c "
import json
a=json.load(open('target/e2e_a.json')); b=json.load(open('target/e2e_b.json'))
print('meta keys same:', list(a['meta'])==list(b['meta']), list(b['meta']))
print('run keys same:', list(a['run'])==list(b['run']), list(b['run']))
print('result keys same:', list(a['results'][0])==list(b['results'][0]), list(b['results'][0]))
"
```
期望：三行全 `True`。（**只比键集**——SERP 内容随时间漂移，逐字节比必假失败。）

### V4 --json 输出内容零变更（同一次查询、同一二进制内自比）
```bash
GSEARCH_SEARXNG_URL=http://192.168.89.249:8888 ./target/release/gsearch.exe search "rust async" --limit 3 --json 2>/dev/null | python -c "
import sys,json; d=json.loads(sys.stdin.read()); print(json.dumps(d,ensure_ascii=False)[:400])"
```
期望：单行紧凑 JSON（无缩进），可 json.loads。

### V5 fetch 默认输出是 md 正文
```bash
./target/release/gsearch.exe fetch https://example.com 2>/dev/null | head -10
```
期望：首行 meta 头，随后 `# <标题>` + md 正文。贴输出。

### V6 doctor / verify 默认输出是 md
```bash
./target/release/gsearch.exe doctor 2>/dev/null | head -10
./target/release/gsearch.exe verify https://example.com 2>/dev/null | head -6
```
期望：doctor 是 `- [OK]/[FAIL] 检查名: message` 清单；verify 是每 URL 一行结果。贴输出。

### V7 AI 消费盲测（核心）
把 V2 的**默认 md 输出原文**写进 `target/e2e_md.txt`（不加工、不裁剪），然后：
- 派**独立 subagent**（不复用本代理上下文）只读 `target/e2e_md.txt`，回答 5 个提取问题；
- ground truth 由 PM 从 `--json` 输出（同一份查询）直接取，逐字核对；
- 判定标准：**5 题全对**，且 agent **能答出 provider 与 truncated**（md meta 头里的信息必须可被读出——若 agent 读不出 meta 头，说明 meta 头格式不可读，需报 FAIL 并给建议）。

问题：
1. 结果一共有几条？
2. 第 2 条结果的 URL 是什么？
3. 第 3 条结果的标题是什么？
4. 第 4 条结果的摘要开头 40 个字符是什么？
5. 第 1 条结果的标题和 URL 分别是什么？
附加题（md 专有）：6. 搜索来源是哪个引擎？7. 结果集是否触顶（还有更多结果）？

### V8 退出码与错误路径无回归
```bash
./target/release/gsearch.exe search "" 2>/dev/null; echo "空query exit=$? (期望 2)"
./target/release/gsearch.exe search "x" --read 99 2>/dev/null; echo "越界 exit=$? (期望 2)"
./target/release/gsearch.exe verify "not-a-url" 2>/dev/null; echo "坏URL exit=$? (期望非 0)"
```
贴输出。

### V9 token 对比（新版默认 vs v0.3.0 默认）
同一查询同一 limit，用 tiktoken（若不可用则按 bytes/4 估，并注明）：
```bash
python -c "
import tiktoken
enc=tiktoken.get_encoding('cl100k_base')
for f,l in [('target/e2e_md.txt','v0.4.0 默认(md)'),('target/e2e_b.json','v0.4.0 --json')]:
    s=open(f,encoding='utf-8').read(); print('%-22s %6dB %5d tok' % (l,len(s.encode()),len(enc.encode(s))))
"
```
期望：md 明显小于 JSON。

## 判定标准

- V1–V6、V8、V9 全 PASS；
- V7 的 5 题全对 + meta 头两附加题能答出；
- 任一 FAIL → 报 `VERDICT: FAIL` 并给出失败项 + 复现命令 + 现象描述（**不要自行改代码修**）。

## 输出

报告写到 `D:/Project/gsearch-rs/reports/v0.4.0-md-default/e2e-report.md`，回执只带：首行 `VERDICT: PASS / FAIL` + 每项 V1–V9 的 PASS/FAIL 一行 + 关键输出片段 + 路径。

**不要 commit、不要 push、不要改代码。**
