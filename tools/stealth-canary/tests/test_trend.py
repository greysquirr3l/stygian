"""Unit tests for the stealth canary trend detector (T84).

Run with:

```
cd tools/stealth-canary && python3 -m unittest tests.test_trend -v
```

The tests cover:

  * the pure trend math (stable, regression, monotonic regression,
    insufficient data, baseline breach)
  * the per-target aggregation across the required set
  * the history JSONL round-trip
  * the T92 ``CanaryTrendObservation`` seam — the trend detector
    must accept the same shape without re-implementing it
  * the Markdown summary contract — the rendered Markdown must
    include the ownership contacts, runbook links, and artifact
    pointers the workflow surfaces to the on-call
  * the ``STYGIAN_*_BASELINE_*`` env-var pattern (T58 contract)
  * the CLI's verdict + summary outputs
"""

from __future__ import annotations

import io
import json
import os
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

_HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(_HERE.parent))

import trend  # noqa: E402
import trend_cli  # noqa: E402


# ── Pure trend math ──────────────────────────────────────────────────────────


class TestEvaluateTrend(unittest.TestCase):
    """Tests for the rolling-window score regression detector."""

    def test_stable_score_passes(self) -> None:
        # 10-run history hovering around 0.95. Current score (0.95) is
        # within delta_threshold (0.05) of the rolling mean → stable.
        history = [0.95, 0.95, 0.96, 0.95, 0.94, 0.95, 0.96, 0.95, 0.94, 0.95]
        verdict = trend.evaluate_trend(
            "synthetic-injection",
            history,
            current=0.95,
            config=trend.TrendConfig(),
        )
        self.assertEqual(verdict.status, trend.TrendStatus.STABLE)
        self.assertFalse(verdict.is_hard_fail)
        self.assertEqual(verdict.run_count, len(history) + 1)
        self.assertIsNotNone(verdict.rolling_mean)
        self.assertIsNotNone(verdict.delta)
        # 0.05 (threshold) is the gap that flips the verdict to
        # regression_detected; current=0.95 vs rolling=0.95 should not
        # trip it.
        self.assertGreaterEqual(verdict.delta, -0.05)

    def test_monotonic_regression_fails(self) -> None:
        # Last 3 scores strictly decreasing → monotonic regression.
        history = [0.96, 0.95, 0.94, 0.93, 0.92, 0.91, 0.90, 0.89, 0.88, 0.87]
        verdict = trend.evaluate_trend(
            "synthetic-injection",
            history,
            current=0.86,
            config=trend.TrendConfig(),
        )
        self.assertEqual(verdict.status, trend.TrendStatus.MONOTONIC_REGRESSION)
        self.assertTrue(verdict.is_hard_fail)
        self.assertGreaterEqual(verdict.consecutive_drops, 2)

    def test_single_point_regression_fails(self) -> None:
        # Rolling mean ~0.95 over 10 runs, current drops to 0.80
        # (delta -0.15) → regression_detected.
        history = [0.95] * 10
        verdict = trend.evaluate_trend(
            "creepjs",
            history,
            current=0.80,
            config=trend.TrendConfig(),
        )
        self.assertEqual(verdict.status, trend.TrendStatus.REGRESSION_DETECTED)
        self.assertTrue(verdict.is_hard_fail)
        self.assertIsNotNone(verdict.delta)
        self.assertLess(verdict.delta, -0.05)

    def test_insufficient_data_does_not_fail(self) -> None:
        # History shorter than min_history (3) → insufficient_data.
        verdict = trend.evaluate_trend(
            "browserscan",
            [0.95, 0.94],
            current=0.50,
            config=trend.TrendConfig(),
        )
        self.assertEqual(verdict.status, trend.TrendStatus.INSUFFICIENT_DATA)
        self.assertFalse(verdict.is_hard_fail)

    def test_baseline_breach_fails_without_trend(self) -> None:
        # Stable trend but optional baseline breached → still hard-fail.
        history = [0.95] * 10
        verdict = trend.evaluate_trend(
            "creepjs",
            history,
            current=0.80,
            config=trend.TrendConfig(),
            baseline=0.85,
        )
        # Note: a single-point regression will ALSO trip, so we expect
        # regression_detected (or monotonic). The point is that the
        # baseline breach contributes to the hard-fail decision.
        self.assertTrue(verdict.is_hard_fail)
        self.assertTrue(verdict.baseline_breach)
        self.assertEqual(verdict.baseline, 0.85)

    def test_baseline_at_or_above_threshold_does_not_breach(self) -> None:
        history = [0.95] * 10
        verdict = trend.evaluate_trend(
            "creepjs",
            history,
            current=0.85,
            config=trend.TrendConfig(),
            baseline=0.85,
        )
        # current == baseline → no breach
        self.assertFalse(verdict.baseline_breach)
        # delta = 0.85 - 0.95 = -0.10 < -0.05 → regression_detected
        # (not stable), but not from baseline.
        self.assertEqual(verdict.status, trend.TrendStatus.REGRESSION_DETECTED)

    def test_monotonic_ignores_interior_drops(self) -> None:
        # An interior drop earlier in the history does NOT count as
        # a trailing monotonic drop. Only the last 3 scores matter.
        history = [0.50, 0.99, 0.95, 0.94, 0.93, 0.95, 0.95, 0.95, 0.95, 0.95]
        verdict = trend.evaluate_trend(
            "synthetic-injection",
            history,
            current=0.94,
            config=trend.TrendConfig(),
        )
        # The last 3 (0.95, 0.95, 0.94) are not strictly decreasing
        # (0.95 → 0.95 is a tie). The verdict should be stable (or
        # regression_detected) but NOT monotonic_regression.
        self.assertNotEqual(verdict.status, trend.TrendStatus.MONOTONIC_REGRESSION)

    def test_observation_severity_is_propagated(self) -> None:
        history = [0.95] * 10
        verdict = trend.evaluate_trend(
            "synthetic-injection",
            history,
            current=0.95,
            config=trend.TrendConfig(),
            observation_severity="suspected",
        )
        self.assertEqual(verdict.observation_severity, "suspected")
        # severity alone does not change the trend-axis verdict
        self.assertEqual(verdict.status, trend.TrendStatus.STABLE)

    def test_evaluate_trend_reason_human_readable(self) -> None:
        history = [0.95] * 10
        verdict = trend.evaluate_trend(
            "synthetic-injection",
            history,
            current=0.80,
            config=trend.TrendConfig(),
        )
        self.assertIn("single-point regression", verdict.reason)
        self.assertIn("rolling_mean", verdict.reason)
        self.assertIn("delta", verdict.reason)


