//! Example: Pixelscan.net fingerprint scan checker
//!
//! Loads <https://pixelscan.net/fingerprint-check> with Advanced stealth,
//! waits for the Angular SPA to finish rendering, then extracts every
//! structured result card (Browser, Location, Proxy, Fingerprint, Bot check)
//! along with the per-section hardware / font / UA detail values.
//!
//! This example is intentionally targeted: it understands the pixelscan DOM
//! layout (`.checker-card-wrapper` Angular components, the `/s/api/*` API
//! calls) and emits a structured JSON report so you can iterate on stealth
//! improvements without re-parsing the page manually each time.
//!
//! Network architecture notes (from inspection):
//!   - Bot check signals are `POSTed` to `/s/api/afp`, `/s/api/cb`, `/s/api/ci`
//!   - Fingerprint data is sent to `/s/api/gf`, `/s/api/co`, `/s/api/a/f`
//!   - All evaluation is *server-side*; the Angular app just renders the result
//!   - Canvas/WebGL blobs ship via `/s/api/cwg` and a blob: URL (canvas worker)
//!
//! ```sh
//! cargo run --example pixelscan_check -p stygian-browser
//!
//! # Pretty-print:
//! cargo run --example pixelscan_check -p stygian-browser | jq .
//! ```

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use stygian_browser::config::{PoolConfig, StealthLevel};
use stygian_browser::{BrowserConfig, BrowserPool, WaitUntil};

const TARGET_URL: &str = "https://pixelscan.net/fingerprint-check";

// ─── helpers ─────────────────────────────────────────────────────────────────

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ─── JS extraction scripts ────────────────────────────────────────────────────

/// Wait until the Angular result cards are present in the DOM **and** the
/// three client-evaluated cards (Browser, Fingerprint, Bot check) have settled.
///
/// Location and Proxy cards depend on pixelscan's own external geo/proxy APIs
/// which can take 30+ seconds; we deliberately do NOT wait on those so they
/// don't gate extraction of the bot-detection results we actually care about.
const READY_SCRIPT: &str = r"
  (() => {
    const cards = Array.from(document.querySelectorAll('.checker-card-wrapper'));
    if (cards.length < 5) return false;
    // Map each card to a short label by its position (Angular renders them in
    // fixed order: Browser, Location, Proxy, Fingerprint, Bot check).
    // Only require the client-side cards (0=Browser, 3=Fingerprint, 4=Bot check)
    // to have non-placeholder content before we proceed.
    const clientCards = [cards[0], cards[3], cards[4]].filter(Boolean);
    return clientCards.every(
      c => c.innerText && !c.innerText.toLowerCase().includes('collecting')
    );
  })()
";

/// Extract the 5 top-level result cards.
/// Each card is `{ title, status, detail }` where status is "pass" | "fail".
const CARDS_SCRIPT: &str = r#"
  (() => {
    const cards = Array.from(document.querySelectorAll('.checker-card-wrapper'));
    return cards.map(card => {
      // Compute background-colour to determine pass/fail — red = fail
      const bg = getComputedStyle(card).backgroundColor;
      const isRed = bg.startsWith('rgb(') && (() => {
        const [r, g, b] = bg.match(/\d+/g).map(Number);
        return r > 150 && g < 100 && b < 100;
      })();

      // Angular sometimes puts the fail flag as a class on the card or a child
      const hasRedClass = card.className.includes('red')
        || card.innerHTML.toLowerCase().includes('isred');

      // Full inner text, cleaned — gives: "<verdict text> <card label>"
      const text = card.innerText.trim().replace(/\s+/g, ' ');

      // Best-effort title: first h3/h4/strong inside the card
      const titleEl = card.querySelector('h3,h4,strong,[class*="title"],[class*="label"]');
      const title = titleEl ? titleEl.innerText.trim() : text.split('\n')[0];

      return {
        title,
        status: (isRed || hasRedClass) ? 'fail' : 'pass',
        text,
      };
    });
  })()
"#;

