# Stealth, TLS Fingerprints, and Diagnostics

This page is the single source of truth for stygian-browser anti-detection behavior.
It consolidates browser stealth, TLS fingerprint control, and runtime verification.

## Optimal stealth profiles

stygian-browser now provides two reusable high-stealth constructors:

1. `BrowserConfig::stealth_profile_without_proxy()`
2. `BrowserConfig::stealth_profile_with_proxy(proxy_url)`

### Direct integration without proxy

```rust,no_run
use stygian_browser::{BrowserConfig, BrowserPool};

let config = BrowserConfig::stealth_profile_without_proxy();
let pool = BrowserPool::new(config).await?;
```

### Direct integration with proxy

```rust,no_run
use stygian_browser::{BrowserConfig, BrowserPool};

let config = BrowserConfig::stealth_profile_with_proxy("http://user:pass@proxy:8080");
let pool = BrowserPool::new(config).await?;
```

### What these profiles set

Both profiles set:

1. `StealthLevel::Advanced`
2. `HeadlessMode::New`
3. `CdpFixMode::AddBinding`
4. Default noise stack enabled
5. Weighted coherent fingerprint profile

Difference:

1. No-proxy profile uses `WebRtcPolicy::BlockAll`
2. Proxy profile uses `WebRtcPolicy::DisableNonProxied`

### Preset matrix

| Preset | Proxy required | WebRTC policy | Compatibility | Recommended use |
| --- | --- | --- | --- | --- |
| `stealth_profile_without_proxy()` | no | `BlockAll` | lower on RTC-heavy sites | direct scraping where no proxy is available and maximum leak prevention is required |
| `stealth_profile_with_proxy(proxy_url)` | yes | `DisableNonProxied` | higher on modern sites that rely on RTC/media surfaces | production traffic routed through trusted residential/datacenter proxy infrastructure |

Operational guidance:

1. Start with the profile that matches your network path (proxy vs non-proxy).
2. If pages fail due to RTC features, prefer the proxy profile over weakening non-proxy settings.
3. Keep `CdpFixMode::AddBinding` and `HeadlessMode::New` unchanged unless you have a
   compatibility break you can reproduce.
4. Validate each target with `verify_stealth_with_transport()` after navigation.

### MCP usage caveat

For browser_acquire, optional fields like stealth_level, webrtc_policy, cdp_fix_mode,
tls_profile, and proxy are metadata labels for session attribution and response echoing.
They do not reconfigure an already pooled browser instance at runtime.

To get true max-stealth behavior through MCP, start the MCP server with a BrowserPool
already configured using one of the profile constructors above.

Example server boot (non-proxy):

```rust,no_run
use stygian_browser::{BrowserConfig, BrowserPool};
use stygian_browser::mcp::McpBrowserServer;

let pool = BrowserPool::new(BrowserConfig::stealth_profile_without_proxy()).await?;
McpBrowserServer::new(pool).run().await?;
```

Example server boot (proxy):

```rust,no_run
use stygian_browser::{BrowserConfig, BrowserPool};
use stygian_browser::mcp::McpBrowserServer;

let pool = BrowserPool::new(BrowserConfig::stealth_profile_with_proxy("http://user:pass@proxy:8080")).await?;
McpBrowserServer::new(pool).run().await?;
```

## Stealth levels

| Level | Navigator and CDP baseline | Fingerprint injection | Noise layers | WebRTC protection | Coherence/peripheral/timing |
| --- | --- | --- | --- | --- | --- |
| none | no | no | no | no | no |
| basic | yes | no | no | no | no |
| advanced | yes | yes | yes | yes | yes |

Notes:

1. Basic uses minimal navigator spoof plus CDP mitigation.
2. Advanced enables the full injection stack in apply_stealth_to_page.
3. The human behavior APIs are available, but are not automatically injected by
   apply_stealth_to_page.

## What advanced actually injects

When stealth_level is advanced, stygian-browser injects the following at new-document time:

