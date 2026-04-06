# Stealth & Anti-Detection

`stygian-browser` implements a layered anti-detection system. Each layer targets a different
class of bot-detection signal.

---

## Stealth levels

| Level | `navigator` spoof | Canvas noise | WebGL random | CDP protection | Human behaviour |
| --- | --- | --- | --- | --- | --- |
| `None` | — | — | — | — | — |
| `Basic` | ✓ | — | — | ✓ | — |
| `Advanced` | ✓ | ✓ | ✓ | ✓ | ✓ |

**Trade-offs:**

- `None` — maximum performance; no evasion. Suitable for internal services or sites
  with no bot detection.
- `Basic` — hides `navigator.webdriver`, masks the headless User-Agent, enables CDP
  protection. Adds < 1 ms overhead. Appropriate for most scraping workloads.
- `Advanced` — full fingerprint injection (canvas, WebGL, audio, fonts, hardware
  concurrency, device memory) plus human-like mouse and keyboard events. Adds 10–30 ms
  per page but passes all major detection suites.

---

## Headless mode

The classic `--headless` flag (`HeadlessMode::Legacy`) is a well-known detection signal:
sites like X/Twitter and LinkedIn inspect the Chrome renderer version string and reject
old-headless sessions before any session state is even checked.

Since Chrome 112, `stygian-browser` defaults to `--headless=new` (`HeadlessMode::New`),
which shares the **same rendering pipeline as headed Chrome** and is significantly
harder to fingerprint-detect.

```rust,no_run
use stygian_browser::{BrowserConfig, HeadlessMode};

// Default: HeadlessMode::New — no change needed for existing code
let config = BrowserConfig::builder()
    .headless_mode(HeadlessMode::New)
    .build();

// Legacy mode: only needed for Chromium < 112
let config = BrowserConfig::builder()
    .headless_mode(HeadlessMode::Legacy)
    .build();
```

Or via env var (no recompilation):

```sh
STYGIAN_HEADLESS_MODE=legacy cargo run   # opt back to old behaviour
```

---

## `navigator` spoofing

Executed on every new document context before any page script runs.

- Sets `navigator.webdriver` → `undefined`
- Patches `navigator.plugins` with a realistic `PluginArray`
- Sets `navigator.languages`, `navigator.language`, `navigator.vendor`
- Aligns `navigator.hardwareConcurrency` and `navigator.deviceMemory` with the
  chosen device fingerprint

Two layers of protection prevent `webdriver` detection:

1. **Instance patch** — `Object.defineProperty(navigator, 'webdriver', { get: () => undefined })` hides the flag from direct access (`navigator.webdriver === undefined`).
2. **Prototype patch** — `Object.defineProperty(Navigator.prototype, 'webdriver', { enumerable: false, ... })` hides the underlying getter from `Object.getOwnPropertyDescriptor(Navigator.prototype, 'webdriver')`, which some scanners (e.g. pixelscan.net, Akamai, Cloudflare Turnstile) probe directly. The `enumerable: false` descriptor matches real Chrome — `enumerable: true` is itself a detectable signal.

Both patches are injected into every new document context before any page script runs.

As of v0.8.2, the following additional signals are also patched:

- **UA version alignment** — all `NavigatorProfile` UA strings use `Chrome/131`, matching the
  default `chrome131` TLS profile. Cloudflare cross-references `navigator.userAgent` against the
  JA3/JA4 TLS fingerprint; a version mismatch (e.g. `Chrome/120` UA with a Chrome 131 TLS
  handshake) is a primary bot signal.
- **`window.chrome` object** — `chrome.runtime`, `chrome.csi`, and `chrome.loadTimes` are
  stubbed. These properties are present in every real Chrome session but absent in headless;
  their absence is directly checked by Turnstile and pixel-scanner suites.
- **`navigator.userAgentData`** — the `brands` array and `uaFullVersion` are spoofed to match
  the `navigator.userAgent` Chrome version. Without this, `userAgentData.brands` reflects the
  actual binary version (e.g. `Chromium/139`) while `userAgent` reports `Chrome/131`,
  creating a cross-referenceable mismatch.

The fingerprint is drawn from statistically-weighted device profiles:

```rust
use stygian_browser::fingerprint::{DeviceProfile, Platform};

let profile = DeviceProfile::random();   // weighted: Windows 60%, Mac 25%, Linux 15%
println!("Platform:    {:?}", profile.platform);
println!("CPU cores:   {}",   profile.hardware_concurrency);
println!("Device RAM:  {} GB", profile.device_memory);
println!("Screen:      {}×{}", profile.screen_width, profile.screen_height);
```

---

## Canvas fingerprint noise

`HTMLCanvasElement.toDataURL()` and `CanvasRenderingContext2D.getImageData()` are patched
to add sub-pixel noise (< 1 px) — visually indistinguishable but unique per page load,
preventing cross-site canvas fingerprint correlation.

The noise function is applied via JavaScript injection into each document:

```javascript
// Simplified representation of the injected script
const origToDataURL = HTMLCanvasElement.prototype.toDataURL;
HTMLCanvasElement.prototype.toDataURL = function(...args) {
    const result = origToDataURL.apply(this, args);
    return injectNoise(result, sessionNoiseSeed);
};
```

---

## WebGL randomisation

GPU-based fingerprinting reads `RENDERER` and `VENDOR` strings from WebGL. These are
intercepted and replaced with plausible — but randomised — GPU family names:

| Real value | Spoofed value (example) |
| --- | --- |
| `ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)` | `ANGLE (NVIDIA, ANGLE Metal Renderer: NVIDIA GeForce RTX 3070 Ti)` |
| `Google SwiftShader` | `ANGLE (Intel, ANGLE Metal Renderer: Intel Iris Pro)` |

