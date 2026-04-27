# Stealth & Regression Incident Runbook

> **Incident Commanders**: @greysquirr3l (primary), @stygian-charon-on-call (secondary)
>
> This runbook provides detection patterns, escalation paths, and diagnostic procedures for stealth and anti-bot regression incidents.

---

## Quick Reference: Signal Regression Map

| Signal ID | Detection Pattern | Typical Root Cause | Escalation Level | Owner |
| --- | --- | --- | --- | --- |
| `js_runtime_and_cookie_lifecycle` | Cloudflare challenge markers missing/inconsistent from HAR | Browser session lifecycle broken, JS disabled, or CDP leak | **P0** | @greysquirr3l |
| `fingerprint_and_identity_consistency` | DataDome markers absent or inconsistent identity fields | Fingerprint coherence broken, WebRTC leak, or navigator mismatch | **P0** | @greysquirr3l (charon) + @stygian-charon-on-call (browser) |
| `adaptive_rate_and_retry_budget` | Blocked ratio SLO assessment changes from acceptable→warning or warning→critical | Rate limiting increased, IP reputation dropped, or policy escalation failed | **P1** | @greysquirr3l |
| `rate_limit_backoff` | Sudden spike in HTTP 429 responses where previously absent | Pacing algorithm degraded, proxy pool exhausted, or target hardened defenses | **P1** | @greysquirr3l |
| `cors_and_header_fidelity` | Preflight failures or `Access-Control` errors in HAR entries | Request choreography diverged from browser behavior or header spoofing broken | **P1** | @greysquirr3l (charon) + graph adapters team |

---

## Incident Categories

### Category A: Fingerprint/Identity Regression

**Signals affected**: `fingerprint_and_identity_consistency`, `js_runtime_and_cookie_lifecycle`

**Detection**: 
- DataDome markers (`x-datadome`, `x-dd-b`, `captcha-delivery.com`) no longer appear in investigation reports
- HTTP status shifts from 200/206 to 403/401 for previously passing URLs
- WebRTC IP leaks detected in browser stealth validation tests

**Root cause candidates**:
1. Browser fingerprint injection disabled or broken
   - Check: `stygian-browser` fingerprint generation pipeline
   - Check: CDP binding injection (Runtime.AddBinding) for WebGL/Canvas overrides
2. Session identity mismatch (cookies/headers)
   - Check: Cookie jar consistency between requests
   - Check: User-Agent, Accept-Language alignment with fingerprint profile
3. WebRTC leak exposing real IP
   - Check: WebRTC IP proxy enforcement in `stygian-browser` session config
   - Check: IP geolocation alignment with proxy

**Escalation procedure**:
1. **Immediate** (first 15 min):
   - Run diagnostic: `browser_stealth_validate` MCP tool against target URL
   - Capture full HAR and stealth report
   - Check if regression is target-specific or systematic
   
2. **Investigation** (15-60 min):
   - If `browser_stealth_validate` passes: fingerprint/identity issue is stealth-layer isolated
     - Escalate to stygian-browser team (@stygian-charon-on-call)
     - Provide HAR, failing URL, stealth report snapshot
   - If `browser_stealth_validate` fails on basic properties (navigator, User-Agent):
     - Check for CDP mode conflicts (cdp_source vs. browser_source)
     - Check FingerprintProfile selection logic
     - Escalate to browser stealth specialist
   
3. **Resolution** (60+ min):
   - Browser team patches fingerprint injection or CDP binding
   - Validate with `browser_stealth_validate` before rollback
   - Update learnings to downstream acquisition/extraction code

**Validation test**:
```bash
# Quick validation: Check fingerprint coherence
cargo test -p stygian-browser --test browser_stealth -- --nocapture 2>&1 | grep "Tier1\|PASS\|FAIL"

# Full diagnostic: Run stealth benchmark
cargo run --example stealth_benchmark -- --target https://example.com --profile advanced 2>&1 | jq '.tier1_results'
```

---

### Category B: Rate Limiting & Backoff Regression

**Signals affected**: `adaptive_rate_and_retry_budget`, `rate_limit_backoff`

**Detection**:
- Blocked ratio (from `investigate_har()`) exceeds SLO thresholds for target class
- HTTP 429 responses increase suddenly (where previously at 0%)
- SLO assessment shifts from acceptable→warning/critical

**Root cause candidates**:
1. Acquisition pacing degraded
   - Check: Rate limit RPS setting in RuntimePolicy
   - Check: Backoff base milliseconds and multiplier in escalation logic
