# stygian-charon Signal Coverage Matrix and Ownership

This document tracks all currently inferred anti-bot requirement signals in Charon.

Source of truth for current requirement IDs:

- `crates/stygian-charon/src/investigation.rs` in `infer_requirements`

## Coverage Matrix

| Signal ID | Source (code + telemetry) | Rationale | Current owner | Gap / follow-up |
| --- | --- | --- | --- | --- |
| `js_runtime_and_cookie_lifecycle` | `infer_requirements`: Cloudflare marker presence (`cf-ray`, `__cf_bm`, `cdn-cgi/challenge-platform`) from `top_markers` | Browser-like JS/session progression is typically required when challenge markers are present | `stygian-charon` maintainers (primary: `@greysquirr3l`) | Add explicit secondary owner for on-call continuity |
| `fingerprint_and_identity_consistency` | `infer_requirements`: DataDome marker presence (`x-datadome`, `x-dd-b`, `datadome=`, `captcha-delivery.com`) from `top_markers` | DataDome-style defenses are sensitive to stable header/cookie/connection identity | Shared: `stygian-charon` + `stygian-browser` maintainers | Ownership is shared but not formally codified in `CODEOWNERS` |
| `adaptive_rate_and_retry_budget` | `infer_requirements`: `blocked_ratio >= 0.10` from report totals | Elevated block ratio often indicates aggressive pacing/retry behavior | `stygian-charon` maintainers | Needs explicit SLO for acceptable blocked ratio by target class |
| `rate_limit_backoff` | `infer_requirements`: `status_429 > 0` from `status_histogram` | HTTP 429 indicates explicit throttling pressure and backoff requirements | `stygian-charon` maintainers | Need runbook link for incident response when sustained 429 is detected |
| `cors_and_header_fidelity` | `infer_requirements`: `preflight` count from `resource_type_histogram` | Preflight-heavy flows can fail when request choreography diverges from browser behavior | Shared: `stygian-charon` + runtime adapter maintainers | Runtime adapter owner is not documented per crate/component |

## Ownership Gaps and Unknowns

- No explicit `CODEOWNERS` mapping currently ties individual requirement IDs to named owners.
- Shared-surface signals (`fingerprint_and_identity_consistency`, `cors_and_header_fidelity`) do not have a single incident commander assignment.
- Escalation path for requirement-regression incidents is not linked from this document yet.

## Minimum Follow-up Actions

1. Add a `CODEOWNERS` entry for `crates/stygian-charon/src/investigation.rs` and this matrix.
2. Record a primary and secondary owner for shared-surface signals.
3. Link this matrix to the stealth/regression incident runbook once CHR-017 is completed.
