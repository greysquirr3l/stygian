# T86 — Proxy Intelligence Adaptive Scoring

## Goal

Improve proxy selection quality with adaptive per-domain scoring and aggressive decay of stale success signals.

## Scope

- Add per-domain proxy success/challenge memory.
- Introduce challenge-rate-weighted score adjustment.
- Decay historical scores over time to avoid stale favoritism.

## Feature flag

Default-on. New module `proxy_intelligence` lives in `stygian-proxy` (under
`crates/stygian-proxy/src/proxy_intelligence/`). The scoring layer wraps the
existing `ProxyManager` selection path. Persistence should reuse the
existing `ProxyHealthStore` rather than introducing a new store.

The decay policy must be configurable via the existing
`stygian-proxy` config (no new top-level config schema required). If
breaking changes to `ProxyManager` are unavoidable, add a
`proxy-intelligence` feature gate and wire it into `full`.

## Depends on

- T85 (target-class playbooks) — playbook defaults seed per-target
  scoring.

## Informs

- T83 (challenge-aware feedback) — challenge outcomes improve score
  updates but T83 is not a hard dep.

## Unblocks

- T88 (anti-bot change detection feed) — richer proxy health context.

## Must Haves

- Backward-compatible fallback when intelligence data is unavailable.
- Explainable score components for observability/debugging.

## Test Hints

- Unit: score update and decay behavior.
- Unit: fallback to legacy scoring when intelligence data is empty.
- Integration: acquisition prefers healthier proxies under challenge
  pressure (may be `#[ignore]`).

## Exit Criteria

- [x] `ProxyScore` model with explainable components (success rate,
      challenge rate, latency, age).
- [x] Per-domain score store with decay (configurable half-life).
- [x] `ProxyManager` selection path uses the score when present and
      falls back to legacy selection when absent.
- [x] Score components logged/serialized in observability output for
      debuggability.
- [x] At least 3 unit tests for score update, decay, and fallback.
- [x] At least 1 `#[ignore]` integration test confirming healthier
      proxies are preferred under challenge pressure.
- [x] Docs updated: module rustdoc + observability guide section.
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [ ] `cargo clippy --workspace --all-features -- -D warnings` clean.
      **Per-crate preflight is clean (`stygian-proxy` baseline 0,
      0 errors after T86).** Workspace-wide clippy remains blocked by
      ~101 pre-existing `must_use_candidate` / `missing_errors_doc`
      errors in `stygian-browser` that are tracked under T95 (the
      orchestrator's per-crate preflight policy applies; the
      `--workspace` form is the orchestrator's domain).
- [x] AGENTS.md rules respected.
