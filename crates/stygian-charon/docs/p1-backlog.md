# Charon P1 Backlog — Advanced SLO Integration & Cross-Crate Validation

> Following completion of **Charon P0** (CHR-001 through CHR-017)
>
> **Phase Goal**: Extend SLO framework into production integrations, optimize performance, and validate against real-world scenarios.

---

## Overview

The P0 phase established the core SLO framework: target classification, blocked ratio assessment, requirement inference, and escalation logic. P1 focuses on integrating these foundations into cross-crate workflows, adding observability, and validating against realistic workloads.

### Success Criteria

- All cross-crate integration tests passing
- Telemetry/metrics exposed for SLO assessment
- Sub-millisecond investigation report generation for common cases
- Zero regressions against P0 test suite
- Live target validation framework available

---

## Proposed P1 Items

### CHR-011: Cross-Crate Integration Tests (stygian-charon ↔ stygian-browser)

**Objective**: Validate that SLO-driven escalation logic in `stygian-charon` correctly drives acquisition strategy changes in `stygian-browser`.

**Scope**:
- Create integration test in `crates/stygian-charon/tests/cross_crate_integration.rs`
- Test flow: HAR investigation → SLO assessment → RuntimePolicy escalation → AcquisitionRunner mode selection
- Verify escalation thresholds map to correct browser strategies (HTTP vs. Browser, sticky session enabling)
- Edge cases: boundary SLO conditions, unknown target class fallback

**Acceptance Criteria**:
- [ ] Test harness loads representative HARs (API, ContentSite, HighSecurity)
- [ ] Each target class escalates correctly at warning/critical thresholds
- [ ] Policy-to-acquisition mapping preserves SLO constraints
- [ ] Test suite runs in < 5s, passes with all features enabled

**Owner**: @greysquirr3l  
**Estimated Size**: 2-3 days  
**Dependencies**: P0 completion (CHR-007, CHR-009, CHR-010)

---

### CHR-012: Telemetry & Metrics Integration

**Objective**: Expose SLO assessment results and escalation decisions as structured metrics for monitoring and alerting.

**Scope**:
- Add optional `metrics` feature to `stygian-charon`
- Define metric types: `slo_assessment_count`, `escalation_triggered_count`, `blocked_ratio_histogram`
- Wire metrics into `investigate_har()` and `build_runtime_policy()` paths
- Provide examples: Prometheus integration, structured logging
- Document metric interpretation for on-call runbooks

**Acceptance Criteria**:
- [ ] `#[cfg(feature = "metrics")]` metrics collected without overhead when disabled
- [ ] At least 5 key metrics covering assessment, escalation, and ratio data
- [ ] Example Prometheus scrape config and dashboard JSON
- [ ] Metrics documented with units and interpretation guidance

**Owner**: @greysquirr3l  
**Estimated Size**: 2-3 days  
**Dependencies**: P0 completion, CHR-011

---

### CHR-013: Live Target SLO Validation Framework

**Objective**: Create a reusable validation harness for testing SLO assessment accuracy against real targets.

**Scope**:
- Build `crates/stygian-charon/examples/live_slo_validator.rs`
- Accept target URL + expected target class as CLI args
- Fetch HAR via `browser_acquire_and_extract` MCP tool or direct CDP
- Run SLO assessment and log findings: observed ratio, SLO zone, requirement escalation
- Optional: Store baseline for regression detection
- Feature-gate as `#[ignore]` test to prevent accidental live calls in CI

**Acceptance Criteria**:
- [ ] CLI tool accepts URL, target class, stealth level
- [ ] Outputs JSON with HAR, report, requirements, escalation details
- [ ] Validates against ≥3 real targets with known anti-bot profiles
- [ ] Baseline storage/diffing supports incremental regression checking

**Owner**: @greysquirr3l  
**Estimated Size**: 3-4 days  
**Dependencies**: P0 completion, CHR-011

---

