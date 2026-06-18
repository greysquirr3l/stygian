# T91 — Challenge Token Lifecycle Contracts

## Goal

Define strict lifecycle contracts for challenge tokens to prevent invalid replay and stale token reuse.

## Scope

- Add token contract model capturing TTL, nonce binding, and single-use semantics.
- Enforce token validity checks before reuse and submission.
- Track invalidation reasons by vendor family and challenge class.

## Feature flag

Default-on. New module `token_lifecycle` lives in `stygian-charon`
(under `crates/stygian-charon/src/token_lifecycle/`). Token contracts
integrate with the existing `ChallengeMemory` (T83) for nonce
bookkeeping and with `VendorClassifier` (T89) for vendor-family
policies.

If the contract model is incompatible with existing token storage
paths, add a `token-lifecycle` feature gate and wire it into `full`.
Otherwise, additive only.

## Depends on

- T79 (freshness contracts) — token TTLs derive from freshness.
- T81 (replay defense) — nonce-bound validity windows.
- T89 (vendor classifier) — vendor-family policy lookup.

## Unblocks

- T92 (JS integrity trap canary) — token contracts inform integrity
  detection.
- T93 (PoW capability profile) — token contracts gate PoW attempts.
- T94 (queue/interstitial routing) — token validity is a routing
  input.

## Must Haves

- Vendor-aware token policy schema.
- Deterministic validation and invalidation flow.
- Structured diagnostics for token lifecycle failures.

## Test Hints

- Unit: TTL expiration, single-use, and nonce mismatch behavior.
- Unit: vendor-aware policy lookup returns documented defaults.
- Integration: invalid token reuse triggers contract violation path
  (may be `#[ignore]`).

## Exit Criteria

- [x] `TokenContract` model (TTL, nonce binding, single-use flag,
      vendor family) implemented with serde + thiserror.
- [x] `TokenValidator` enforces TTL, single-use, and nonce match
      before submission.
- [x] `InvalidationReason` enum includes vendor family and challenge
      class for diagnostic routing.
- [x] At least 4 unit tests covering TTL expiration, single-use
      replay, nonce mismatch, and vendor policy lookup.
- [x] At least 1 `#[ignore]` integration test confirming an
      invalid token reuse triggers the violation path.
- [x] Docs updated: module rustdoc + vendor policy table.
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [x] `cargo clippy --workspace --all-features -- -D warnings` clean.
      (`stygian-charon` baseline 0 → 0; the 105 workspace errors
      are the pre-existing `stygian-browser` baseline documented
      in T79's learnings and are not touched by T91.)
- [x] AGENTS.md rules respected.
