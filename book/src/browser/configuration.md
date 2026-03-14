# Configuration

All browser behaviour is controlled through `BrowserConfig`. Every field can be set
programmatically or overridden at runtime via environment variables — no recompilation needed.

---

## Builder pattern

```rust,no_run
use stygian_browser::{BrowserConfig, HeadlessMode, StealthLevel};
use stygian_browser::config::PoolConfig;
use stygian_browser::webrtc::{WebRtcConfig, WebRtcPolicy};
use std::time::Duration;

let config = BrowserConfig::builder()
    // ── Browser ──────────────────────────────────────────────────────────
    .headless(true)
    .headless_mode(HeadlessMode::New)      // default; --headless=new shares headed rendering
    .window_size(1920, 1080)
    // .chrome_path("/usr/bin/google-chrome".into())   // auto-detect if omitted
    // .user_data_dir("/tmp/my-profile")   // omit for auto unique temp dir per instance

    // ── Stealth ───────────────────────────────────────────────────────────
    .stealth_level(StealthLevel::Advanced)

    // ── Network ───────────────────────────────────────────────────────────
    // .proxy("http://user:pass@proxy.example.com:8080".to_string())
    .webrtc(WebRtcConfig {
        policy: WebRtcPolicy::DisableNonProxied,
        ..Default::default()
    })

    // ── Pool ──────────────────────────────────────────────────────────────
    .pool(PoolConfig {
        min_size:        2,
        max_size:        10,
        idle_timeout:    Duration::from_secs(300),
        acquire_timeout: Duration::from_secs(10),
    })
    .build();
```

---

## Field reference

### Browser settings

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `headless` | `bool` | `true` | Run without visible window |
| `headless_mode` | `HeadlessMode` | `New` | `New` = `--headless=new` (full Chromium rendering, default since Chrome 112, **only mode since Chrome 132**); `Legacy` = `chrome-headless-shell` / pre-112 `--headless` |
| `window_size` | `Option<(u32, u32)>` | `(1920, 1080)` | Browser viewport dimensions |
| `chrome_path` | `Option<PathBuf>` | auto-detect | Path to Chrome/Chromium binary |
| `stealth_level` | `StealthLevel` | `Advanced` | Anti-detection level |
| `proxy` | `Option<String>` | `None` | Proxy URL (`http://`, `https://`, `socks5://`) |
| `user_data_dir` | `Option<PathBuf>` | auto-generated | Per-instance temp dir (`$TMPDIR/stygian-<id>`); set explicitly to share a persistent profile. Auto-generation prevents `SingletonLock` races between concurrent pools. |
| `args` | `Vec<String>` | `[]` | Additional Chrome command-line flags |

### Pool settings (`PoolConfig`)

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `min_size` | `usize` | `2` | Browsers kept warm at all times |
| `max_size` | `usize` | `10` | Maximum concurrent browsers |
| `idle_timeout` | `Duration` | `5 min` | Evict idle browser after this duration |
| `acquire_timeout` | `Duration` | `30 s` | Max wait for a pool slot |

### WebRTC settings (`WebRtcConfig`)

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `policy` | `WebRtcPolicy` | `DisableNonProxied` | WebRTC IP leak policy |
| `location` | `Option<ProxyLocation>` | `None` | Simulated geo-location for WebRTC |

`WebRtcPolicy` variants:

| Variant | Behaviour |
| --- | --- |
| `Allow` | Default browser behaviour — real IPs may leak |
| `DisableNonProxied` | Block direct connections; only proxied paths allowed |
| `BlockAll` | Block all WebRTC — safest for anonymous scraping |

---

## Environment variable overrides

All config values can be overridden without touching source code:

| Variable | Default | Description |
| --- | --- | --- |
| `STYGIAN_CHROME_PATH` | auto-detect | Path to Chrome/Chromium binary |
| `STYGIAN_HEADLESS` | `true` | Set `false` for headed mode |
| `STYGIAN_HEADLESS_MODE` | `new` | `new` (`--headless=new`) or `legacy` (`chrome-headless-shell`; old `--headless` removed in Chrome 132) |
| `STYGIAN_STEALTH_LEVEL` | `advanced` | `none`, `basic`, `advanced` |
| `STYGIAN_POOL_MIN` | `2` | Minimum warm browsers |
| `STYGIAN_POOL_MAX` | `10` | Maximum concurrent browsers |
| `STYGIAN_POOL_IDLE_SECS` | `300` | Idle timeout before browser eviction |
| `STYGIAN_POOL_ACQUIRE_SECS` | `5` | Seconds to wait for a pool slot |
| `STYGIAN_LAUNCH_TIMEOUT_SECS` | `10` | Browser launch timeout |
| `STYGIAN_CDP_TIMEOUT_SECS` | `30` | Per-operation CDP timeout |
| `STYGIAN_CDP_FIX_MODE` | `addBinding` | `addBinding`, `isolatedworld`, `enabledisable` |
| `STYGIAN_PROXY` | — | Proxy URL |
| `STYGIAN_PROXY_BYPASS` | — | Comma-separated proxy bypass list (e.g. `<local>,localhost`) |
| `STYGIAN_DISABLE_SANDBOX` | auto-detect | `true` inside containers, `false` on bare metal |

---

## Examples

### Minimal — fast text scraping

```rust,no_run
use stygian_browser::{BrowserConfig, StealthLevel};

let config = BrowserConfig::builder()
    .headless(true)
    .stealth_level(StealthLevel::None)  // no overhead
    .build();
```

### Headed debugging session

```rust,no_run
let config = BrowserConfig::builder()
    .headless(false)
    .stealth_level(StealthLevel::Basic)
    .build();
```

### Proxy with full stealth

```rust,no_run
use stygian_browser::webrtc::{WebRtcConfig, WebRtcPolicy};

let config = BrowserConfig::builder()
    .headless(true)
    .stealth_level(StealthLevel::Advanced)
    .proxy("socks5://user:pass@proxy.example.com:1080".to_string())
    .webrtc(WebRtcConfig { policy: WebRtcPolicy::BlockAll, ..Default::default() })
    .build();
```

### Anti-detection for JS-heavy sites (X/Twitter, LinkedIn)

`StealthLevel::Advanced` combined with `HeadlessMode::New` is the most evasion-resistant
configuration. `HeadlessMode::New` is the **default** since v0.1.11 — existing code
elevates automatically.

```rust,no_run
use stygian_browser::{BrowserConfig, HeadlessMode, StealthLevel};
use stygian_browser::webrtc::{WebRtcConfig, WebRtcPolicy};

let config = BrowserConfig::builder()
    .headless(true)
    .headless_mode(HeadlessMode::New)   // default; shared rendering pipeline with headed Chrome
    .stealth_level(StealthLevel::Advanced)
    .webrtc(WebRtcConfig { policy: WebRtcPolicy::BlockAll, ..Default::default() })
    .build();
```

For Chromium ≥ 112 (all modern Chrome / Chromium builds), `New` is the right
choice. `Legacy` targets are rare: pre-112 Chromium or the separately distributed
`chrome-headless-shell` binary for lightweight CI workloads where full rendering
fidelity is not required.

> **Note:** As of Chrome 132 the old `--headless` flag is removed entirely.
> `HeadlessMode::Legacy` now maps to `chrome-headless-shell` semantics — avoid it
> unless you are explicitly targeting that binary.

```rust,no_run
// Only needed for Chromium < 112 or chrome-headless-shell
let config = BrowserConfig::builder()
    .headless_mode(HeadlessMode::Legacy)
    .build();
```
