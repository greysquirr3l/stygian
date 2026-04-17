# Stealth v2 — TLS Fingerprinting & Self-Diagnostics

Stealth v2 adds two orthogonal capabilities that complement the browser-level
evasion techniques described in [Stealth & Anti-Detection](./stealth.md):

1. **TLS fingerprint control** — make plain-HTTP scraping sessions present a
   browser-authentic `ClientHello`, defeating network-layer JA3/JA4 detection.
2. **Stealth self-diagnostic** — run the full suite of browser detection checks
   against your own session and get a machine-readable pass/fail report.

---

## Why TLS fingerprinting matters

When your code opens a TLS connection the handshake `ClientHello` message carries
a fingerprint: the ordered list of cipher suites, extension IDs, elliptic-curve
groups, and ALPN protocols. This fingerprint is stable per TLS library — a
`reqwest` client backed by `rustls` produces a different JA3 hash than Chrome.

Anti-bot services such as Cloudflare, Akamai, and Datadome routinely check JA3/JA4
hashes alongside the `User-Agent` header. A Chrome `User-Agent` paired with a
Rust/`rustls` TLS fingerprint is an immediate automation signal.

**TLS fingerprint control** rewires the `rustls` cipher-suite, key-exchange-group,
ALPN, and version ordering to match a real browser's `ClientHello`.

---

## Built-in TLS profiles

Four static profiles ship with measured cipher-suite and extension ordering from
real browser captures:

| Profile | Browser | Static |
| --- | --- | --- |
| `CHROME_131` | Google Chrome 131 | `stygian_browser::tls::CHROME_131` |
| `FIREFOX_133` | Mozilla Firefox 133 | `stygian_browser::tls::FIREFOX_133` |
| `SAFARI_18` | Apple Safari 18 | `stygian_browser::tls::SAFARI_18` |
| `EDGE_131` | Microsoft Edge 131 | `stygian_browser::tls::EDGE_131` |

Each profile exposes its JA3 and JA4 representations:

```rust,no_run
use stygian_browser::tls::{CHROME_131, FIREFOX_133};

let ja3 = CHROME_131.ja3();
println!("JA3 raw:  {}", ja3.raw);   // "772,4865-4866-...,23-65281-...,29-23-24,0"
println!("JA3 hash: {}", ja3.hash);  // 32-char MD5 hex

let ja4 = FIREFOX_133.ja4();
println!("JA4: {}", ja4.fingerprint); // "t13d1715h2_..._..."
```

### Weighted random selection

`TlsProfile::random_weighted(seed)` picks a profile weighted by real browser
market share (Windows/macOS/Linux × Chrome/Firefox/Safari/Edge):

```rust,no_run
use stygian_browser::tls::TlsProfile;

// Deterministic from seed (e.g. derived from target URL hash).
let profile = TlsProfile::random_weighted(42);
println!("selected: {}", profile.name);
```

---

## HTTP scraping with a TLS profile

Use `build_profiled_client` (feature `tls-config`) to get a `reqwest::Client`
whose TCP connections present the chosen browser fingerprint:

```rust,no_run
use stygian_browser::tls::{build_profiled_client, CHROME_131};

// Build a reqwest::Client that fingerprints as Chrome 131.
let client = build_profiled_client(&CHROME_131, None)?;
let resp = client.get("https://example.com/data").send().await?;
```

The client automatically:

- Sets `User-Agent` to match the profile's browser (Chrome, Firefox, Safari, or Edge).
- Enables cookie storage, gzip, and brotli decompression.
- Configures cipher-suite ordering, key-exchange groups, and ALPN to match the profile.

Pass a proxy URL as the second argument to route through a residential proxy:

```rust,no_run
let client = build_profiled_client(&CHROME_131, Some("http://user:pass@proxy:8080"))?;
```

### TLS + profile consistency

Anti-bot systems cross-reference the `User-Agent` header against the TLS fingerprint.
Always use the matching `User-Agent` when sending a specific profile:

```rust,no_run
use stygian_browser::tls::{build_profiled_client, default_user_agent, SAFARI_18};

// default_user_agent returns the correct UA string for the profile's browser.
let ua = default_user_agent(&SAFARI_18);
let client = reqwest::Client::builder()
    .user_agent(ua)
    .build()?;
// ... or use build_profiled_client which sets the UA automatically.
```

---

## Browser TLS alignment