/// Extract the "What Websites See About You" detail sections.
const DETAILS_SCRIPT: &str = r"
  (() => {
    const out = {};

    // ── Location ──────────────────────────────────────────────────────────────
    const locRows = () => {
      const rows = {};
      document.querySelectorAll('[class*=location] [class*=row],[class*=ip] [class*=row]')
        .forEach(r => {
          const cells = r.querySelectorAll('[class*=label],[class*=val],[class*=key],[class*=item]');
          if (cells.length >= 2)
            rows[cells[0].innerText.trim()] = cells[1].innerText.trim();
        });
      return rows;
    };
    out.location = locRows();

    // ── User-Agent ────────────────────────────────────────────────────────────
    const uaSection = document.querySelector('[class*=user-agent],[class*=useragent]');
    if (uaSection) {
      out.user_agent = {
        http: uaSection.querySelectorAll('[class*=val],[class*=value]')[0]?.innerText.trim(),
        js:   uaSection.querySelectorAll('[class*=val],[class*=value]')[1]?.innerText.trim(),
      };
    }

    // ── Hardware (WebGL / Canvas / Audio hashes) ───────────────────────────
    const hwSection = document.querySelector('[class*=hardware],[class*=Hardware]');
    if (hwSection) {
      out.hardware = hwSection.innerText
        .trim()
        .split('\n')
        .map(l => l.trim())
        .filter(Boolean)
        .reduce((acc, line, i, arr) => {
          if (i % 2 === 0 && arr[i + 1]) acc[line] = arr[i + 1];
          return acc;
        }, {});
    }

    // ── Fonts ─────────────────────────────────────────────────────────────────
    const fontSection = document.querySelector('[class*=font],[class*=Font]');
    if (fontSection) {
      const fontHash = fontSection.querySelector('[class*=hash]')?.innerText.trim();
      const fontList = Array.from(fontSection.querySelectorAll('[class*=item],[class*=name]'))
        .map(e => e.innerText.trim())
        .filter(Boolean)
        .slice(0, 20);
      out.fonts = { hash: fontHash, sample: fontList };
    }

    // ── Screen ────────────────────────────────────────────────────────────────
    const screenSection = document.querySelector('[class*=screen],[class*=Screen]');
    if (screenSection) {
      out.screen = screenSection.innerText
        .trim()
        .split('\n')
        .map(l => l.trim())
        .filter(Boolean)
        .reduce((acc, line, i, arr) => {
          if (i % 2 === 0 && arr[i + 1]) acc[line] = arr[i + 1];
          return acc;
        }, {});
    }

    // ── Raw navigator signals (for debugging) ─────────────────────────────────
    out.nav_signals = {
      webdriver:        navigator.webdriver,
      webdriverProto:   (() => {
        try { return Object.getOwnPropertyDescriptor(Navigator.prototype, 'webdriver')?.get?.(); }
        catch (_) { return 'error'; }
      })(),
      pluginsLen:       navigator.plugins.length,
      platform:         navigator.platform,
      userAgent:        navigator.userAgent,
      vendor:           navigator.vendor,
      hardwareConcurrency: navigator.hardwareConcurrency,
      deviceMemory:     navigator.deviceMemory,
      languages:        navigator.languages,
      connectionType:   navigator.connection?.effectiveType,
      hasBattery:       typeof navigator.getBattery === 'function',
    };

    return out;
  })()
";

/// Overall verdict text ("Your Browser Fingerprint is consistent/inconsistent").
const VERDICT_SCRIPT: &str = r"
  (() => {
    // The verdict heading is typically the largest h1/h2 on the page
    const h = document.querySelector('h1,h2');
    if (h) return h.innerText.trim();
    // Fallback: find any element containing the verdict phrase
    const all = Array.from(document.querySelectorAll('*'));
    const el = all.find(e =>
      e.children.length === 0 &&
      e.innerText &&
      e.innerText.toLowerCase().includes('fingerprint is')
    );
    return el ? el.innerText.trim() : null;
  })()
";

// ─── main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("[pixelscan] target : {TARGET_URL}");

    // ── Browser pool ──────────────────────────────────────────────────────────
    let config = BrowserConfig::builder()
        .headless(true)
        .stealth_level(StealthLevel::Advanced)
        .pool(PoolConfig {
            min_size: 1,
            max_size: 2,
            idle_timeout: Duration::from_mins(1),
            acquire_timeout: Duration::from_secs(30),
        })
        .build();

    eprintln!("[pixelscan] warming browser pool...");
    let pool = BrowserPool::new(config).await?;

    let handle = pool.acquire().await?;
    let browser = handle
        .browser()
        .ok_or("browser pool returned an expired handle")?;
    let mut page = browser.new_page().await?;

    // ── Navigate ──────────────────────────────────────────────────────────────
    // NetworkIdle ensures all /s/api/* fingerprint POST calls have completed and
    // the Angular app has rendered the verdict cards before we begin extraction.
    eprintln!("[pixelscan] navigating...");
    let t0 = Instant::now();
    page.navigate(TARGET_URL, WaitUntil::NetworkIdle, Duration::from_mins(1))
        .await?;
    let load_time_ms = u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX);
    eprintln!("[pixelscan] loaded in {load_time_ms}ms, waiting for cards...");

    // ── Poll for Angular cards (up to 30 s extra) ─────────────────────────────
    // NetworkIdle can fire before the server-side /s/api/* scoring calls
    // complete (pixelscan POSTs bot/canvas/font fingerprints to its backend and
    // renders the pass/fail verdict only after those return). Poll every 500 ms
    // until all cards have non-placeholder content.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let ready: bool = page.eval(READY_SCRIPT).await.unwrap_or(false);
        if ready {
            eprintln!("[pixelscan] cards settled");
            break;
        }
        if Instant::now() >= deadline {
            eprintln!(
                "[pixelscan] note: Location/Proxy cards still loading (pixelscan geo-API latency); \
                 extracting available results"
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // ── Extract ───────────────────────────────────────────────────────────────
    eprintln!("[pixelscan] extracting results...");

    let final_url = page.url().await.unwrap_or_else(|_| TARGET_URL.to_string());
    let verdict: Option<String> = page.eval(VERDICT_SCRIPT).await.ok();
    let cards: Value = page.eval(CARDS_SCRIPT).await.unwrap_or(json!([]));
    let details: Value = page
        .eval(DETAILS_SCRIPT)
        .await
        .unwrap_or_else(|_| json!({}));

    // ── Build report ──────────────────────────────────────────────────────────
    let report = json!({
        "url":          TARGET_URL,
        "final_url":    final_url,
        "verdict":      verdict,
        "cards":        cards,
        "details":      details,
        "load_time_ms": load_time_ms,
        "scraped_at":   epoch_secs(),
    });

    println!("{}", serde_json::to_string_pretty(&report)?);

    // ── Cleanup ───────────────────────────────────────────────────────────────
    page.close().await.ok();
    handle.release().await;

    eprintln!("[pixelscan] done.");
    Ok(())
}