2. Proxy pool insufficient or unhealthy
   - Check: Proxy pool health metrics (availability, response time)
   - Check: IP reputation drops (too many URLs from same IP)
3. Target hardened defenses
   - Check: User-Agent rotation enabled and working
   - Check: Request delay distribution (should be human-like)
   - Check: TLS fingerprint alignment with user-agent

**Escalation procedure**:
1. **Immediate** (first 10 min):
   - Check blocked_ratio in latest investigation report
   - Run `infer_requirements_with_target_class(report, target_class)` to assess SLO level
   - If critical: apply emergency escalation (reduce RPS, enable sticky session)
   
2. **Investigation** (10-45 min):
   - Collect metrics: blocked_ratio, 429_count, response_time_p95
   - If target_class = HighSecurity and blocked_ratio >= 0.30:
     - Escalate to browser team: fingerprint coherence needed
     - Check WebRTC, navigator, canvas noise injection
   - If target_class = ContentSite and blocked_ratio is new:
     - Check proxy pool health and IP reputation
     - Check request delays for human-like distribution
   
3. **Resolution** (45+ min):
   - If proxy root cause: rotate to fresh IP pool or add whitelist delay
   - If pacing root cause: apply SLO-based escalation settings from policy.rs
   - If fingerprint root cause: coordinate with browser team for coherence fix

**Validation test**:
```bash
# Run SLO assessment test
cargo test -p stygian-charon --test slo_integration -- --nocapture 2>&1

# Check escalation logic
cargo test -p stygian-charon slo_assessment_for_each_target_class -- --nocapture 2>&1
```

---

### Category C: Preflight/Header Fidelity Regression

**Signals affected**: `cors_and_header_fidelity`

**Detection**:
- Preflight request count increases in HAR entries
- `Access-Control-Allow-*` headers missing or mismatched
- Simple POST requests fail (should be preflight-free)

**Root cause candidates**:
1. Request header spoofing broken
   - Check: User-Agent, Referer, Origin headers match fingerprint profile
   - Check: Accept, Accept-Language, Accept-Encoding consistency
2. Request choreography diverged from browser
   - Check: Cookie order and Path/Domain constraints
   - Check: Conditional request headers (If-Modified-Since, ETag)
3. Graph adapter misconfiguration
   - Check: HTTP adapter request builder (headers.rs)
   - Check: Middleware layer stripping or reordering headers

**Escalation procedure**:
1. **Immediate** (first 10 min):
   - Capture full HAR and identify preflight entries
   - Check if target requires special CORS handling (old API versions, JSONP fallbacks)
   
2. **Investigation** (10-40 min):
   - Run `investigate_har(har_content).unwrap()` and check `preflight` count
   - If preflight > 0 and request is simple (GET/POST with standard headers):
     - Check header generation in request builder
     - Compare HAR request headers with browser default behavior
   - If all graph adapters affected: escalate to graph team
   - If adapter-specific: check adapter request builder
   
3. **Resolution** (40+ min):
   - Update header spoofing rules or browser profile
   - Validate with `investigate_har()` preflight count returning to baseline
   - Add regression test for affected URL pattern

**Validation test**:
```bash
# Quick header fidelity check
cargo test -p stygian-charon infer_requirements -- --nocapture 2>&1 | grep "cors_and_header_fidelity\|PASS"
```

---

## Diagnostic Procedures

### Step 1: Collect Investigation Report

Start with a HAR capture from the failing target:

```bash
# Via MCP (if using stygian-graph or hosted agent):
tools:
  - name: "browser_acquire_and_extract"
    params:
      url: "https://target-url.com/path"
      mode: "investigate"
      output_shape: "investigation_report"

# Locally via Rust:
let har_content = /* HAR JSON string from browser capture */;
let report = stygian_charon::investigate_har(&har_content)
    .expect("investigation failed");
println!("{}", serde_json::to_string_pretty(&report).unwrap());
```

### Step 2: Run Requirement Inference

Determine which signals are triggering:

```rust
use stygian_charon::{infer_requirements_with_target_class, TargetClass};

let requirements = infer_requirements_with_target_class(&report, TargetClass::HighSecurity);
println!("Requirements:");
for req in requirements.requirements {
    println!("  - {}: {} (level={})", 
        req.id, req.why, req.level);
}
```

### Step 3: Assess SLO Status

Check if blocked ratio is in acceptable/warning/critical zone:

