# Changelog

All notable changes to the stygian workspace will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `stygian-browser/tests`: direct MCP integration test `mcp_acquire_navigate_release_round_trip`
  in `mcp_integration.rs` exercises JSON-RPC `tools/call` flow for
  `browser_acquire` -> `browser_navigate` -> `browser_release` against a live browser
- `stygian-browser (mcp)`: new `browser_attach` tool contract for future
  extension/CDP attach workflows. The tool validates mode/endpoint intent and
  reports attach capability status in a machine-readable payload for client
  feature detection
- `stygian-browser`: Tier 1 validator implementations for `CreepJS` and `BrowserScan`
  now run real browser-backed checks (navigate, probe score/block state, capture
  failure screenshot, and release pooled session)

### Changed

- `stygian-browser (mcp)`: session model now persists a page per MCP session
  instead of opening a fresh page per call, preserving navigation state across
  `browser_navigate`, `browser_eval`, `browser_content`, and `browser_screenshot`
- `stygian-browser (mcp)`: `browser_acquire` accepts `target_profile`
  (`default|reddit`) and `browser_navigate` now returns challenge status metadata
  (`challenge_detected`, `challenge_cleared`) for challenge-heavy targets
- `stygian-browser/tests`: `mcp_session_save_restore_and_humanize_round_trip`
  now also exercises the `browser_attach` capability contract path
- `stygian-browser (validation)`: `ValidationSuite::run_one` awaits async Tier 1
  validators, replacing prior stub-only behavior for baseline observatory checks

## [0.9.4] - 2026-04-17

### Added

- `stygian-browser`: three new MCP tools — `browser_extract_with_fallback` (tries multiple CSS
  selectors in priority order, advancing only when at least one record is extracted),
  `browser_extract_resilient` (schema-driven extraction with configurable per-field fallback
  selectors), and `browser_proxied` cross-crate tool forwarded by `stygian-mcp`
- `stygian-proxy`: three new MCP tools — `proxy_acquire_with_capabilities` (acquire a proxy
  constrained by capabilities bitmask: `anonymous`, `rotating`, `residential`, `datacenter`,
  `socks`), `proxy_fetch_freelist` (fetch and import proxies from a named free-list source;
  SOCKS4/5 sources gated behind the `socks` feature), and `proxy_fetch_freeapiproxies`
  (import proxies from the free-api-proxies provider)
- `stygian-mcp`: MCP tool matrix documenting all `graph_*`, `browser_*`, `proxy_*`, and
  cross-crate tools added to root README and book overview

### Changed

- `stygian-proxy`: `proxy_fetch_freelist` schema enum for `source` field is now conditionally
  built by `freelist_source_enum_values()` — SOCKS4/5 source variants (`the_speedx_socks4`,
  `the_speedx_socks5`) are included only when the `socks` feature is enabled, keeping the
  schema honest at compile time
- `stygian-browser`: `browser_extract_with_fallback` selector logic now only advances
  `matched_selector` and breaks when at least one record is actually extracted from
  a selector, preventing false-positive selector matches on empty result sets
- `book/mcp`: overview page updated to document MCP `2025-11-25` as the current protocol
  version; proxy-tools and browser-tools pages updated with full reference entries for all
  new tools

### Refactored

- `stygian-browser`: `tool_browser_extract_with_fallback` split into `parse_root_selectors`
  and `parse_extract_schema` helpers to satisfy `clippy::too_many_lines` — no behaviour change

### Fixed

- `stygian-browser`: broken intra-doc links in `page.rs` (`BrowserError::QuerySelectorFailed`
  replaced with `BrowserError::CdpError`) and `tls.rs` (`ProfileChannel::from_str` replaced
  with `std::str::FromStr::from_str`) that caused `RUSTDOCFLAGS=-Dwarnings` Documentation CI failures

### CI

- `release.yml` synced with updated workflow template: tag trigger narrowed to semver
  `v[0-9]*.[0-9]*.[0-9]*`, `workflow_dispatch` accepts a `tag` input, concurrency group
  added (`cancel-in-progress: false`), `resolve` job gains `release_sha` output and
  `set -euo pipefail`, `verify-ci` uses `needs.resolve.outputs.release_sha`

## [0.9.3] - 2026-04-16

### Added

- `stygian-browser`: `ProxySource` and `ProxyLease` traits defined in the browser crate
  (domain-side) so the hexagonal boundary is respected — adapters depend inward on ports
- `stygian-browser`: `BrowserConfigBuilder::proxy_source()` wires a `ProxySource`
  implementation into `BrowserPool`; proxy leases are acquired and released across the full
  page lifecycle
- `ci`: expanded cross-platform matrix to cover Windows (`windows-latest`) in addition to
  macOS — build, test, clippy, and MSRV checks now run on both targets
- `ci`: Dependabot auto-merge workflow for patch and minor dependency updates

### Changed

- `stygian-proxy`: implements the `ProxySource` port trait from `stygian-browser` rather
  than defining its own, completing the port-inversion for proxy infrastructure
- `stygian-browser/README.md`: feature flag names, integration examples, and dependency
  snippets corrected; `stygian_proxy` integration example marked `rust,ignore` to prevent
  doctest compile failures when the crate is not a direct dependency

### Fixed

- `stygian-mcp`: MCP serialization error path now extracts the JSON-RPC `id` before
  the failed serialization and emits a proper `-32603` error response with
  `tracing::error!` instead of silently discarding the message
- `stygian-graph (cli)`: `cmd_check` replaced `std::process::exit(1)` with
  `anyhow::bail!` propagation; `#[allow(clippy::unnecessary_wraps)]` and
  `#[allow(clippy::expect_used)]` suppressions removed, return types corrected
- `stygian-browser (config)`: `PoisonError` recovery in `RwLock` guards now emits
  `tracing::warn!` before recovering instead of recovering silently
- `stygian-browser`: resolve CI test and doctest regressions in `cdp_hardening`,
  `peripheral_stealth`, and `profile` modules; test assertions aligned with generated JS
  and `validate()` rules
- `stygian-browser`: fix rustdoc intra-doc links (`[Type]` references) that caused the
  documentation CI job to fail with `-D warnings`
- `stygian-browser`: align stealth seed coherence and validation tests with updated
  `FingerprintProfile::validate()` rules
- `stygian-browser`: simplify MCP target selection logic to resolve `clippy::pedantic`
  lint
- `workspace`: `core-foundation` duplicate version (0.9.x and 0.10.x, both transitive
  via `chromiumoxide`/`reqwest`) added to `deny.toml` skip list to unblock `cargo deny`
  CI — no direct upgrade path available upstream

## [0.9.2] - 2026-04-15

### Changed

- `workspace`: papertowel cleanup pass reduced AI-fingerprint noise in comments/docs and
  refreshed `.papertowelignore` suppressions for known false-positive areas (for example,
  JA3/JA4 MD5 usage in TLS fingerprinting and placeholder credential strings in docs)

### Fixed

- `workspace`: doctest reliability under `--all-features` improved across browser/docs
  examples — async `main` snippets marked appropriately, browser extraction/navigation
  examples corrected, and empty/incomplete rustdoc fragments repaired to satisfy
  `RUSTDOCFLAGS=-Dwarnings`
- `workspace`: `rustls-webpki` updated to `0.103.12` in lockfile to resolve
  `RUSTSEC-2026-0098` and `RUSTSEC-2026-0099` reported by scheduled `cargo deny` checks
