# Charon Baseline Fixtures

This directory stores deterministic baseline fixtures used for compatibility and regression checks.

## Reproducible generation command

Run from repository root:

```bash
.github/scripts/generate-charon-fixtures.sh
```

Optional arguments:

```bash
.github/scripts/generate-charon-fixtures.sh <output-dir> <generation-version>
```

Defaults:
- output-dir: `crates/stygian-charon/docs/examples/fixtures`
- generation-version: `v1`

## Baseline metadata contract

Each generated fixture includes metadata fields:
- `metadata.fixture_source`: path of the source snapshot used to generate the fixture
- `metadata.fixture_generation_version`: generator/version marker for traceability

A companion `manifest.json` includes:
- `generation_version`
- `sources`
- `fixtures`

## Review process before merge

1. Run `.github/scripts/generate-charon-fixtures.sh`.
2. Review diff in `crates/stygian-charon/docs/examples/fixtures`.
3. Confirm only expected snapshot/metadata changes are present.
4. Confirm no volatile fields (`capture_nonce`, `generated_at`, `request_id`, `run_id`, `session_id`, `trace_id`) appear in fixture metadata.
5. Commit regenerated fixtures in the same PR as schema/collector changes.

CI enforces this by rerunning generation and failing if fixture output differs from the committed baseline.