```rust
use stygian_charon::{BlockedRatioSlo, TargetClass};

let slo = BlockedRatioSlo::for_class(TargetClass::HighSecurity);
let blocked_ratio = report.blocked_requests as f64 / report.total_requests as f64;
let (acceptable, warning, critical) = slo.assess(blocked_ratio);

println!("Blocked ratio: {:.2}%", blocked_ratio * 100.0);
println!("Acceptable: {}, Warning: {}, Critical: {}", 
    acceptable, warning, critical);
```

### Step 4: Check Marker Presence

Identify which anti-bot systems are active:

```rust
// Check investigation report directly
println!("Top markers (anti-bot signals):");
for (marker, count) in &report.top_markers {
    println!("  - {}: {} occurrences", marker, count);
}

// Examples:
// - Cloudflare: "cf-ray", "__cf_bm", "challenge-platform"
// - DataDome: "x-datadome", "x-dd-b", "datadome="
// - Generic: "403 Forbidden", "429 Too Many Requests"
```

### Step 5: Validate Stealth Configuration

If fingerprint regression suspected, run stealth validation:

```bash
# Via MCP tool (requires stygian-graph):
tools:
  - name: "browser_stealth_validate"
    params:
      url: "https://target-url.com"
      stealth_level: "advanced"
      expected_properties:
        - "navigator.webdriver"
        - "navigator.plugins"
        - "canvas.fingerprint"

# Locally via test:
cargo test -p stygian-browser stealth_validation -- --nocapture
```

---

## Escalation Matrix

### P0 Incidents (Immediate Response Required)

| Category | Signal | Action | Owner | SLA |
| --- | --- | --- | --- | --- |
| Fingerprint/Identity | `fingerprint_and_identity_consistency` | Page 0 (Slack + SMS) | @greysquirr3l + @stygian-charon-on-call | 15 min acknowledgment |
| Fingerprint/Identity | `js_runtime_and_cookie_lifecycle` | Page 0 (Slack + SMS) | @greysquirr3l | 15 min acknowledgment |

**Immediate actions**:
1. Acknowledge in #stygian-incidents channel
2. Collect HAR and investigation report
3. Run `browser_stealth_validate` to isolate scope
4. Create incident thread with findings
5. Assign secondary investigator

### P1 Incidents (Expedited Response)

| Category | Signal | Action | Owner | SLA |
| --- | --- | --- | --- | --- |
| Rate Limiting | `adaptive_rate_and_retry_budget` | Page 1 (Slack) | @greysquirr3l | 30 min acknowledgment |
| Rate Limiting | `rate_limit_backoff` | Page 1 (Slack) | @greysquirr3l | 30 min acknowledgment |
| Preflight/Headers | `cors_and_header_fidelity` | Slack + triage meeting | @greysquirr3l + graph team | 60 min acknowledgment |

**Immediate actions**:
1. Post in #stygian-incidents with signal ID and blocking_ratio/429_count
2. Run `infer_requirements_with_target_class()` to confirm SLO level
3. Apply temporary escalation if critical SLO zone (reduce RPS, enable sticky session)
4. Investigate root cause in parallel

---

## Post-Incident: Learning & Prevention

After any regression is resolved:

1. **Update the Coverage Matrix**
   - If detection pattern was unclear, improve signal description
   - If escalation path was inefficient, update this runbook

2. **Add Regression Test**
   - Capture HAR that triggered the regression
   - Add test case to `crates/stygian-charon/tests/slo_integration.rs`
   - Ensure test fails before fix, passes after

3. **Document in CHANGELOG**
   - Link PR or commit that fixed the regression
   - Briefly describe detection pattern that would catch similar issues

4. **Update CODEOWNERS**
   - If a new shared-surface signal emerges, add explicit owner assignment

---

## Key Contacts

| Role | Primary | Secondary | Escalation |
| --- | --- | --- | --- |
| Charon Lead | @greysquirr3l | @stygian-charon-on-call | #stygian-incidents |
| Browser/Stealth | @stygian-charon-on-call | @greysquirr3l | #stygian-incidents |
| Graph/Adapters | graph-adapters-team | @greysquirr3l | #stygian-incidents |
| On-Call Duty | @stygian-charon-on-call | (rotates weekly) | #stygian-incidents |

---

## Related Documents

- [Signal Coverage Matrix](./signal-coverage-matrix.md) — Detailed ownership and detection patterns for each signal
- [CODEOWNERS](.github/CODEOWNERS) — Code ownership and review authority
- [Architecture](../../../docs/architecture.md) — Hexagonal architecture and data flow
- [Performance Guide](../../../docs/performance.md) — Optimization guidance for throughput/latency