- `ci(stealth-canary)`: added `models: read` workflow permission so the automated
  GitHub Models autofix step can authenticate instead of failing with HTTP 4xx/exit 22

### Documentation

- `stygian-browser`: doc examples and API docs in `lib.rs`, `page.rs`, `extract.rs`,
  `tls.rs`, `behavior.rs`, and `tests/ui.rs` were completed/reworded to remove truncated
  sentences and empty Rust code blocks flagged during Copilot review and docs CI

## [0.9.1] - 2026-04-12

### Added

- `stygian-browser`: PX VM environment-bitmask stealth checks (bits 0–7) — tests browser
  API presence: `matchMedia`, `elementFromPoint`, `requestAnimationFrame`, `getComputedStyle`,
  `CSS.supports`, `sendBeacon`, `execCommand`, and Node.js absence; `DiagnosticReport`
  stores the result of each check individually
- `stygian-browser`: `Http3Perk` type models HTTP/3 SETTINGS fingerprints (settings
  identifiers, pseudo-header ordering, GREASE presence); `TlsProfile::http3_perk()` returns
  the expected perk for Chrome, Edge, and Firefox profiles; Safari returns `None`
- `stygian-browser`: `expected_http3_perk_from_user_agent()`, `expected_tls_profile_from_user_agent()`,
  `expected_ja3_from_user_agent()`, and `expected_ja4_from_user_agent()` resolve fingerprint
  expectations from a UA string for early mismatch detection
- `stygian-browser`: `TransportObservations` and `TransportDiagnostic` types expose
  expected vs. observed JA3/JA4/HTTP3 transport fingerprints in `DiagnosticReport.transport`
- `stygian-browser`: `PageHandle::verify_stealth_with_transport()` accepts optional observed
  transport values and returns a full `DiagnosticReport` including the new transport field
- `stygian-browser`: MCP `browser_verify_stealth` tool now accepts four optional observed
  transport fields (`observed_ja3_hash`, `observed_ja4`, `observed_http3_perk_text`,
  `observed_http3_perk_hash`) and includes `TransportDiagnostic` in the response
- `stygian-browser/examples/stealth_probe`: `--ja3-hash`, `--ja4`, `--http3-perk-text`,
  and `--http3-perk-hash` CLI flags pass observed transport values into the canary probe;
  transport section emitted directly from `DiagnosticReport`

### Fixed

- `stygian-browser`: `TlsProfile::http3_perk()` no longer falls through to a Chrome UA
  for Safari profiles — Safari now correctly returns `None`
- `stygian-browser`: `transport_match: Some(true)` false-positive suppressed — when the
  observed UA is unknown and no expected values can be derived, explicit mismatch entries
  are pushed so `transport_match` resolves to `Some(false)`
- `stygian-browser`: `SCRIPT_NODEJS_ABSENT` hardened against `process.versions === null`
  (e.g. in Deno) — property access was guarded with `||process.versions==null` to avoid
  a potential thrown exception before the `node` field could be read
- `stygian-browser/examples/stealth_probe`: `--threshold` now validates the supplied value
  is within `0.0..=1.0`; previously the error message claimed a range that was not enforced
- `stygian-browser`: diagnostic test renamed from `all_checks_returns_ten_entries` to
  `all_checks_returns_eighteen_entries` to match the actual assertion count

### Documentation

- `book/browser/stealth-v2.md`: updated `DiagnosticReport` table (10→18 checks), added 8
  PX env-bitmask checks to Detection checks table, added new Transport diagnostics section
  documenting `verify_stealth_with_transport()` and `TransportDiagnostic` fields
- `book/mcp/browser-tools.md`: fixed `browser_verify_stealth` JSON example to show actual
  `DiagnosticReport` structure with all 18 check IDs
- `examples/stealth-audit.toml`: updated check count (10→18) and added PX bit descriptions
- `crates/stygian-graph/README.md`: updated test count badge (209→1639)
- `book/reference/testing.md`: updated workspace test count (209→1639)

## [0.9.0] - 2026-04-11

### Changed

- `workspace`: all crates now pass `clippy::pedantic` at maximum sensitivity with zero
  warnings — `expect_used`, `unwrap_used`, `indexing_slicing`, `panic`, and `needless_pass_by_value`
  are all denied workspace-wide; tests use `Result`-returning signatures with `?` propagation
  and `pointer()`/`get()` chains for JSON field access instead of index operators
- `stygian-mcp`: `ok_response` helper now accepts `result: &Value` (was owned) to fix
  `needless_pass_by_value`; all call sites updated; `mcp_error_message` lifetime annotations
  elided; `map_or(true/false, ...)` replaced with `is_none_or()`/`is_some_and()`
- `stygian-mcp`: `main.rs` gains `#![allow(clippy::multiple_crate_versions)]` — binary
  targets are checked separately from the library and require their own allow attribute
- `stygian-proxy`: `client()` on `ProfiledRequester` is now `const fn`
- `stygian-browser`: `stygian-extract-derive` added as unconditional dev-dependency so
  trybuild UI tests compile regardless of features; trybuild does not forward feature flags
  to subprocess compilations, so UI test files now import directly from `stygian_extract_derive`

### Fixed

- `stygian-browser`: trybuild UI tests (`extract_enum`, `extract_missing_selector`) no
  longer fail with `unresolved import stygian_browser::extract` — the `extract` feature is
  not forwarded by trybuild to subprocess builds; tests now import the proc-macro directly
- `stygian-browser`: `[[test]] required-features = ["extract"]` removed — the UI test
  binary no longer needs to be feature-gated since the proc-macro is always available as a
  dev-dependency

### Docs

- `book/src/browser/extract.md`: dependency section corrected — users should enable
  `features = ["extract"]` on `stygian-browser`, not add `stygian-extract-derive` directly;
  import paths updated from `stygian_extract_derive::Extract` to `stygian_browser::extract::Extract`
- `crates/stygian-browser/README.md`: structured extraction added to the feature table with
  opt-in note (`features = ["extract"]`)

## [0.8.5] - 2026-04-08

### Fixed

- `stygian-mcp`: `initialize` now negotiates MCP `protocolVersion` correctly, accepting
  `2025-11-25`, `2025-06-18`, and `2024-11-05`, and returning `-32602` for unsupported
  versions instead of silently replying with a mismatched protocol line
- `stygian-mcp`: JSON-RPC notification handling now suppresses responses only for valid
  JSON-RPC 2.0 notifications; malformed notification-like requests still return `-32600`
  Invalid Request with `id: null`
- `stygian-graph`, `stygian-proxy`, and `stygian-browser`: stdio MCP servers no longer
  emit JSON-RPC responses for notifications, while still returning parse/invalid-request
  errors as required by JSON-RPC 2.0
- `stygian-graph`, `stygian-proxy`, and `stygian-browser`: reported MCP `protocolVersion`
  updated to `2025-11-25` for consistency with the current spec line

### Added

- `book`: MCP overview documentation now describes negotiated protocol-version support and
  explicitly documents that notifications (requests without `id`) do not produce responses

## [0.8.4] - 2026-04-08

### Added

- `stygian-proxy`: `FreeListFetcher::fetch()` now uses true concurrent fetching via
  `futures::future::join_all()` instead of sequential iteration; previously concurrent
  sources were fetched sequentially despite comments claiming concurrency
