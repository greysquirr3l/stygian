# Charon Overview

`stygian-charon` is the diagnostics and planning crate for anti-bot operations in the Stygian
stack. It converts request/response evidence into reproducible assessments and actionable runtime
acquisition guidance.

---

## What Charon provides

| Capability | Outcome |
| --- | --- |
| HAR forensics | Convert HAR payloads into normalized anti-bot reports |
| Provider classification | Detect likely providers (for example Cloudflare/DataDome patterns) |
| Target-aware SLO assessment | Score blocked-ratio health by target class |
| Runtime policy planning | Produce retry, warmup, and escalation guidance |
| Acquisition mapping | Translate policy into acquisition mode hints |
| Snapshot drift checks | Validate and compare normalized fingerprint snapshots |

---

## Target classes

Charon supports target-aware behavior so the same blocked ratio can be interpreted differently
for different systems:

- `Api`
- `ContentSite`
- `HighSecurity`

Use these classes when inferring SLO requirements and selecting escalation posture.

---

## Feature flags

| Feature | Purpose |
| --- | --- |
| `metrics` | Emit counters and blocked-ratio aggregates for observability |
| `caching` | Enable in-memory caching for repeated investigations |
| `redis-cache` | Add Redis-backed caching on top of `caching` |
| `live-validation` | Enable live URL validation example tooling |

---

## Core workflow

1. Investigate HAR evidence into a normalized report.
2. Infer requirements for the target class and current SLO posture.
3. Build a runtime policy from observed behavior.
4. Map that policy into acquisition hints for graph runners/adapters.

This keeps diagnostics deterministic and portable across teams and environments.

---

## Next steps

- Continue with [Getting Started](./getting-started.md).
- Review [SLO & Policy Planning](./slo-policy.md).
- Use [Operations & Runbooks](./operations.md) for rollout and incident response.
