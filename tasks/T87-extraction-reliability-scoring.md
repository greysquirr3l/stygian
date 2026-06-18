# T87 — Extraction Reliability Scoring

## Goal

Add reliability scoring for extraction outputs so fallback chains optimize for data quality, not only fetch success.

## Scope

- Define extraction reliability score from:
  - schema completeness
  - confidence and transformation outcomes
  - retry/fallback path quality signals
- Add score to extraction result metadata.
- Use score in fallback chain selection policy.

## Feature flag

Default-on. New module `reliability` lives in `stygian-plugin` (under
`crates/stygian-plugin/src/reliability/`) since it scores extraction
outputs. The score field is added to the existing `ExtractionResult`
metadata struct (T71) as an additive optional field — no breaking
change for existing consumers.

If the fallback chain integration requires a `FallbackChainService` config
change, add a `reliability-scoring` feature gate and wire it into `full`.

## Depends on

- T85 (target-class playbooks) — playbook controls consume reliability
  policy knobs.
- Phase 12 plugin extraction primitives (T71–T76) — extraction result
  metadata and fallback chain hooks.

## Unblocks

- T88 (anti-bot change detection feed) — content-quality regressions
  are a change signal.

## Must Haves

- Stable scoring function with documented interpretation.
- Non-breaking output extension for existing consumers.

## Test Hints

- Unit: score computation over synthetic extraction outcomes.
- Unit: backward-compat — existing consumers that don't read the
  `reliability` field continue to work.
- Integration: fallback chooses higher reliability path when
  available (may be `#[ignore]`).

## Exit Criteria

- [x] `ReliabilityScore` model (0.0–1.0 with documented interpretation
      bands: high / medium / low) implemented with serde.
- [x] Score added to `ExtractionResult` metadata as an additive
      optional field.
- [x] `FallbackChainService` selection policy consults the score
      (configurable weight).
- [x] At least 3 unit tests for score computation across
      representative extraction outcomes.
- [x] At least 1 backward-compat unit test confirming legacy callers
      still compile and pass.
- [x] At least 1 `#[ignore]` integration test confirming fallback
      chooses the higher-reliability path.
- [x] Docs updated: module rustdoc + score-band interpretation table.
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [~] `cargo clippy --workspace --all-features -- -D warnings` clean.
      _Touched crate (`stygian-plugin`) is 0-error vs the 24-error pre-fix baseline
      (mechanical bonus, like T86). Workspace-wide clippy still reports the
      92 pre-existing `stygian-browser` errors tracked under Phase 13
      maintenance task T95 (unrelated to T87). Per the Phase 13 per-crate
      preflight policy, the touched-crate check `cargo clippy -p stygian-plugin
      --all-features --all-targets -- -D warnings` reports 0 errors in
      `stygian-plugin` itself (the 92 reported errors are all in the
      transitive `stygian-browser` dependency)._
- [x] AGENTS.md rules respected.
