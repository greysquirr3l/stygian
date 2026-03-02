# Configuration

All browser behaviour is controlled through `BrowserConfig`. Every field can be set
programmatically or overridden at runtime via environment variables — no recompilation needed.

---

## Builder pattern

```rust,no_run
use stygian_browser::{BrowserConfig, StealthLevel};
use stygian_browser::config::PoolConfig;
use stygian_browser::webrtc::{WebRtcConfig, WebRtcPolicy};
use std::time::Duration;

let config = BrowserConfig::builder()
    // ── Browser ──────────────────────────────────────────────────────────
    .headless(true)
    .window_size(1920, 1080)
    // .chrome_path("/usr/bin/google-chrome".into())   // auto-detect if omitted

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
|---|---|---|---|
| `headless` | `bool` | `true` | Run without visible window |
| `window_size` | `(u32, u32)` | `(1920, 1080)` | Browser viewport dimensions |
| `chrome_path` | `Option<PathBuf>` | auto-detect | Path to Chrome/Chromium binary |
| `stealth_level` | `StealthLevel` | `Advanced` | Anti-detection level |
| `proxy` | `Option<String>` | `None` | Proxy URL (`http://`, `https://`, `socks5://`) |
| `user_data_dir` | `Option<PathBuf>` | fresh profile | Chrome user data directory |
| `extra_args` | `Vec<String>` | `[]` | Additional Chrome command-line flags |

### Pool settings (`PoolConfig`)

| Field | Type | Default | Description |
|---|---|---|---|
| `min_size` | `usize` | `2` | Browsers kept warm at all times |
| `max_size` | `usize` | `10` | Maximum concurrent browsers |
| `idle_timeout` | `Duration` | `5 min` | Evict idle browser after this duration |
| `acquire_timeout` | `Duration` | `30 s` | Max wait for a pool slot |

### WebRTC settings (`WebRtcConfig`)

| Field | Type | Default | Description |
|---|---|---|---|
| `policy` | `WebRtcPolicy` | `DisableNonProxied` | WebRTC IP leak policy |
| `location` | `Option<ProxyLocation>` | `None` | Simulated geo-location for WebRTC |

`WebRtcPolicy` variants:

| Variant | Behaviour |
|---|---|
| `Allow` | Default browser behaviour — real IPs may leak |
| `DisableNonProxied` | Block direct connections; only proxied paths allowed |
| `BlockAll` | Block all WebRTC — safest for anonymous scraping |

---

## Environment variable overrides

All config values can be overridden without touching source code:

| Variable | Default | Description |
|---|---|---|
| `MYCELIUM_CHROME_PATH` | auto-detect | Path to Chrome/Chromium binary |
| `MYCELIUM_HEADLESS` | `true` | Set `false` for headed mode |
| `MYCELIUM_STEALTH_LEVEL` | `advanced` | `none`, `basic`, `advanced` |
| `MYCELIUM_POOL_MIN` | `2` | Minimum warm browsers |
| `MYCELIUM_POOL_MAX` | `10` | Maximum concurrent browsers |
| `MYCELIUM_POOL_ACQUIRE_TIMEOUT_SECS` | `30` | Seconds to wait for a pool slot |
| `MYCELIUM_CDP_FIX_MODE` | `addBinding` | `addBinding`, `isolatedworld`, `enabledisable` |
| `MYCELIUM_PROXY` | — | Proxy URL |

---

## Loading config from environment

```rust,no_run
use stygian_browser::BrowserConfig;

// All fields read from environment; builder values serve as defaults
let config = BrowserConfig::from_env()?;
let pool   = BrowserPool::new(config).await?;
```

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
