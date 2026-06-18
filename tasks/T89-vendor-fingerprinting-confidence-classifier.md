# T89 — Vendor Fingerprinting Confidence Classifier

## Goal

Automatically identify likely anti-bot vendor(s) for a target and provide confidence-scored evidence for policy routing.

## Scope

- Add classifier inputs from response cookies, headers, challenge URLs, and body markers.
- Support multi-vendor detection with ranked confidence output.
- Emit evidence bundle suitable for diagnostics and runbook use.

## Feature flag

Default-on. New module `vendor_classifier` lives in `stygian-charon`
(under `crates/stygian-charon/src/vendor_classifier/`) since it feeds
policy routing. Vendor definitions ship as a data file (JSON/TOML) in
`crates/stygian-charon/data/vendors/`.

If the classifier adds a new public charon type that breaks downstream
callers, add a `vendor-classifier` feature gate and wire it into
`full`. Otherwise, additive only.

## Depends on

- T83 (challenge-aware feedback) — challenge outcomes sharpen
  classification.
- T85 (target-class playbooks) — playbook taxonomy informs vendor
  resolution.

## Unblocks

- T90 (vendor-to-playbook auto-resolution).
- T88 (anti-bot change detection feed) — vendor attribution improves
  change-event diagnostics.

## Must Haves

- Stable taxonomy for vendor identifiers and detection signals.
- Deterministic confidence scoring with configurable thresholds.
- Explainable output listing matched signals by source.

## Test Hints

- Unit: confidence scoring for known cookie/header/body combinations.
- Unit: multi-vendor ranking logic and tie handling.
- Integration: classifier output appears in diagnostics payload
  (may be `#[ignore]`).

## Exit Criteria

- [x] `VendorId` enum or newtype covering at least the Tier 1 vendors
      (DataDome, PerimeterX/HUMAN, Akamai Bot Manager, Cloudflare).
- [x] `VendorClassifier` consumes cookies, headers, challenge URLs,
      and body markers, with signal sources labeled in the evidence
      bundle.
- [x] Multi-vendor detection with ranked confidence (0.0–1.0) and a
      deterministic tie-break rule (documented).
- [x] Configurable confidence threshold for "high confidence" routing.
- [x] At least 4 unit tests covering single-vendor, multi-vendor,
      tie, and below-threshold cases.
- [x] At least 1 `#[ignore]` integration test confirming the
      classifier output appears in diagnostics payload.
- [x] Docs updated: module rustdoc + vendor taxonomy table.
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [!] `cargo clippy --workspace --all-features -- -D warnings` clean. Per-crate
      preflight on the touched crate (`stygian-charon`) is 0 vs 0 baseline
      (mandatory gate). Full-workspace clippy has 104 pre-existing errors
      in `stygian-browser` (`must_use_candidate`, `missing_errors_doc`,
      `struct_excessive_bools`) — same baseline as before T89. T95
      (workspace clippy baseline cleanup) is the tracked task that
      re-enables the full-workspace preflight; until then the per-crate
      preflight is the authoritative gate per `ORCHESTRATOR.md` and
      `PROGRESS.md` (2026-06-17).
- [x] AGENTS.md rules respected.