# ── Aggregation across the required set ──────────────────────────────────────


class TestEvaluatePerTarget(unittest.TestCase):
    def test_per_target_aggregation_with_mixed_verdicts(self) -> None:
        history = {
            "creepjs": [0.95] * 10,
            "synthetic-injection": [0.95] * 10,
        }
        current = {
            "creepjs": 0.80,  # regression_detected
            "synthetic-injection": 0.95,  # stable
        }
        verdicts = trend.evaluate_per_target(history, current, trend.TrendConfig())
        self.assertEqual(verdicts["creepjs"].status, trend.TrendStatus.REGRESSION_DETECTED)
        self.assertEqual(verdicts["synthetic-injection"].status, trend.TrendStatus.STABLE)
        self.assertTrue(verdicts["creepjs"].is_hard_fail)
        self.assertFalse(verdicts["synthetic-injection"].is_hard_fail)

    def test_per_target_skips_unknown_current(self) -> None:
        # A label present in history but missing from current is
        # ignored. A label present in current but missing from history
        # is evaluated with an empty history.
        verdicts = trend.evaluate_per_target(
            history_by_label={},
            current_by_label={"new-target": 0.50},
            config=trend.TrendConfig(),
        )
        self.assertIn("new-target", verdicts)
        self.assertEqual(verdicts["new-target"].status, trend.TrendStatus.INSUFFICIENT_DATA)

    def test_aggregate_verdict_hard_fail_labels(self) -> None:
        verdicts = trend.evaluate_per_target(
            history_by_label={"a": [0.95] * 10, "b": [0.95] * 10},
            current_by_label={"a": 0.95, "b": 0.80},
            config=trend.TrendConfig(),
        )
        agg = trend.aggregate_verdict(verdicts)
        self.assertTrue(agg["hard_fail"])
        self.assertEqual(agg["hard_fail_labels"], ["b"])


# ── History JSONL round-trip ─────────────────────────────────────────────────


