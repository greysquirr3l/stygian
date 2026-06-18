# T82 — Transport Realism Expansion

## Goal

Expand transport-layer diagnostics and profile realism checks beyond current TLS-only baselines.

## Scope

- Extend transport diagnostics with additional HTTP/2 behavior checks where observable.
- Add per-target transport compatibility scoring and recommendation output.
- Feed compatibility outcome into acquisition strategy hints.

## Feature flag

Default-on. New module `transport_realism` lives in `stygian-browser` (under
`crates/stygian-browser/src/transport_realism/`). HTTP/2 checks are
observable via the existing `tls_validation` module (T46); integrate rather
than duplicate.

The per-target compatibility score should be exposed as a strategy hint
via the `AcquisitionRunner` config so downstream policy mapping (T83) can
consume it. If that integration requires a public type change, add a
`transport-realism` feature gate and wire it into `full`.

## Depends on

- T79 freshness contracts (transport profiles must be freshness-gated
  before reuse).
- Existing `tls_validation` module (T46, Stealth v3 Phase 5).

## Informs

- T83 challenge-aware policy feedback loop (transport compatibility
  outcomes are challenge-adjacent).
- T85 target-class playbooks (per-target scoring seeds playbook
  defaults).

## Unblocks

- T83 (challenge-aware feedback).
- T85 (target-class playbooks).
- T93 (PoW capability profile — transport scoring contributes to
  pacing decisions).

## Must Haves

- Backward-compatible diagnostic schema extension.
- Clear confidence/coverage markers for transport observations.

## Test Hints

- Unit: compatibility scoring for matching/mismatching transport
  observations.
- Unit: confidence/coverage markers default to a known value when
  HTTP/2 observations are unavailable.
- Integration: diagnostic payload includes new transport sections when
  available (may be `#[ignore]`).

## Exit Criteria

- [x] HTTP/2 behavior checks added to transport diagnostics
      (SETTINGS frame fingerprint, header order, etc., where observable).
- [x] `TransportCompatibility` score with per-target output and
      confidence/coverage markers.
- [x] Score exposed as an `AcquisitionRunner` strategy hint (typed
      config field).
- [x] Diagnostic schema extended in a backward-compatible way (additive
      JSON fields, no renames).
- [x] At least 2 unit tests for scoring (match + mismatch).
- [x] At least 1 `#[ignore]` integration test confirming the new
      sections appear in the diagnostic payload.
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [!] `cargo clippy --workspace --all-features -- -D warnings` clean.
      Workspace preflight is blocked by 104 pre-existing baseline
      errors tracked as T95 (concentrated in `stygian-browser`).
      Per-crate clippy on `stygian-browser` adds zero new errors
      above the baseline (104 → 104). The `transport_realism/`
      module and the diagnostic integration paths are clippy-clean.
- [x] AGENTS.md rules respected.
