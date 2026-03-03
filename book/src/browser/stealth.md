# Stealth & Anti-Detection

`stygian-browser` implements a layered anti-detection system. Each layer targets a different
class of bot-detection signal.

---

## Stealth levels

| Level | `navigator` spoof | Canvas noise | WebGL random | CDP protection | Human behaviour |
|---|---|---|---|---|---|
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

Since v0.1.11, `stygian-browser` defaults to `--headless=new` (`HeadlessMode::New`),
which shares the **same rendering pipeline as headed Chrome** and is significantly
harder to fingerprint-detect.

```rust,no_run
use stygian_browser::{BrowserConfig, HeadlessMode};

// Default since v0.1.11 — no change needed for existing code
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
|---|---|
| `ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)` | `ANGLE (NVIDIA, ANGLE Metal Renderer: NVIDIA GeForce RTX 3070 Ti)` |
| `Google SwiftShader` | `ANGLE (Intel, ANGLE Metal Renderer: Intel Iris Pro)` |

The spoofed values are consistent within a session and coherent with the chosen
device profile.

---

## CDP leak protection

The Chrome DevTools Protocol itself can expose automation. Three modes are available,
set via `STYGIAN_CDP_FIX_MODE` or `BrowserConfig::cdp_fix_mode`:

| Mode | Protection | Compatibility |
|---|---|---|
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

## Fingerprint consistency

All spoofed signals are derived from a single `DeviceProfile` generated at browser
launch. The profile is consistent across tabs and across the entire session, preventing
inconsistency-based detection (e.g. a Windows User-Agent combined with macOS font metrics).
