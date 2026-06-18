# Target-Class Playbooks Integration Guide (T85)

This guide covers the `playbooks` module in `stygian-charon`.

## What it does

The playbooks module codifies anti-bot strategy per **target class** to reduce operator guesswork and configuration drift. Each [`Playbook`](https://docs.rs/stygian-charon/latest/stygian_charon/playbooks/struct.Playbook.html) bundles four operator-facing knobs:

1. **Acquisition defaults** — mode, execution mode, session mode, telemetry level, sticky-session TTL, retry budget, backoff base, warmup flag.
2. **Proxy preference** — preferred wire protocol, sticky / residential flags, max latency.
3. **Pacing profile** — sustained rate limit, jitter, minimum inter-request interval.
4. **Escalation strategy** — `Capped { ceiling }` or `Linear { steps }`.

The [`PlaybookResolver`](https://docs.rs/stygian-charon/latest/stygian_charon/playbooks/struct.PlaybookResolver.html) merges three layers deterministically:

```
request override  >  playbook default  >  global default
```

Every field on the resulting [`ResolvedPlaybook`](https://docs.rs/stygian-charon/latest/stygian_charon/playbooks/struct.ResolvedPlaybook.html) carries a [`ResolutionSource`](https://docs.rs/stygian-charon/latest/stygian_charon/playbooks/enum.ResolutionSource.html) tag so downstream observers can verify which layer contributed the value.

## Baseline playbooks

Four baseline TOML data files ship in `crates/stygian-charon/data/playbooks/`:

| Id              | Target class  | Mode      | Notes                              |
| --------------- | ------------- | --------- | ---------------------------------- |
| `tier1-static`  | `content_site` | `fast`    | HTTP-direct, no warmup, low retry  |
| `tier1-js`      | `content_site` | `resilient` | Browser-stealth, sticky session    |
| `tier2-hostile` | `high_security` | `hostile` | Sticky residential proxies, deep telemetry |
| `unknown`       | `unknown`     | `resilient` | Always-safe fallback              |

Each file is **embedded into the binary at compile time** via `include_str!`. The resolver's `with_builtin_defaults()` constructor parses + validates every embedded file; if any baseline is broken, the resolver fails fast on startup.

## Feature flag

Default-on. The `playbooks` module is part of the `stygian-charon` default feature set (no new feature gate).

## Loader / validator

```rust
use stygian_charon::playbooks::PlaybookResolver;

let resolver = PlaybookResolver::with_builtin_defaults();
assert!(resolver.contains("tier1-static"));
assert!(resolver.contains("tier1-js"));
assert!(resolver.contains("tier2-hostile"));
assert!(resolver.contains("unknown"));
```

The embedded TOML files are validated at compile time by the
`compile_check_builtin_playbooks` test in
`crates/stygian-charon/src/playbooks/builtin.rs`. A broken baseline is a
build failure, not a runtime panic.

## Resolution example

```rust
use stygian_charon::playbooks::{
    AcquisitionOverrides, PlaybookOverrides, PlaybookResolver,
};
use stygian_charon::acquisition::AcquisitionModeHint;
use stygian_charon::types::{ExecutionMode, SessionMode, TargetClass};

let resolver = PlaybookResolver::with_builtin_defaults();
let overrides = PlaybookOverrides {
    acquisition: AcquisitionOverrides {
        mode: Some(AcquisitionModeHint::Hostile),
        execution_mode: Some(ExecutionMode::Browser),
        ..AcquisitionOverrides::default()
    },
    ..PlaybookOverrides::default()
};
let resolved = resolver
    .resolve(TargetClass::ContentSite, "tier1-js", &overrides)
    .expect("resolve");

// Request override wins for the fields it sets.
assert_eq!(resolved.acquisition.mode, AcquisitionModeHint::Hostile);
assert_eq!(resolved.acquisition.execution_mode, ExecutionMode::Browser);

// Playbook default fills the rest.
assert_eq!(resolved.acquisition.session_mode, SessionMode::Sticky);
```

## Actionable validation errors

Every [`ValidationError`](https://docs.rs/stygian-charon/latest/stygian_charon/playbooks/enum.ValidationError.html) variant embeds the **playbook id**, **field path**, and **bad value**:

```text
playbook 'broken': field 'acquisition.retry_budget' has invalid value '0': retry_budget must be > 0
```

The `field_path()` and `bad_value()` accessors let operator tooling extract
the structured fields for a GUI / IDE plugin without re-parsing the
display message.

## Driving `AcquisitionRunner`

The `ResolvedPlaybook` carries every field a downstream `AcquisitionRunner`
config needs:

- `mode` → `AcquisitionRequest::mode` (Fast / Resilient / Hostile / Investigate)
- `retry_budget` → `AcquisitionRequest::max_retries`
- `backoff_base_ms` → `AcquisitionRequest::backoff_base_ms`
- `enable_warmup` → `AcquisitionRequest::enable_warmup`
- `sticky_session` (derived from `session_mode == Sticky`) → `BrowserPool::acquire_for(host)`
- `telemetry_level` → `DiagnosticReport::telemetry_level`

Use `resolved.to_runtime_policy_hints()` to feed the resolved acquisition
block into the canonical `stygian_charon::acquisition::map_policy_hints`
helper, which produces the same `AcquisitionPolicy` the downstream runner
expects.

## Custom playbooks

Operators can register custom playbooks via `PlaybookResolver::from_playbooks`:

```rust
use stygian_charon::playbooks::{
    AcquisitionDefaults, EscalationStrategy, PacingProfile, Playbook,
    PlaybookResolver, ProxyPreference,
};
use stygian_charon::acquisition::AcquisitionModeHint;
use stygian_charon::types::{ExecutionMode, SessionMode, TargetClass, TelemetryLevel};

let custom = Playbook {
    id: "my-tier3-rotate".to_string(),
    target_class: TargetClass::HighSecurity,
    description: "rotate residential proxy every 30s".to_string(),
    acquisition: AcquisitionDefaults {
        mode: AcquisitionModeHint::Hostile,
        execution_mode: ExecutionMode::Browser,
        session_mode: SessionMode::Sticky,
        telemetry_level: TelemetryLevel::Deep,
        sticky_session_ttl_secs: Some(30),
        enable_warmup: true,
        retry_budget: 3,
        backoff_base_ms: 400,
    },
    proxy_preference: ProxyPreference {
        preferred_protocol: "https".to_string(),
        require_sticky: true,
        require_residential: true,
        max_latency_ms: Some(500),
    },
    pacing: PacingProfile {
        rate_limit_rps: 0.2,
        jitter_pct: 0.30,
        min_request_interval_ms: 5_000,
    },
    escalation: EscalationStrategy::Linear {
        steps: vec![
            AcquisitionModeHint::Fast,
            AcquisitionModeHint::Resilient,
            AcquisitionModeHint::Hostile,
        ],
    },
};

let resolver = PlaybookResolver::from_playbooks(vec![custom])
    .expect("validate");
assert!(resolver.contains("my-tier3-rotate"));
```

## Migration guide

T85 is **additive only**. The public `AcquisitionRunner` config schema is
unchanged. Existing callers that build an `AcquisitionRequest` directly
keep working; new callers can opt into the resolver to pick up the
codified defaults.

The only existing enum renamed by T85 is:

- `TargetClass` (`Api` / `ContentSite` / `HighSecurity` / `Unknown`):
  JSON serialization changed from PascalCase to `snake_case`
  (`"content_site"`, `"high_security"`, etc.). No existing call sites
  relied on the PascalCase JSON form, so the rename is backward
  compatible. `serde_json::to_string(&TargetClass::ContentSite)` now
  produces `"\"content_site\""` (previously `"\"ContentSite\""`).
- `ExecutionMode` / `SessionMode` / `TelemetryLevel`: same
  snake_case rename, same backward-compat status.
- `AcquisitionModeHint` already used snake_case before T85; no change.

If your downstream tooling depends on the PascalCase form, pin to
`< 0.13.6` for the affected enums and file an issue.

## Tests

- `cargo test -p stygian-charon --lib` exercises the schema validation,
  default construction, and resolver precedence logic.
- `cargo test -p stygian-charon --test playbooks_integration` covers
  the full request-override / playbook / global-default precedence matrix
  end-to-end, plus the `#[ignore]`-gated
  `resolved_playbook_drives_acquisition_runner_config` test that confirms
  the resolved playbook contains every field a real `AcquisitionRunner`
  config needs.

Run the ignored test on demand:

```sh
cargo test -p stygian-charon --test playbooks_integration \
    resolved_playbook_drives_acquisition_runner_config -- --ignored --nocapture
```