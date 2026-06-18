# Backwards Compatibility Policy

This page tracks API and behavioural compatibility guarantees for the
stygian workspace, plus migration guidance for consumers upgrading
across releases.

The project is **pre-1.0** (currently `0.13.x`). Per the
[OpenSSF best practices for pre-1.0 crates](https://www.bestpractices.dev/),
breaking changes are allowed in minor releases but should be:
- documented in the PR description,
- called out in `PROGRESS.md` accumulated learnings,
- covered with migration steps in this guide.

---

## Active compatibility notes

The following compatibility observations apply to the latest
phase-13 feature wave. Each entry lists the change, the version it
shipped in, who is affected, and how to migrate.

### `stygian-charon` default features now include `caching`

**Shipped in:** `0.13.x` (Phase 13 wave)

Consumers depending on `stygian-charon` with default features now
transitively pull in the `lru` crate (via the T83/T88/T91/T93 modules
that share the `LruTtlStore` primitive).

**Who is affected:** Any consumer using `stygian-charon = "0.13"` (or
newer) without explicitly opting out of default features.

**Migration:**

```toml
# Preserve previous behaviour
stygian-charon = { version = "0.13", default-features = false }
```

No source-level changes are required for downstream code that does
not import the new `challenge_feedback`, `change_feed`,
`pow_profile`, or `token_lifecycle` modules.

### Serde representation of 4 `stygian-charon` enums changed to snake_case

**Shipped in:** `0.13.x` (Phase 13 wave)

`TargetClass`, `ExecutionMode`, `SessionMode`, and `TelemetryLevel` now
serialise as `snake_case` instead of the prior `PascalCase` default.

Example:

| Before | After |
|---|---|
| `"target_class": "ContentSite"` | `"target_class": "content_site"` |
| `"execution_mode": "Browser"` | `"execution_mode": "browser"` |

**Who is affected:** Any consumer with stored `InvestigationBundle`,
`RuntimePolicy`, or `RequirementsProfile` JSON / TOML / YAML payloads
in PascalCase. In-memory types are unchanged.

**Migration:** regenerate stored payloads from the current version, or
pin to the previous `0.13.x` patch release that still emits
PascalCase. A one-shot migration script can `jq` the affected string
fields.

### New `pub` fields on non-`#[non_exhaustive]` structs

**Shipped in:** `0.13.x` (Phase 13 wave)

The following structs gained new `pub Option<…>` fields without
`#[non_exhaustive]`, breaking any downstream code that constructs them
via struct literals:

| Struct | Crate | New fields |
|---|---|---|
| `DiagnosticReport` | `stygian-browser` | `transport_realism`, `integrity_canary` |
| `AcquisitionRequest` | `stygian-browser` | `freshness_contract`, `replay_defense`, `transport_realism`, `interstitial` |
| `AcquisitionResult` | `stygian-browser` | `freshness`, `replay_defense`, `transport_realism`, `interstitial` |
| `ExtractionMetadata` | `stygian-plugin` | `reliability` |

All new fields are properly annotated
`#[serde(default, skip_serializing_if = "Option::is_none")]`, so
**serialization is safe**; only struct-literal Rust callers break.

**Who is affected:** Downstream code that constructs these structs
directly via struct literal.

**Migration:** use `Default::default()` plus the builder-style
methods (`with_freshness`, `with_integrity_canary`, `with_reliability`,
etc.), or pin to the previous `0.13.x` patch release.

### `StageFailureKind` gained two variants

**Shipped in:** `0.13.x` (Phase 13 wave)

`crates/stygian-browser/src/acquisition.rs`: added
`ReplayDefenseTriggered` and `InterstitialRouted` variants to a
non-`#[non_exhaustive]` enum.

**Who is affected:** Downstream code that does exhaustive pattern
matches on `StageFailureKind` without a wildcard arm.

**Migration:** add a `_` arm to any exhaustive `match`, or pin to the
previous `0.13.x` patch release.

---

## Recent risky changes (no source-level break)

These do not break compilation but may affect observable behaviour.
Document them in your test plans before upgrading.

### `hickory-resolver` 0.24 → 0.26.1 in `stygian-proxy`

**Shipped in:** `0.13.x` (Phase 13 wave)

Under the existing `dns-fetcher` feature. The `tokio-runtime`
feature was renamed to `tokio`. New transitive dependencies:
`hickory-net`, `moka`, `crossbeam-channel`, `critical-section`,
`ndk-context`, `jni`, `prefix-trie`, `tagptr`, `simdutf8`, `simd_cesu8`.

No public API change, but the dependency graph shifts. Cargo's
`[patch.crates-io]` or `[replace]` directives targeting the old
versions will need updating.

### `NodeHandle::outer_html()` now has a JS-evaluation fallback

**Shipped in:** `0.13.x` (Phase 13 wave)

`crates/stygian-browser/src/page.rs`: if the primary CDP path returns
an empty payload, the method now falls back to a JS evaluation
(`this.outerHTML` via `XMLSerializer`) before returning `""`.

Signature unchanged. Highly dynamic pages that previously returned
empty may now return the serialised DOM. Any consumer that asserted
on empty results for known-empty elements will need to update its
expectations.

### `mul_add` reorder in `build_runtime_policy`

**Shipped in:** `0.13.x` (Phase 13 wave)

`crates/stygian-charon/src/policy.rs`: `f64::mul_add` is bit-exact
vs. the prior two-rounding expression. Output may differ by 1 ULP
in edge cases. The score is clamped to `[0.0, 1.0]` and the function
is deterministic, so this should not produce visibly different
runtime behaviour — but consumers with snapshot tests on exact
float values should regenerate those snapshots after upgrading.

### MCP `acquisition_result_to_tool_output` gains a top-level `freshness` field

**Shipped in:** `0.13.x` (Phase 13 wave)

`crates/stygian-browser/src/mcp.rs`: the JSON object returned to
MCP clients of the browser server now includes a top-level
`"freshness"` key.

Strict JSON-schema validators that assert no additional properties
may fail. Most MCP clients ignore unknown fields; this is a minor
concern for downstream consumers of the MCP surface.

### `cargo l` alias now passes `--all-features`

**Shipped in:** `0.13.x` (Phase 13 wave)

`.cargo/config.toml`: the `l` alias now runs strict clippy with
`--all-features` permanently. Local preflight takes longer (compiles
feature-gated lints). Intentional — codifies the feature-gated
lesson learned during Phase 13 (see `PROGRESS.md`
`T-feature-gated-clippy:` entry).

---

## How compatibility changes are decided

1. **Additive changes are preferred.** New modules, new optional
   fields with `#[serde(default, skip_serializing_if = "Option::is_none")]`,
   new trait impls on local types, and new variants on enums marked
   `#[non_exhaustive]` are all preferred over breaking changes.

2. **Breaking changes require a documented migration path** in
   this page AND in the PR description before merge.

3. **Pre-1.0 minor releases may include breaking changes** with
   the conditions above. Post-1.0 (planned for a future major
   version line), breaking changes will be limited to major releases.

4. **Behavioural drift without a signature change** (e.g.
   `NodeHandle::outer_html()`'s new fallback) is documented as
   "risky" with concrete test-impact guidance.

---

## Compatibility tools

- **`cargo l`** — strict workspace clippy with `--all-features`. The
  CI gate for new code.
- **`cargo test --workspace --all-features`** — full test suite
  (48 groups, 11 ignored live tests).
- **`cargo build --workspace --all-features`** — release build gate.
- **`actionlint`** for `.github/workflows/*.yml` files.

---

## Related

- [Security Policy](./security-policy.md)
- [Environment Variables](./env-vars.md)
- [Testing & Coverage](./testing.md)
- [Charon Release Notes](./charon-release.md)