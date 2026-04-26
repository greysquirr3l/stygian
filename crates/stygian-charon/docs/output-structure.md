# stygian-charon Output Structure

This document specifies the full output contract for Charon's analysis and planning pipeline.

## Primary entrypoint

- Function: `analyze_and_plan(har_json: &str) -> Result<InvestigationBundle, HarError>`
- Module: `src/policy.rs`

`InvestigationBundle` is the canonical top-level output.

## Normalized fingerprint snapshots

Charon also defines a versioned normalized fingerprint snapshot schema for
cross-mode compatibility checks.

- Schema: `docs/normalized-fingerprint-snapshot.schema.json`
- Compatible examples:
  - `docs/examples/fingerprint-snapshot-v1-http.json`
  - `docs/examples/fingerprint-snapshot-v1-browser.json`

Required top-level fields:

- `schema_version`
- `snapshot_id`
- `mode`
- `captured_at`
- `signals`

Optional top-level fields:

- `metadata`

Deprecated top-level fields:

- `legacy_user_agent` (deprecated mirror of `signals.user_agent`)
- `legacy_ja3_hash` (deprecated mirror of `signals.tls.ja3_hash`)

Canonical ordering and type constraints:

- Canonical key order is defined in schema metadata (`x-canonical-order`)
  for top-level and nested objects.
- Type constraints use JSON Schema definitions and conditional rules:
  - `mode = Http` requires `signals.tls`
  - `mode = Browser` requires `signals.webgl`
- Snapshot compatibility checks are implemented in
  `src/snapshot.rs::validate_snapshot_compatibility`.

## Top-level type

### InvestigationBundle

Fields:

- `report: InvestigationReport`
- `requirements: RequirementsProfile`
- `policy: RuntimePolicy`

---

## Report layer

### InvestigationReport

Raw and aggregated telemetry from HAR parsing plus provider classification.

Fields:

- `page_title: Option<String>`
- `total_requests: u64`
- `blocked_requests: u64`
- `status_histogram: BTreeMap<u16, u64>`
- `resource_type_histogram: BTreeMap<String, u64>`
- `provider_histogram: BTreeMap<AntiBotProvider, u64>`
- `top_markers: Vec<MarkerCount>`
- `hosts: Vec<HostSummary>`
- `suspicious_requests: Vec<HarRequestSummary>`
- `aggregate: Detection`

Operational semantics:

- `blocked_requests` counts HTTP 403 and 429.
- `top_markers` is sorted by descending frequency and truncated to a bounded list.
- `suspicious_requests` includes blocked requests and requests with recognized provider signals.

### HarRequestSummary

Per-request projection used in both report and suspicious subsets.

Fields:

- `url: String`
- `status: u16`
- `resource_type: Option<String>`
- `detection: Detection`

### Detection

Provider detection output.

Fields:

- `provider: AntiBotProvider`
- `confidence: f64` in `[0.0, 1.0]`
- `markers: Vec<String>`

### MarkerCount

Fields:

- `marker: String`
- `count: u64`

### HostSummary

Fields:

- `host: String`
- `total_requests: u64`
- `blocked_requests: u64`

---

## Requirements layer

Coverage and ownership for inferred requirement signals is tracked in:

- `docs/signal-coverage-matrix.md`

### RequirementsProfile

Inferred operational requirements and strategic recommendation.

Fields:

- `provider: AntiBotProvider`
- `confidence: f64`
- `requirements: Vec<AntiBotRequirement>`
- `recommendation: IntegrationRecommendation`

### AntiBotRequirement

Fields:

- `id: String`
- `title: String`
- `why: String`
- `evidence: Vec<String>`
- `level: RequirementLevel`

### RequirementLevel

Enum values:

- `Low`
- `Medium`
- `High`

### IntegrationRecommendation

Fields:

- `strategy: AdapterStrategy`
- `rationale: String`
- `required_stygian_features: Vec<String>`
- `config_hints: BTreeMap<String, String>`

### AdapterStrategy

Enum values:

- `DirectHttp`
- `BrowserStealth`
- `StickyProxy`
- `SessionWarmup`
- `InvestigateOnly`

---

## Policy layer

### RuntimePolicy

Concrete planning output intended for adapter/runtime wiring.

Fields:

- `execution_mode: ExecutionMode`
- `session_mode: SessionMode`
- `telemetry_level: TelemetryLevel`
- `rate_limit_rps: f64`
- `max_retries: u32`
- `backoff_base_ms: u64`
- `enable_warmup: bool`
- `enforce_webrtc_proxy_only: bool`
- `sticky_session_ttl_secs: Option<u64>`
- `required_stygian_features: Vec<String>`
- `config_hints: BTreeMap<String, String>`
- `risk_score: f64` in `[0.0, 1.0]`

### ExecutionMode

Enum values:

- `Http`
- `Browser`

### SessionMode

Enum values:

- `Stateless`
- `Sticky`

### TelemetryLevel

Enum values:

- `Basic`
- `Standard`
- `Deep`

---

## Diff output (related API)

`compare_reports` returns `InvestigationDiff`.

### InvestigationDiff

Fields:

- `baseline_total_requests: u64`
- `candidate_total_requests: u64`
- `baseline_blocked_requests: u64`
- `candidate_blocked_requests: u64`
- `blocked_ratio_delta: f64`
- `likely_regression: bool`
- `provider_delta: BTreeMap<AntiBotProvider, i64>`
- `new_markers: Vec<String>`

---

## Serialization details

All output types derive `Serialize` and `Deserialize`.

Notes for JSON consumers:

- Enum values are serialized as strings matching Rust variant names.
- Map keys are JSON strings.
  - For numeric-key maps (for example `status_histogram`), keys appear as strings in JSON objects.
  - For enum-key maps (for example `provider_histogram`), keys appear as enum variant strings.
- Optional fields are nullable in schema terms.
