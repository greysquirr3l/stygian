# Backwards Compatibility Policy

This page tracks API and behavioural compatibility guarantees for the
stygian workspace, plus migration guidance for consumers upgrading
across releases.

The project is **pre-1.0** (currently `0.14.x`). Per the
[OpenSSF best practices for pre-1.0 crates](https://www.bestpractices.dev/),
breaking changes are allowed in minor releases but should be:
- documented in the PR description,
- called out in `PROGRESS.md` accumulated learnings,
- covered with migration steps in this guide.

---

## 0.14.0 (Phase 14 wave — 2026-06-19)

The 0.14.0 cut is **strictly additive**: every change is a new public API
addition or a new optional field with `#[serde(default)]`. No signatures
were renamed, removed, or reordered. No `pub` enums gained variants. No
behavioural defaults changed in a way that breaks existing tests.

Migration burden for downstream consumers: **none** for code that does
not explicitly opt into the new types. For code that wants to use them,
each new type is in a new module — adding `use stygian_proxy::stickiness::*;`
or `use stygian_browser::OuterHtmlStrategy` is the only change required.

### New modules and types

| Crate | New type / module | Purpose |
| --- | --- | --- |
| `stygian-proxy` | `stickiness::{StickinessPolicy, VendorStickinessMap, SessionDecision}` (feature `vendor-stickiness`) | Per-vendor sticky session routing |
| `stygian-proxy` | `coherence::{CoherencePort, DefaultCoherenceValidator, CoherenceContext, CoherencePolicy, CoherenceVerdict, MismatchField, MismatchSeverity}` (feature `coherence-validation`) | Five-vector identity coherence check |
| `stygian-proxy` | `strategy::thompson::ThompsonStrategy` (feature `bayesian-rotation`) | Per-proxy `Beta(α, β)` Bayesian rotation |
| `stygian-proxy` | `vendor_quirks::{VendorQuirk, QuirkSeverity, QuirkMatch, ProxyUrl, check, VENDOR_QUIRKS, CRAWLERA_8011_QUIRK, ZYTE_8011_QUIRK, BRD_SUPERPROXY_QUIRK, IPROYAL_QUIRK}` (always compiled) | Ingest-time vendor URL trap detection |
| `stygian-browser` | `OuterHtmlStrategy`, `OuterHtmlResult`, `NodeHandle::outer_html_with_strategy` | Deep outer-HTML resolution (issue #66) |

### New `pub` fields on existing types (safe serde, safe struct literal with `..Default::default()`)

| Type | New fields |
| --- | --- |
| `ProxyCapabilities` (`stygian-proxy`) | `ip_class: IpClass`, `target_compatibility: TargetVendorCompatibility`, `asn: Option<u32>`, `city: Option<String>`, `postal_code: Option<String>` |
| `Proxy` (`stygian-proxy`) | `ip_class: IpClass`, `target_compatibility: TargetVendorCompatibility` |
| `CapabilityRequirement` (`stygian-proxy`) | `require_ip_class: Option<IpClassRequirement>`, `target_vendor: Option<VendorId>`, `require_asn: Option<u32>`, `require_city: Option<String>`, `require_postal_code: Option<String>` |

All new fields are `#[serde(default, skip_serializing_if = "Option::is_none")]`
(or `#[serde(default)]` for plain types like `IpClass`). Serialised
`Proxy` / `ProxyCapabilities` / `CapabilityRequirement` payloads from
0.13.x deserialise unchanged into 0.14.0. Struct-literal Rust callers
can either use `..Default::default()` or pin to 0.13.x.

### New error variants

| Error | New variants | `#[non_exhaustive]`? |
| --- | --- | --- |
| `stygian_proxy::error::ProxyError` | `CoherenceMismatch { field, observed, expected }`, `InvalidGeoMetadata { reason }` | No (the enum is already `#[non_exhaustive]`) |

`ProxyError` was already `#[non_exhaustive]` going into 0.14.0, so
exhaustive pattern matches in downstream code are not broken by the
new variants. If you have wildcard `_ =>` arms you will see no compile
break; if you have an exhaustive `match` (rare), the `#[non_exhaustive]`
attribute already requires a wildcard arm.

### Behavioural drift (no signature change)

- **Free-list fetchers now tag ingested proxies as `IpClass::Datacenter`** with
  `TargetVendorCompatibility::default_blocked()` so operators cannot
  accidentally route premium-vendor traffic through public free-list
  pools. Consumers using `FreeListFetcher`, `FreeApiProxiesFetcher`,
  or `DnsTxtFetcher` without an explicit `target_vendor` filter will
  see the same proxies as before but with a vendor-compat gate that
  fails closed. **Who is affected:** any consumer whose free-list
  pipeline relied on `target_vendor = None` as a permissive default.
  **Migration:** add `target_vendor: None` explicitly to the
  `CapabilityRequirement` only if you genuinely want to bypass the
  gate; otherwise leave the field absent so the gate stays fail-secure.

- **`NodeHandle::outer_html()` is now a thin wrapper** around
  `outer_html_with_strategy(OuterHtmlStrategy::Current)`. The existing
  `Result<String>` contract is preserved (Empty and Failed both flatten
  to `Ok(String::new())`). Consumers that want to distinguish Empty /
  Content / Failed should call `outer_html_with_strategy` directly and
  inspect the `OuterHtmlResult` variant. The new `Recursive` strategy
  resolves the Wix Studio / Editor X / large-SPA empty-payload case
  (issue #66) without any Wix-specific selectors.

---

## Active compatibility notes (carry-over from 0.13.x)

The following observations still apply. New consumers upgrading from
0.12.x or earlier should read these too.

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
stygian-charon = { version = "0.14", default-features = false }
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

### `StageFailureKind` gained two variants (0.13.x, still active)

`crates/stygian-browser/src/acquisition.rs`: added
`ReplayDefenseTriggered` and `InterstitialRouted` variants to a
non-`#[non_exhaustive]` enum. Downstream exhaustive `match` without
a wildcard arm may need a `_` arm.

---

## Recent risky changes (no source-level break)

These do not break compilation but may affect observable behaviour.
Document them in your test plans before upgrading.

### `NodeHandle::outer_html()` now has a `Recursive` strategy (0.14.0)

`crates/stygian-browser/src/page.rs`: the new
`outer_html_with_strategy(OuterHtmlStrategy::Recursive)` uses CDP
`DOM.getOuterHTML` (single round-trip, browser-side serialisation,
shadow-DOM included by default) with a Rust-side `DOM.describeNode`
walk fallback. Consumers that previously observed intermittent empty
payloads from heavily dynamic pages (Wix Studio / Editor X meshes,
large SPAs, shadow-DOM subtrees) may now see content they used to
see as empty. The legacy `Current` strategy is the default and
preserves the historical behaviour.

### Free-list ingest tags `IpClass::Datacenter` (0.14.0)

See the "Behavioural drift" section above. Documented again here for
visibility — any test that asserts on the `ip_class` of an
auto-ingested free-list proxy must be updated.

### `hickory-resolver` 0.24 → 0.26.1 in `stygian-proxy` (0.13.x)

Under the existing `dns-fetcher` feature. The `tokio-runtime`
feature was renamed to `tokio`. New transitive dependencies:
`hickory-net`, `moka`, `crossbeam-channel`, `critical-section`,
`ndk-context`, `jni`, `prefix-trie`, `tagptr`, `simdutf8`, `simd_cesu8`.

No public API change, but the dependency graph shifts. Cargo's
`[patch.crates-io]` or `[replace]` directives targeting the old
versions will need updating.

### `NodeHandle::outer_html()` now has a JS-evaluation fallback (0.13.x)

`crates/stygian-browser/src/page.rs`: if the primary CDP path returns
an empty payload, the method now falls back to a JS evaluation
(`this.outerHTML` via `XMLSerializer`) before returning `""`.

Signature unchanged. Highly dynamic pages that previously returned
empty may now return the serialised DOM. Any consumer that asserted
on empty results for known-empty elements will need to update its
expectations.

### `mul_add` reorder in `build_runtime_policy` (0.13.x)

`crates/stygian-charon/src/policy.rs`: `f64::mul_add` is bit-exact
vs. the prior two-rounding expression. Output may differ by 1 ULP
in edge cases. The score is clamped to `[0.0, 1.0]` and the function
is deterministic, so this should not produce visibly different
runtime behaviour — but consumers with snapshot tests on exact
float values should regenerate those snapshots after upgrading.

### MCP `acquisition_result_to_tool_output` gains a top-level `freshness` field (0.13.x)

`crates/stygian-browser/src/mcp.rs`: the JSON object returned to
MCP clients of the browser server now includes a top-level
`"freshness"` key.

Strict JSON-schema validators that assert no additional properties
may fail. Most MCP clients ignore unknown fields; this is a minor
concern for downstream consumers of the MCP surface.

### `cargo l` alias now passes `--all-features` (0.13.x)

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
  (lib + integration + doctest across all 7 crates; a handful of
  `#[ignore]` tests require live Chrome).
- **`cargo build --workspace --all-features`** — release build gate.
- **`actionlint`** for `.github/workflows/*.yml` files.

---

## Related

- [Security Policy](./security-policy.md)
- [Environment Variables](./env-vars.md)
- [Testing & Coverage](./testing.md)
- [Charon Release Notes](./charon-release.md)
