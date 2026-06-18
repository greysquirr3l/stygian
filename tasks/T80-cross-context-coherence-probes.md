# T80 — Cross-Context Coherence Probes

## Goal

Verify stealth coherence across main window, iframe, and worker contexts.

## Scope

- Add probe runner that executes the same coherence checks in:
  - top-level document
  - same-origin iframe
  - dedicated/shared worker where available
- Compare key identity surfaces across contexts and emit drift diagnostics.

## Feature flag

Default-on. New module `coherence` lives in `stygian-browser` (under
`crates/stygian-browser/src/coherence/`). The probe runner requires CDP and
should compile only under the existing `browser-cdp` (or equivalent)
feature. Worker-context probes are best-effort and silently skip when the
runtime does not support them (no panic, structured `Skipped` field in the
report).

If the probe work needs to ship opt-in (e.g., to avoid surprising existing
diagnostic consumers with a new report section), add a `coherence-probes`
feature gate and wire it into `full`.

## Depends on

- T79 freshness contracts (probe inputs must be freshness-gated).
- Existing stealth diagnostic surfaces in `stygian-browser` (Phase 9
  foundations, Browser T21).

## Unblocks

- T83 challenge-aware policy feedback loop — uses cross-context drift as
  challenge context.
- T88 anti-bot change detection feed — uses drift deltas as a change
  signal.

## Must Haves

- Structured coherence-drift report.
- Clear separation between hard failures and known limitations.

## Test Hints

- Unit: drift detection logic for mismatched context outputs.
- Unit: `Skipped` field is emitted (not panic) when worker context is
  unavailable.
- Integration: page-level report includes all available context sections
  (may be `#[ignore]`).

## Exit Criteria

- [x] `CoherenceProbe` runner executes checks in top-level, iframe, and
      (best-effort) worker contexts.
- [x] `CoherenceDriftReport` schema covers per-context identity surfaces
      with `Skipped` markers for unavailable contexts.
- [x] Drift diagnostics exposed in report output (no panic on partial
      availability).
- [x] At least 2 unit tests for drift detection (match + mismatch).
- [x] At least 1 `#[ignore]` integration test that loads a fixture page
      and emits a report.
- [x] Public types and methods have rustdoc with an example.
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [x] `cargo clippy --workspace --all-features -- -D warnings` clean.
- [x] AGENTS.md rules respected.
