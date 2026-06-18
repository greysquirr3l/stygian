# Stealth Canary — Trend-Aware Gating (T84)

This module contains the **operational tooling** for the stealth
canary hard-gate. The CI workflow under
[`.github/workflows/stealth-canary.yml`](../../.github/workflows/stealth-canary.yml)
calls `trend_cli.py` after every probe run, which:

1. Reads the per-run `probe-report.json` produced by the
   `stealth_probe` example.
2. Reads the rolling trend history from
   `history/canary-history.jsonl` (persisted across runs via
   `actions/cache`).
3. Appends the current run to the history.
4. Evaluates the **trend verdict** for every required (non-advisory)
   canary target.
5. Emits a JSON verdict file (consumed by the workflow exit step)
   and a Markdown summary (appended to `$GITHUB_STEP_SUMMARY`).

The trend detector composes with the T92
[`CanaryTrendObservation`](../../crates/stygian-browser/src/integrity_canary/trend.rs)
seam — the JSON shape emitted by the JavaScript integrity trap
canary is accepted as a complementary input without re-parsing
the probe catalogue.

## File Layout

```
tools/stealth-canary/
├── README.md                  ← you are here
├── trend.py                   ← pure-Python trend detector library
├── trend_cli.py               ← CLI entry point (consumed by CI)
├── data/
│   └── required-targets.toml  ← required hard-fail target set
└── tests/
    └── test_trend.py          ← unit tests
```

## Trend Detector Semantics

The detector applies two rules to every required target's
chronological score history:

| Rule | Trigger | Default |
|---|---|---|
| **Single-point regression** | `current < rolling_mean(window) − delta_threshold` | `window=10`, `delta_threshold=0.05` |
| **Monotonic regression** | Last `monotonic_runs` scores strictly decreasing | `monotonic_runs=3` |

A target is hard-fail when **either** rule trips, when the probe
itself reported `ok=false`, or when an optional per-target
baseline (e.g. `STYGIAN_TIER1_BASELINE_CREEPJS`) was provided and
the current score is below the baseline.

When the history is shorter than `min_history=3` runs the verdict
is `insufficient_data` — the target neither passes nor fails on
the trend axis (it still passes/fails on the probe and baseline
axes).

All knobs are also exposed as `STYGIAN_TREND_*` env vars so
operators can override them per-run without editing the script:

| Env var | Default | Meaning |
|---|---|---|
| `STYGIAN_TREND_WINDOW` | `10` | Rolling window size |
| `STYGIAN_TREND_DELTA_THRESHOLD` | `0.05` | Score drop that trips single-point regression |
| `STYGIAN_TREND_MONOTONIC_RUNS` | `3` | Consecutive drops that trip monotonic regression |
| `STYGIAN_TREND_MIN_HISTORY` | `3` | Minimum history points before evaluating either rule |

The baseline pattern follows T58's `STYGIAN_TIER1_BASELINE_*`
convention (one var per required target, prefix-uppercase label,
value in `[0.0, 1.0]`, unset = no enforcement).

## Verdict JSON

The CLI writes the per-run aggregate to `--verdict`:

```json
{
  "run_id": "ci-123",
  "verdict_count": 2,
  "hard_fail": true,
  "hard_fail_labels": ["creepjs"],
  "verdicts": {
    "creepjs": {
      "label": "creepjs",
      "current_score": 0.80,
      "status": "regression_detected",
      "reason": "single-point regression: current=0.8000 dropped 0.1500 below rolling_mean=0.9500 (threshold=0.0500)",
      "run_count": 11,
      "rolling_mean": 0.95,
      "delta": -0.15,
      "consecutive_drops": 0,
      "baseline": null,
      "baseline_breach": false,
      "observation_severity": null,
      "is_hard_fail": true
    }
  }
}
```

## Markdown Summary

The CLI writes a summary to `--summary` with three sections:

1. **Trend verdicts** — per-target table with status, current
   score, rolling mean, delta, consecutive-drops count, optional
   baseline, and T92 `CanaryTrendObservation` severity.
2. **Ownership, runbook & artifacts** — ownership / runbook /
   artifact pointers from `data/required-targets.toml`. Hard-fail
   rows are marked with 🛑.
3. **Uploaded artifacts** — the artifact list passed to the CLI
   via repeated `--artifact` flags.

The summary is appended to `$GITHUB_STEP_SUMMARY` so the
ownership contacts, runbook links, and HAR / canary JSON
artifact pointers are visible directly in the GitHub Actions UI.

## CLI Usage

```sh
python3 trend_cli.py \
    --probe-report probe-report.json \
    --canary-config .github/stealth-canary.toml \
    --required-targets tools/stealth-canary/data/required-targets.toml \
    --history history/canary-history.jsonl \
    --run-id "$GITHUB_RUN_ID" \
    --run-url "$GITHUB_RUN_URL" \
    --verdict /tmp/canary-verdict.json \
    --summary /tmp/canary-summary.md \
    --artifact probe-report.json \
    --artifact history/canary-history.jsonl
```

Exit codes: `0` = all required targets stable (or
trend-insufficient), `1` = at least one required target is
hard-fail.

## Unit Tests

```sh
cd tools/stealth-canary
python3 -m unittest tests.test_trend -v
```

The tests cover:

* Pure trend math (stable, single-point regression, monotonic
  regression, insufficient data, baseline breach)
* Per-target aggregation across the required set
* History JSONL round-trip
* T92 `CanaryTrendObservation` seam (severity propagation)
* T58 `STYGIAN_*_BASELINE_*` env-var pattern (precedence, parsing,
  out-of-range rejection, garbage-string rejection)
* Markdown summary contract (ownership contacts, runbook links,
  artifact pointers, hard-fail marker)
* End-to-end CLI smoke (verdict + summary + history append)

## Validation

The data file is also validated by
[`.github/scripts/validate_stealth_canary.py`](../../.github/scripts/validate_stealth_canary.py)
so a missing `label` or bad `url` field fails the workflow
**before** the trend detector runs.
