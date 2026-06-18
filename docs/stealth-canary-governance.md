# Stealth Canary Governance (T84)

This document is the **authoritative reference** for the stealth
canary hard-gate. It records the targets, semantics, and ownership
for the gate so a new on-call can understand the system without
reading the source.

> Source of truth: [`tools/stealth-canary/data/required-targets.toml`](../tools/stealth-canary/data/required-targets.toml)
> and [`.github/workflows/stealth-canary.yml`](../.github/workflows/stealth-canary.yml).
> This document explains the **why**; the data file and the
> workflow are the **what**.

## 1. What fails the build today, what warns, and why

The stealth canary has **two distinct failure axes** that compose:

### 1.1 Probe axis (binary, per-run)

The probe runs the `stealth_probe` example against every
`[[canary]]` entry in
[`.github/stealth-canary.toml`](../.github/stealth-canary.toml).
For each entry:

* **`advisory = false`** (the **default**): if the per-run
  `ok == false` (probe `score < threshold`), the build fails.
  This is a hard merge block.
* **`advisory = true`**: a failure opens/updates a
  "Stealth regression detected" issue, but **does not fail
  the build**. This is a soft signal — useful for monitoring
  external sites that change their detection logic on their
  own schedule.

The T58 baseline env vars (`STYGIAN_TIER1_BASELINE_CREEPJS`,
`STYGIAN_TIER1_BASELINE_BROWSERSCAN`) tighten the probe axis
opt-in: when supplied, the per-run score must be at or above
the baseline for the test to pass.

### 1.2 Trend axis (rolling, T84)

A run can pass the probe axis but still fail the build on the
**trend axis** if the rolling-window score regression detector
in
[`tools/stealth-canary/trend.py`](../tools/stealth-canary/trend.py)
flags a regression. The trend axis is opt-in (it can be tuned
via `STYGIAN_TREND_*` env vars) and has two rules:

| Rule | Default trigger | Override |
|---|---|---|
| **Single-point regression** | `current < rolling_mean(window) − delta_threshold` | `STYGIAN_TREND_WINDOW` (default 10), `STYGIAN_TREND_DELTA_THRESHOLD` (default 0.05) |
| **Monotonic regression** | Last `monotonic_runs` scores strictly decreasing | `STYGIAN_TREND_MONOTONIC_RUNS` (default 3) |

When the history is shorter than `STYGIAN_TREND_MIN_HISTORY`
(default 3 runs) the verdict is `insufficient_data` — the
target neither passes nor fails on the trend axis (it still
passes/fails on the probe and baseline axes).

The trend axis **composes** with the T58 baseline env vars:
an operator can pin `STYGIAN_TIER1_BASELINE_CREEPJS=0.55` to
enforce a per-run minimum, and the trend detector will still
fire if the score drops monotonically even when the absolute
baseline is met.

### 1.3 Composition summary

For every required (non-advisory) canary target, the build
fails when **any** of these conditions hold:

1. Probe `ok == false` (probe axis).
2. `STYGIAN_TIER1_BASELINE_<LABEL>` is supplied **and** the
   current score is below the baseline (T58 closure).
3. `STYGIAN_TREND_BASELINE_<LABEL>` is supplied **and** the
   current score is below the baseline (T84 trend-axis
   baseline override).
4. The trend detector fires
   `regression_detected` or `monotonic_regression` (T84
   trend axis).

## 2. Required (non-advisory) target set

The required target set is committed as data in
[`tools/stealth-canary/data/required-targets.toml`](../tools/stealth-canary/data/required-targets.toml).
The canary workflow will not run a target listed there that
is missing from
[`.github/stealth-canary.toml`](../.github/stealth-canary.toml)
— `validate_stealth_canary.py` enforces the label overlap at
workflow start-up.

### 2.1 Current targets

#### synthetic-injection

