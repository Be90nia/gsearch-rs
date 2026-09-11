//! M10 browser stealth helpers: init-script fingerprint patches and small human-like interactions.
//!
//! This module is intentionally separate from the search state machine so browse/login
//! keep their current lifecycle while `search` opts into the anti-bot behavior.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chromiumoxide::Page;

const WARMUP_URLS: &[&str] = &[
    "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    "https://github.com/",
    "https://news.ycombinator.com/",
];

/// JavaScript installed on every new document created by a page.
/// M14-2A 起文本搬到 lib 单一事实源（`gsearch::browser::STEALTH_INIT_SCRIPT`），
/// launch 层（chaser-stealth feature）与本调用方（--humanize）共用；别名保持旧路径零变化。
pub const INIT_SCRIPT: &str = gsearch::browser::STEALTH_INIT_SCRIPT;

/// Install the fingerprint patch before the first navigation.
pub async fn install_init_script(page: &Page) -> Result<()> {
    page.add_init_script(INIT_SCRIPT).await?;
    Ok(())
}

/// Visit one benign site, scroll a little, then pause.  The short visit gives
/// Chrome a normal document history and timing before the first Google request.
pub async fn warmup(page: &Page) -> Result<()> {
    let mut random = Jitter::new();
    let url = WARMUP_URLS[random.range(0, WARMUP_URLS.len() as u64) as usize];
    let url_json = serde_json::to_string(url).context("序列化 warmup URL 失败")?;
    tracing::debug!("humanize warmup target: {url_json}");

    page.goto(url).await?;
    let scroll_count = 1 + random.range(0, 2) as u32;
    for _ in 0..scroll_count {
        page.evaluate("window.scrollBy(0, Math.floor(200 + Math.random() * 400));")
            .await?;
        tokio::time::sleep(Duration::from_millis(random.range(180, 420))).await;
    }
    tokio::time::sleep(Duration::from_millis(random.range(1_000, 3_001))).await;
    Ok(())
}


struct Jitter(u64);

impl Jitter {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self(nanos ^ 0x9e37_79b9_7f4a_7c15)
    }

    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn range(&mut self, low: u64, high: u64) -> u64 {
        debug_assert!(high > low);
        low + self.next() % (high - low)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn init_script_contains_top_ten_markers() {
        for marker in [
            "Navigator.prototype, 'plugins'",
            "navigator, 'languages'",
            "navigator, 'hardwareConcurrency'",
            "navigator, 'deviceMemory'",
            "navigator, 'maxTouchPoints'",
            "window.chrome, 'runtime'",
            "37445",
            "domAutomationController",
            "broken-image",
        ] {
            assert!(super::INIT_SCRIPT.contains(marker), "missing marker: {marker}");
        }
    }

    #[test]
    fn jitter_stays_in_range() {
        let mut random = super::Jitter::new();
        for _ in 0..1000 {
            let value = random.range(100, 200);
            assert!((100..200).contains(&value));
        }
    }
}
