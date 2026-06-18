# T81 — Adaptive Session Replay Defense Mode

## Goal

Reduce replay-style detections by introducing session lifecycle policies that
adapt to challenge volatility.

## Scope

- Add per-domain session lifecycle policy primitives:
  - rotation interval
  - nonce-bound validity windows
  - forced reset on challenge-signature drift
- Integrate with acquisition/session management paths.

## Feature flag

Default-on. New module `replay_defense` lives in `stygian-browser` (under
`crates/stygian-browser/src/replay_defense/`). The policy types should be
serializable (serde) so they can be persisted in the existing session
snapshot path (Browser T23).

If the integration with `BrowserPool`/`AcquisitionRunner` reveals a
breaking change for existing callers, add a `replay-defense` feature gate
and wire it into `full`. Otherwise, ship enabled by default.

## Depends on

- T79 freshness contracts (rotation interval + signature drift both
  derive from freshness primitives).

## Informs

- T80 cross-context coherence — drift findings can trigger a session
  reset, but T80 is not a hard dep.

## Unblocks

- T83 challenge-aware policy feedback loop.
- T91 challenge token lifecycle (nonce-bound validity windows).

## Must Haves

- Policy-driven invalidation hooks.
- Deterministic fallback behavior when policy state is missing.

## Test Hints

- Unit: rotation and nonce window decisions.
- Unit: deterministic default when no policy is configured.
- Integration: signature drift triggers forced session refresh
  (may be `#[ignore]`).

## Exit Criteria

- [x] `ReplayDefensePolicy` model with rotation interval, nonce-bound
      validity window, and forced-reset-on-drift boolean.
- [x] Hooks into `BrowserPool`/`AcquisitionRunner` so signature drift
      triggers a forced refresh.
- [x] Deterministic default policy when no operator override is set.
- [x] At least 3 unit tests covering rotation, nonce window, and
      default-fallback paths.
- [x] At least 1 `#[ignore]` integration test for signature-drift
      forced refresh.
- [x] Docs updated (module rustdoc + integration guide section).
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [!] `cargo clippy --workspace --all-features -- -D warnings` clean.
      Workspace preflight is blocked by 104 pre-existing baseline
      errors tracked as T95 (concentrated in `stygian-browser`).
      Per-crate clippy on `stygian-browser` adds zero new errors
      above the baseline (104 → 104).
- [x] AGENTS.md rules respected.
