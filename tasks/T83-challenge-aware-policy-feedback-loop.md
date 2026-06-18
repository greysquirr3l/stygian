# T83 — Challenge-Aware Policy Feedback Loop

## Goal

Automatically feed challenge outcomes back into strategy planning so runtime policy improves over time.

## Scope

- Add normalized challenge outcome labels for acquisition attempts.
- Persist short-horizon outcome memory per domain/target class.
- Use outcome memory in policy mapping to influence next-run strategy.

## Feature flag

Default-on. New module `challenge_feedback` lives in `stygian-charon`
(under `crates/stygian-charon/src/challenge_feedback/`), since this is
runtime policy memory. Persist via the existing Charon cache trait (T68)
with a short-horizon TTL and bounded growth (LRU + max-entries cap).

Influence bounds are critical: ship with conservative defaults and
clamp risk-score adjustments to a documented maximum. Add a
`challenge-feedback` feature gate if charon already has one and reuse
it; otherwise add a new gate and wire it into `full`.

## Depends on

- T81 (replay defense) — challenge outcome labels align with replay-defense
  invalidation events.
- T82 (transport realism) — transport compatibility outcomes are
  challenge-adjacent and contribute to outcome memory.

## Informs

- T80 (cross-context coherence) — coherence findings are additional
  challenge context but are not a hard dep.
- T86 (proxy intelligence) — challenge outcomes improve proxy score
  updates.
- T89 (vendor classifier) — challenge outcomes sharpen vendor
  classification.

## Unblocks

- T84 (canary hard-gate maturity).
- T85 (target-class playbooks).
- T88 (anti-bot change detection feed).
- T89 (vendor classifier).

## Must Haves

- Stable challenge outcome taxonomy.
- Deterministic influence bounds to prevent runaway strategy escalation.

## Test Hints

- Unit: outcome-memory update and decay logic.
- Unit: risk-score clamp at documented maximum.
- Integration: prior challenge outcomes alter policy recommendation as
  expected (may be `#[ignore]`).

## Exit Criteria

- [x] `ChallengeOutcome` enum (Pass | SoftChallenge | HardChallenge |
      Blocked | Captcha) with stable string serialization.
- [x] `ChallengeMemory` LRU store keyed by domain/target-class, with
      TTL and max-entries cap.
- [x] Policy mapping in `stygian-charon` consumes memory and clamps
      risk-score adjustments to a documented maximum.
- [x] At least 3 unit tests for memory update, decay, and clamp.
- [x] At least 1 `#[ignore]` integration test verifying that prior
      outcomes alter the next policy recommendation.
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [x] `cargo clippy -p stygian-charon --all-features --all-targets -- -D warnings` clean (0 errors vs 0 baseline; workspace clippy is blocked by 104 pre-existing baseline errors per T95 backlog).
- [x] AGENTS.md rules respected.