| Field | Value |
|---|---|
| URL | `about:blank` |
| Threshold | 0.95 |
| Description | Synthetic self-test: `about:blank` + Advanced stealth. Scores below 0.95 mean our own injection scripts are broken. |
| Owner | `@greysquirr3l` |
| Secondary | `@stygian-charon-on-call` |
| Artifacts | `probe-report.json`, `probe-stderr.txt`, `history/canary-history.jsonl` |

Synthetic self-test that runs the full `verify_stealth()`
suite against an empty page. A regression here is almost
always a bug in **our** stealth injection paths and is
guaranteed actionable.

#### creepjs

| Field | Value |
|---|---|
| URL | `https://abrahamjuliot.github.io/creepjs/` |
| Threshold | 0.50 |
| Description | CreepJS open-source fingerprint observatory (Tier 1). Detects surface-shape leaks (`navigator`, prototypes, fonts, canvas). |
| Owner | `@greysquirr3l` |
| Secondary | `@stygian-charon-on-call` |
| Optional baseline | `STYGIAN_TIER1_BASELINE_CREEPJS=0.55` (T58 closure) |
| Artifacts | `probe-report.json`, `probe-stderr.txt`, `history/canary-history.jsonl` |

Open-source fingerprint observatory. CI-safe (no auth).
Detects surface-shape leaks at the prototype level. Pairs
with the optional `STYGIAN_TIER1_BASELINE_CREEPJS` env var
for non-regression detection.

#### browserscan

| Field | Value |
|---|---|
| URL | `https://www.browserscan.net/` |
| Threshold | 0.90 |
| Description | `BrowserScan` authenticity percentage (Tier 1). Detects automation/timing/canvas/navigator drift. |
| Owner | `@greysquirr3l` |
| Secondary | `@stygian-charon-on-call` |
| Optional baseline | `STYGIAN_TIER1_BASELINE_BROWSERSCAN=0.92` (T58 closure) |
| Artifacts | `probe-report.json`, `probe-stderr.txt`, `history/canary-history.jsonl` |

Open-source authenticity percentage. CI-safe (no auth).
Detects automation, timing, canvas, and navigator drift.
Pairs with the optional `STYGIAN_TIER1_BASELINE_BROWSERSCAN`
env var.

### 2.2 Why these three

* `synthetic-injection` is the **internal floor** — it
  catches regressions in our own injection scripts before
  any external site can mask them.
* `creepjs` and `browserscan` are the **external floor** —
  they are CI-safe (no auth, no IP allowlist) and exercise
  the full fingerprint surface against real-world detection
  logic. The T58 closure confirmed these as the Tier 1
  observatory baseline.

Adding a new required target:

1. Add a `[[canary]]` entry to
   `.github/stealth-canary.toml` (with `advisory = false`).
2. Add a `[[required]]` entry to
   `tools/stealth-canary/data/required-targets.toml` with
   owner / secondary / runbook / artifacts populated.
3. Run `python3 .github/scripts/validate_stealth_canary.py`
   locally to confirm the label overlap is clean.

## 3. How to add an opt-in baseline

The T58 `STYGIAN_TIER1_BASELINE_*` env vars and the T84
`STYGIAN_TREND_BASELINE_*` env vars follow the same
convention:

| Env var | Scope | Default behaviour |
|---|---|---|
| `STYGIAN_TIER1_BASELINE_<LABEL>` | T58 closure | Set on a job to pin a per-run minimum for the named target. Unset = no enforcement. |
| `STYGIAN_TREND_BASELINE_<LABEL>` | T84 trend axis | Same as above. Useful for an operator to add a baseline **only for the trend gate** without forcing a hard per-run floor. |

The trend-axis baseline takes precedence over the T58
baseline in `trend_cli.py`'s `_resolve_baselines` helper;
operators who want both can set both.

`<LABEL>` is the uppercased, hyphen-to-underscore
`[[canary]]` / `[[required]]` label. Examples:
`STYGIAN_TIER1_BASELINE_CREEPJS`,
`STYGIAN_TREND_BASELINE_BROWSERSCAN`.

