# SearXNG upstream issue draft — bing engine returns unrelated results

> 状态：待用户审核后由本人账号（Be90nia）提交 upstream。
> 背景：旧 issue #6937（2026-11-13）被 maintainer 以 "heavy AI / unreadable" 关闭，且**其 TLS 指纹结论经复测为误判**。
> 本稿遵守 SearXNG `AI_POLICY.rst`：短、结论先行、单一诉求、给出可自查的复现步骤；正文为中文底稿的英文翻译，措辞保持直白。
> 维护者明确表示 "if you want to make a new issue that is fine too"，并会 reopen PR #6671。

---

## Title

`bing web engine: results unrelated to query — caused by a missing market parameter`

## Body

```markdown
## Summary

The `bing` web engine can return a result set that does not correspond to the query at all. The trigger is that `bing.py` sends **no market parameter** for the `us`, `cn` and `ru` locales.

This replaces my earlier report #6937, which blamed the TLS fingerprint. That was wrong — I measured it again and the fingerprint makes no difference (see below). Sorry for the noise.

## Reproduce

Same query, same IP, no TLS impersonation needed — just open these two URLs:

- https://www.bing.com/search?q=how+to+install+docker+on+debian&setlang=en
- https://www.bing.com/search?q=how+to+install+docker+on+debian&setlang=en&mkt=en-US

The first one returns results unrelated to the query (Dingbats/logos, Zhihu, Meta, …) and the set changes on every request. The second one returns the correct results (Install Docker Engine on Debian | Docker Docs, …).

## Why

In `searx/engines/bing.py`, `request()`:

    if cc and cc not in ("us", "cn", "ru"):  # bing just sends junk for these
        query_params["cc"] = cc

For `en-US` this leaves only `setlang=en` in the query string. `bing_news` and `bing_videos` call `get_locale_params()` and send `mkt`; the web engine never does.

Measured on my instance (same IP, interleaved, 3–4 runs per row):

| request | correct results |
|---|---|
| `setlang=en` (what bing.py sends for en-US) | 0/4 |
| `setlang=en&cc=us` | 4/4 |
| `setlang=en&mkt=en-US` | 4/4 |
| `setlang=de&cc=de` | 3/3 |
| `setlang=ja&cc=jp` | 3/3 |
| `setlang=zh&mkt=zh-cn` | 0/3 |

TLS fingerprint is not involved: plain `urllib` and `curl_cffi` with `impersonate="chrome"` give identical results for the same URL.

## Suggestion

Send `mkt` (or `cc`) for all locales, like `bing_news` / `bing_videos` do.

About the `# bing just sends junk for these` comment: for `us` I measure the opposite — with `cc=us` the results are correct, without it they are garbage. For `cn` and `ru` the results are wrong with and without a market parameter, so those two may be a separate issue.

Environment: official `searxng/searxng:latest` image, version 2026.11.12, `curl_cffi` 0.17, datacenter egress IP.

Related: #4964 (same symptom, closed without a cause), #6671, #5461.
```

## 提交方式

```bash
gh issue create --repo searxng/searxng \
  --title "bing web engine: results unrelated to query — caused by a missing market parameter" \
  --body-file <(sed -n '/^## Body/,/^## 提交方式/p' docs/research/searxng-bing-upstream-issue-draft.md | sed '1d;$d')
```

## 复现环境（本地已验证的对照脚本思路）

1. 同一 IP、同一时刻，交错请求 A（`setlang=en`）与 B（`setlang=en&mkt=en-US`），每组 3-4 轮
2. 判定：结果标题是否含 query 词元（docker / debian）
3. 对照组：换客户端（`urllib` 无 impersonation vs `curl_cffi impersonate=chrome`）→ 结果一致 ⇒ 排除指纹
4. 可证伪预测：`bing.py` 跳过 cc 的三市场（us/cn/ru）应全坏，其余（de/ja/gb/fr）应全好 —— 实测吻合