1. CDP hardening script.
2. CDP protection script (mode-driven).
3. Navigator spoof script.
4. Fingerprint script.
5. WebRTC script.
6. Canvas noise.
7. WebGL noise.
8. Audio noise.
9. Rects/TextMetrics noise.
10. Navigator coherence script.
11. Timing noise.
12. Peripheral stealth script.

## Headless mode

Use HeadlessMode::New unless you are forced onto old Chromium builds.

```rust,no_run
use stygian_browser::{BrowserConfig, HeadlessMode};

let cfg = BrowserConfig::builder()
    .headless_mode(HeadlessMode::New)
    .build();
```

Environment override:

```sh
STYGIAN_HEADLESS_MODE=new cargo run
```

## CDP leak mitigation

Set via BrowserConfig::cdp_fix_mode or STYGIAN_CDP_FIX_MODE.

| Mode | Summary |
| --- | --- |
| addBinding | recommended default; best overall |
| isolatedWorld | compatibility fallback |
| enableDisable | broad fallback |
| none | no mitigation |

## WebRTC policy

Set via BrowserConfig::webrtc.

| Policy | Effect |
| --- | --- |
| allow_all | no restriction |
| disable_non_proxied | force disable_non_proxied_udp |
| block_all | strongest leakage prevention; may break RTC apps |

For highest non-proxy stealth, prefer block_all unless target compatibility requires otherwise.

## Fingerprint coherence

Advanced mode derives identity from one coherent profile per browser instance.
If you do not set fingerprint_profile explicitly, a weighted fallback profile is chosen.

Current weighted fallback mapping:

1. Windows: 65%
2. macOS: 20%
3. Linux: 5%
4. Android: 10%

## TLS fingerprint control

TLS fingerprint control is orthogonal to browser JS stealth and requires feature tls-config.
Use it for HTTP clients where JA3/JA4 consistency matters.

Built-in profiles:

1. CHROME_131
2. FIREFOX_133
3. SAFARI_18
4. EDGE_131

```rust,no_run
use stygian_browser::tls::{build_profiled_client, CHROME_131};

let client = build_profiled_client(&CHROME_131, None)?;
let response = client.get("https://example.com").send().await?;
```

For browser contexts, chrome_tls_args can constrain TLS version behavior, but exact Chrome
cipher ordering remains tied to the browser binary.

## Runtime stealth diagnostics

Run diagnostics from a live page handle:

```rust,no_run
let report = page.verify_stealth().await?;
let report_with_transport = page.verify_stealth_with_transport(None).await?;
```

DiagnosticReport includes:

1. checks
2. passed_count
3. failed_count
4. known_limitations
5. transport

There are currently 25 built-in JS checks.

Known limitation probes currently include:

1. WebGPU surface exposure
2. performance.memory exposure

Treat known_limitations as visible surfaces not yet fully spoofed/validated.

## Detection vectors covered

| Vector | Primary layer |
| --- | --- |
| navigator.webdriver and descriptor shape | navigator spoof and diagnostic checks |
| chrome runtime/csi/loadTimes shape | navigator/chrome object spoof |
| plugins, languages, UA data coherence | fingerprint and navigator coherence |
| canvas, webgl, audio, rects fingerprints | noise modules and checks |
| CDP protocol leakage | cdp_hardening and cdp_protection |
| headless UA marker | headless new + spoofing checks |
| WebRTC leak surface | webrtc policy and script |
| JA3/JA4 and HTTP3 coherence | tls profiles + transport diagnostics |

## Human interaction APIs

These are available as explicit APIs and MCP interaction controls.

1. MouseSimulator
2. TypingSimulator
3. InteractionSimulator
4. RequestPacer

Use them when the target expects user-like interaction cadence. They are not automatically
invoked by apply_stealth_to_page.

## Cloudflare Turnstile reality check

1. Invisible/non-interactive modes are primarily fingerprint-driven.
2. Managed mode may require explicit user interaction and cannot be bypassed solely
   by improving fingerprint quality.

For managed flows, rely on sanctioned session bootstrap methods such as prior trusted
clearance state, human-in-the-loop, or site-approved challenge workflows.
