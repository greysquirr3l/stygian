# Charon Release Notes

`stygian-charon` is now part of the published Stygian documentation set.

This crate adds anti-bot diagnostics and policy-planning workflows to the ecosystem, bridging raw
network evidence with acquisition decisions that can be consumed by runners and operators.

---

## What shipped

| Area | Summary |
| --- | --- |
| HAR investigation | Normalizes HAR evidence into reusable reports |
| Classification | Detects likely anti-bot providers from transaction signals |
| SLO assessment | Evaluates blocked-ratio health by target class |
| Policy planning | Produces retry, warmup, and escalation recommendations |
| Acquisition mapping | Converts policy into runtime acquisition hints |
| Snapshot drift | Validates and compares normalized identity snapshots |

---

## Per-release changelog

### 0.14.0 (2026-06-19)

The 0.14.0 cut shipped **only a clippy-baseline cleanup** for
`stygian-charon` — no new public API. The `token_lifecycle` module now
sits at the same zero-warning bar as the rest of the workspace under
the strict pre-push clippy profile.

The CodeQL `rust/hard-coded-cryptographic-value` alert that was
silenced on 13 deterministic test-label callsites in
`crates/stygian-charon/src/token_lifecycle/` is a **false positive on
test-fixture labels**. Production token-lifecycle nonces remain
server-issued random material at runtime — the suppression comments
document the rationale at each callsite.

No migration action required for consumers upgrading from 0.13.x.

### 0.13.5 (2026-05-26)

Initial published release of `stygian-charon` as part of the Stygian
documentation set. See the crate-level rustdoc for the full type
catalogue and the [Charon overview](../charon/overview.md) chapter for
the workflow.

---

## Start here

- [Charon Overview](../charon/overview.md)
- [Getting Started](../charon/getting-started.md)
- [SLO & Policy Planning](../charon/slo-policy.md)
- [Operations & Runbooks](../charon/operations.md)

---

## Release readiness checklist

1. Verify `cargo test -p stygian-charon --all-features` is green.
2. Verify `cargo clippy -p stygian-charon --all-features --examples --tests -- -D warnings` is green.
3. Confirm feature-flag documentation matches deployment configuration.
4. Review incident/runbook guidance before rolling out changes to production workflows.

---

## Related crate docs

Charon complements the rest of the Stygian stack:

- `stygian-graph` executes acquisition pipelines.
- `stygian-browser` handles browser-based acquisition.
- `stygian-proxy` manages rotation and sticky session behavior.
- `stygian-charon` analyzes evidence and recommends how those systems should behave.
