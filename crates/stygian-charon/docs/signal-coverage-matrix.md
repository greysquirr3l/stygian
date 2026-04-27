# stygian-charon Signal Coverage Matrix and Ownership

This document tracks all currently inferred anti-bot requirement signals in Charon.

Source of truth for current requirement IDs:

- `crates/stygian-charon/src/investigation.rs` in `infer_requirements`

## Coverage Matrix

| Signal ID | Source (code + telemetry) | Rationale | Current owner | Gap / follow-up |
| --- | --- | --- | --- | --- |
| `js_runtime_and_cookie_lifecycle` | `infer_requirements`: Cloudflare marker presence (`cf-ray`, `__cf_bm`, `cdn-cgi/challenge-platform`) from `top_markers` | Browser-like JS/session progression is typically required when challenge markers are present | Primary: `@greysquirr3l` / Secondary: `@stygian-charon-on-call` | ✅ Codified in `.github/CODEOWNERS` (CHR-006) |
| `fingerprint_and_identity_consistency` | `infer_requirements`: DataDome marker presence (`x-datadome`, `x-dd-b`, `datadome=`, `captcha-delivery.com`) from `top_markers` | DataDome-style defenses are sensitive to stable header/cookie/connection identity | Primary: `@greysquirr3l` (charon) / Secondary: `@stygian-charon-on-call` (browser) | ✅ Codified in `.github/CODEOWNERS` (CHR-006); incident runbook link pending (CHR-017) |
| `adaptive_rate_and_retry_budget` | `infer_requirements`: `blocked_ratio >= 0.10` from report totals | Elevated block ratio often indicates aggressive pacing/retry behavior | Primary: `@greysquirr3l` / Secondary: `@stygian-charon-on-call` | ✅ SLO thresholds defined by target class (CHR-007) |
| `rate_limit_backoff` | `infer_requirements`: `status_429 > 0` from `status_histogram` | HTTP 429 indicates explicit throttling pressure and backoff requirements | Primary: `@greysquirr3l` / Secondary: `@stygian-charon-on-call` | Runbook link pending (CHR-017) |
| `cors_and_header_fidelity` | `infer_requirements`: `preflight` count from `resource_type_histogram` | Preflight-heavy flows can fail when request choreography diverges from browser behavior | Primary: `@greysquirr3l` (charon) / Secondary: graph adapters team | ✅ Codified in `.github/CODEOWNERS` (CHR-006) |

## Ownership Gaps and Unknowns

- ✅ **RESOLVED (CHR-006)**: Explicit `CODEOWNERS` mapping now ties all Charon signals to primary (@greysquirr3l) and secondary (@stygian-charon-on-call) owners.
- ✅ **RESOLVED (CHR-006)**: Shared-surface signals (`fingerprint_and_identity_consistency`, `cors_and_header_fidelity`) now have explicit incident commander assignments.
- ⏳ **PENDING (CHR-017)**: Escalation path for requirement-regression incidents is not yet linked from this document.

## Minimum Follow-up Actions

1. ✅ Add a `CODEOWNERS` entry for `crates/stygian-charon/src/investigation.rs` and this matrix. (**CHR-006 — Completed**)
2. ✅ Record a primary and secondary owner for shared-surface signals. (**CHR-006 — Completed**)
3. ⏳ Link this matrix to the stealth/regression incident runbook once CHR-017 is completed. (**CHR-017 — Pending**)

## Charon P0 Backlog Progress

- ✅ **CHR-001**: Signal coverage matrix and ownership
- ✅ **CHR-002**: Normalized fingerprint snapshot schema and compatibility checks
- ✅ **CHR-003**: Deterministic snapshot collector
- ✅ **CHR-004**: Baseline fixture generation workflow
- ✅ **CHR-005**: Snapshot drift test suite
- ✅ **CHR-006**: CODEOWNERS codification and explicit ownership
- ✅ **CHR-007**: Blocked ratio SLOs by target class (API, ContentSite, HighSecurity)
- ✅ **CHR-008**: SLO usage documentation and integration tests
- ⏳ **CHR-017**: Stealth/regression incident runbook (pending)
