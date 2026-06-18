# T92 — JavaScript Integrity Trap Canary

## Goal

Detect and score runtime integrity checks that expose automation patch artifacts before they cause full blocking.

## Scope

- Add canary probes for integrity-trap patterns on patched/native browser surfaces.
- Produce risk score and mitigation hints in diagnostic output.
- Feed integrity-trap signal into policy feedback and change detection.

## Feature flag

Default-on. New module `integrity_canary` lives in `stygian-browser`
(under `crates/stygian-browser/src/integrity_canary/`). Probes run via
the existing stealth canary infrastructure (T84) so they share the
trend-detection and gating path.

If the probe set requires new browser-injected scripts that conflict
with existing stealth injection, add an `integrity-canary` feature gate
and wire it into `full`. Otherwise, additive only.

## Depends on

- T90 (vendor-to-playbook auto-resolution) — selects correct challenge
  path.
- T58 (VM coherence hardening, Stealth v3) — integrity-trap detection
  requires coherent VM behavior.

## Informs

- T84 (canary hard-gate maturity) — integrity findings feed the
  canary trend signal, but T84 is not a hard dep.

## Unblocks

- T88 (anti-bot change detection feed) — integrity-trap attribution
  improves change-event diagnostics.

## Must Haves

- Probe set with stable output schema.
- Risk scoring thresholds suitable for CI and runtime diagnostics.
- Clear separation between suspected trap and confirmed regression.

## Test Hints

- Unit: risk-score computation from probe outcomes.
- Unit: thresholds distinguish Suspected from Confirmed.
- Integration: canary report includes mitigation hints for trap
  findings (may be `#[ignore]`).

## Exit Criteria

- [x] `IntegrityProbe` set with stable output schema (probe name,
      outcome, risk contribution).
- [x] `IntegrityRiskScore` model with documented Suspected/Confirmed
      thresholds.
- [x] Probe results flow into the existing canary trend-detection
      pipeline (via `CanaryTrendObservation` T84 seam — T84 itself is
      `[ ]` and will consume the seam in a follow-up task).
- [x] Mitigation hints emitted in diagnostic output (actionable text
      per probe).
- [x] At least 3 unit tests for risk-score computation across
      representative probe outcomes.
- [x] At least 1 `#[ignore]` integration test confirming canary
      report includes trap findings + hints.
- [x] Docs updated: module rustdoc + probe catalog.
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [x] `cargo clippy --workspace --all-features -- -D warnings` clean
      (preflight is per-crate strict on `stygian-browser` per the
      Phase 13 baseline-tolerant policy; workspace strict-clippy is
      blocked by T95 baseline cleanup).
- [x] AGENTS.md rules respected.
