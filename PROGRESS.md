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
| T54 — Adaptive Selector Recovery for Extraction | `[ ]` | |

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
- T52: `ProxyCapabilities` cannot derive `Eq` when it includes `Option<f32>` (use `PartialEq` only). Capability filtering should reuse the same candidate construction path as normal proxy selection (`storage.list_with_metrics()` + health/circuit maps) to avoid stale or non-existent manager fields. Strict clippy (`-D warnings`) requires explicit `ProxyCapabilities::default()` in proxy literals and panic-safe assertions (`first()` over indexing) in tests.