- `stygian-browser`: `RequestPacer::with_timing()` now normalizes inverted bounds
  (`min_ms > max_ms`) by swapping them, `with_rate()` docs now explicitly describe the
  minimum `0.01 rps` clamp, and async throttling behavior is covered by unit tests

### Fixed

- `stygian-proxy`: `ProxyType::Socks4` and `ProxyType::Socks5` enum variants now properly
  feature-gated behind `#[cfg(feature = "socks")]` in all match expressions; previously they
  were guarded in the enum definition but unconditionally matched in `url()` and `proxy_type()`
  methods, causing compile failures when the feature was disabled
- `stygian-proxy`: `ProxyType::Https` now correctly maps to `"https"` scheme (was `"http"`)
- `stygian-proxy`: `FreeListFetcher::new()` now logs warnings on TLS client build failures
  instead of silently falling back with `unwrap_or_default()`
- `stygian-browser`: `do_keyactivity()` now logs CDP evaluation failures with context instead
  of silently discarding them via `.ok()`
- `stygian-browser`: `fingerprint::font_measurement_intercept()` docstring corrected to match
  implementation (checks visibility/position only, not font-family); `getBoundingClientRect`
  now returns `new DOMRect(...)` for proper prototype chain instead of plain object literal
- `stygian-proxy`: `FreeListFetcher` parsing is now bracket-aware for IPv6 (`[addr]:port`),
  and invalid entries are rejected earlier (empty host, `[]`, and port `0`)
- `stygian-proxy`: `FreeListFetcher::fetch()` now returns a clear `ConfigError` when no
  sources are configured instead of a non-diagnostic fetch failure with empty origin text
- `stygian-proxy`: `ProxyError` is now marked `#[non_exhaustive]` to support adding variants
  more safely in future releases
- `stygian-browser`: `navigator.storage.estimate()` spoof now returns a merged object
  (`Object.assign`) instead of mutating the original result in-place for better compatibility

### Breaking changes

- `stygian-proxy`: `ProxyError` is now marked `#[non_exhaustive]`. Downstream crates that
  exhaustively match on this enum must add a wildcard arm to remain source-compatible.

## [0.8.3] - 2026-04-03

### Added

- `stygian-browser`: Stealth regression canary — daily GitHub Actions workflow
  (`stealth-canary`) runs `verify_stealth()` against `about:blank` to detect injection
  script regressions. On failure, calls GitHub Models (gpt-4o) with the `DiagnosticReport`
  JSON and the full `stealth.rs` source to generate a targeted fix, validates it compiles,
  and opens a PR automatically; falls back to opening an issue with Copilot instructions if
  `cargo check` fails
- `stygian-browser`: New `stealth_probe` example binary — run `verify_stealth()` against
  any number of URLs from the CLI with a configurable pass threshold; exits non-zero on
  regression

### Fixed

- `stygian-browser`: `browser_eval`, `browser_screenshot`, and `browser_content` MCP tool
  handlers now release the sessions `MutexGuard` before performing any browser I/O,
  eliminating unnecessary lock contention when multiple sessions make concurrent tool calls

## [0.8.2] - 2026-04-03

### Breaking changes

- `stygian-browser`: `navigator.userAgent` now returns `Chrome/131.0.0.0` instead of
  `Chrome/120.0.0.0` for all built-in `NavigatorProfile` variants (`windows_chrome`,
  `mac_chrome`, `linux_chrome`). Code that asserts on the exact UA string or parses the
  Chrome major version from `navigator.userAgent` or `navigator.userAgentData` will need
  to be updated. This was a correctness fix — the old version mismatched the default
  `chrome131` TLS profile and was a primary Cloudflare Turnstile detection signal.
- `stygian-browser`: `window.chrome.runtime`, `window.chrome.csi`, and
  `window.chrome.loadTimes` are now present in every `Basic`/`Advanced` stealth session.
  Page scripts that tested for the *absence* of these properties to detect headless mode
  will now see them populated with stub objects/functions.
- `stygian-browser`: `navigator.userAgentData.brands` is now spoofed to match the
  `Chrome/131` UA. Any code that reads `navigator.userAgentData` directly will see
  different brand/version values than in v0.8.1.

### Fixed

- `stygian-browser`: `NavigatorProfile` UA strings updated from Chrome 120 → Chrome 131 to
  match the default `chrome131` TLS profile; mismatched version between JA3/JA4 fingerprint
  and `navigator.userAgent` was a primary Cloudflare Turnstile detection signal
- `stygian-browser`: `Navigator.prototype.webdriver` prototype patch now uses `enumerable: false`
  (was `true`); `enumerable: true` is detectable via `Object.getOwnPropertyDescriptor` and is
  a Turnstile bot signal — real Chrome exposes this property as non-enumerable
- `stygian-browser`: Added `chrome_object_script()` injection — stubs `window.chrome.runtime`,
  `window.chrome.csi`, and `window.chrome.loadTimes` which are present in every real Chrome
  session but absent in headless; absence is a Turnstile detection signal
- `stygian-browser`: Added `user_agent_data_script()` injection — spoof `navigator.userAgentData`
  brands and version to match the `navigator.userAgent` Chrome version; Cloudflare
  cross-references both and a version mismatch (e.g. `userAgent=Chrome/120` vs
  `userAgentData.brands=[Chromium/139]`) reliably triggers the bot challenge

## [0.8.1] - 2026-04-03

### Breaking changes

- `stygian-browser`: `browser_content`, `browser_eval`, and `browser_screenshot` MCP tools
  now return the content of the *current page* rather than always returning an empty
  `about:blank` response. This was a bug fix, but callers that relied on the (incorrect)
  empty response will observe different output.

### Fixed

- `stygian-browser`: `browser_content`, `browser_eval`, and `browser_screenshot` MCP tools
  navigated to `about:blank` instead of the URL from the preceding `browser_navigate` call,
  returning empty or near-empty HTML for every page. Fixed by persisting the last navigated
  URL in the session and resolving it at call time; all three tools now also respect
  `timeout_secs` (previously hardcoded to 5 s)

## [0.8.0] - 2026-04-03

### Added

- `stygian-browser`: DOM traversal on `NodeHandle` — `parent()`, `next_sibling()`, and
  `previous_sibling()` methods for relative DOM navigation without additional CDP round-trips
- `stygian-browser`: `#[derive(Extract)]` macro via new `stygian-extract-derive` proc-macro
  crate — annotate struct fields with `#[selector("css")]`, `#[selector("css", attr = "name")]`,
  or `#[selector("css", nested)]` to generate a typed `Extractable` implementation;
  `PageHandle::extract_all::<T>(root_selector)` collects all matching root nodes into
  `Vec<T>` in a single traversal; `ExtractionError` covers missing required fields,
  CDP failures, and nested extraction failures
- `stygian-browser`: Adaptive element similarity via `find_similar()` — `ElementFingerprint`
  captures tag, sorted class list, attribute names, and depth-from-body; `jaccard_weighted()`
  computes a weighted Jaccard score (tag 0.4 / classes 0.35 / attrs 0.15 / depth 0.1);
  `PageHandle::find_similar(reference, config)` scans the live DOM and returns
  `Vec<SimilarMatch>` ordered by score; feature-gated behind `similarity`
- `stygian-browser`: Three new MCP browser tools — `browser_query` (CSS selector +
  optional per-field attribute map → text or structured results), `browser_extract`
  (schema-driven structured extraction; runtime equivalent of `#[derive(Extract)]`),
  and `browser_find_similar` (adaptive element search returning scored candidates;
  requires `similarity` feature)
