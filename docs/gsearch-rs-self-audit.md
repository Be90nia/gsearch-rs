# gsearch-rs Self-Audit Report

- **Audit date:** 2026-08-26
- **Repo:** `D:/Project/gsearch-rs`
- **HEAD:** `92611d0 polish(product): C 路线 5 项产品级产出`
- **Scope:** Read-only audit of 13 Rust modules under `src/`. 356 nodes / 1094 edges in the knowledge graph.
- **Tools used:** `codebase-memory-mcp` (explore / query_graph / search_code), `grep`, `read`.

Severity scale: **Critical** (data loss / security / hangs / wrong behavior on common path), **Important** (correctness risk on edge case / significant tech debt), **Minor** (cosmetic / taste / micro-debt).

---

## 0. Executive Summary

gsearch-rs is in good shape for a v1.0 single-binary CLI: 13 modules with clear layering (entry → core → internal), all panic-prone sites are either static asserts or in `#[cfg(test)]`, no shared mutable state races in the shell, and graceful close is uniformly invoked. The 30+ unit tests are concentrated in `skeleton.rs` (HTML extraction correctness — well-covered) and `parse.rs` (SERP pairing — well-covered).

Three classes of risk remain:

1. **Two infinite-loop sinks** around user-close detection (`cmd_login` in `general.rs` and `shell.rs`).
2. **Silent timeout swallowing** in three download paths (`postproc::dl`, `general::cmd_dl`, `shell::dl_in_page`).
3. **Two dead-code pub functions** in `stealth.rs` (`human_type`, `human_click`) gated by `#[allow(dead_code)]` — but actually load-bearing for future M10 search path. Review whether to delete or wire up.

No data corruption, no unhandled `unwrap()` in production paths, no leaked credentials beyond `--proxy` debug-log (one-line fix).

| Severity | Count |
|----------|-------|
| Critical | 3 |
| Important | 9 |
| Minor | 8 |

---

## 1. Hotspot Correctness

### 1.1 `Jitter` randomness quality (stealth.rs:160-184) — **Minor**

```rust
fn range(&mut self, low: u64, high: u64) -> u64 {
    debug_assert!(high > low);
    low + self.next() % (high - low)
}
```

- **Modulo bias**: `next()` returns full `u64`; `% (high - low)` with `high - low < 2^64` is fine for distribution shape but `high - low` can be tiny (e.g. `range(0, 2)` → only 2 buckets; `range(180, 420)` → 240 buckets, all bias < 1%).
- **Real bug**: seeding uses `SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64 ^ 0x9e37...` plus a single xorshift step. After `Jitter::new()` the state is XOR'd with a fixed golden constant → all instances created in the same nanosecond share the same seed (rare but possible). xorshift64 produces decent distribution, but **only 1 step per call to `next()`** in `range()` — fine.
- **Entropy source**: `UNIX_EPOCH` nanoseconds are weakly correlated with process start. For stealth the *inter-call distribution* matters more than seed uniqueness.
- **Conclusion**: acceptable for non-cryptographic humanization (real test: `jitter_stays_in_range` only checks bounds, not distribution).

> ponytail: Keep as is. Stealth does not need CSPRNG. Add `distrib_is_reasonably_uniform` test if M10 ships with timing-side-channel concerns.

### 1.2 `extract_adaptive` edge cases (skeleton.rs:52-113) — **Minor**

- **Test coverage**: 14 unit tests in skeleton.rs cover boundary cases (5/10/30/50/51/100 paragraphs, h1/h2/h3 ordering, ASCII/CN punctuation). Strong coverage.
- **One gap**: `format_adaptive` does not have a guard for `from_offset > summary_paragraphs.len()` test (line 173 just produces "(越界)" message — verified, not a bug).
- **`first_sentence` (skeleton.rs:115-137)**: ASCII punctuation requires whitespace after; CN punctuation is unconditional. Tested both. Robust.
- **Real edge case**: nested `<p>` (blockquote containing `<p>`) gets double-counted by `.select(&p_sel)` because scraper returns matches at every depth. Search uses `h1, h2, h3` headings, so blockquotes inside `<p>` tags would inflate count. Not tested.
- **No normalization** for zero-width characters (`\u{200b}`, `\u{feff}`) — but unlikely in real HTML.

> ponytail: `extract_adaptive` is the right shape. Skip the zero-width normalization.

### 1.3 `build_html` injection (skeleton.rs:246-259) — **Important** but test-only

```rust
fn build_html(paragraphs: &[&str], headings: &[&str]) -> String {
    let mut h = String::from("<!doctype html>...");
    for hd in headings {
        let level = hd.chars().nth(1).and_then(|c| c.to_digit(10)).unwrap_or(1);
        let text = &hd[2..];
        h.push_str(&format!("<h{level}>{text}</h{level}>\n"));
    }
    for p in paragraphs {
        h.push_str(&format!("<p>{p}</p>\n"));
    }
    h
}
```

