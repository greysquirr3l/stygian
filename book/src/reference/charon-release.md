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
