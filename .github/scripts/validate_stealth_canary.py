#!/usr/bin/env python3
"""Validate .github/stealth-canary.toml shape and safety constraints."""

from __future__ import annotations

import pathlib
import sys
import tomllib


def fail(message: str) -> None:
    print(f"stealth-canary config error: {message}", file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    config_path = pathlib.Path(".github/stealth-canary.toml")
    if not config_path.exists():
        fail(f"missing config file: {config_path}")

    try:
        data = tomllib.loads(config_path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        fail(f"invalid TOML: {exc}")

    canaries = data.get("canary")
    if not isinstance(canaries, list) or not canaries:
        fail("expected at least one [[canary]] entry")

    has_non_advisory = False

    for idx, entry in enumerate(canaries, start=1):
        if not isinstance(entry, dict):
            fail(f"entry #{idx} must be a table")

        url = entry.get("url")
        if not isinstance(url, str) or not url.strip():
            fail(f"entry #{idx} requires non-empty string field 'url'")

        label = entry.get("label")
        if not isinstance(label, str) or not label.strip():
            fail(f"entry #{idx} requires non-empty string field 'label'")

        threshold = entry.get("threshold", 0.90)
        if not isinstance(threshold, (int, float)):
            fail(f"entry #{idx} field 'threshold' must be numeric")
        threshold = float(threshold)
        if threshold < 0.0 or threshold > 1.0:
            fail(f"entry #{idx} field 'threshold' must be within [0.0, 1.0]")

        advisory = entry.get("advisory", False)
        if not isinstance(advisory, bool):
            fail(f"entry #{idx} field 'advisory' must be boolean when present")

        if not advisory:
            has_non_advisory = True

    if not has_non_advisory:
        fail("at least one non-advisory canary entry is required")

    print(f"validated {len(canaries)} canary entries from {config_path}")


if __name__ == "__main__":
    main()
