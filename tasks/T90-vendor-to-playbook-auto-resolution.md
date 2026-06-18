# T90 — Vendor-to-Playbook Auto-Resolution

## Goal

Resolve detected anti-bot vendor profile to an actionable playbook automatically, including mixed-vendor cases.

## Scope

- Add resolver from vendor-classifier output to playbook defaults.
- Implement precedence and merge rules for multiple detected vendors.
- Surface final selected strategy with rationale in diagnostics.

## Feature flag

Default-on. New module `vendor_resolver` lives in `stygian-charon`
(under `crates/stygian-charon/src/vendor_resolver/`) since it bridges
the classifier (T89) and playbooks (T85). Resolution rules ship as
data in `crates/stygian-charon/data/vendor_playbook_rules/`.

If the resolver adds a new charon config field, add a
`vendor-resolver` feature gate and wire it into `full`. Otherwise,
additive only.

## Depends on

- T89 (vendor classifier).
- T85 (target-class playbooks).

## Informs

- T83 (challenge-aware feedback) — challenge outcomes can adjust
  resolution confidence but T83 is not a hard dep.

## Unblocks

- T92 (JS integrity trap canary) — selects correct challenge path.
- T93 (PoW capability profile) — selects correct PoW path.
- T94 (queue/interstitial routing) — selects correct routing path.

## Must Haves

- Deterministic resolution rules for single and multi-vendor outcomes.
- Explicit fallback path when classifier confidence is low.
- Non-breaking integration with existing manual mode selection.

## Test Hints

- Unit: precedence and merge behavior for conflicting vendor
  recommendations.
- Unit: low-confidence fallback path (verify manual mode is preserved).
- Integration: resolver chooses expected playbook for synthetic
  vendor signatures (may be `#[ignore]`).

## Exit Criteria

- [x] `VendorResolver` consumes `VendorClassifier` output and returns
      a resolved `Playbook` reference plus a rationale bundle.
- [x] Multi-vendor precedence and merge rules are documented in
      module rustdoc and shipped as data.
- [x] Low-confidence fallback returns a `Manual` strategy marker so
      existing manual mode selection continues to work.
- [x] At least 3 unit tests for single-vendor, multi-vendor, and
      low-confidence cases.
- [x] At least 1 `#[ignore]` integration test for synthetic vendor
      signatures mapping to expected playbooks.
- [x] Docs updated: module rustdoc + resolution rule table.
- [x] `cargo build --workspace --all-features` clean.
- [x] `cargo test --workspace --all-features` clean.
- [x] `cargo clippy -p stygian-charon --all-features --all-targets -- -D warnings` clean (0 errors vs 0 baseline).
- [x] AGENTS.md rules respected.
