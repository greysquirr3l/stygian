# T93 — Proof-of-Work Capability Profile

## Goal

Quantify proof-of-work challenge handling capabilities and route strategy based on observed solve characteristics.

## Scope

- Add PoW capability profile metrics: solve latency, solve success rate, retry profile, and failure modes.
- Persist per-target and per-vendor PoW capability summaries.
- Feed profile into policy escalation and pacing decisions.

## Feature flag

Default-on. New module `pow_profile` lives in `stygian-charon` (under
`crates/stygian-charon/src/pow_profile/`) since it informs policy
decisions. Persistence reuses the existing `ChallengeMemory` store
(T83) with a key namespace for PoW.

If the metric schema is a breaking change for charon consumers, add a
`pow-profile` feature gate and wire it into `full`. Otherwise,
additive only.

## Depends on

- T91 (token lifecycle contracts) — token validity gates PoW attempts.
- T90 (vendor-to-playbook auto-resolution) — selects correct PoW path.

## Informs

- T82 (transport realism) — transport characteristics inform PoW
  pacing but T82 is not a hard dep.
- T83 (challenge-aware feedback) — challenge outcomes inform PoW
  scoring but T83 is not a hard dep.

## Unblocks

- T94 (queue/interstitial routing) — PoW capability drives
  interstitial handling strategy.
- T88 (anti-bot change detection feed) — PoW capability regressions
  strengthen event diagnostics.

## Must Haves

- Stable metric schema and sampling windows.
- Deterministic capability scoring for policy decisions.
- Safe fallback when PoW telemetry is sparse.

## Test Hints

- Unit: capability scoring over synthetic PoW traces.
- Unit: sparse-telemetry fallback returns documented default.
- Integration: policy mapping consumes PoW capability profile
  (may be `#[ignore]`).

## Exit Criteria

- [x] `PowCapabilityProfile` schema covering solve latency, success
      rate, retry count, and failure modes.
- [x] Sampling window configurable (default documented).
- [x] `PowCapabilityScorer` returns a deterministic score with sparse
      data falling back to a documented default.
- [x] Policy mapping in charon consumes the score and adjusts
      escalation/pacing.
- [x] At least 3 unit tests for scoring (good/poor/sparse telemetry).
- [x] At least 1 `#[ignore]` integration test confirming policy
      mapping reflects a synthetic PoW profile.
- [x] Docs updated: module rustdoc + sampling/window defaults.
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [x] `cargo clippy -p stygian-charon --all-features --all-targets -- -D warnings` clean
      (0 errors vs 0 baseline on `stygian-charon`).
- [x] AGENTS.md rules respected.