The spoofed values are consistent within a session and coherent with the chosen
device profile.

---

## CDP leak protection

The Chrome DevTools Protocol itself can expose automation. Three modes are available,
set via `STYGIAN_CDP_FIX_MODE` or `BrowserConfig::cdp_fix_mode`:

| Mode | Protection | Compatibility |
| --- | --- | --- |
| `AddBinding` (default) | Wraps calls to hide `Runtime.enable` side-effects | Best overall |
| `IsolatedWorld` | Runs injection in a separate execution context | Moderate |
| `EnableDisable` | Toggles enable/disable around each command | Broad |

---

## Human behaviour simulation (`Advanced` only)

### Mouse movement — `MouseSimulator`

Generates Bézier-curve paths with natural arc shapes:

- Distance-aware step counts (12 steps for < 100 px, up to 120 for > 1 000 px)
- Perpendicular control-point offsets for curved trajectories
- Sub-pixel micro-tremor jitter (± 0.3 px per step)
- 10–50 ms inter-event delays

```rust,no_run
use stygian_browser::behavior::MouseSimulator;

let sim = MouseSimulator::new();
// Move from (100, 200) to (450, 380) with realistic arc
sim.move_to(&page, 100.0, 200.0, 450.0, 380.0).await?;
sim.click(&page, 450.0, 380.0).await?;
```

### Keyboard — `TypingSimulator`

Models realistic typing cadence:

- Per-key WPM variation (70–130 WPM base rate)
- Configurable typo-and-correct probability
- Burst/pause rhythm typical of human typists

```rust,no_run
use stygian_browser::behavior::TypingSimulator;

let typer = TypingSimulator::new()
    .wpm(90)
    .typo_rate(0.03);   // 3% typo probability

typer.type_into(&page, "#search-input", "rust async web scraping").await?;
```

---

## Network Information API spoofing

`navigator.connection` (Network Information API) reveals connection quality and type.  
Headless browsers return `null` here, which is an immediate headless signal on connection-aware scanners.

`Advanced` stealth injects a realistic `NetworkInformation`-like object:

| Property | Spoofed value |
| --- | --- |
| `effectiveType` | `"4g"` |
| `type` | `"wifi"` |
| `downlink` | Seeded from `performance.timeOrigin` (stable per session, ≈ 10 Mbps range) |
| `rtt` | Seeded jitter (50–100 ms range) |
| `saveData` | `false` |

---

## Battery Status API spoofing

`navigator.getBattery()` returns `null` in headless Chrome — a clear automation signal
for scanners that enumerate battery state.

`Advanced` stealth overrides `getBattery()` to resolve with a plausible disconnected-battery state:

| Property | Spoofed value |
| --- | --- |
| `charging` | `false` |
| `chargingTime` | `Infinity` |
| `dischargingTime` | Seeded (≈ 3600–7200 s) |
| `level` | Seeded (0.65–0.95) |

The seed values are derived from `performance.timeOrigin` so they are stable within a page
load but differ across sessions, preventing replay detection.

---

## Fingerprint consistency

All spoofed signals are derived from a single `DeviceProfile` generated at browser
launch. The profile is consistent across tabs and across the entire session, preventing
inconsistency-based detection (e.g. a Windows User-Agent combined with macOS font metrics).

---

## Cloudflare Turnstile

Cloudflare Turnstile operates in three challenge modes. They have fundamentally different
bypass characteristics and it is important to understand which mode a target site uses.

### Non-Interactive and Invisible modes

In these modes Turnstile makes a pass/fail decision based on the request fingerprint alone
— no user action is required and no visible widget appears. The fingerprint checks include
(among others) TLS/JA3 profile vs `navigator.userAgent` version alignment, `webdriver`
descriptor properties, presence of `window.chrome.runtime`, and consistency of
`navigator.userAgentData`.

`stygian-browser` v0.8.2 addresses all four of these primary signals (see
[`navigator` spoofing](#navigator-spoofing) above). Sites using Invisible or Non-Interactive
Turnstile should pass without any additional configuration beyond `StealthLevel::Basic`.

A scheduled canary (`.github/workflows/stealth-canary.yml`) runs `verify_stealth()` daily
against the built-in check suite to detect regressions automatically. On failure it calls
GitHub Models to propose and validate a fix, then opens a pull request.

### Managed mode (requires user interaction)

Managed mode displays a visible checkbox widget that **must be clicked by a human**,
regardless of fingerprint quality. Even a perfect, undetectable fingerprint will not
bypass a Managed-mode challenge — Turnstile will present the checkbox and wait.

This is an explicit site operator choice, not a fingerprinting failure. Sites like
`help.ui.com` use Managed mode as a deliberate security policy.

> **stygian-browser cannot bypass Managed-mode Turnstile automatically.**
> The stealth improvements in v0.8.2 have no effect on Managed-mode challenges.

Options for sites using Managed mode:

1. **Cookie injection** — if you have obtained a valid clearance cookie from a prior
   human session, inject it with `inject_cookies()` before navigating. The clearance
   cookie has a TTL (typically 30 minutes–24 hours) and may be reused until it expires.
2. **CAPTCHA solver integration** — third-party services (2captcha, CapSolver, etc.) can
   interact with the Turnstile iframe on your behalf. Integrate via their REST API before
   the page load completes.
3. **Residential proxy with pre-warmed cookies** — some residential proxy providers
   bundle Turnstile cookie caches; check your provider's documentation.

To determine which mode a site uses, inspect the `<div data-sitekey="...">` widget
attributes: `data-appearance="always"` indicates Managed; `data-appearance="interaction-only"`
or `data-appearance="never"` indicates Non-Interactive or Invisible respectively.
