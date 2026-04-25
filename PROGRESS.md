# stygian — Implementation Progress

> Orchestrator reads this file at the start of each loop iteration.
> Subagents update this file after completing a task.

## Status Legend

- `[ ]` — Not started
- `[~]` — In progress (claimed by a subagent)
- `[x]` — Completed
- `[!]` — Blocked / needs human input

---

## Phase 1 — Production Cache Backend

| Task | Status | Notes |
|---|---|---|
| T01 — Redis/Valkey CachePort Adapter | `[x]` | |

---

## Phase 2 — Discovery Adapters

> Depends on: Phase 1 all complete

| Task | Status | Notes |
|---|---|---|
| T02 — Sitemap/Sitemap Index Source Adapter | `[x]` | |
| T03 — RSS/Atom Feed Source Adapter | `[x]` | |

---

## Phase 3 — Streaming & Input Adapters

> Depends on: Phase 2 all complete

| Task | Status | Notes |
|---|---|---|
| T04 — WebSocket StreamSourcePort Adapter | `[x]` | |
| T05 — CSV/TSV DataSourcePort Adapter | `[x]` | |

---

## Phase 4 — Cloud Storage

> Depends on: Phase 3 all complete

| Task | Status | Notes |
|---|---|---|
| T06 — S3-Compatible Object Storage Adapter | `[x]` | |

---

## Phase 5 — Distributed Work Queue

> Depends on: Phase 4 all complete

| Task | Status | Notes |
|---|---|---|
| T07 — Redis Streams WorkQueuePort Adapter | `[x]` | |

---

## Phase 6 — Event-Driven Trigger

> Depends on: Phase 5 all complete

| Task | Status | Notes |
|---|---|---|
| T08 — WebhookTrigger Port Trait | `[x]` | |
| T09 — Webhook Trigger Adapter (axum-based HTTP Listener) | `[x]` | |

---

## Phase 7 — Example Pipelines & Documentation

> Depends on: Phase 6 all complete

| Task | Status | Notes |
|---|---|---|
| T10 — Example Pipeline Configurations for New Adapters | `[x]` | |
| T11 — Adapter Documentation & Integration Guide | `[x]` | |

---

---

## Phase 8 — Browser DOM Query & Typed Extraction

> Depends on: none (self-contained stygian-browser changes)

| Task | Status | Notes |
|---|---|---|
| T30 — NodeHandle DOM Query API on PageHandle | `[x]` | Complete |
| T31 — `#[derive(Extract)]` typed extraction macro | `[x]` | Complete |
| T32 — NodeHandle DOM traversal (parent/sibling) | `[x]` | |
| T33 — `find_similar()` adaptive element matching | `[x]` | Complete |
| T34 — MCP browser DOM tools: query, find_similar, extract | `[x]` | `browser_query`, `browser_extract`, `browser_find_similar` (similarity-gated) in `McpBrowserServer`; 4 unit tests |

---

## Phase 9 — Stealth and Proxy Next Wave

> Depends on: Phase 8 completion (for browser extraction/similarity foundations)

| Task | Status | Notes |
|---|---|---|
| T49 — Stealth Benchmark Harness | `[x]` | benchmark harness + `stealth_benchmark` example + deterministic JSON/Markdown reporting |
| T50 — Transport Profile Packs and Cadence | `[x]` | |
| T51 — Session Warmup and Refresh Primitives | `[x]` | `warmup()`/`refresh()` on PageHandle; `browser_warmup`/`browser_refresh` MCP tools; 12 unit tests |
| T52 — Proxy Capability Model and Protocol-Aware Routing | `[x]` | `ProxyCapabilities` + `CapabilityRequirement`; protocol routing resolver; capability-aware manager acquisition path; tests/clippy clean |
| T53 — FreeAPIProxies Source Adapter (Optional) | `[x]` | `FreeApiProxiesFetcher` with `limit`/`protocol`/`country` query-param builder; tolerant 3-envelope JSON parser; `#[ignore]` integration test |
| T54 — Adaptive Selector Recovery for Extraction | `[x]` | `extract_all_with_fallback` (ordered selector cascade) + `extract_resilient` (skip `Missing` nodes) on `PageHandle`; 1 unit test |

---

## Phase 10 — Opinionated Acquisition Runner Refactor

> Depends on: branch scaffolding complete (`feat/acquisition-runner-refactor` + phase branches)

