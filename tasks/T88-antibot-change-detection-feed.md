# T88 — Anti-Bot Change Detection Feed

## Goal

Detect anti-bot behavior shifts early and emit actionable incident packets for rapid response.

## Scope

- Add detector that monitors canary/diagnostic deltas for likely wall-logic changes.
- Emit structured change events with:
  - affected targets
  - delta summary
  - recommended mitigation path
- Integrate event emission with existing observability/runbook flows.

## Feature flag

Default-on. New module `change_feed` lives in `stygian-charon` (under
`crates/stygian-charon/src/change_feed/`) since it consumes canary and
proxy/extraction signals from charon-managed flows. Events are emitted
via the existing `stygian-charon` metrics surface (T64) and the
diagnostics payload.

If the event emission surface is a breaking change for existing metrics
consumers, add a `change-feed` feature gate and wire it into `full`.
Otherwise, additive only.

## Depends on

- T84 (canary hard-gate maturity) — reliable canary deltas.
- T86 (proxy intelligence scoring) — proxy health context.
- T87 (extraction reliability scoring) — content-quality regressions.

## Informs

- T80 (cross-context coherence) — coherence deltas are an input but
  not a hard dep.
- T83 (challenge-aware feedback) — challenge outcomes are an input
  but not a hard dep.

## Must Haves

- Low-noise heuristics with configurable thresholds.
- Clear distinction between transient noise and probable vendor
  rotation.

## Test Hints

- Unit: delta classification into noise vs probable change.
- Unit: threshold configuration round-trips through the config struct.
- Integration: event packet generation from synthetic canary
  regressions (may be `#[ignore]`).

## Exit Criteria

- [ ] `ChangeDetector` consumes canary, proxy, and extraction delta
      streams and classifies each as Noise | Suspected | Probable.
- [ ] `ChangeEvent` payload includes affected targets, delta summary,
      and recommended mitigation path.
- [ ] Configurable thresholds for noise/suspected/probable bands.
- [ ] Events emitted via charon metrics surface and surfaced in
      runbook diagnostics.
- [ ] At least 3 unit tests for delta classification across
      representative delta sequences.
- [ ] At least 1 `#[ignore]` integration test generating an event
      packet from synthetic canary regressions.
- [ ] Docs updated: module rustdoc + runbook consumption guide.
- [ ] `cargo build --workspace --all-features` clean.
- [ ] `cargo test --workspace --all-features` clean.
- [ ] `cargo clippy --workspace --all-features -- -D warnings` clean.
- [ ] AGENTS.md rules respected.