When running a headed or headless Chrome session, `chrome_tls_args(profile)` returns
the launch flags that constrain the Chrome TLS version range to match the profile:

```rust,no_run
use stygian_browser::tls::{chrome_tls_args, FIREFOX_133};

// Returns flags like ["--ssl-version-max=tls1.2"] when the profile is TLS 1.2-only.
let args = chrome_tls_args(&FIREFOX_133);
for arg in &args {
    println!("chrome flag: {arg}");
}
```

> **Note:** Chrome's BoringSSL fixes cipher-suite ordering at compile time.
> `chrome_tls_args` controls the **version** range; precise JA3/JA4 matching
> requires either a patched Chromium build or an external TLS proxy fed by
> `to_rustls_config()`.

---

## `to_rustls_config` (advanced)

For direct `tokio-rustls` or custom `reqwest` builder use, `TlsProfile::to_rustls_config()`
builds an `Arc<rustls::ClientConfig>` with exactly the profile's cipher-suite and
key-exchange-group ordering:

```rust,no_run
use stygian_browser::tls::CHROME_131;
use reqwest::ClientBuilder;

let tls = CHROME_131.to_rustls_config()?;
let client = ClientBuilder::new()
    .use_preconfigured_tls((*tls.into_inner()).clone())
    .build()?;
```

---

## Stealth self-diagnostic

`PageHandle::verify_stealth()` runs the full suite of JavaScript detection checks
in the active page and returns a `DiagnosticReport`:

```rust,no_run
use stygian_browser::{BrowserConfig, BrowserPool, StealthLevel, WaitUntil};
use std::time::Duration;

let pool = BrowserPool::new(BrowserConfig::builder()
    .stealth_level(StealthLevel::Advanced)
    .build())
    .await?;

let handle = pool.acquire().await?;
let browser = handle
    .browser()
    .ok_or_else(|| std::io::Error::other("browser handle already released"))?;
let page = browser.new_page().await?;
page.navigate("https://example.com", WaitUntil::Load, Duration::from_secs(30)).await?;  // navigate for realistic context

let report = page.verify_stealth().await?;

if report.is_clean() {
    println!("all {} checks passed ✓", report.passed_count);
} else {
    println!("{} / {} checks failed:", report.failed_count, report.checks.len());
    for failure in report.failures() {
        println!("  {:?}: {}", failure.id, failure.details);
    }
}

println!("coverage: {:.1}%", report.coverage_pct());
```

### DiagnosticReport

| Method / field | Description |
| --- | --- |
| `checks: Vec<CheckResult>` | Results for all 18 checks, in definition order |
| `passed_count: usize` | Number of checks that passed |
| `failed_count: usize` | Number of checks that failed |
| `is_clean() -> bool` | `true` when all checks passed |
| `coverage_pct() -> f64` | `passed_count / checks.len() × 100.0` |
| `failures() -> Vec<&CheckResult>` | Filtered slice of failed results |
| `transport: Option<TransportDiagnostic>` | Transport fingerprint diagnostics (v0.9.1+) |

### Detection checks

| ID | What is being tested |
| --- | --- |
| `WebDriverFlag` | `navigator.webdriver` is `undefined` or not present |
| `ChromeObject` | `window.chrome` exists with expected structure |
| `PluginCount` | `navigator.plugins.length > 0` (headless returns 0) |
| `LanguagesPresent` | `navigator.languages` is non-empty |
| `CanvasConsistency` | Canvas `.toDataURL()` does not throw or return blank |
| `WebGlVendor` | `WEBGL_debug_renderer_info` returns a non-empty string |
| `AutomationGlobals` | `__webdriver_evaluate`, `_phantom`, `callPhantom` etc. absent |
| `OuterWindowSize` | `window.outerWidth > 0` (some headless configs return 0) |
| `HeadlessUserAgent` | `navigator.userAgent` does not contain `"HeadlessChrome"` |
| `NotificationPermission` | `Notification.permission !== "denied"` (instantly denied in headless) |
| `MatchMediaPresent` | `window.matchMedia` is a function (PX env-bitmask bit 0) |
| `ElementFromPointPresent` | `document.elementFromPoint` is a function (PX env-bitmask bit 1) |
| `RequestAnimationFramePresent` | `window.requestAnimationFrame` is a function (PX env-bitmask bit 2) |
| `GetComputedStylePresent` | `window.getComputedStyle` is a function (PX env-bitmask bit 3) |
| `CssSupportsPresent` | `CSS.supports` exists and is callable (PX env-bitmask bit 4) |
| `SendBeaconPresent` | `navigator.sendBeacon` is a function (PX env-bitmask bit 5) |
| `ExecCommandPresent` | `document.execCommand` is a function (PX env-bitmask bit 6) |
| `NodeJsAbsent` | `process.versions.node` is absent — not a Node.js runtime (PX env-bitmask bit 7) |