### CHR-014: Performance Optimization: Investigation Report Caching

**Objective**: Reduce latency for repeated assessments of the same target by caching normalized investigation reports.

**Scope**:
- Add optional `caching` feature with `InvestigationReportCache` trait
- Implement `MemoryCache` (simple LRU) and optional `RedisCache` (requires redis feature)
- Cache key: hash of HAR content + target class
- TTL: configurable, default 5 minutes
- Invalidation: explicit clear or automatic on TTL expiry
- Benchmark: measure latency improvement for 1K/10K repeated assessments

**Acceptance Criteria**:
- [ ] Cache hits skip `investigate_har()` entirely
- [ ] LRU eviction prevents unbounded memory growth
- [ ] Redis backend functional with optional feature flag
- [ ] Benchmark shows ≥50% latency improvement for cache hits

**Owner**: @greysquirr3l  
**Estimated Size**: 2-3 days  
**Dependencies**: P0 completion

---

### CHR-015: Dynamic SLO Tuning Based on Historical Regression

**Objective**: Allow SLO thresholds to adapt based on observed patterns to reduce false positives/negatives.

**Scope**:
- Add `AdaptiveSloPolicy` trait allowing custom SLO selection logic
- Implement `RegressionHistoryPolicy`: tracks target-specific block ratios over time, adjusts thresholds
- Store historical data: URL → [blocked_ratio, timestamp, escalation_level]
- Prevent threshold drift: bounds for min/max adjustments
- Example: ContentSite target with 20% baseline ratio → raise acceptable threshold incrementally
- Warning: document trade-offs (safety vs. responsiveness)

**Acceptance Criteria**:
- [ ] `AdaptiveSloPolicy` trait allows pluggable SLO selection
- [ ] `RegressionHistoryPolicy` implementation tracks ≥10 targets
- [ ] Historical data persisted (JSON or SQLx backend)
- [ ] Unit tests verify threshold adjustments preserve SLO zones

**Owner**: @greysquirr3l  
**Estimated Size**: 3-4 days  
**Dependencies**: P0 completion, CHR-012 (metrics context helpful)

---

### CHR-016: Graph-Level SLO Application (stygian-graph Bridge)

**Objective**: Enable `stygian-graph` adapters to apply SLO-aware acquisition strategies when fetching via browser nodes.

**Scope**:
- Extend `stygian-graph` `browser` node config to accept optional `target_class` override
- Document: how graph users specify target classification (config vs. inference)
- Integrate: `infer_requirements_with_target_class()` call in graph acquisition bridge
- Example pipelines: API scrape with API class, HighSecurity banking site with HighSecurity class
- Validation: e2e test fetching ≥2 target types with different SLO expectations

**Acceptance Criteria**:
- [ ] Graph browser node config supports `target_class` field
- [ ] `acquisition` block in browser node applies SLO escalation
- [ ] Example configs for each target class in `examples/`
- [ ] E2E test validates escalation behavior across target classes

**Owner**: @greysquirr3l  
**Estimated Size**: 2-3 days  
**Dependencies**: P0 completion, T62 (graph acquisition bridge)

---

### CHR-017a: Incident Runbook Validation Tests

**Objective**: Ensure diagnostic procedures in the incident runbook are executable and effective.

**Scope**:
- Create `crates/stygian-charon/tests/runbook_diagnostics.rs` with test cases for each incident category
- Simulate Category A regression: DataDome markers missing → verify detection path
- Simulate Category B regression: blocked ratio spike → verify SLO assessment path
- Simulate Category C regression: preflight count increase → verify header analysis path
- Each test validates: detection pattern, diagnostic outputs, escalation recommendation

**Acceptance Criteria**:
- [ ] Test fixtures for each incident category (synthetic HARs)
- [ ] Diagnostic procedures execute without errors
- [ ] Output format matches runbook examples
- [ ] Tests guide on-call teams to correct resolution path