- `stygian-extract-derive`: new `stygian-extract-derive` crate — proc-macro crate
  implementing `#[derive(Extract)]` for `stygian-browser`'s `Extractable` trait

## [0.7.0] - 2026-04-02

### Added

- `stygian-graph`: Graph introspection API — new `domain::introspection` module with types
  for runtime graph inspection: `NodeInfo`, `EdgeInfo`, `ExecutionWave`, `CriticalPath`,
  `ConnectivityMetrics`, `GraphSnapshot`, `NodeQuery`, `DependencyChain`, and `ImpactAnalysis`
- `stygian-graph`: `DagExecutor` introspection methods — `node_count()`, `edge_count()`,
  `node_ids()`, `get_node()`, `predecessors()`, `successors()`, `topological_order()`,
  `execution_waves()`, `node_info()`, `connectivity()`, `critical_path()`, `impact_analysis()`,
  `query_nodes()`, and `snapshot()` for comprehensive graph inspection at runtime
- `stygian-graph`: MCP introspection tools — four new tools for graph analysis via MCP:
  `inspect` (complete graph snapshot), `node_info` (single node details), `impact` (change
  impact analysis), and `query_nodes` (filtered node search by service, depth, root/leaf status)
- `stygian-mcp`: Graph introspection tools pass through aggregator as `graph_inspect`,
  `graph_node_info`, `graph_impact`, and `graph_query_nodes`

## [0.6.0] - 2026-03-31

### Added