### Check granularity

Each `CheckResult` carries:

```rust,no_run
pub struct CheckResult {
    pub id:          CheckId,
    pub description: String,
    pub passed:      bool,
    pub details:     String,  // human-readable reason on failure
}
```

If the JavaScript evaluation itself fails (e.g. the browser is in a state where
`eval` is unavailable), the check is recorded as failed with `details` beginning
with `"script error: "` — no panic, no error propagation.

### Serialization

`DiagnosticReport` and `CheckResult` are fully JSON-serializable via `serde`:

```rust,no_run
let json = serde_json::to_string_pretty(&report)?;
// {"checks": [{...},...], "passed_count":18, "failed_count":0}
```

---

## Transport diagnostics (v0.9.1+)

`PageHandle::verify_stealth_with_transport()` extends the standard diagnostic with
transport-layer fingerprint verification. Pass observed JA3/JA4/HTTP3 values and
the report will compare them against expectations derived from the page's User-Agent:

```rust,no_run
use stygian_browser::diagnostic::TransportObservations;

let observations = TransportObservations {
    ja3_hash: Some("abc123...".to_string()),
    ja4: Some("t13d1715h2_...".to_string()),
    http3_perk_text: None,
    http3_perk_hash: None,
};

let report = page.verify_stealth_with_transport(Some(observations)).await?;

if let Some(transport) = &report.transport {
    println!("Expected profile: {:?}", transport.expected_profile);
    println!("Transport match:  {:?}", transport.transport_match);
    for mismatch in &transport.mismatches {
        println!("  mismatch: {mismatch}");
    }
}
```

### TransportDiagnostic fields

| Field | Description |
| --- | --- |
| `user_agent` | User-Agent sampled from the live page |
| `expected_profile` | Built-in TLS profile name inferred from UA (e.g. `"Chrome 131"`) |
| `expected_ja3_hash` | Expected JA3 MD5 hash from the inferred profile |
| `expected_ja4` | Expected JA4 fingerprint from the inferred profile |
| `expected_http3_perk_text` | Expected HTTP/3 perk text (settings + pseudo-headers) |
| `expected_http3_perk_hash` | Expected HTTP/3 perk MD5 hash |
| `observed` | Caller-supplied `TransportObservations` |
| `transport_match` | `Some(true)` if all observations match; `Some(false)` on mismatch; `None` if no observations |
| `mismatches` | Human-readable mismatch reasons |

### Unknown User-Agent handling

If the page's User-Agent cannot be mapped to a known browser profile, no expected
fingerprints are derived. When observations are provided but expectations are
missing, `transport_match` resolves to `Some(false)` with explicit mismatch entries
explaining that no comparison was possible.

---

## Detection landscape (v2)

Stealth v2 covers the following detection vectors, building on the base stealth layers
documented in [Stealth & Anti-Detection](./stealth.md):

| Detection vector | Layer |
| --- | --- |
| `navigator.webdriver` | Browser stealth scripts |
| Canvas / WebGL fingerprint | Browser stealth scripts (Advanced) |
| TLS fingerprint (JA3/JA4) | `build_profiled_client` / `to_rustls_config` |
| HTTP/3 fingerprint (perk) | `Http3Perk` / `expected_http3_perk_from_user_agent` |
| `User-Agent` / TLS mismatch | `default_user_agent` alignment |
| PX env-bitmask (bits 0–7) | Browser stealth scripts (v0.9.1+) |
| CDP protocol leaks | CDP fix mode (`AddBinding` / `IsolatedWorld`) |
| Headless `User-Agent` string | `HeadlessMode::New` + stealth UA patching |
| IP session consistency | Sticky-session proxy rotation |
| Anti-bot challenge pages | Tiered escalation pipeline |
| Passive self-verification | `verify_stealth()` / `verify_stealth_with_transport()` |