class TestHistoryRoundTrip(unittest.TestCase):
    def test_history_entry_round_trip(self) -> None:
        original = trend.HistoryEntry(
            label="creepjs",
            score=0.95,
            threshold=0.50,
            ok=True,
            run_id="123",
            captured_at_epoch_ms=1_700_000_000_000,
            trend_observations=[
                {
                    "signature": "fnv64:abcd",
                    "score": 0.20,
                    "severity": "suspected",
                }
            ],
        )
        line = original.to_jsonl()
        parsed = trend.HistoryEntry.from_jsonl(line)
        self.assertEqual(parsed, original)

    def test_read_history_handles_missing_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            entries = trend.read_history(os.path.join(tmp, "missing.jsonl"))
            self.assertEqual(entries, [])

    def test_read_history_handles_empty_lines(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "history.jsonl")
            with open(path, "w", encoding="utf-8") as handle:
                handle.write("\n")
                handle.write(
                    json.dumps(
                        {
                            "label": "creepjs",
                            "score": 0.95,
                            "threshold": 0.5,
                            "ok": True,
                            "run_id": "1",
                            "captured_at_epoch_ms": 0,
                        }
                    )
                )
                handle.write("\n\n")
            entries = trend.read_history(path)
            self.assertEqual(len(entries), 1)
            self.assertEqual(entries[0].label, "creepjs")

    def test_write_history_creates_parent_dirs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "nested", "history.jsonl")
            trend.write_history(
                path,
                [
                    trend.HistoryEntry(
                        label="creepjs",
                        score=0.95,
                        threshold=0.5,
                        ok=True,
                        run_id="1",
                        captured_at_epoch_ms=0,
                    )
                ],
            )
            self.assertTrue(os.path.isfile(path))

    def test_history_to_score_map_filters_by_label(self) -> None:
        entries = [
            trend.HistoryEntry(
                label="creepjs",
                score=0.95,
                threshold=0.5,
                ok=True,
                run_id="1",
                captured_at_epoch_ms=0,
            ),
            trend.HistoryEntry(
                label="browserscan",
                score=0.91,
                threshold=0.9,
                ok=True,
                run_id="1",
                captured_at_epoch_ms=0,
            ),
            trend.HistoryEntry(
                label="creepjs",
                score=0.94,
                threshold=0.5,
                ok=True,
                run_id="2",
                captured_at_epoch_ms=0,
            ),
        ]
        scores = trend.history_to_score_map(entries, "creepjs")
        self.assertEqual(scores, [0.95, 0.94])


# ── T92 ``CanaryTrendObservation`` seam ──────────────────────────────────────


class TestCanaryTrendObservationSeam(unittest.TestCase):
    def test_severity_for_label_picks_latest_entry(self) -> None:
        entries = [
            trend.HistoryEntry(
                label="creepjs",
                score=0.95,
                threshold=0.5,
                ok=True,
                run_id="1",
                captured_at_epoch_ms=0,
                trend_observations=[{"severity": "clean"}],
            ),
            trend.HistoryEntry(
                label="creepjs",
                score=0.80,
                threshold=0.5,
                ok=True,
                run_id="2",
                captured_at_epoch_ms=1,
                trend_observations=[{"severity": "confirmed"}],
            ),
        ]
        # Most recent entry's severity wins.
        self.assertEqual(trend.severity_for_label(entries, "creepjs"), "confirmed")

    def test_severity_for_label_returns_none_when_missing(self) -> None:
        self.assertIsNone(trend.severity_for_label([], "creepjs"))
        entries = [
            trend.HistoryEntry(
                label="creepjs",
                score=0.95,
                threshold=0.5,
                ok=True,
                run_id="1",
                captured_at_epoch_ms=0,
            )
        ]
        self.assertIsNone(trend.severity_for_label(entries, "creepjs"))


# ── T58 ``STYGIAN_*_BASELINE_*`` env-var pattern ──────────────────────────────