- `stygian-browser`: DOM query API on `PageHandle` — new `NodeHandle` struct backed by
  V8 `RemoteObjectId`s for querying the live DOM without HTML serialization. Eliminates
  the `page.content()` + `scraper` round-trip, improving performance on large documents.
  Methods include `attr()` (single attribute), `attr_map()` (all attributes in one CDP
  round-trip), `text_content()`, `inner_html()`, `outer_html()`, `ancestors()` (parent
  chain via single JS eval), and `children_matching(selector)` for scoped node queries.
  (closes [#21](https://github.com/greysquirr3l/stygian/issues/21))
- `stygian-browser`: `PageHandle::query_selector_all(selector)` — queries the live DOM
  and returns `Vec<NodeHandle>` backed by stable CDP references; eliminates the need
  for DOM re-parsing in userland
- `stygian-browser`: `BrowserError::StaleNode` — new error variant for when a `NodeHandle`
  reference has been invalidated (e.g., after page navigation or DOM node removal);
  allows callers to distinguish stale-reference errors from other CDP failures
- `stygian-mcp`: new crate — unified MCP (Model Context Protocol) aggregator binary that
  merges `stygian-graph`, `stygian-browser`, and `stygian-proxy` capabilities into a single
  JSON-RPC 2.0 stdin/stdout server; `McpAggregator` dispatches namespaced tool calls
  (`graph_*` → graph, `browser_*` → browser, `proxy_*` → proxy) to the appropriate
  sub-server and provides two cross-crate tools: `scrape_proxied` (HTTP scrape routed
  through the proxy pool) and `browser_proxied` (browser session with proxy from the pool)
- `stygian-graph`: MCP server — `McpGraphServer` exposes seven tools for HTTP scraping,
  API querying, and pipeline execution over JSON-RPC 2.0; feature-gated behind `mcp`
- `stygian-proxy`: MCP server — `McpProxyServer` exposes six proxy-pool tools plus a
  `proxy://pool/stats` resource over JSON-RPC 2.0; adds `start_background()` so
  health-checking and sticky-session purging run correctly in aggregated (non-`run()`) mode;
  feature-gated behind `mcp`
- `book`: MCP documentation section — five new pages covering the aggregator architecture
  and tool namespace conventions, graph tools reference, browser tools reference, proxy
  tools reference, and integration guides for VS Code and other MCP clients

## [0.5.0] - 2026-03-24

### Added

- `stygian-browser`: `McpBrowserServer` — MCP server exposing eight browser automation
  tools over JSON-RPC 2.0; feature-gated behind `mcp`

### Changed

- `stygian-browser`: `McpBrowserServer` internal refactor — tool schema data extracted
  into a module-level `static TOOL_DEFINITIONS: LazyLock<Vec<Value>>`; repeated seven-line
  session-lookup blocks consolidated into `session_handle()` and
  `session_handle_and_stealth()` private async helpers; no public API changes

## [0.4.0] - 2026-03-23

### Added

- `stygian-browser`: TLS fingerprint profiles — `TlsProfile` domain type with JA3/JA4
  representation; built-in profiles for Chrome 131, Firefox 133, Safari 18, and Edge 131;
  `random_weighted()` for session-varied selection
- `stygian-browser`: rustls `ClientConfig` builder — `TlsProfile::to_rustls_config()`
  maps profile cipher suites, ALPN, and TLS version ordering into a rustls config;
  feature-gated behind `tls-config`
- `stygian-browser`: `build_profiled_client()` — constructs a `reqwest::Client` with a
  fully profiled TLS fingerprint and optional proxy URL; `default_user_agent()` returns
  the UA string matching the chosen profile
- `stygian-browser`: Chrome launch flags for TLS consistency — `chrome_tls_args()` emits
  `--cipher-suite-blacklist` and related flags so the browser's TLS matches the profile
- `stygian-browser`: Stealth diagnostic module — 10 JavaScript detection checks
  (`WebDriverFlag`, `ChromeObject`, `PluginCount`, `LanguagesPresent`, `CanvasConsistency`,
  `WebGlVendor`, `AutomationGlobals`, `OuterWindowSize`, `HeadlessUserAgent`,
  `NotificationPermission`) with JSON output, `DetectionCheck::parse_output()`, and
  `DiagnosticReport` aggregation (`is_clean()`, `coverage_pct()`, `failures()`);
  feature-gated behind `stealth`
- `stygian-browser`: `PageHandle::verify_stealth()` — runs all diagnostic checks via CDP
  `Runtime.evaluate`, returns a `DiagnosticReport`; individual script errors are captured
  as failed checks (non-fatal) so the full report is always returned
- `stygian-proxy`: sticky session support — `StickyPolicy` enum (`Disabled` / `Domain {
  ttl }`), `SessionMap` with `bind()` / `lookup()` / `unbind()` / `purge_expired()`,
  and `acquire_for_domain()` on `ProxyManager` to pin a domain to a proxy for the session TTL
- `stygian-graph`: `EscalationPolicy` port trait — `EscalationTier` enum (HttpPlain →
  HttpTlsProfiled → BrowserBasic → BrowserAdvanced), `ResponseContext`, and async
  `select_tier()` / `record_tier_success()` / `record_failure()` methods
- `stygian-graph`: `DefaultEscalationPolicy` — per-domain tier learning cache with
  challenge/CAPTCHA detection (Cloudflare, DataDome, PerimeterX); configurable
  `max_tier`, `base_tier`, and `cache_ttl`
- `stygian-graph`: `EscalatingScrapingService` — graph service adapter registered as
  `"http_escalating"` that drives tier promotion automatically and annotates node
  metadata with `escalation_tier` and `escalation_path`
- `book`: three new mdBook chapters — `browser/stealth-v2.md` (TLS profiles, JA3/JA4,
  `verify_stealth()` API, 10-check detection landscape), `proxy/sticky-sessions.md`
  (session lifecycle, `acquire_for_domain()`, failure handling), `graph/escalation.md`
  (tier comparison, `EscalationPolicy` trait, `DefaultEscalationPolicy` challenge detection)
- `examples`: four stealth v2 pipeline configs — `tls-profiled-scrape.toml`,
  `escalation-pipeline.toml`, `sticky-proxy-session.toml`, `stealth-audit.toml`

### Fixed

- `stygian-browser`: clippy warnings in `diagnostic.rs` — `is_clean()` promoted to
  `const fn`, `cast_precision_loss` suppressed with allow annotation, `indexing_slicing`
  suppressed in test module, `needless_collect` eliminated in test assertion
- `stygian-browser`: clippy warnings in `tls.rs` — `wildcard_imports` suppressed in
  `mod rustls_config` and `mod reqwest_client` inner modules, `indexing_slicing`
  suppressed in ALPN order test

## [0.3.0] - 2026-03-22

### Added

- `stygian-graph`: `BudgetGuard` RAII wrapper for GraphQL query cost tracking — acquires
  budget on creation and guarantees release via `Drop`, eliminating leaked budget on
  early returns or panics; `GraphqlAdapter` refactored to use guard at both paginated
  and single-request paths
- `stygian-graph`: API discovery types — `JsonType`, `PaginationStyle`, `ResponseShape`,
  and `DiscoveryReport` in `domain::discovery` for representing discovered API structure
  and capabilities
- `stygian-graph`: `OpenApiGenerator` — generates OpenAPI 3.0 specs from `DiscoveryReport`
  with configurable `SpecConfig` (title, version, server URL); outputs `openapiv3::OpenAPI`
- `stygian-graph`: `DataSourcePort` trait — database-backed data source abstraction with
  `QueryParams`, `query()`, and `healthcheck()` methods
- `stygian-graph`: `DatabaseSource` adapter — PostgreSQL implementation of `DataSourcePort`
  via `sqlx::PgPool`; feature-gated behind `postgres`
- `stygian-graph`: `DocumentSourcePort` trait — file system document source abstraction with
  `DocumentQuery`, `Document`, glob pattern matching, and MIME type filtering
- `stygian-graph`: `DocumentSource` adapter — local filesystem implementation with
  case-insensitive glob/MIME matching and symlink following for cross-platform compatibility
- `stygian-graph`: `StreamSourcePort` trait — streaming data source abstraction with
  `StreamEvent` and `subscribe()` returning a pinned async stream
- `stygian-graph`: `SseSource` adapter — Server-Sent Events implementation of
  `StreamSourcePort` via `reqwest`
- `stygian-graph`: `AgentSourcePort` trait — AI agent interaction abstraction with
  `AgentRequest`/`AgentResponse` for wrapping AI provider calls
- `stygian-graph`: `AgentSource` adapter — delegates to any `Arc<dyn AIProvider>` for
  LLM-backed data extraction and transformation
- `stygian-graph`: Redis/Valkey `CachePort` adapter — connection-pooled cache backend with
  key namespacing via configurable prefix and per-entry TTL; feature-gated behind `redis`
- `stygian-graph`: Sitemap/Sitemap-index `ScrapingService` adapter — XML parsing with gzip
  support, priority/changefreq filtering, and recursive sitemap index traversal
- `stygian-graph`: RSS/Atom feed `ScrapingService` adapter — `feed-rs` parsing with item
  filtering by date/count, returning structured `ScrapingResult` entries
- `stygian-graph`: WebSocket `StreamSourcePort` adapter — reconnection with exponential
  backoff, auth headers, message filtering, and configurable ping/pong
- `stygian-graph`: CSV/TSV `DataSourcePort` adapter — configurable delimiters, header
  mapping, row skip/limit, and streaming reads via the `csv` crate
- `stygian-graph`: S3-compatible object storage `StoragePort` adapter — `rust-s3` backend
  with list/get/put/delete/exists operations; feature-gated behind `object-storage`
- `stygian-graph`: Redis Streams `WorkQueuePort` adapter — consumer groups, idle message
  claiming, and connection pooling; feature-gated behind `redis`
- `stygian-graph`: `WebhookTrigger` port trait — event-driven trigger abstraction with
  `start_listening`, `stop_listening`, and async event stream
- `stygian-graph`: Axum webhook trigger adapter — HTTP listener with HMAC-SHA256 signature
  verification, broadcast channels, health endpoint; feature-gated behind `api`
- `examples`: 6 pipeline configuration examples — `sitemap-crawl.toml`, `rss-monitor.toml`,
  `websocket-stream.toml`, `csv-transform.toml`, `webhook-triggered.toml`,
  `distributed-redis.toml`
- `book`: production adapters chapter with config references, feature flags table, Mermaid
  architecture diagrams, and "choosing the right backend" decision trees

### Fixed

- `book`: corrected documentation inaccuracies in production adapter config params to match
  actual implementation (Redis cache, Redis work queue, CSV, WebSocket, sitemap field names
  and types)
- `book`: fixed `distributed.md` usage example and feature flag references

### Security

- Updated `aws-lc-sys` 0.38 → 0.39 (RUSTSEC-2026-0044, RUSTSEC-2026-0048)
- Updated `rustls-webpki` 0.103.9 → 0.103.10 (RUSTSEC-2026-0049)

## [0.2.1] - 2026-03-17

### Added

- `stygian-browser`: `Navigator.prototype.webdriver` prototype-level patch — previously only the
  instance property was overridden; scanners such as pixelscan.net and Akamai probe
  `Object.getOwnPropertyDescriptor(Navigator.prototype, 'webdriver')` directly,
  so the prototype getter is now also patched on every new document context
- `stygian-browser`: Network Information API spoofing — `navigator.connection` (previously
  `null` in headless, an immediate detection signal) is replaced with a realistic
  `NetworkInformation`-like object (`effectiveType: "4g"`, `type: "wifi"`, seeded
  `downlink`/`rtt` values stable within a session)
- `stygian-browser`: Battery Status API spoofing — `navigator.getBattery()` (previously
  `null` in headless) now resolves with a plausible disconnected-battery state; `level`,
  `dischargingTime` are seeded from `performance.timeOrigin` to vary across sessions
- `stygian-browser`: `examples/scraper_cli.rs` — generic CLI scraper using `StealthLevel::Advanced`,
  `WaitUntil::NetworkIdle`; emits structured JSON (title, description, headings, links,
  text excerpt, timing); successfully scrapes Cloudflare-protected sites (CNN.com, etc.)
- `stygian-browser`: `examples/pixelscan_check.rs` — targeted pixelscan.net fingerprint scan
  example; polls until client-side result cards settle; extracts verdict, per-card pass/fail
  status, hardware/font/UA detail sections, and live `nav_signals` for stealth regression testing
- `stygian-graph`: `SigningPort` trait — request-signing abstraction for attaching HMAC tokens,
  AWS Signature V4, OAuth 1.0a, device attestation tokens, or any per-request auth material
  without coupling adapters to signing scheme
- `stygian-graph`: `NoopSigningAdapter` — passthrough signer for testing and optional-signer defaults
- `stygian-graph`: `HttpSigningAdapter` — delegates signing to any external sidecar over HTTP POST
  (e.g. a Frida RPC bridge exposing a `/sign` endpoint); configurable timeout and retries
- `book`: stealth guide updated — prototype-level webdriver patch, Network Information API
  spoofing, and Battery Status API spoofing sections added

### Fixed

- `stygian-browser`: `outerWidth`/`outerHeight` now set via `screen_script` injection to match
  the spoofed screen resolution (headless Chrome returns `0` without this)
- `stygian-browser`: `navigator.plugins` spoofed with a realistic 5-entry `PluginArray`
  (PDF Viewer entries + `navigator.mimeTypes` with 2 entries), eliminating the
  empty-plugins headless signal

## [0.2.0] - 2026-03-16

### Added

- `stygian-graph`: `OpenApiAdapter` — OpenAPI 3.x introspection adapter (`kind = "openapi"`)
  - Fetches and caches parsed OpenAPI specs (JSON or YAML) per spec URL for the lifetime of the adapter
  - Resolves operations by `operationId` (e.g. `"listPets"`) or `"METHOD /path"` syntax (e.g. `"GET /pet/findByStatus"`)
  - Binds `params.args` to path parameters (substituted into the URL template), query parameters, and request body
  - Delegates all HTTP calls to the inner `RestApiAdapter`, inheriting full auth support, retries, and 429 handling
  - Optional proactive rate limiting via `params.rate_limit` (sliding-window or token-bucket, same shape as GraphQL rate limiter)
  - `params.server.url` overrides `servers[0].url` from the spec at runtime
  - Two new workspace dependencies: `openapiv3 = "1"`, `serde_yaml = "0.9"` (always compiled; pure Rust)
  - `book`: OpenAPI adapter section added to [Built-in Adapters](./book/src/graph/adapters.md)

## [0.1.20] - 2026-03-16

### Added

- `stygian-proxy`: new crate — high-performance, resilient proxy pool with per-proxy circuit breakers, configurable rotation strategies (round-robin, random, weighted, failover), SOCKS4/5 support, health scoring, and in-memory storage; `socks` feature enables SOCKS proxy types via reqwest
- `stygian-proxy`: `CircuitBreaker` per-proxy state machine — open/half-open/closed transitions with configurable failure threshold and recovery window; wired into `ProxyManager::acquire_proxy` so unhealthy proxies are skipped automatically
- `stygian-proxy`: `ProxyManager` — unified pool orchestrator exposing `add_proxy`, `remove_proxy`, `acquire_proxy`, `release_proxy`, and `pool_stats`; every operation is traced and metered
- `stygian-proxy`: `ProxyBrowserPool` — `browser` feature flag wires the proxy pool into `stygian-browser`; browsers are launched with a live proxy from the pool and the proxy is re-evaluated on browser release
- `stygian-graph`: `RestApiAdapter` proxy integration via stygian-proxy (`graph` feature on stygian-proxy)
- `book`: `stygian-proxy` mdBook module added — architecture overview, circuit breaker semantics, rotation strategies, and browser/graph integration guides

### Changed

- Workspace MSRV updated to Rust 1.94.0 (aligned with stable feature usage: async closures, `LazyLock`, let chains)

### Fixed

- `stygian-proxy`: eliminated TOCTOU race in `add_proxy` — the `circuit_breakers` write lock is now held for the full duration of `storage.add()`, ensuring `acquire_proxy` can never observe a proxy record without a corresponding circuit breaker
- `stygian-browser`: CDP protection removes `cdc_*` and `domAutomation` automation artifact properties from `window` on every navigation, reducing fingerprint surface for Akamai/PerimeterX detectors

### Tests

- `stygian-graph`: 12 live integration tests against crawllab.dev (all `#[ignore]`) — status code classification (200/404/429/500), redirect following, redirect cycle detection, JSON/text/HTML content types, 204 No Content, and paginated forum endpoint
- `stygian-browser`: 4 live browser integration tests against crawllab.dev (all `#[ignore]`) — inline JS rendering, external script rendering, basic navigation, and JS eval with stealth verification

## [0.1.19] - 2026-03-15

### Changed

- `stygian-graph`: `CloudflareCrawlAdapter::new()` and `with_config()` now return `Result<Self>` instead of panicking on reqwest TLS init failure; `Default` impl removed (breaking change for direct construction)

### Fixed

- `stygian-graph`: `RestApiAdapter::parse_spec` body selection uses `match` instead of `if let/else` chain (lint)
- `stygian-browser`: `BrowserPool::acquire_inner` fast path drops mutex guard before spawning disposal tasks, reducing lock contention under unhealthy-pool conditions

## [0.1.18] - 2026-03-15

### Fixed

- `stygian-graph`: `RestApiAdapter` now checks `body_raw` before `body` when both are present, matching the documented precedence contract
- `stygian-graph`: `RestApiAdapter` 429 responses now return `ServiceError::RateLimited` with the parsed `Retry-After` value; `send_one` honours the server-specified delay instead of blind exponential backoff
- `stygian-graph`: token-bucket rate limiter guards against `max_requests = 0` or zero-duration window configs that previously caused a division-by-zero panic via `Duration::from_secs_f64(inf)`
- `stygian-graph`: `CloudflareCrawlAdapter::with_config` now panics with a clear message on TLS init failure instead of silently falling back to a misconfigured default `reqwest::Client`
- `book`: Cloudflare crawl adapter metadata example corrected to `job_id`, `pages_crawled`, `output_format` (was `pages`, `url_count`)
- `book`: `HeadlessMode::Legacy` docs across configuration and env-vars pages corrected to "classic `--headless` for Chromium < 112" (was incorrectly referencing `chrome-headless-shell` and Chrome 132 removal)

## [0.1.17] - 2026-03-14

### Added

- `stygian-browser`: context-scoped browser pool segregation — `BrowserPool::acquire_for(context_id)` returns browsers isolated by context (bot, tenant, etc.) so multiple consumers share one pool without cross-contamination; `release_context(id)` drains all idle browsers for a context; `context_ids()` lists active contexts; `BrowserHandle::context_id()` exposes the owning context
- `stygian-browser`: pool eviction now walks both shared and per-context queues, pruning empty context entries automatically

### Changed

- `stygian-browser`: `PoolInner` internal structure changed from single `VecDeque` to shared + `HashMap<String, VecDeque>` scoped queues (no public API break — `acquire()` remains fully backward-compatible)

## [0.1.16] - 2026-03-13

### Changed

- Workspace license updated to `AGPL-3.0-only OR LicenseRef-Commercial` dual licensing; added `LICENSE-COMMERCIAL.md`
- `thiserror` 1.0 → 2.0 (unifies with chromiumoxide transitive dep)
- `toml` pinned at 0.8 (unifies with figment; previous bump to 1.0 caused duplicate)
- `reqwest` 0.12 → 0.13 (feature `rustls-tls` renamed to `rustls`; added `query` feature)
- `toml` 0.8 → 1.0
- `scraper` 0.20 → 0.25
- `mockall` 0.13 → 0.14
- `criterion` 0.5 → 0.8
- `prometheus-client` 0.22 → 0.24
- `indicatif` 0.17 → 0.18

## [0.1.15] - 2026-03-13

### Added

- `stygian-graph`: `RestApiAdapter` — flexible REST JSON API adapter with 5 auth schemes (Bearer, Basic, API key header/query, none), 4 pagination strategies (none, offset, cursor, RFC 8288 Link header), dot-path JSON response extraction, configurable retries with exponential backoff, and 24 unit tests; registered as `"rest-api"`
- `stygian-graph`: `CloudflareCrawlAdapter` — delegates whole-site crawling to the Cloudflare Browser Rendering `/crawl` endpoint (open beta); polls until complete, aggregates page results, configurable poll interval and job timeout; gated behind `cloudflare-crawl` feature flag
- `examples/rest-api-scrape.toml` — example pipeline demonstrating unauthenticated GET, Bearer-auth + Link-header pagination, and API-key + cursor pagination patterns

### Fixed

- `stygian-graph`: resolved all `clippy -D warnings` lint failures in `rest_api.rs` and `cloudflare_crawl.rs` — `indexing_slicing`, `map_unwrap_or`, `manual_map`, `if_not_else`, `option_if_let_else`, `unnecessary_map_or`, `cast_possible_truncation`, `ignore_without_reason`, `panic` in tests

## [0.1.14] - 2026-03-04

### Fixed

- `stygian-graph`: corrected broken rustdoc intra-doc links in `graphql_throttle` module (`pre_flight_delay` → `pre_flight_reserve`, removed unresolvable link brackets from module-level prose)
- `stygian-graph`: fixed `cargo fmt` import ordering in `graphql.rs` (`graphql_rate_limit` before `graphql_throttle`)

## [0.1.13] - 2026-03-04

### Added

- `stygian-graph`: `RateLimitConfig` (port layer) + `RequestRateLimit` / `RequestWindow` sliding-window request-count rate limiter — complements the existing leaky-bucket cost throttle; tracks up to `max_requests` in any rolling `window` duration using a `VecDeque<Instant>`; `rate_limit_acquire` sleeps until a slot is free before each request; `rate_limit_retry_after` imposes a hard block to honour server-returned `Retry-After` headers (a shorter value can never shorten an existing block); `parse_retry_after` parses integer seconds from a header string
- `stygian-graph`: `GraphQlTargetPlugin::rate_limit_config()` default method — plugins opt in to per-plugin request-count limiting by returning a `RateLimitConfig`; operates in parallel with `cost_throttle_config()` and both can be active simultaneously

## [0.1.12] - 2026-03-04

### Added

- `stygian-graph`: `AuthPort` trait for runtime credential management — load, expiry-check, and refresh tokens without pre-loading static credentials; includes `ErasedAuthPort` object-safe wrapper (`Arc<dyn ErasedAuthPort>`) with a blanket impl, `EnvAuthPort` convenience implementation (reads from an env var), and `resolve_token` helper
- `stygian-graph`: `CostThrottleConfig` (port layer) + `LiveBudget` / `PluginBudget` proactive throttle system — tracks the rolling point budget from Shopify/Jobber-style `extensions.cost.throttleStatus` response envelopes; `pre_flight_delay` sleeps before each request if the projected budget is too low, eliminating wasted throttled requests; `reactive_backoff_ms` computes exponential back-off from a throttle response
- `stygian-graph`: `GenericGraphQlPlugin` builder API — construct a fully configured `GraphQlTargetPlugin` without writing a dedicated struct; fluent builder with `.name()`, `.endpoint()`, `.bearer_auth()`, `.auth()`, `.header()`, `.headers()`, `.cost_throttle()`, `.page_size()`, `.description()`, `.build()`
- `stygian-graph`: `GraphQlService::with_auth_port()` — attach a runtime `ErasedAuthPort` to the service; token is resolved lazily per-request and overridden by any static per-plugin auth
- `stygian-graph`: `GraphQlTargetPlugin::cost_throttle_config()` default method — plugins opt in to proactive throttling by returning a `CostThrottleConfig`

### Removed

- `stygian-graph`: `JobberPlugin` and `jobber_integration` tests removed from the library — consumer-specific plugins belong in the consuming application; use `GenericGraphQlPlugin` or implement `GraphQlTargetPlugin` directly

## [0.1.11] - 2026-03-02

### Added

- `stygian-browser`: `HeadlessMode` enum with `New` (default, `--headless=new`) and `Legacy` (old `--headless`) variants — exposes Chrome's new headless rendering pipeline which shares the same code path as headed Chrome and is significantly harder to fingerprint-detect; fixes bot detection on sites like X/Twitter that reject the old headless mode before any session state is checked (closes [#13](https://github.com/greysquirr3l/stygian/issues/13))
- `stygian-browser`: `BrowserConfig::headless_mode` field and `BrowserConfigBuilder::headless_mode(HeadlessMode)` setter — opt back to `HeadlessMode::Legacy` if targeting Chromium < 112; configurable via `STYGIAN_HEADLESS_MODE` env var (`new`/`legacy`)

## [0.1.10] - 2026-03-03

### Added

- `stygian-browser`: `PageHandle::inject_cookies()` — seed session cookies on a page without a full `SessionSnapshot` round-trip and without a direct `chromiumoxide` dependency in calling code (closes [#11](https://github.com/greysquirr3l/stygian/issues/11))
- `stygian-browser`: `BrowserConfig::builder().user_data_dir(path)` builder method — set a custom Chrome profile directory; when omitted each browser instance now auto-generates a unique temporary directory (`$TMPDIR/stygian-<id>`) so concurrent pools no longer race on `SingletonLock` (closes [#12](https://github.com/greysquirr3l/stygian/issues/12))

## [0.1.9] - 2026-03-02

### Fixed

- `stygian-browser`: `set_resource_filter` previously called `Fetch.enable` but never subscribed to `Fetch.requestPaused` events, causing Chrome to intercept and hang every network request indefinitely — navigation always timed out when a resource filter was active. A background task now processes each paused event: blocked resource types receive `Fetch.failRequest(BlockedByClient)`, all others receive `Fetch.continueRequest` (closes [#9](https://github.com/greysquirr3l/stygian/issues/9))
- `stygian-browser`: Chrome launch arguments including `--disable-blink-features=AutomationControlled` were being passed with a double-dash prefix (`----arg`) because chromiumoxide's `ArgsBuilder` prepends `--` to every key and stygian was also including the `--` in the string. The arg builder in `browser.rs` now strips the `--` prefix before handing each argument to chromiumoxide, so the stealth flag reaches Chrome correctly (closes [#10](https://github.com/greysquirr3l/stygian/issues/10))

## [0.1.8] - 2026-03-02

### Changed

- Version bump to verify end-to-end release pipeline (Cargo.toml → crates.io)

## [0.1.7] - 2026-03-02

### Fixed

- `stygian-browser`: `navigate()` race condition — `EventLoadEventFired` subscription now registered **before** `goto()` so Chrome cannot fire the event before the listener is in place; previously this caused 100% timeouts with any `DomContentLoaded` or `NetworkIdle` wait strategy ([#7](https://github.com/greysquirr3l/stygian/issues/7))
- `stygian-browser`: `WaitUntil::DomContentLoaded` now subscribes to `Page.domContentEventFired` (fires when the DOM is ready, before subresources load) instead of `Page.loadEventFired`, matching its documented semantics
- `stygian-browser`: `WaitUntil::NetworkIdle` now implements genuine network-idle detection — in-flight request count is tracked via `Network.requestWillBeSent` / `Network.loadingFinished` / `Network.loadingFailed`; navigation resolves when ≤ 2 requests remain in-flight for at least 500 ms after the load event (equivalent to Playwright's `networkidle2`)

### Changed

- `stygian-browser`: `chromiumoxide` upgraded from `0.7` → `=0.9.1` (pinned exact version)
- `stygian-graph`: `JobberPlugin` promoted from unit struct to a struct with an injectable `token` field; `default_auth` now reads from `self.token` set at construction time instead of accessing the environment on every call; new `with_token(impl Into<String>)` constructor bypasses env lookup entirely — eliminates all unsafe env mutations from the test suite (closes [#5](https://github.com/greysquirr3l/stygian/pull/5), [#6](https://github.com/greysquirr3l/stygian/pull/6))

## [0.1.6] - 2026-03-02

### Security

- `stygian-browser`: added `#![deny(unsafe_code)]` — unsafe code is now a compile error in library code; test helpers that require it carry an explicit `#[allow(unsafe_code)]`
- `stygian-graph`: env-var mutations in `jobber` plugin tests are now serialised with `ENV_LOCK` (eliminates a potential race when tests run in parallel)

### Documentation

- `page-operations.md`: added `status_code()` to the "Reading page content" snippet and the complete usage example

## [0.1.5] - 2026-03-02

### Added

- `stygian-browser`: `PageHandle::url()` — returns the post-navigation, post-redirect page URL via CDP `Target.getTargetInfo` ([#1](https://github.com/greysquirr3l/stygian/issues/1))
- `stygian-browser`: `PageHandle::status_code()` — returns the HTTP status code of the most recent main-frame navigation, captured atomically from `Network.responseReceived` before `goto()` is called; returns `None` for `file://` navigations or before `navigate()` is called ([#1](https://github.com/greysquirr3l/stygian/issues/1))

## [0.1.4] - 2026-03-02

### Fixed

- **Security**: `--no-sandbox` was passed unconditionally to Chrome, disabling Chromium's built-in renderer sandbox on bare-metal hosts. It is now only passed when running inside a container (auto-detected via `/.dockerenv` / `/proc/1/cgroup` on Linux) or when explicitly set via `STYGIAN_DISABLE_SANDBOX=true`. macOS and Windows are never affected (their native sandbox mechanisms differ). (`stygian-browser`)
- Fixed fmt CI failures: import ordering across 23 files in `stygian-graph`

### Changed

- `BrowserConfig` gains a `disable_sandbox: bool` field and `.disable_sandbox()` builder method
- `STYGIAN_DISABLE_SANDBOX` env var overrides auto-detection
- Updated `stygian-browser` README: `STYGIAN_DISABLE_SANDBOX` added to env-var table; FAQ expanded with sandbox guidance
- Repository homepage updated from `greysquirr3l.github.io/mycelium` to `greysquirr3l.github.io/stygian`
- Documentation links point to GitHub Pages instead of docs.rs (crates not yet on crates.io)

## [0.1.3] - 2026-03-02

### Changed

- Relicensed from `MIT OR Apache-2.0` to `AGPL-3.0-only`
- Replaced `LICENSE-MIT` and `LICENSE-APACHE` with a single `LICENSE` (AGPL-3.0)
- Updated license badges and license sections in all README files
- Updated `deny.toml` to reflect AGPL-3.0-only as the project license

## [0.1.2] - 2026-03-01

### Changed

- Renamed all `MYCELIUM_*` environment variables to `STYGIAN_*` across both crates, docs, examples, and plan files
- Updated JSON schema `$id` URIs from `https://mycelium/schemas/...` to `https://stygian/schemas/...`
- Updated `.gitattributes` workspace comment to reference stygian

### Fixed

- Clippy `redundant_closure_for_method_calls`: replaced `|e| e.into_inner()` with `PoisonError::into_inner` in `config.rs`
- Clippy `default_constructed_unit_structs`: replaced `PipelineExecutor::default()` with direct struct literal in test
- Clippy `useless_format`: replaced `format!(r#"..."#)` with `.to_string()` in `pipeline_parser.rs` test
- Added `#[allow(clippy::missing_const_for_fn)]` with explanation to `rss_bytes()` — non-Linux targets see only `{ 0 }` but the Linux branch uses file I/O, making `const fn` impossible cross-platform

## [0.1.1] - 2026-03-01

### Added

- GitHub Actions CI/CD workflows: `ci.yml`, `docs.yml`, `release.yml`, `scorecard.yml`, `security.yml`
- `deny.toml` cargo-deny configuration for supply-chain security (advisories, licenses, bans)
- mdBook documentation site (`book/`) with 17 chapters across three parts: Graph Engine, Browser Automation, and Reference
- GitHub Pages deployment via `docs.yml` — mdBook at `/` with rustdoc API reference merged at `/api/`
- Additional unit tests for `FileStorage`, `Config`, `Executor`, and `Cli` modules (storage cross-pipeline retrieve, default config assertions, executor concurrency, CLI parse coverage)
- OpenAI, Gemini, and GitHub Copilot AI adapter test coverage improvements

### Changed

- README files updated with CI status badges and coverage reporting
- `.gitignore` extended to exclude `book/book/` build output and Coraline artifacts

### Added

#### stygian-graph

- Initial release of graph-based scraping engine
- Hexagonal architecture with ports and adapters pattern
- HTTP adapter with configurable timeouts and retries
- Browser adapter with JavaScript rendering support (via stygian-browser)
- AI extraction adapters: Claude, OpenAI, Gemini, GitHub Copilot, Ollama
- Multi-modal support for images, PDFs, videos
- Distributed execution via work queue abstraction
- Local work queue implementation for single-node deployments
- Pipeline validation: cycle detection, reachability checks, node validation
- Idempotency system for safe retries
- Circuit breaker pattern for graceful degradation
- Prometheus metrics collection
- Comprehensive test suite (280+ tests)

#### stygian-browser

- Initial release of anti-detection browser automation library
- Browser pool with warm instance reuse (<100ms acquisition)
- CDP-based automation via chromiumoxide
- Stealth features: navigator spoofing, canvas noise, WebGL randomization
- Human behavior simulation: Bézier mouse paths, realistic typing
- Configurable wait strategies: DOM loaded, network idle, selector waits
- Page lifecycle management with automatic cleanup
- Resource management and memory monitoring
- Comprehensive test suite (80+ tests)

### Changed

- N/A (initial release)

### Deprecated

- N/A (initial release)

### Removed

- N/A (initial release)

### Fixed

- N/A (initial release)

### Security

- N/A (initial release)

---

## [0.1.0] - 2026-02-28

Initial pre-release for testing and validation.

### Notes

This is a pre-1.0 release. Breaking changes may occur between minor versions until 1.0.0 is reached.

Both crates are functional and well-tested, but APIs may evolve based on community feedback.

---

[Unreleased]: https://github.com/greysquirr3l/stygian/compare/v0.9.4...HEAD
[0.9.4]: https://github.com/greysquirr3l/stygian/compare/v0.9.3...v0.9.4
[0.9.3]: https://github.com/greysquirr3l/stygian/compare/v0.9.2...v0.9.3
[0.9.2]: https://github.com/greysquirr3l/stygian/compare/v0.9.1...v0.9.2
[0.9.1]: https://github.com/greysquirr3l/stygian/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/greysquirr3l/stygian/compare/v0.1.9...v0.9.0
[0.1.9]: https://github.com/greysquirr3l/stygian/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/greysquirr3l/stygian/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/greysquirr3l/stygian/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/greysquirr3l/stygian/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/greysquirr3l/stygian/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/greysquirr3l/stygian/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/greysquirr3l/stygian/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/greysquirr3l/stygian/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/greysquirr3l/stygian/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/greysquirr3l/stygian/releases/tag/v0.1.0