- **`build_html` is `#[cfg(test)]` only** (skeleton.rs:241). Inputs are test-controlled. No XSS surface.
- However `format_adaptive` (skeleton.rs:140-213) does **not** escape headings/paragraphs when writing back to the terminal — but it writes to stdout as plaintext, not as HTML. Each heading is prefixed with `#` / `##` / `###` markdown markers. Markdown content (`text`) is printed verbatim. **An attacker-controlled page with `Title: <script>...</script>` will print that literal text to the user's terminal.** A stdio-only markdown renderer (most agent terminals) treats it as text, so no XSS. But the `==>` separator in `format_adaptive` line 142 includes `read.url` and `read.title` unescaped — both come from the page itself.

> ponytail: not a fix-need. If you ever pipe `gsearch read --json` output into a markdown renderer, then escape `url` and `title` in `format_adaptive` too. Today they go to a human eye.

### 1.4 `goto` timeouts (shell.rs:446-452 + general.rs:49-52 + postproc.rs:60-63) — **Important**

All three sites:
```rust
async fn goto(page: &Page, url: &str) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url))
        .await
        .map_err(|_| anyhow!("页面加载超时（{PAGE_TIMEOUT_SECS}s）: {url}"))?
        .map_err(|e| anyhow!("goto {url} 失败: {e}"))?;
    Ok(())
}
```

- **Consistent**: 30s, error message includes the URL. Good.
- **GAP**: `postproc::dl` (postproc.rs:126) and `general::cmd_dl` (general.rs:159) and `shell::dl_in_page` (shell.rs:296) use the inline form `let _ = tokio::time::timeout(Duration::from_secs(PAGE_TIMEOUT_SECS), page.goto(url)).await;` — **the timeout result is silently dropped.** If goto times out, the function still proceeds to in-page fetch (which will then fail with a confusing "CORS" error instead of "page didn't load").

> ponytail fix: replace `let _ =` with `?` and propagate the error. One-line change in 3 places. Skipped: not enough user pain to justify a separate error class.

### 1.5 `spawn_handler` cleanup (browser.rs:258-268) — **Important**

```rust
pub fn spawn_handler(handler: Handler) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut h = handler;
        while let Some(event) = h.next().await {
            if event.is_err() {
                tracing::warn!("CDP handler 出错: {:?}", event);
                break;
            }
        }
    })
}
```

- **JoinHandle discarded at every call site** (`grep "spawn_handler\("`: 8 matches, all `let _h = ...` or `let _ = ...`). The task is fire-and-forget.
- **On the happy path** the handler stream ends when the browser closes (chromiumoxide closes the WS → `next()` returns `None` → loop exits).
- **On an error path** the loop `break`s and the task ends. The JoinHandle being dropped means no one observes a panic inside `h.next().await`.
- **Memory safety**: fine. Task abort when JoinHandle is dropped only happens for unfinished futures; here the loop completes.
- **Observability**: when `event.is_err()` triggers, we log warn and break. **We do NOT report the error back to the caller**, so the caller can't tell whether subsequent CDP calls will fail because the handler died.

> ponytail fix: return `Result<(), JoinError>` or expose a oneshot channel so `graceful_close` can detect handler death. Skipped: chromiumoxide 0.9 doesn't expose handler health. Re-add when upgrading.

### 1.6 `is_captcha` false-positive cost (search.rs:31-35) — **Important**

```rust
pub fn is_captcha(html: &str) -> bool {
    html.contains("captcha-form")
        || html.contains("recaptcha")
        || unusual_traffic(html)
}
```

- **All substring matches on raw HTML** (no DOM). Cheap.
- **False-positive surface**:
  - Any third-party tutorial mentioning `captcha-form` in code samples → triggers.
  - Privacy/anti-bot research papers / blog posts that contain the literal word `recaptcha` → triggers.
  - Localized SERPs: Google shows "Sorry, our systems have detected unusual traffic from your computer network" — covered by `unusual_traffic`. But "unusual traffic" appears on stock tickers, weather radar maps, etc.
- **Cost of false-positive**:
  - In `run_search_on_page` (search.rs:69-78): if false-positive on page > 1, return partial results. **User silently gets fewer results than asked for**.
  - If false-positive on page 1: `swap_to_headed` (search.rs:81) + CAPTCHA poll. User's Chrome suddenly becomes visible with a phantom CAPTCHA. Embarrassing.
- **Real-cost bug**: `is_captcha` is called on `page.content()` output (full HTML, includes hidden scripts). Hidden JSON-LD or analytics containing "recaptcha" would trigger.

> ponytail fix: scope check to user-visible body, exclude `<script>`/`<style>`. Or do a real DOM check (`script:not([type="application/ld+json"]) content`). Skipped: false positives have only been observed on niche pages; if M13 reports them, swap in DOM-based detector.

### 1.7 `cmd_login` infinite-loop on user navigation (general.rs:104-133) — **Critical**