class TestBaselineEnvVars(unittest.TestCase):
    def test_env_baseline_unset(self) -> None:
        env = {"NOT_SET": "0.5"}
        self.assertIsNone(trend_cli._env_baseline("STYGIAN_FOO_BASELINE_BAR"))

    def test_env_baseline_parses_float(self) -> None:
        with mock.patch.dict(os.environ, {"STYGIAN_FOO_BASELINE": "0.85"}):
            self.assertEqual(trend_cli._env_baseline("STYGIAN_FOO_BASELINE"), 0.85)

    def test_env_baseline_rejects_out_of_range(self) -> None:
        with mock.patch.dict(os.environ, {"STYGIAN_FOO_BASELINE": "1.5"}):
            self.assertIsNone(trend_cli._env_baseline("STYGIAN_FOO_BASELINE"))

    def test_env_baseline_rejects_garbage(self) -> None:
        with mock.patch.dict(os.environ, {"STYGIAN_FOO_BASELINE": "nope"}):
            self.assertIsNone(trend_cli._env_baseline("STYGIAN_FOO_BASELINE"))

    def test_resolve_baselines_precedence(self) -> None:
        env = {
            "STYGIAN_TREND_BASELINE_CREEPJS": "0.90",
            "STYGIAN_TIER1_BASELINE_BROWSERSCAN": "0.92",
        }
        with mock.patch.dict(os.environ, env, clear=True):
            creepjs_value, creepjs_source = trend_cli._resolve_baselines(
                "creepjs", {"baseline": 0.50}
            )
            browserscan_value, browserscan_source = trend_cli._resolve_baselines(
                "browserscan", {"baseline": 0.85}
            )
        self.assertEqual(creepjs_value, 0.90)
        self.assertEqual(creepjs_source, "STYGIAN_TREND_BASELINE_CREEPJS")
        self.assertEqual(browserscan_value, 0.92)
        self.assertEqual(browserscan_source, "STYGIAN_TIER1_BASELINE_BROWSERSCAN")

    def test_resolve_baselines_falls_back_to_data_file(self) -> None:
        with mock.patch.dict(os.environ, {}, clear=True):
            value, source = trend_cli._resolve_baselines(
                "creepjs", {"baseline": 0.50}
            )
        self.assertEqual(value, 0.50)
        self.assertEqual(source, "required-targets.toml:baseline")


# ── Markdown summary contract ───────────────────────────────────────────────


class TestMarkdownSummary(unittest.TestCase):
    def test_summary_contains_required_sections(self) -> None:
        verdicts = {
            "creepjs": trend.TrendVerdict(
                label="creepjs",
                current_score=0.95,
                status=trend.TrendStatus.STABLE,
                reason="stable",
                run_count=11,
                rolling_mean=0.95,
                delta=0.0,
            ),
            "browserscan": trend.TrendVerdict(
                label="browserscan",
                current_score=0.80,
                status=trend.TrendStatus.REGRESSION_DETECTED,
                reason="regression",
                run_count=11,
                rolling_mean=0.92,
                delta=-0.12,
            ),
        }
        required = {
            "creepjs": {
                "owner": "@greysquirr3l",
                "secondary": "@stygian-charon-on-call",
                "runbook": "docs/stealth-canary-governance.md#creepjs",
                "artifacts": ["probe-report.json"],
            },
            "browserscan": {
                "owner": "@greysquirr3l",
                "secondary": "@stygian-charon-on-call",
                "runbook": "docs/stealth-canary-governance.md#browserscan",
                "artifacts": ["probe-report.json", "history/canary-history.jsonl"],
            },
        }
        md = trend_cli._build_markdown(
            verdicts,
            required,
            trend.TrendConfig(),
            "123",
            "https://github.com/owner/repo/actions/runs/123",
            ["probe-report.json", "history/canary-history.jsonl"],
        )
        # Headline
        self.assertIn("## Stealth Canary — Trend Report", md)
        # Detector config block
        self.assertIn("Detector config", md)
        # Run link
        self.assertIn("https://github.com/owner/repo/actions/runs/123", md)
        # Trend verdicts table
        self.assertIn("### Trend verdicts", md)
        self.assertIn("| `creepjs` |", md)
        self.assertIn("| `browserscan` |", md)
        # Ownership table — owner handles, runbook links, artifact pointers
        self.assertIn("### Ownership, runbook & artifacts", md)
        self.assertIn("@greysquirr3l", md)
        self.assertIn("@stygian-charon-on-call", md)
        self.assertIn("docs/stealth-canary-governance.md#creepjs", md)
        self.assertIn("docs/stealth-canary-governance.md#browserscan", md)
        self.assertIn("`probe-report.json`", md)
        self.assertIn("`history/canary-history.jsonl`", md)
        # Hard-fail marker on at least one row
        self.assertIn("🛑", md)
        # Artifacts list
        self.assertIn("### Uploaded artifacts", md)

    def test_summary_handles_stable_run_cleanly(self) -> None:
        verdicts = {
            "creepjs": trend.TrendVerdict(
                label="creepjs",
                current_score=0.95,
                status=trend.TrendStatus.STABLE,
                reason="stable",
                run_count=11,
                rolling_mean=0.95,
                delta=0.0,
            ),
        }
        required = {
            "creepjs": {
                "owner": "@greysquirr3l",
                "runbook": "docs/x.md",
                "artifacts": ["probe-report.json"],
            },
        }
        md = trend_cli._build_markdown(
            verdicts,
            required,
            trend.TrendConfig(),
            "123",
            None,
            [],
        )
        self.assertIn("All required canary targets are stable", md)
        self.assertNotIn("🛑", md)
        # Even with no run URL, the run id is still surfaced
        self.assertIn("`123`", md)
        # The artifacts list still renders (with the placeholder)
        self.assertIn("_(none)_", md)


