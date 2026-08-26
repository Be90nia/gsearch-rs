#!/usr/bin/env bash
# Benchmark gsearch-rs vs Python plsearch reference.
# 用法：bash bench.sh [iterations=3]
#
# 环境：需同时有 Python plsearch 与本二进制。
# Python 版跑同一个 query，对比耗时 + 结果数。

set -euo pipefail

ITERS="${1:-3}"
QUERY="${QUERY:-fastapi tortoise orm tutorial}"
LIMIT=3
BIN="${BIN:-./target/release/gsearch.exe}"

echo "===== Benchmark gsearch-rs vs plsearch ====="
echo "Query: \"$QUERY\"  Iterations: $ITERS  Limit: $LIMIT"
echo

if [ ! -x "$BIN" ]; then
    echo "ERROR: binary not found at $BIN"
    echo "Build first: cargo build --release"
    exit 1
fi

# --- gsearch-rs ---
echo "--- gsearch-rs ---"
gsearch_total=0
gsearch_pass=0
for i in $(seq 1 "$ITERS"); do
    start=$(date +%s%3N)
    count=$("$BIN" search "$QUERY" --limit "$LIMIT" --json 2>/dev/null | grep -c '"url"' || true)
    end=$(date +%s%3N)
    elapsed=$((end - start))
    gsearch_total=$((gsearch_total + elapsed))
    [ "$count" -ge 1 ] && gsearch_pass=$((gsearch_pass + 1))
    echo "  iter $i: ${elapsed}ms, ${count} results"
done
gsearch_avg=$((gsearch_total / ITERS))
echo "  avg: ${gsearch_avg}ms, success: ${gsearch_pass}/${ITERS}"
echo

# --- Python plsearch ---
PLSEARCH="${PLSEARCH:-python D:/Sdk/plsearch/src/plsearch/main.py}"
if [ -n "${SKIP_PYTHON:-}" ] || ! command -v python >/dev/null 2>&1; then
    echo "--- Python plsearch: SKIPPED (set SKIP_PYTHON=0 to force) ---"
else
    echo "--- Python plsearch (reference) ---"
    py_total=0
    py_pass=0
    for i in $(seq 1 "$ITERS"); do
        start=$(date +%s%3N)
        # 仅当用户配置 PLSEARCH 环境变量时跑
        if [ -f "${PLSEARCH%% *}" ] 2>/dev/null; then
            count=$($PLSEARCH "$QUERY" --limit "$LIMIT" 2>/dev/null | grep -c '"url"' || true)
        else
            count=0
        fi
        end=$(date +%s%3N)
        elapsed=$((end - start))
        py_total=$((py_total + elapsed))
        [ "$count" -ge 1 ] && py_pass=$((py_pass + 1))
        echo "  iter $i: ${elapsed}ms, ${count} results"
    done
    py_avg=$((py_total / ITERS))
    echo "  avg: ${py_avg}ms, success: ${py_pass}/${ITERS}"
fi

echo
echo "===== Summary ====="
echo "gsearch-rs avg:  ${gsearch_avg}ms"
[ -n "${py_avg:-}" ] && echo "plsearch avg:    ${py_avg}ms"
echo "Target (PLAN §6): 7-10s for unblocked IP, single iteration"
echo
echo "Note: 撞码场景会卡 120s（CAPTCHA 人工解）；本 benchmark 默认直连未撞码时跑。"