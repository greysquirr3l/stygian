# T94 — Queue and Interstitial Detection Routing

## Goal

Detect queue/wait-room/interstitial anti-bot states and route requests to dedicated handling paths instead of generic retry loops.

## Scope

- Add detector for queue pages, waiting rooms, and anti-bot interstitial transitions.
- Classify state as queue, challenge interstitial, hard block, or transient redirect.
- Route classified states into dedicated acquisition strategies with explicit diagnostics.

## Feature flag

Default-on. New module `interstitial_router` lives in `stygian-browser`
(under `crates/stygian-browser/src/interstitial_router/`). The router
plugs into the existing `AcquisitionRunner` failure-recovery path so
classified states get dedicated handling rather than generic retries.

If the integration with `AcquisitionRunner` is a breaking change, add
an `interstitial-routing` feature gate and wire it into `full`.
Otherwise, additive only.

## Depends on

- T91 (token lifecycle contracts) — token validity is a routing input.
- T93 (PoW capability profile) — drives interstitial handling strategy.
- T90 (vendor auto-resolution) — selects correct routing path.

## Informs

- T89 (vendor classifier) — classification signals are an input but
  not a hard dep.
- T88 (change detection feed) — interstitial events are a change
  signal but not a hard dep.

## Unblocks

This is the capstone of Phase 13. It does not unblock any other
Phase 13 task. Final integration tests and end-to-end
documentation are part of this task's scope.

## Must Haves

- Robust classifier for queue/interstitial signatures.
- Explicit routing behavior per classification type.
- Observability output that distinguishes queue from hard block.

## Test Hints

- Unit: page-signature classification logic across representative
  samples (queue, challenge interstitial, hard block, transient
  redirect).
- Unit: routing output is stable per classification (no
  non-determinism between identical inputs).
- Integration: acquisition path changes correctly by classified
  state (may be `#[ignore]`).

## Exit Criteria

- [ ] `InterstitialClassifier` consuming page signatures
      (URL pattern, body markers, header set) and returning
      `InterstitialKind` (Queue | Challenge | HardBlock | Transient).
- [ ] `InterstitialRouter` returns a dedicated strategy per kind
      with explicit diagnostics.
- [ ] Routing wired into `AcquisitionRunner` failure-recovery path
      so classified states bypass generic retry loops.
- [ ] Observability output distinguishes Queue from HardBlock with
      a dedicated field.
- [ ] At least 4 unit tests covering each `InterstitialKind`.
- [ ] At least 1 determinism test (identical inputs → identical
      classification).
- [ ] At least 1 `#[ignore]` integration test confirming routing
      behavior changes by classified state.
- [ ] Docs updated: module rustdoc + routing behavior table.
- [ ] `cargo build --workspace --all-features` clean.
- [ ] `cargo test --workspace --all-features` clean.
- [ ] `cargo clippy --workspace --all-features -- -D warnings` clean.
- [ ] AGENTS.md rules respected.