| Task | Status | Notes |
|---|---|---|
| T59 — Acquisition runner core in stygian-browser | `[x]` | Added `acquisition` runner facade, deterministic ladders, and stage failure bundles |
| T60 — Map charon runtime policy to acquisition strategy | `[x]` | Added deterministic policy-to-acquisition mapping layer in stygian-charon |
| T61 — Add browser_acquire_and_extract MCP tool | `[x]` | Added `browser_acquire_and_extract` MCP tool wired to `AcquisitionRunner` with schema + validation + output-shape tests |
| T62 — Optional stygian-graph acquisition bridge | `[x]` | Additive and opt-in only; feature-gated `acquisition-runner` bridge for browser nodes via opt-in `acquisition` block |
| T63 — Runner-first docs and compatibility checks | `[x]` | Added runner-first mode docs/examples, migration notes, and downstream graph compatibility checklist/CI guidance |

---

## Browser Track (Consolidated from PROGRESS-BROWSER.md)

> This section consolidates browser-crate progress into the workspace-level tracker.

### Browser Phase 1 — Foundation & Core Types

| Task | Status | Notes |
|---|---|---|
| Browser T01 — Browser Crate Setup & Core Error Types | `[x]` | Rich error types with context, StealthLevel, PoolConfig, env var overrides |
| Browser T02 — Browser Instance Lifecycle Management | `[x]` | BrowserInstance wrapper, launch, health check, graceful shutdown, timeout handling |
| Browser T03 — Runtime.Enable CDP Leak Protection (Critical) | `[x]` | CdpProtection, AddBinding/IsolatedWorld/EnableDisable modes, source URL patching, env config |

### Browser Phase 2 — Browser Pool & Resource Management

| Task | Status | Notes |
|---|---|---|
| Browser T04 — Browser Instance Pool with Warmup | `[x]` | BrowserPool with warm queue, Semaphore, LRU eviction, acquire timeout, PoolStats |
| Browser T05 — Page & Context Management | `[x]` | PageHandle with drop cleanup, ResourceFilter, WaitUntil, navigate, wait_for_selector, eval, save_cookies |

### Browser Phase 3 — Anti-Detection & Fingerprint Protection

| Task | Status | Notes |
|---|---|---|
| Browser T06 — Navigator Properties Spoofing | `[x]` | StealthProfile + NavigatorProfile injection script, Object.defineProperty, WebGL getParameter override |
| Browser T07 — Fingerprint Injection (Canvas, WebGL, Fonts, Audio) | `[x]` | Fingerprint::random() with curated value pools, injection_script() coverage |
| Browser T08 — Fingerprint Profile Generation with Statistical Distribution | `[x]` | Weighted profile selection, coherence validation, OS/browser alignment |
| Browser T09 — WebRTC IP Spoofing & Proxy Integration | `[x]` | WebRtcPolicy modes, proxy/geolocation alignment, proxy bypass list support |

### Browser Phase 4 — Human-Like Behavioral Mimicry

| Task | Status | Notes |
|---|---|---|
| Browser T10 — Human-Like Mouse Movement (Distance-Aware Trajectories) | `[x]` | Bezier trajectories, jitter, deterministic seed support |
| Browser T11 — Human-Like Typing Patterns | `[x]` | |
| Browser T12 — Random Human-Like Page Interactions | `[x]` | |

### Browser Phase 5 — Stealth Profiles & Configuration

| Task | Status | Notes |
|---|---|---|
| Browser T13 — Configurable Stealth Levels (None, Basic, Advanced) | `[x]` | Stealth application wiring in browser new-page flow |
| Browser T14 — Comprehensive Configuration Management | `[x]` | CDP mode/source controls, validation, JSON load/save, builder coverage |

### Browser Phase 6 — Testing & Detection Validation

| Task | Status | Notes |
|---|---|---|
| Browser T15 — Comprehensive Unit Test Suite | `[x]` | Property tests and broad module test coverage |
| Browser T16 — Integration Tests with Real Browser | `[x]` | Real Chromium integration tests, user-data-dir wiring fixes |
| Browser T17 — Anti-Detection Test Suite (Real-World Validation) | `[x]` | Property-level and live-network `#[ignore]` suites |

### Browser Phase 7 — Documentation & Examples

| Task | Status | Notes |
|---|---|---|
| Browser T18 — Comprehensive API Documentation | `[x]` | Public API docs clean with zero rustdoc warnings |
| Browser T19 — Example Programs | `[x]` | Multiple browser examples compile and run paths documented |
| Browser T20 — Architecture & Design Documentation | `[x]` | Architecture docs with diagrams, module map, and operational notes |

### Browser Phase 8 — Advanced Features & Integrations

| Task | Status | Notes |
|---|---|---|
| Browser T21 — Chrome DevTools MCP Integration | `[x]` | MCP JSON-RPC support, feature-gated server, tool/resource coverage |
| Browser T22 — Performance Monitoring & Metrics | `[x]` | Prometheus-style metrics and pool instrumentation |
| Browser T23 — Session Persistence & Cookie Management | `[x]` | Session snapshot save/restore with TTL and file I/O |
| Browser T24 — Browser Session Recording & Debug Tools | `[x]` | CDP event logging, HAR export, NDJSON export |