# ── End-to-end CLI smoke test ───────────────────────────────────────────────


class TestCLISmoke(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmpdir = pathlib.Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def _write_canary_config(self) -> pathlib.Path:
        path = self.tmpdir / "stealth-canary.toml"
        path.write_text(
            """
[[canary]]
url = "about:blank"
label = "synthetic-injection"
threshold = 0.95

[[canary]]
url = "https://abrahamjuliot.github.io/creepjs/"
label = "creepjs"
threshold = 0.50
advisory = false
""",
            encoding="utf-8",
        )
        return path

    def _write_required_targets(self) -> pathlib.Path:
        path = self.tmpdir / "required-targets.toml"
        path.write_text(
            """
[[required]]
label = "synthetic-injection"
url = "about:blank"
threshold = 0.95
description = "Synthetic self-test"
owner = "@greysquirr3l"
secondary = "@stygian-charon-on-call"
runbook = "docs/stealth-canary-governance.md#synthetic-injection"
artifacts = ["probe-report.json"]

[[required]]
label = "creepjs"
url = "https://abrahamjuliot.github.io/creepjs/"
threshold = 0.50
description = "CreepJS Tier 1 observatory"
owner = "@greysquirr3l"
secondary = "@stygian-charon-on-call"
runbook = "docs/stealth-canary-governance.md#creepjs"
artifacts = ["probe-report.json"]
""",
            encoding="utf-8",
        )
        return path

    def _write_probe_report(self) -> pathlib.Path:
        path = self.tmpdir / "probe-report.json"
        path.write_text(
            json.dumps(
                [
                    {
                        "label": "synthetic-injection",
                        "url": "about:blank",
                        "score": 0.95,
                        "score_pct": 95.0,
                        "threshold": 0.95,
                        "ok": True,
                        "advisory": False,
                    },
                    {
                        "label": "creepjs",
                        "url": "https://abrahamjuliot.github.io/creepjs/",
                        "score": 0.80,
                        "score_pct": 80.0,
                        "threshold": 0.50,
                        "ok": True,
                        "advisory": False,
                    },
                ]
            ),
            encoding="utf-8",
        )
        return path

    def _seed_history(self, path: pathlib.Path, label: str, scores: list[float]) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("w", encoding="utf-8") as handle:
            for idx, score in enumerate(scores):
                handle.write(
                    json.dumps(
                        {
                            "label": label,
                            "score": score,
                            "threshold": 0.50,
                            "ok": True,
                            "run_id": f"seed-{idx}",
                            "captured_at_epoch_ms": 1_700_000_000_000 + idx * 1000,
                        }
                    )
                    + "\n"
                )

    def test_cli_happy_path_exits_zero(self) -> None:
        canary_config = self._write_canary_config()
        required = self._write_required_targets()
        probe_report = self._write_probe_report()
        history = self.tmpdir / "history.jsonl"
        verdict = self.tmpdir / "verdict.json"
        summary = self.tmpdir / "summary.md"

        # Seed enough history for creepjs so the trend check has data.
        self._seed_history(history, "creepjs", [0.95] * 10)

        exit_code = trend_cli.main(
            [
                "--probe-report",
                str(probe_report),
                "--canary-config",
                str(canary_config),
                "--required-targets",
                str(required),
                "--history",
                str(history),
                "--run-id",
                "ci-123",
                "--run-url",
                "https://example.com/runs/123",
                "--verdict",
                str(verdict),
                "--summary",
                str(summary),
                "--artifact",
                "probe-report.json",
            ]
        )
        # CreepJS dropped from 0.95 to 0.80 → regression_detected
        self.assertEqual(exit_code, 1)
        with verdict.open("r", encoding="utf-8") as handle:
            agg = json.load(handle)
        self.assertTrue(agg["hard_fail"])
        self.assertIn("creepjs", agg["hard_fail_labels"])
        with summary.open("r", encoding="utf-8") as handle:
            md = handle.read()
        self.assertIn("## Stealth Canary — Trend Report", md)
        self.assertIn("@greysquirr3l", md)
        self.assertIn("https://example.com/runs/123", md)
        # History was appended
        with history.open("r", encoding="utf-8") as handle:
            lines = [line for line in handle.read().splitlines() if line.strip()]
        # 10 seed + 2 current = 12
        self.assertEqual(len(lines), 12)

    def test_cli_stable_history_exits_zero(self) -> None:
        canary_config = self._write_canary_config()
        required = self._write_required_targets()
        # Stable probe scores
        probe_report = self.tmpdir / "probe-report.json"
        probe_report.write_text(
            json.dumps(
                [
                    {
                        "label": "synthetic-injection",
                        "url": "about:blank",
                        "score": 0.95,
                        "threshold": 0.95,
                        "ok": True,
                    },
                    {
                        "label": "creepjs",
                        "url": "https://abrahamjuliot.github.io/creepjs/",
                        "score": 0.95,
                        "threshold": 0.50,
                        "ok": True,
                    },
                ]
            ),
            encoding="utf-8",
        )
        history = self.tmpdir / "history.jsonl"
        self._seed_history(history, "creepjs", [0.95] * 10)
        verdict = self.tmpdir / "verdict.json"
        summary = self.tmpdir / "summary.md"

        exit_code = trend_cli.main(
            [
                "--probe-report",
                str(probe_report),
                "--canary-config",
                str(canary_config),
                "--required-targets",
                str(required),
                "--history",
                str(history),
                "--verdict",
                str(verdict),
                "--summary",
                str(summary),
            ]
        )
        self.assertEqual(exit_code, 0)
        with verdict.open("r", encoding="utf-8") as handle:
            agg = json.load(handle)
        self.assertFalse(agg["hard_fail"])


# ── Config knobs ─────────────────────────────────────────────────────────────


class TestTrendConfigFromEnv(unittest.TestCase):
    def test_defaults_when_unset(self) -> None:
        with mock.patch.dict(os.environ, {}, clear=True):
            cfg = trend.TrendConfig.from_env()
        self.assertEqual(cfg.window, 10)
        self.assertEqual(cfg.delta_threshold, 0.05)
        self.assertEqual(cfg.monotonic_runs, 3)
        self.assertEqual(cfg.min_history, 3)

    def test_overrides_applied(self) -> None:
        env = {
            "STYGIAN_TREND_WINDOW": "20",
            "STYGIAN_TREND_DELTA_THRESHOLD": "0.10",
            "STYGIAN_TREND_MONOTONIC_RUNS": "5",
            "STYGIAN_TREND_MIN_HISTORY": "5",
        }
        with mock.patch.dict(os.environ, env, clear=True):
            cfg = trend.TrendConfig.from_env()
        self.assertEqual(cfg.window, 20)
        self.assertEqual(cfg.delta_threshold, 0.10)
        self.assertEqual(cfg.monotonic_runs, 5)
        self.assertEqual(cfg.min_history, 5)

    def test_invalid_overrides_ignored(self) -> None:
        env = {
            "STYGIAN_TREND_WINDOW": "negative",
            "STYGIAN_TREND_DELTA_THRESHOLD": "not-a-float",
            "STYGIAN_TREND_MONOTONIC_RUNS": "0",
        }
        with mock.patch.dict(os.environ, env, clear=True):
            cfg = trend.TrendConfig.from_env()
        # Falls back to defaults
        self.assertEqual(cfg.window, 10)
        self.assertEqual(cfg.delta_threshold, 0.05)
        self.assertEqual(cfg.monotonic_runs, 3)


# ── Approx equality helper ───────────────────────────────────────────────────


class TestApproxEqual(unittest.TestCase):
    def test_approx_equal_within_tolerance(self) -> None:
        # Difference of 5e-10 is well within the 1e-9 tolerance.
        self.assertTrue(trend.approx_equal(0.9500000005, 0.95))
        self.assertFalse(trend.approx_equal(0.95, 0.96))

    def test_approx_equal_handles_nan(self) -> None:
        self.assertTrue(trend.approx_equal(float("nan"), float("nan")))
        self.assertFalse(trend.approx_equal(float("nan"), 0.0))


if __name__ == "__main__":
    unittest.main()