## 4. Trend detector knobs

| Env var | Default | Meaning |
|---|---|---|
| `STYGIAN_TREND_WINDOW` | `10` | Rolling window size for the single-point regression check |
| `STYGIAN_TREND_DELTA_THRESHOLD` | `0.05` | Score drop (vs rolling mean) that trips single-point regression |
| `STYGIAN_TREND_MONOTONIC_RUNS` | `3` | Consecutive drops that trip monotonic regression |
| `STYGIAN_TREND_MIN_HISTORY` | `3` | Minimum history points before evaluating either rule |

Invalid values (negative numbers, non-numeric strings, out-of-range
floats) are silently ignored so a bad override never blocks CI.

## 5. T92 `CanaryTrendObservation` integration

The trend detector reuses the
[T92 `CanaryTrendObservation`](../crates/stygian-browser/src/integrity_canary/trend.rs)
seam without re-implementing it. A probe report entry may
include a `trend_observations: [...]` field whose elements
match the T92 schema (`signature`, `score`, `severity` =
`clean` / `suspected` / `confirmed`, `fired_probe_ids`,
`captured_at_epoch_ms`, …). The CLI:

1. Stores the most-recent severity on the verdict's
   `observation_severity` field (so the Markdown summary
   surfaces it).
2. Treats a `clean` → `suspected` → `confirmed` climb over
   `monotonic_runs` consecutive runs as a trend regression
   for the parent target.

This means a future canary integration that emits T92
`CanaryTrendObservation` JSON does not need to change the
trend detector's source — the same trend signal flows
through the probe → history → verdict path.

## 6. Workflow artifacts

| Artifact | Contents |
|---|---|
| `probe-report` | `probe-report.json` (per-target results) + `probe-stderr.txt` |
| `trend-report` | `canary-verdict.json` (the trend gate verdict), `canary-summary.md` (the rendered Markdown), and the updated `history/canary-history.jsonl` |

The workflow step summary surfaces:

* Per-target trend verdicts (status, current score, rolling
  mean, delta, consecutive drops, baseline, observation
  severity).
* The ownership / runbook / artifact pointer table from
  `tools/stealth-canary/data/required-targets.toml` (with a
  🛑 marker on hard-fail rows).
* The list of uploaded artifacts (HAR, canary JSON, history).

## 7. Adding a new required target

1. Add the target to `.github/stealth-canary.toml`
   (`advisory = false`).
2. Add the matching `[[required]]` entry to
   `tools/stealth-canary/data/required-targets.toml` with
   the required fields (label, url, threshold, description,
   owner, runbook, artifacts).
3. Run `python3 .github/scripts/validate_stealth_canary.py`
   locally to confirm the labels overlap.
4. Open a PR. The `Stealth Canary` workflow will probe the
   new target on the next push to `main` and add it to the
   trend history.

## 8. Removing a required target

1. Remove the `[[required]]` entry from
   `tools/stealth-canary/data/required-targets.toml`.
2. Decide whether to keep the probe alive
   (`.github/stealth-canary.toml` `[[canary]]` entry with
   `advisory = true` is a soft signal) or remove it entirely.
3. Open a PR. The trend detector will stop evaluating the
   target on the next push; existing history rows for the
   label are ignored but not purged (a future re-add can
   resume trend detection without losing continuity).

## 9. Open questions / future work

* **T88 anti-bot change detection feed**: the trend detector
  is one of the primary inputs to T88. Once T88 lands, the
  trend history will be streamable to the change feed via
  the `canary-verdict.json` artifact.
* **Smaller windows for fast detection**: the current
  `window=10` / `monotonic_runs=3` is a balance between
  signal-to-noise and time-to-detect. Operators who need
  faster detection can override the env vars; the defaults
  are documented above.
* **Per-target baseline as data**: today, the per-target
  baseline lives in env vars (T58 contract) and as an
  optional `baseline` field in the data file. A future
  iteration could promote the baseline to a first-class
  data field on the required-target entry.
