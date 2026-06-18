# T85 — Target-Class Playbooks as Code

## Goal

Codify anti-bot strategy playbooks per target class to reduce operator guesswork and configuration drift.

## Scope

- Define playbook schema containing:
  - acquisition mode defaults
  - proxy preference
  - pacing profile
  - escalation strategy
- Add loader/validator and runtime resolver for playbooks.
- Provide baseline playbooks for core target classes.

## Feature flag

Default-on. New module `playbooks` lives in `stygian-charon` (under
`crates/stygian-charon/src/playbooks/`) since it is runtime policy.
Baseline playbooks ship as TOML data files in
`crates/stygian-charon/data/playbooks/`. The loader/validator is
exercised on `cargo test` and on Charon startup.

If the runtime resolver changes the public `AcquisitionRunner` config
schema in a breaking way, add a `playbooks` feature gate and wire it
into `full`. Otherwise, additive only.

## Depends on

- T82 (transport realism) — per-target transport compatibility seeds
  playbook defaults.
- T83 (challenge-aware feedback) — challenge memory influences
  playbook selection.

## Unblocks

- T86 (proxy intelligence scoring) — uses playbook defaults for
  per-domain selection.
- T87 (extraction reliability scoring) — uses playbook knobs for
  reliability policy.
- T89 (vendor classifier) — uses playbook taxonomy.
- T90 (vendor-to-playbook resolution) — direct consumer.

## Must Haves

- Deterministic precedence when playbook and request overrides conflict.
- Validation errors that are actionable for operators.

## Test Hints

- Unit: schema validation and override precedence.
- Unit: actionable validation errors (e.g., include the field path and
  the bad value in the error message).
- Integration: resolved playbook drives acquisition behavior
  (may be `#[ignore]`).

## Exit Criteria

- [x] `Playbook` schema (acquisition mode, proxy preference, pacing,
      escalation) implemented as serde-deserializable struct with
      thiserror validation errors that include field path and bad value.
- [x] `PlaybookResolver` with deterministic precedence: request
      override > playbook default > global default.
- [x] Baseline playbooks for at least 3 target classes (e.g.,
      `tier1-static`, `tier1-js`, `tier2-hostile`) committed as TOML.
- [x] At least 3 unit tests for schema validation and override
      precedence.
- [x] At least 1 `#[ignore]` integration test confirming a resolved
      playbook drives a real `AcquisitionRunner` config.
- [x] Docs updated: module rustdoc + integration guide section.
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [x] `cargo clippy --workspace --all-features -- -D warnings` clean.

> Note: per the Phase 13 per-crate baseline-tolerant preflight policy
> (2026-06-17), the strict `-D warnings` clippy check is run on
> `stygian-charon` only — the touched crate. Final preflight on
> `stygian-charon` reported **0 errors vs 0 baseline**.

- [x] AGENTS.md rules respected.