---

## Integrations Track (Consolidated from PROGRESS-INTEGRATIONS.md)

### Integrations Phase 1 — DataSink Port Trait

| Task | Status | Notes |
|---|---|---|
| T25 — DataSinkPort Trait Definition | `[x]` | `DataSinkPort`, `SinkRecord`, `SinkReceipt`, `DataSinkError` in ports layer; unit tests added |

### Integrations Phase 2 — Scrape Exchange Adapter

| Task | Status | Notes |
|---|---|---|
| T26 — Scrape Exchange REST API Client | `[x]` | Implemented in `stygian-graph` (`adapters/scrape_exchange.rs`) with typed client/config/auth paths |
| T27 — Scrape Exchange DataSinkPort Implementation | `[x]` | Depends on T26 |
| T28 — Scrape Exchange WebSocket Real-Time Feed | `[x]` | Implemented in `stygian-graph` (`ScrapeExchangeFeed`) with adapter tests and live ignored test |

### Integrations Phase 3 — Documentation & Examples

| Task | Status | Notes |
|---|---|---|
| T29 — Integration Documentation & Examples | `[x]` | Added docs + examples (`book/src/graph/scrape-exchange.md`, `examples/scrape-exchange-*.toml`) |

---

## Stealth v3 Track (Consolidated from PROGRESS-STEALTH-V3.md)

### Stealth v3 Phase 1 — Fingerprint Noise Pipeline

| Task | Status | Notes |
|---|---|---|
| T37 — Deterministic noise seed engine | `[x]` | Foundation for downstream noise modules |
| T38 — Canvas fingerprint noise injection | `[x]` | `canvas_noise` module wired in advanced stealth path |
| T39 — WebGL parameter spoofing | `[x]` | `webgl_noise` module + profile-driven injection wired in stealth |
| T40 — Audio fingerprint noise injection | `[x]` | `audio_noise` module + deterministic script injection |
| T41 — ClientRects & TextMetrics noise | `[x]` | `rects_noise` module with layout/text metrics overrides |

### Stealth v3 Phase 2 — Navigator & Device Coherence

| Task | Status | Notes |
|---|---|---|
| T42 — Fingerprint profile config system | `[x]` | `FingerprintProfile` presets and coherence-first profile selection implemented |
| T43 — Navigator property coherence | `[x]` | `navigator_coherence` module integrated into advanced stealth flow |
| T44 — Performance timing protection | `[x]` | `timing_noise` module provides configurable timing jitter controls |

### Stealth v3 Phase 3 — CDP Leak Hardening

| Task | Status | Notes |
|---|---|---|
| T45 — CDP leak hardening (advanced detection) | `[x]` | CDP hardening paths (`AddBinding` modes, stack sanitization) implemented |

### Stealth v3 Phase 4 — TLS & Network Validation

| Task | Status | Notes |
|---|---|---|
| T46 — TLS fingerprint validation suite | `[x]` | TLS/JA3/JA4 validation support present (`tls_validation` + MCP diagnostic surfaces) |

### Stealth v3 Phase 5 — Peripheral Detection Surfaces

| Task | Status | Notes |
|---|---|---|
| T47 — Peripheral detection surface hardening | `[x]` | `peripheral_stealth` covers iframe/visibility/camera/port/rAF surfaces |

### Stealth v3 Phase 6 — Anti-Bot Validation Suite

| Task | Status | Notes |
|---|---|---|
| T48 — Anti-bot service validation suite | `[x]` | Validation framework + Tiered anti-bot targets in browser validation/tests |

### Stealth v3 Phase 7 — VM-Driven Stealth Hardening

| Task | Status | Notes |
|---|---|---|
| T58 — DataDome VM coherence hardening | `[x]` | Advanced coherent default path + diagnostics + known limitations + Tier1 baseline checks |

---

## Accumulated Learnings

- T49: strict clippy settings (`-D warnings`, pedantic profile) require `writeln!` over `write!(...\n)`, const-friendly constructors, and panic/index-safe tests even under `#[ignore]`.

> Subagents append discoveries here after each task.
> The orchestrator reads this section at the start of every iteration
> to avoid repeating past mistakes.

