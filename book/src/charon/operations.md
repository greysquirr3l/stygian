# Operations & Runbooks

This section collects the release-day and production references for `stygian-charon`.

---

## Test and validation commands

```bash
# Crate tests with all features
cargo test -p stygian-charon --all-features

# Strict linting for the crate
cargo clippy -p stygian-charon --all-features --examples --tests -- -D warnings
```

Optional integration checks:

- Redis cache integration test requires `STYGIAN_REDIS_URL`.
- Live target validation smoke test requires `STYGIAN_LIVE_URL`.

---

## Diagnostic and integration guides

The crate ships additional guides under `crates/stygian-charon/docs/`:

- `caching-integration-guide.md`
- `metrics-integration-guide.md`
- `slo-usage-guide.md`
- `output-structure.md`
- `signal-coverage-matrix.md`
- `incident-runbook.md`

Use these during rollout planning and incident triage to keep operator behavior consistent.

---

## Suggested release checklist

1. Run crate tests and clippy with all release features enabled.
2. Verify fixture drift checks are green.
3. Confirm docs for metrics/caching match enabled feature flags in deployment manifests.
4. Validate that runbook references map to current alerting and on-call workflows.

---

## Where Charon fits

Charon is a diagnostics-and-guidance component. It does not replace execution adapters.

- Use `stygian-graph` to run pipelines.
- Use `stygian-browser` / `stygian-proxy` for acquisition execution.
- Use Charon output to choose and tune those execution strategies.

---

## Cloaking detector — `ContentTypeShiftDetector` (T104)

A publisher that serves a clean `200 OK` to ordinary browsers but
hands AI-bot UAs an HTML-stripped Markdown stub at the same URL is
**cloaking** — the response shape silently changes between user
agents. The cloaked response is a valid HTTP success that scrapers
will happily parse, but the data is different (or empty). This is
the
[Web Scraping Guide §Post-extract](https://web-scraping-guide.com/#post-extract)
"silent 200" failure mode.

`stygian_charon::ContentTypeShiftDetector` is the consumer-owned port
that flags cloaking. `RollingBaselineDetector` is the default
adapter; it tracks a rolling baseline of the target's
`(content-type, byte-count)` over the last N fetches and emits a
cloaking signal when either:

- the **MIME class** shifts (e.g. `text/html` → `text/markdown` or
  `application/octet-stream`), or
- the **byte count** collapses (response shrinks by more than a
  configurable threshold against the baseline median).

The detector is conservative: a one-off shift is treated as noise
(blended into the rolling baseline) but a sustained shift triggers
the signal. Wire it into your pipeline by calling
`detector.observe(&fetch_outcome)` after every successful fetch and
quarantining the target on the first cloaking signal — the publisher
has now demonstrated that they differentiate, so subsequent fetches
need a different UA or a different acquisition path.

## Poisoned-data detector — `FieldAnomalyDetector` (T107)

The flip side of cloaking is a target that serves a clean `200` with
all the right HTML structure but subtly wrong field values —
inflated prices, dropped listings, fabricated rows, stale snapshots.
The detector doesn't observe the network shape; it observes the
data values directly.

`stygian_charon::FieldAnomalyDetector` (behind the `field-anomaly`
feature, off by default) is the consumer-owned port. The default
adapter, `StatisticalFieldAnomalyDetector`, watches every published
field value against a rolling per-`(schema_id, field_path)` baseline
and emits one of five signal kinds:

| Signal | Trigger |
| --- | --- |
| `PriceDrift` | A numeric field's value drifts outside the rolling mean ± kσ |
| `Outlier` | A single observation is more than the configurable outlier threshold from the median |
| `ListingReorder` | The field is a list and the order/contents change beyond the reorder threshold against the baseline |
| `Staleness` | The field's `last_seen` is older than the staleness window |
| `CardinalityShift` | A categorical field's distinct-value count changes by more than the cardinality threshold |

`FieldAnomalyDetector::observe(&field_report)` is called once per
published field value; the detector returns an
`AnomalyReport { field, schema_id, signal, severity, reason }` for
each violation. Operators typically surface `Error` severity into
their incident workflow and `Warning` severity into the run report.

Enable the feature in your `Cargo.toml`:

```toml
stygian-charon = { version = "0.17", features = ["field-anomaly"] }
```

The feature is off by default so existing charon users aren't forced
to opt into the per-field baseline storage cost. Once enabled, the
adapter holds the rolling baseline in memory — bounded by the
configured window size — so it's safe to run inline in your pipeline
without a separate persistence layer.