**Owner**: @greysquirr3l  
**Estimated Size**: 2-3 days  
**Dependencies**: CHR-017 (incident runbook completion)

---

### CHR-018: Comprehensive P0→P1 Integration Test Suite

**Objective**: Validate full workflows from HAR acquisition through policy escalation to acquisition runner execution.

**Scope**:
- Create end-to-end test combining: browser acquisition → HAR → investigation → SLO → escalation → runner execution
- Cover happy path: acceptable → warning → critical zone transitions
- Cover edge cases: mixed signals (high block ratio + 429s), boundary SLO conditions
- Include real-world scenarios: e-commerce site, banking portal, API endpoint
- Measure latency: full pipeline should complete in < 1s for typical HARs

**Acceptance Criteria**:
- [ ] E2E test runs in < 5s per scenario
- [ ] All three target classes validated in happy path
- [ ] At least 3 edge cases with explicit expectations
- [ ] Latency profiling shows < 1s per investigation

**Owner**: @greysquirr3l  
**Estimated Size**: 3-5 days  
**Dependencies**: CHR-011, CHR-012, CHR-016

---

## Proposed Timeline

| Phase | Items | Duration | Owner | Dependencies |
|-------|-------|----------|-------|--------------|
| Early | CHR-011, CHR-012 | Week 1 | @greysquirr3l | P0 completion |
| Mid | CHR-013, CHR-014, CHR-017a | Week 2 | @greysquirr3l | CHR-011 |
| Late | CHR-015, CHR-016, CHR-018 | Week 3 | @greysquirr3l | CHR-012, CHR-013 |

---

## Implementation Notes

### Testing Strategy

All P1 items should follow P0 patterns:
- Unit tests in `#[cfg(test)]` blocks within source modules
- Integration tests in `crates/stygian-charon/tests/`
- Live/online tests marked with `#[ignore]` to prevent CI blocking
- Strict clippy: `-D warnings`, no unwrap/panic in library code

### Feature Gating

Optional items (metrics, caching, adaptive policies) must be feature-gated:
- `metrics`: off by default, enable with `cargo test --features metrics`
- `caching`: off by default, enable with `--features caching`
- `redis`: off by default, for cache backends (depends on `caching`)

### Documentation

Each item should include:
- Usage examples in crate docs
- Integration guide linking from SLO usage guide
- Performance notes (latency, memory, throughput)
- Migration guide (if changing APIs)

---

## Success Metrics (Phase-Level)

By end of P1:
- [ ] All 8 items implemented and passing tests
- [ ] Zero clippy warnings (strict config)
- [ ] Investigation report latency ≤ 100ms for typical HARs (≤ 10ms with caching)
- [ ] Metrics exposed for all SLO assessment operations
- [ ] Live target validation framework available for on-call teams
- [ ] Cross-crate integration fully validated
- [ ] Graph bridge fully operational with SLO awareness

---

## Open Questions

1. **Regression History Storage**: JSON file vs. database (SQLx) vs. Redis? → *Proposal: Start with JSON for simplicity, add Redis option later*
2. **Adaptive Policy Safety**: How to prevent threshold drift from becoming a security liability? → *Proposal: Bounds + explicit validation in runbook*
3. **Live Validation Frequency**: How often should baseline targets be re-validated? → *Proposal: On-demand manual runs, not automated CI*
4. **Metrics Cardinality**: Risk of explosion with high-cardinality target URLs? → *Proposal: Hash URLs, limit series to top 100 by frequency*

---

## Related Documents

- [Charon P0 Backlog](./signal-coverage-matrix.md) — Completed foundation items
- [Incident Runbook](./incident-runbook.md) — Operational procedures for regressions
- [SLO Usage Guide](./slo-usage-guide.md) — How to apply SLOs to targets
- [Acquisition Runner Refactor Guide](../../../docs/acquisition-runner-refactor-guide.md) — Policy-to-runner mapping