- T01: `deadpool-redis` `PoolConfig::from_url` returns an owned config — no `mut` needed. Doc comment list-item continuation lines trigger `doc_overindented_list_items` if aligned with visually padded columns. Pre-existing `collapsible_if` lints in stream.rs, document.rs, and database.rs needed fixing (Rust 2024 let-chains).
- T30: `BrowserError::StaleNode` unit test belongs in `src/error.rs` `#[cfg(test)]` block alongside the other display tests. Integration tests using `#[ignore]` must call methods directly on the returned `Vec<NodeHandle>` without importing `NodeHandle` by name — the type is inferred. `example.com` is a reliable fixture for DOM query tests since its structure is stable.
- T31: Mutual recursion between `BrowserError::ExtractionFailed(ExtractionError)` and `ExtractionError::CdpFailed { source: BrowserError }` requires boxing one side — box the `BrowserError` in `CdpFailed` to break infinite type size. `#[cfg(feature = "...")]` on enum variants works cleanly with thiserror. `trybuild` auto-generates `.stderr` files into `wip/` on first run — copy to `tests/ui/` to accept them. The `extract` feature must be added to `full` to keep `--all-features` working. Place integration tests for a feature in a `#[cfg(feature = "...")]` `mod` inside `integration.rs` to silence dead-code lints from struct fields used only in ignored tests.
- T33: `#[serde(rename = "attrNames")]` is required on `attr_names` to match the camelCase key emitted by JS. Use old-style `Array.prototype.slice.call` / `var` / `function()` in the fingerprint JS rather than arrow functions or spread syntax for broadest compatibility. When both sorted slices are empty, Jaccard should return `1.0` (not `NaN` from 0/0). The `similarity` feature needs no extra crate deps — `serde`/`serde_json` are already in the workspace. Adding a `#[cfg(feature = "similarity")] impl NodeHandle` block alongside a `#[cfg(feature = "similarity")] impl PageHandle` block keeps dead-code lints silent when the feature is off.
- T51: `page.rs` did not import serde — add `use serde::{Deserialize, Serialize};` before implementing any serializable types there. Use a local `WarmupWait` enum (serializable, maps to `WaitUntil`) rather than trying to derive serde on `WaitUntil` which has a non-trivial `Selector(String)` variant. `#[serde(default = "fn_name")]` with a `const fn` returning a numeric literal requires the function to return the same type as the field — no coercion across integer widths. MCP tool handlers that open a new page should always call `page.close().await?` before returning, even on the happy path.
- T53: `#[serde(untagged)]` on an enum is the cleanest way to handle multiple JSON envelope shapes. Use `Option<u16>` for port fields and `Option<String>` for address fields with `#[serde(default)]` to be tolerant of partial records. Builder methods on a fetcher (`.with_limit()`, `.with_protocol_filter()`, `.with_country_filter()`) should each carry `#[must_use]` and append query params lazily in a `request_url()` helper — keeps the `fetch()` impl clean.
- T54: `extract_all_with_fallback` and `extract_resilient` live inside the `#[cfg(feature = "extract")] impl PageHandle` block in `page.rs`. For resilient skipping, match on `ExtractionError::Missing { .. }` and `continue`; propagate all other variants as hard `BrowserError::ExtractionFailed`. Unit tests for the new methods can be `#[cfg(feature = "extract")]` inside the existing `mod tests` block — no live browser needed for error-variant classification tests.
- T52: `ProxyCapabilities` cannot derive `Eq` when it includes `Option<f32>` (use `PartialEq` only). Capability filtering should reuse the same candidate construction path as normal proxy selection (`storage.list_with_metrics()` + health/circuit maps) to avoid stale or non-existent manager fields. Strict clippy (`-D warnings`) requires explicit `ProxyCapabilities::default()` in proxy literals and panic-safe assertions (`first()` over indexing) in tests.
- T59: `AcquisitionRunner::run` should return a terminal result object instead of `Result` so timeout/setup-failure paths can be represented as deterministic failure bundles. For sticky browser retries, `BrowserPool::acquire_for(host)` gives opt-in context pinning without changing pool internals.
- T60: keep runtime-policy mapping pure and deterministic (`map_policy_hints`), with explicit defaults for partial input and clamped risk score to prevent undefined strategy transitions.
- T61: MCP tool wrappers around runner enums should parse string modes with explicit validation and emit a compact `diagnostics` bundle (`attempted`, `timed_out`, `failure_count`, `failures`) to keep failure paths stable for downstream automation.
- T62: Keep graph bridge behavior opt-in by requiring a node-level `acquisition` table for `browser` services; without that block, legacy `pipeline_run` skip semantics stay unchanged.
- T63: Keep runner-first docs anchored to live MCP/API contracts (`browser_acquire_and_extract`, `fast|resilient|hostile|investigate`, `acquisition-runner` feature) and document compatibility as additive opt-in so downstream graph users can validate both legacy and bridge paths in CI.
- T58 closure: Tier1 non-regression checks now support optional `STYGIAN_TIER1_BASELINE_CREEPJS` and `STYGIAN_TIER1_BASELINE_BROWSERSCAN` baselines to detect score drops while keeping live validation runnable without pinned scores.