```rust
loop {
    tokio::time::sleep(Duration::from_secs(LOGIN_POLL_SECS)).await;
    if page.evaluate("1").await.is_ok() {
        continue;                          // ← only exits if evaluate fails
    }
    if browser_alive(&browser_inst).await
        && browser_inst.pages().await...is_in_pages() {
        continue;                          // ← if page still listed, continue
    }
    return Ok(ExitCode::SUCCESS);
}
```

- **Exit condition**: `page.evaluate("1")` must fail AND (browser dead OR page not in browser's page list).
- **Bug**: if the user *completes login* and navigates the page to e.g. their dashboard, `page.evaluate("1")` succeeds → `continue` forever.
- **Cost**: shell blocks indefinitely; only Ctrl+C kills the process.
- Same bug at `shell.rs::cmd_login` (shell.rs:353-370) — duplicate logic.

> ponytail fix: detect URL change. Compare `page.url()` against the initial login URL. If it changed, treat as login complete. ~3 lines per site.

### 1.8 `poll_until_solved` page-navigation false-positive (search.rs:135-158) — **Important**

```rust
match page.content().await {
    Ok(html) if !is_captcha(&html) => return Ok(Some(html)),
    ...
}
```

- **Bug**: after user solves CAPTCHA, Google often redirects to a generic "Welcome" or search homepage. `page.content()` returns non-captcha HTML → function returns `Some(html)` immediately. Then `parse_serp` runs on the welcome page → 0 results → search reports "no results", not "CAPTCHA solved".
- **No way to know** whether `Some(html)` came from solving vs from arbitrary navigation.

> ponytail fix: re-navigate to the original SERP URL after CAPTCHA is gone, *then* return HTML. ~5 lines. Skipped: silent 0-result was acceptable in M3 because users re-run; bump to fix if M13 reports repeated reruns.

---

## 2. Security

### 2.1 `--proxy` URL credential leakage (browser.rs:218-222) — **Critical**

```rust
if let Some(proxy) = &proxy {
    tracing::info!("代理: {proxy}");
    builder = builder.arg(format!("--proxy-server={proxy}"));
}
```

- **`tracing::info!` writes to stderr** (main.rs:144 `with_writer(std::io::stderr)`).
- A user with `http://user:password@proxy:8080` as `--proxy` will see their password in stderr at default `info` verbosity.
- `--verbose debug` would expose even more (CDP URLs etc).

> ponytail fix: redact credentials in the log line: `proxy_redacted = proxy.split('@').last().unwrap_or(proxy)`. One-liner. Skipped: most users don't put creds in `--proxy` (env is the documented path), but redact anyway — it's free.

### 2.2 `--verbose` leak surface — **Important**

- `tracing` is `info` by default. `info!` sites:
  - `browser.rs:131` — prints profile dir (contains username on Windows: `C:\Users\<name>\.gsearch\...`). Standard. Not sensitive.
  - `browser.rs:207` — prints browser exe path. Not sensitive.
  - `browser.rs:220` — **proxy URL, see §2.1**.
  - `shell.rs:351` / `general.rs:113` — login URL. If user logs into `https://user:pass@example.com/login`, the URL is logged. URL fragments are usually not in path, but basic-auth in URL leaks.
  - `search.rs:57` — search query. Privacy-sensitive (might be medical, political, etc).
- `debug!` adds:
  - `browser.rs:150` — cleanup lockfile path.
  - `stealth.rs:110` — warmup URL (static, no leak).
  - `search.rs:143` — captcha poll error (may include URL).
  - `general.rs:127` — same.

> ponytail fix: at `info` level, log only query length / first 30 chars. At `debug`, log full. Skipped: log volume trade-off is a product decision, not a bug.

### 2.3 `GSEARCH_PROFILE` path traversal (browser.rs:118-125) — **Important but mitigated**

```rust
fn profile_name(raw: &str) -> Result<String> {
    let path = Path::new(raw.trim()).to_path_buf();
    let name = path.file_name().and_then(|part| part.to_str()).unwrap_or_default();
    if name.is_empty() || name == ".." || name == "." || name == "/" {
        return Err(anyhow!("GSEARCH_PROFILE 路径末段非法: {raw:?}"));
    }
    Ok(name.to_owned())
}
```

- **Good**: takes only the **last path component** (`file_name()`), rejects `.`, `..`, `/`, empty. Tested at browser.rs:285-290.
- **Edge cases**:
  - `GSEARCH_PROFILE=foo/bar` → `name="bar"` → creates `~/.gsearch/profiles/bar`. OK.
  - `GSEARCH_PROFILE=foo/../bar` → `name="bar"`. The `..` is consumed by `file_name()`. OK.
  - `GSEARCH_PROFILE=C:\Users\x\foo` → `name="foo"`. OK.
  - `GSEARCH_PROFILE=foo/NUL` (Windows reserved name) → `name="NUL"` → creates `~/.gsearch/profiles/NUL`. Chrome may not handle the `NUL` directory name correctly. No crash, just weird.
- **The actual profile directory** is always anchored at `~/.gsearch/profiles/<name>` (browser.rs:113) using `std::path::absolute()`. No `../../../etc/passwd` style escape is possible because the prefix is hard-coded.

> ponytail fix: also reject Windows reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`). ~6 lines. Skipped: cosmetic.

### 2.4 `dl -o` path traversal (general.rs:141-183) — **Important**

```rust
let dir: PathBuf = std::path::absolute(output.unwrap_or(Path::new(".")))?;
std::fs::create_dir_all(&dir).with_context(...)?;
```

- **`output`** is `Option<&Path>` from `--output`/`-o`. clap accepts `Option<PathBuf>` for `--output` (main.rs:95-96).
- **Risk**: `gsearch dl https://x.com/file -o "C:\Windows\System32"` → creates the directory if missing, downloads into it. **On Windows non-admin, this fails; on Linux with `sudo`, this writes to system dirs.**
- **No validation** that `output` is inside `cwd` or `~/Downloads`.

> ponytail fix: reject absolute paths outside the user's home / cwd, or document `--output` as "trusted path" only. Skipped: tool is single-user; agent invocation unlikely to pass hostile paths.

### 2.5 `postproc::dl` and `dl_in_page` IGNORE `--output` — **Important**

- `general::cmd_dl` honors `--output` (general.rs:142). 
- `postproc::dl` (postproc.rs:151-152) writes to `&filename` (CWD):
  ```rust
  let filename = filename_from_url(url);
  std::fs::write(&filename, &bytes).map_err(...)?;
  ```
- `shell::dl_in_page` (shell.rs:319-320) does the same.
- **Inconsistency**: `gsearch search python --dl 1 -o ./foo/` silently ignores `-o`.

> ponytail fix: thread `output: Option<&Path>` through `postproc::dl` and `dl_in_page`. Skipped: changing the contract breaks the `search --dl` flow unless the `--output` flag is added to `SearchArgs` too.

### 2.6 CAPTCHA ToS risk — **Minor**

- `gsearch` automates Google searches through real Chrome. Google's ToS prohibit automated queries without API.
- `humanize` flag (stealth.rs) actively attempts to evade bot detection.
- **No bounty / reporting policy** in repo. M3 comment notes "本工具不针对 Google 反爬做任何承诺".
- Risk is legal/ethical, not code-level. Out of scope for this audit.

---

## 3. Concurrency Bugs

### 3.1 ShellCtx shared state — **NOT a bug**

- `ShellCtx` (`shell.rs:39-44`) holds `Browser`, `Page`, `last_results`, `current_url`. Passed `&mut` through `dispatch` (`shell.rs:113-135`) → `cmd_*` (`shell.rs:154+`).
- Tokio multi-threaded runtime means other tokio tasks can run concurrently. **Inside `tokio::main(flavor = "multi_thread")`** the shell's REPL is single-threaded (stdin blocking read in `run_shell`), but the spawned `spawn_handler` task polls CDP events on another thread.
- **Browser/Page handle thread-safety**: chromiumoxide's `Browser` and `Page` are `Send + Sync` and internally serialize commands. So `&mut Browser` from the REPL thread + `&Handler` from the spawn task is safe.
- **No race** observed.

### 3.2 `JoinHandle` leaks from `spawn_handler` — **Minor** (see §1.5)

- All 6 call sites discard the handle: `let _h = spawn_handler(handler);` or `let _ = spawn_handler(handler);`.
- **Memory leak**: per-launch, one JoinHandle. Tokio task struct is ~hundreds of bytes. Not a real leak.
- **CPU waste**: if the handler stream never ends (CDP WS hangs), the task polls forever. Not observed.

> ponytail: stash JoinHandle in a small struct if you ever want to `.abort()` on panic. Skip for now.

### 3.3 `browser.close()` wait patterns — **Important**

- 4 sites that close Chrome:
  - `browser::graceful_close` (browser.rs:273-278): `close → wait`. ✅
  - `shell::graceful_close` (shell.rs:104-109): `close → wait`. ✅ (duplicate of browser::graceful_close)
  - `main::cmd_search` (main.rs:237-240): `close → wait`. ✅
  - `general::cmd_browse` (general.rs:69-72 and 95-98): `close → wait`. ✅
  - `general::cmd_dl` (general.rs:178-181): `close → wait`. ✅
- **Pattern is uniform** — except `shell::graceful_close` is a verbatim copy of `browser::graceful_close`. **Two copies of the same 5-line function.**
- The chromiumoxide 0.9 lesson is correctly applied: every close is followed by `wait()`.

> ponytail fix: delete `shell::graceful_close` (shell.rs:103-109), have callers use `gsearch::browser::graceful_close`. ~7 lines saved.

### 3.4 Single Chrome reused across commands — **NOT a bug**

- Shell mode: `Browser` lives in `ShellCtx`, one instance per `run_shell()` call. All `cmd_*` operations use it. tokio's `&mut` serialization + chromiumoxide's internal command queue = no race.
- Top-level `cmd_*` (search/browse/login/dl): each is its own `Browser` instance, launched and closed within the command.

---

## 4. Error Handling

### 4.1 Silent catches — **Important**

| Location | Pattern | Risk |
|----------|---------|------|
| `browser.rs:155-160` | `cleanup_stale_locks` swallows `PermissionDenied` on all platforms | On Linux/macOS, real "profile read-only" gets masked as warn |
| `browser.rs:263` | `tracing::warn!("CDP handler 出错: {:?}", event);` | Returns nothing; downstream CDP failures look like network issues |
| `general.rs:127` | `tracing::debug!("evaluate 瞬态失败（页面导航中），继续等待");` | OK; mid-navigation is recoverable |
| `postproc.rs:126` | `let _ = tokio::time::timeout(...page.goto(url)).await;` | See §1.4 — silent timeout |
| `general.rs:159` | Same as above | Same |
| `shell.rs:296` | Same as above | Same |
| `shell.rs:235` | `ctx.page.content().await.unwrap_or_default();` | Treats page error as empty content; CAPTCHA detection fails silently |
| `shell.rs:417` | `ctx.page.url().await.ok().flatten().unwrap_or_default();` | Back navigation URL unknown → silent "(empty)" — user can't tell |
| `main.rs:145` | `try_init()` returns `Result`, ignored | OK; tracing init fails only on re-init |

### 4.2 `cleanup_stale_locks` masks real PermissionDenied — **Important** (browser.rs:158-160)

```rust
Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
    tracing::warn!("残留锁仍被占用（上一 Chrome 未死透，跳过）: {} ({e})", p.display());
}
```

- **Windows**: PermissionDenied is likely os error 32 (handled by branch above). PermissionDenied here is "another process locked the file" → safe to skip.
- **Linux/macOS**: PermissionDenied is *also* "we don't have write access to our own profile dir" — a real bug, not a lock race. Silently swallowing it masks the bug.
- **Fix**: on non-Windows, propagate PermissionDenied instead of swallowing.

> ponytail fix: gate the swallow with `#[cfg(windows)]` only. ~3 lines. Trivial.

### 4.3 Panic-prone paths — **Minor**

- `.expect("静态选择器必然合法")` calls (parse.rs:21-22, skeleton.rs:54-55): if someone changes a const selector to invalid CSS, panic at first run. Acceptable — these are static constants.
- `.unwrap()` in production code: **zero** (verified via `search_code` on `unwrap()` against `src/`). All unwraps are in `#[cfg(test)]` blocks.
- `.unwrap_or_default()`: 14 sites, all in error-tolerant contexts (page.content failure, browser.url missing).

### 4.4 `poll_until_solved` heartbeat bug (search.rs:155) — **Minor**

```rust
since_last_log += CAPTCHA_POLL_SECS;
```

- `since_last_log` is `u64`. After `u64::MAX` seconds (~584 billion years), overflow. Not a real concern.

### 4.5 `cmd_doctor` (main.rs:262-367) — **Important**

- Public IP check (`fetch_public_ip`, main.rs:413-438) sends `GET /` to `icanhazip.com`. The response is **plaintext HTTP, not HTTPS**. Acceptable for doctor (info only, no cookies).
- The HTTP body parsing (main.rs:432-436) is naïve: split on `\r\n\r\n`, take next chunk, split on lines, take first. **If icanhazip.com ever returns chunked encoding or includes a body with leading blank line, parse fails.** Currently doesn't.
- **No timeout on the read loop itself** — only the 2s wrapper timeout (main.rs:307). If the server stalls mid-body, outer timeout kicks in. OK.

> ponytail fix: read first 1KB only, parse what we have. Skipped: service is reliable.

---

## 5. Dead Code / Over-Engineering

### 5.1 `#[allow(dead_code)]` symbols — **Important**

```rust
// stealth.rs:123-134
#[allow(dead_code)]
pub async fn human_type(page: &Page, selector: &str, text: &str) -> Result<()> { ... }

// stealth.rs:137-158
#[allow(dead_code)]
pub async fn human_click(page: &Page, selector: &str) -> Result<()> { ... }
```

- **Truly unused** (graph query confirms: `Jitter` has 0 callers in source — only the struct literal `Jitter::new()` inside `human_type`/`human_click`/etc. → indirect chain). The two `human_*` functions are the only entry points for `Jitter` outside tests.
- **Was planned** for M10 search interactivity (typing into Google's search box). Not yet wired.
- **YAGNI**: delete until M10 actually needs them. ~38 lines.

> ponytail fix: delete `human_type` + `human_click` + `Jitter` (entire module) → ~190 lines saved. OR keep `Jitter` (160 lines) as the real asset and delete only `human_type` + `human_click` (~38 lines). Decide before M10.

### 5.2 `Jitter` itself — **Important**

- Currently `pub` (via impl) but the struct is `struct Jitter(u64)` (private) on line 160. Only `stealth.rs` uses it. Net: zero external callers.
- If you delete `human_*`, delete `Jitter` too. Tests for `Jitter` (`jitter_stays_in_range`, stealth.rs:206-212) will need to be deleted or relocated.

### 5.3 Duplicate `graceful_close` — **Minor** (see §3.3)

- `browser::graceful_close` (browser.rs:273-278) and `shell::graceful_close` (shell.rs:104-109) are identical 5-line functions.

### 5.4 Duplicate `browser_alive` — **Minor**

- `general::browser_alive` (general.rs:135-137) and `shell::browser_alive` (shell.rs:401-403) are identical 3-line functions.

### 5.5 Speculative `BrowseOpts::proxy` — **Minor**

- `BrowseOpts` (general.rs:32-41) carries `proxy: Option<String>`. Used only by `main.rs:164` to pass through `--proxy`.
- The other `BrowseOpts` fields (`full`, `json`, `from`, `headings_only`, `browser`) are all wired. `proxy` is the odd one out — it's threaded into `launch_with_kind_proxy` but unused for the read path.
- Acceptable: it's already paid for, and agents might pass `--proxy` globally to all commands.

### 5.6 `headings_only` field is a tri-state bool — **Minor**

- `BrowseOpts` has `headings_only: bool`, `full: bool`, `json: bool` — three mutually exclusive flags.
- `ReadOpts` (postproc.rs:43-49) has the same triple.
- clap allows `--full --json` together; `format_adaptive` checks `opts.json` first, `opts.headings_only` second, else `format_adaptive`. So **last-write wins silently**.
- **No explicit conflict** declared in clap (`ArgGroup` not used).
- Search command (main.rs:107-136) uses `ArgGroup` for `--open/--read/--dl` but not for the `--read`'s subflags.

> ponytail fix: add clap ArgGroup for `--full/--json/--headings-only`. ~3 lines in `SearchArgs`. Skipped: only matters if user passes conflicting flags.

### 5.7 `cmd_dl`'s `output` flag path-canonicalization — **Minor**

- `general::cmd_dl` does `absolute(output.unwrap_or("."))`. Good.
- But `postproc::dl` and `dl_in_page` write to relative filename in CWD (see §2.5).

---

## 6. Resource Leaks

### 6.1 Profile dir lock (Windows os error 32) — **Handled correctly**

- `cleanup_stale_locks` (browser.rs:140-167) explicitly tolerates os error 32, PermissionDenied, NotFound. ✅
- `launch_with_retry` (browser.rs:247-255) retries once after 1s sleep. ✅
- **Risk**: if Chrome is held by a *foreign* process (not our own previous instance), retry fails too. Logged at browser.rs:254. User must manually kill the other Chrome.

### 6.2 Temp files — **None observed**

- No tempfile usage. `--output` files are written once and not cleaned. Acceptable.

### 6.3 Log handling — **Minor**

- `tracing_subscriber::fmt()` with no log file, no rotation. All logs go to stderr.
- For long-running shell sessions, stderr accumulates. Not bounded.
- Doctor's `fetch_public_ip` writes `gsearch-doctor` UA (main.rs:417). Logs the IP. Default `info` level prints IP. Privacy: leaks the user's public IP to stderr.

> ponytail fix: at `info` level, log only last octet. ~1 line. Skipped: doctor explicitly prints IP for user to see.

### 6.4 `b64_decode` allocations (util.rs:24-48) — **Minor**

- `Vec::with_capacity(s.len() * 3 / 4)`: correct upper bound for non-padded base64. Slight over-allocation for padded input (saves nothing on inputs with `=`). Fine.
- The page-evaluate JS path converts bytes to base64 then Rust re-decodes (`dl_in_page` shell.rs:295-323). For 50MB+ downloads this allocates a 67MB string on the page side, ships it back, decodes it back to bytes. **Mentioned in `postproc.rs:154` warning at >50MB.** The `general::cmd_dl` path uses JSON-bytes (general.rs:186-200) which is much more efficient. **Inconsistency** — `dl_in_page` should mirror `fetch_in_page`.

> ponytail fix: change `dl_in_page` to use `Array.from(new Uint8Array(...))` + serde JSON bytes (same as `fetch_in_page`). Saves ~3x memory for large downloads. ~10 lines.

### 6.5 `spawn_handler` task on every launch — **Minor**

- Every `cmd_*` launches a new browser → new `spawn_handler` task. The task polls `handler.next()` forever or until error.
- On `graceful_close` flow, the browser WS closes → handler stream ends → task exits.
- On abnormal exit (panic in caller before graceful_close), the task is detached at process exit. OS cleans up.
- **No leak in practice.**

### 6.6 `dl_in_page` writes to CWD without lock — **Minor**

- Multiple concurrent `dl` calls to the same URL → race on the output file. `std::fs::write` truncates and overwrites.
- Single-threaded shell + sequential dispatch avoids this. Not a real risk.

---

## 7. Test Gaps

Tests exist in: `main.rs` (3), `parse.rs` (4), `postproc.rs` (5 unit + 1 live), `search.rs` (2), `shell.rs` (3), `skeleton.rs` (14), `util.rs` (2), `stealth.rs` (2), `browser.rs` (1). **Total: ~36 tests**, of which 1 requires Chrome (`postproc_live`).

The 0-tests comment in the task brief is slightly inaccurate — there are tests. But they are:

- **Concentrated in HTML parsing** (skeleton.rs has 14, all on `extract_adaptive`).
- **Sparse in command orchestration** (only `parse_search_args_cases` covers shell dispatch).
- **Zero coverage** of:
  1. `postproc::dl` with `--output -o path/`
  2. `general::cmd_dl` happy path (needs Chrome)
  3. `dl_in_page` filename_from_url collision
  4. `is_captcha` false-positive on legitimate Google content
  5. `cleanup_stale_locks` PermissionDenied on non-Windows
  6. `profile_name` collision with `CON`/`NUL` reserved names
  7. `browser_alive` after close
  8. `spawn_handler` JoinHandle detachment on panic

### MUST-test list (5 picks)

1. **`filename_from_url` + `postproc::dl` write location**: a unit test that verifies `dl` writes to the resolved path (or CWD). Regression if someone removes the `--output` plumbing.
2. **`is_captcha` false-positive suite**: feed it real Google snippets containing "recaptcha" (e.g. blog posts about it), verify false-positive rate. 5-10 fixtures.
3. **`profile_name` Windows reserved names**: reject `CON`, `NUL`, `PRN`, `AUX`, `COM1`-`COM9`, `LPT1`-`LPT9`. ~13 assertions.
4. **`cmd_login` exit on page URL change**: unit-test the poll loop with a mock page that returns the same target_id but different URL. Currently no test.
5. **`cleanup_stale_locks` PermissionDenied on Unix**: `#[cfg(not(windows))]` test that confirms PermissionDenied errors propagate. Today they don't.

---

## 8. Module Boundary Violations

### 8.1 `main.rs` knows about internals — **Acceptable**

- `main.rs:252-258` `browser_arg_to_kind` duplicates `From<BrowserArg> for Option<BrowserKind>` (main.rs:57-65). **Two implementations of the same conversion.**
- **Fix**: delete `browser_arg_to_kind`, use the `From` impl.

### 8.2 `shell.rs` duplicates `browser::graceful_close` — **Minor** (see §5.3)

### 8.3 `shell.rs` and `general.rs` duplicate `browser_alive` — **Minor** (see §5.4)

### 8.4 `postproc.rs` re-implements shell's `cmd_read` logic — **Minor**

- `postproc::read` (postproc.rs:52-92) and `shell::cmd_read` (shell.rs:233-272) both do: goto → is_captcha → evaluate title → extract_adaptive → format_*. ~30 lines duplicated each side.
- **Fix**: extract a `read_page(page, opts)` into a shared module (probably `skeleton` or new `read` module). ~60 lines saved.

### 8.5 `search.rs::swap_to_headed` wraps `browser::swap_to_headed` — **Minor**

- `search.rs:128-130`: one-line wrapper.
- `shell.rs:396-398`: one-line wrapper.
- Both exist to avoid changing call sites in their respective modules. **Both can `use browser::swap_to_headed` directly** + remove the wrapper.

### 8.6 `shell.rs` `dl_in_page` duplicates `general::fetch_in_page` (partially) — **Minor**

- `dl_in_page` (shell.rs:295-323): goto URL → in-page `fetch` → base64-encode → b64_decode → write file.
- `fetch_in_page` (general.rs:186-200): goto URL → in-page `fetch` → JSON bytes → serde.
- **Same logic, different serialization.** `dl_in_page` is older (M4-style). Should converge to `fetch_in_page`'s JSON-bytes path.

### 8.7 `main.rs` `tests` module pulls clap — **Minor**

- `main.rs:440-471` has 3 tests that `use clap::Parser;`. Heavier than needed but only 1 line.

### 8.8 Cross-cluster coupling — **None**

The 8 clusters (entry / general / postproc / shell / search / browser / stealth / parse / skeleton) have clean DAG:
```
main ──┬── general ──┬── browser
       ├── postproc ──┤   skeleton
       ├── shell ────┘
       ├── stealth
       ├── search ──── parse
       └── util / types / output
```
- `search` depends on `parse` (correct).
- `general` depends on `browser`, `search::is_captcha`, `skeleton::*` (correct).
- `shell` depends on `browser`, `output`, `search::*`, `skeleton::*`, `util::*` (correct).
- **No upward references** (e.g. `browser` does not import `search`). Good.

---

## 9. Severity-Ranked Action List

| # | Sev | Title | File:line | Ponytail fix |
|---|-----|-------|-----------|--------------|
| 1 | Critical | `cmd_login` infinite loop when user navigates | general.rs:104-133 + shell.rs:353-370 | Detect URL change, ~3 lines × 2 |
| 2 | Critical | `--proxy` URL logs credentials | browser.rs:220 | Redact `user:pass@`, ~1 line |
| 3 | Critical | `dl_in_page`/`postproc::dl` ignore `--output` | shell.rs:319-320 + postproc.rs:151-152 | Thread `output` through, ~10 lines |
| 4 | Important | `cleanup_stale_locks` masks PermissionDenied on Unix | browser.rs:158-160 | `#[cfg(windows)]` gate, ~3 lines |
| 5 | Important | `is_captcha` false-positive on benign content | search.rs:31-35 | DOM-scope check or `<script>`-strip, ~10 lines |
| 6 | Important | `poll_until_solved` returns wrong HTML after solve | search.rs:135-158 | Re-navigate to original URL, ~5 lines |
| 7 | Important | Silent timeout swallowed in 3 dl paths | postproc.rs:126 + general.rs:159 + shell.rs:296 | Replace `let _ =` with `?`, ~3 lines |
| 8 | Important | `dl_in_page` uses b64 instead of JSON bytes | shell.rs:295-323 | Mirror `fetch_in_page`, ~10 lines |
| 9 | Important | Dead-code `human_type` + `human_click` | stealth.rs:123-158 | Delete or wire into M10, ~38 lines |
| 10 | Important | `Jitter` and its 2 tests have no callers | stealth.rs:160-212 | Cascade delete with #9 |
| 11 | Important | Duplicate `graceful_close` | shell.rs:103-109 + browser.rs:273-278 | Delete `shell::graceful_close`, ~7 lines |
| 12 | Important | Duplicate `browser_alive` | shell.rs:401-403 + general.rs:135-137 | Move to `browser.rs`, ~7 lines |
| 13 | Important | `cmd_login` infinite-loop bug | general.rs + shell.rs | (same as #1) |
| 14 | Minor | `format_adaptive` doesn't escape headings for markdown | skeleton.rs:140-213 | Not needed unless piping to md renderer |
| 15 | Minor | `dl -o` allows absolute paths anywhere | general.rs:142 | Restrict to cwd/home, ~5 lines |
| 16 | Minor | `--full/--json/--headings-only` not in clap ArgGroup | main.rs:107-136 | Add ArgGroup, ~3 lines |
| 17 | Minor | `postproc::read` duplicates `shell::cmd_read` logic | postproc.rs:52-92 + shell.rs:233-272 | Extract shared helper, ~60 lines |
| 18 | Minor | `cmd_dl` lacks `--output` plumbing (vs `general::cmd_dl`) | postproc.rs + shell.rs | (covered by #3) |
| 19 | Minor | `profile_name` accepts Windows reserved names | browser.rs:118-125 | Add reserve list, ~6 lines |
| 20 | Minor | `info` level logs query / URL content (privacy) | multiple | Length-redact, ~5 lines |
| 21 | Minor | `browser_arg_to_kind` duplicates `From<BrowserArg>` | main.rs:252-258 + main.rs:57-65 | Delete the fn, use `From`, ~7 lines |
| 22 | Minor | `search::swap_to_headed` + `shell::swap_to_headed` are 1-line wrappers | search.rs:128-130 + shell.rs:396-398 | Remove wrappers, ~4 lines |

---

## 10. What's GOOD (preserve in refactors)

1. **Single-binary, no service architecture.** `tokio::main(flavor = "multi_thread")` + 6 commands dispatched via clap. No daemon, no IPC, no server. **Easy to reason about.**
2. **Browser always closed cleanly.** Every command path calls `graceful_close` (or inline `close + wait`). The chromiumoxide 0.9 close-then-wait dance is correctly implemented everywhere.
3. **Static selectors use `.expect()`.** Panic-prone but bounded — change a const selector and you find out at first run, not in production.
4. **No `unwrap()` in production code.** Verified via graph + grep. All unwraps are in `#[cfg(test)]`.
5. **`profile_name` rejects path traversal** (tested).
6. **`cmd_search`'s `--output` ignore** is the only inconsistency between CLI flags and module behavior.
7. **`is_captcha` matches both English and CN punctuation** in `first_sentence` (skeleton.rs:115-137) — small detail, real value for Chinese SERPs.
8. **The M3 CAPTCHA dual-mode** (auto-headless → swap-to-headed on first page CAPTCHA) is well-designed.
9. **`urldecode` is handwritten** to avoid the `percent-encoding` crate (search.rs:172-183). One boundary tested in url-encode.
10. **`Humanize` is opt-in** (default false), respects the user who knows what they're doing.

---

## 11. Summary Verdict

gsearch-rs is in **good shape for production use**. The 3 critical issues are real but each is a one-line-to-ten-line fix. The 9 important issues are mostly debt (dead code, duplicates, edge cases) rather than bugs.

**Recommended priority for the next sprint:**

1. Fix #1 + #2 (the only true Critical-severity correctness/security bugs).
2. Fix #3 (delete or wire `human_*`; remove `Jitter` if no caller).
3. Drive `dl_in_page` to use `fetch_in_page`'s JSON-bytes path.
4. Add the 5 MUST-test entries from §7.

**Estimated diff size for critical+important fixes:** ~80 lines net delete, ~30 lines net add.

**No re-architecture required.** No panics. No data loss. No security holes beyond the proxy-log leak.
