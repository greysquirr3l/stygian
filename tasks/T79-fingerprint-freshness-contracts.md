# T79 — Fingerprint Freshness Contracts

## Goal

Introduce freshness contracts for browser identity artifacts so stale
fingerprints and stale challenge context cannot be reused beyond safe TTL.

## Scope

- Add a freshness contract model for acquisition/browser sessions:
  - capture timestamp
  - target/domain binding
  - challenge signature/version hash (when available)
  - max age policy
- Enforce freshness checks before reuse of persisted or pooled identity state.
- Add explicit invalidation reasons for stale/rotated contexts.

## Feature flag

Default-on. New module `freshness` lives in `stygian-browser` (under
`crates/stygian-browser/src/freshness/`). Wire the freshness decision into
the existing `acquisition-runner` and `stealth-v3` feature paths so it is
exercised by the integration tests gated on those features. No new feature
gate required.

If the implementation reveals that the freshness check is incompatible with
existing session persistence (e.g., it would reject currently-pooled
sessions), add a `freshness` feature gate and wire it into the workspace
`full` meta-feature. Document the gate in the module-level rustdoc.

## Depends on

- Existing session/acquisition primitives from Phases 10 to 12 (T59–T62,
  T71–T78).

## Unblocks

- T80 (cross-context coherence probes) — needs freshness to gate identity
  reuse across contexts.
- T81 (adaptive replay defense) — needs freshness to define session TTLs.
- T82 (transport realism) — needs freshness to gate transport profile
  reuse.
- T91 (token lifecycle) — needs freshness primitives for token TTLs.

## Non-Goals

- Implementing new anti-bot bypass techniques.

## Must Haves

- Deterministic freshness decision function.
- Domain-aware TTL defaults with config overrides.
- Telemetry/log fields that explain why a session was invalidated.

## Test Hints

- Unit: TTL and signature-change invalidation logic.
- Unit: determinism — same `(timestamp, domain, signature, policy)` inputs
  always produce the same `FreshnessDecision`.
- Integration: stale session is rejected and re-acquired (may be
  `#[ignore]`).

## Exit Criteria

- [x] `FreshnessContract` model implemented (capture ts, domain binding,
      signature hash, max-age policy) with serde + thiserror.
- [x] `FreshnessDecision` enum (Valid | StaleTtl | SignatureMismatch |
      DomainMismatch) with `InvalidationReason` debug fields.
- [x] Reuse path in `acquisition` runner / `BrowserPool` enforces
      freshness policy and emits a structured rejection on invalidation.
- [x] At least 3 unit tests covering TTL, signature change, and domain
      mismatch invalidation.
- [x] At least 1 determinism test (same inputs → same decision) under
      `#[cfg(test)]`.
- [x] At least 1 `#[ignore]` integration test for stale-session rejection
      and re-acquisition, with a runnable path documented in the test
      docstring.
- [x] Public types and methods have rustdoc with an example.
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [!] `cargo clippy --workspace --all-features -- -D warnings` clean.
      The freshness module itself is clippy-clean (zero
      `freshness/`-located warnings under default or pedantic lint
      sets). The 104 pre-existing clippy errors on `main` (unrelated
      to this task — `#[must_use]`, `missing_errors_doc`,
      `struct_excessive_bools`, etc. across `behavior.rs`, `tls.rs`,
      `page.rs`, `mcp.rs`, etc.) are out of scope for T79 and were
      failing on baseline `main` before any T79 changes.
- [x] AGENTS.md rules respected: `thiserror` errors, no `.unwrap()` in
      library code, no `anyhow` outside CLI, hexagonal layering preserved.
