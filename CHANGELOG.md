# Changelog

All notable changes to the stygian workspace will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.11] - 2026-03-02

### Added

- `stygian-browser`: `HeadlessMode` enum with `New` (default, `--headless=new`) and `Legacy` (old `--headless`) variants — exposes Chrome's new headless rendering pipeline which shares the same code path as headed Chrome and is significantly harder to fingerprint-detect; fixes bot detection on sites like X/Twitter that reject the old headless mode before any session state is checked (closes [#13](https://github.com/greysquirr3l/stygian/issues/13))
- `stygian-browser`: `BrowserConfig::headless_mode` field and `BrowserConfigBuilder::headless_mode(HeadlessMode)` setter — opt back to `HeadlessMode::Legacy` if targeting Chromium < 112; configurable via `STYGIAN_HEADLESS_MODE` env var (`new`/`legacy`)

## [0.1.10] - 2026-03-03

### Added

- `stygian-browser`: `PageHandle::inject_cookies()` — seed session cookies on a page without a full `SessionSnapshot` round-trip and without a direct `chromiumoxide` dependency in calling code (closes [#11](https://github.com/greysquirr3l/stygian/issues/11))
- `stygian-browser`: `BrowserConfig::builder().user_data_dir(path)` builder method — set a custom Chrome profile directory; when omitted each browser instance now auto-generates a unique temporary directory (`$TMPDIR/stygian-<id>`) so concurrent pools no longer race on `SingletonLock` (closes [#12](https://github.com/greysquirr3l/stygian/issues/12))

## [0.1.9] - 2026-03-02

### Fixed

- `stygian-browser`: `set_resource_filter` previously called `Fetch.enable` but never subscribed to `Fetch.requestPaused` events, causing Chrome to intercept and hang every network request indefinitely — navigation always timed out when a resource filter was active. A background task now processes each paused event: blocked resource types receive `Fetch.failRequest(BlockedByClient)`, all others receive `Fetch.continueRequest` (closes [#9](https://github.com/greysquirr3l/stygian/issues/9))
- `stygian-browser`: Chrome launch arguments including `--disable-blink-features=AutomationControlled` were being passed with a double-dash prefix (`----arg`) because chromiumoxide's `ArgsBuilder` prepends `--` to every key and stygian was also including the `--` in the string. The arg builder in `browser.rs` now strips the `--` prefix before handing each argument to chromiumoxide, so the stealth flag reaches Chrome correctly (closes [#10](https://github.com/greysquirr3l/stygian/issues/10))

## [0.1.8] - 2026-03-02

### Changed

- Version bump to verify end-to-end release pipeline (Cargo.toml → crates.io)

## [0.1.7] - 2026-03-02

### Fixed

- `stygian-browser`: `navigate()` race condition — `EventLoadEventFired` subscription now registered **before** `goto()` so Chrome cannot fire the event before the listener is in place; previously this caused 100% timeouts with any `DomContentLoaded` or `NetworkIdle` wait strategy ([#7](https://github.com/greysquirr3l/stygian/issues/7))
- `stygian-browser`: `WaitUntil::DomContentLoaded` now subscribes to `Page.domContentEventFired` (fires when the DOM is ready, before subresources load) instead of `Page.loadEventFired`, matching its documented semantics
- `stygian-browser`: `WaitUntil::NetworkIdle` now implements genuine network-idle detection — in-flight request count is tracked via `Network.requestWillBeSent` / `Network.loadingFinished` / `Network.loadingFailed`; navigation resolves when ≤ 2 requests remain in-flight for at least 500 ms after the load event (equivalent to Playwright's `networkidle2`)

### Changed

- `stygian-browser`: `chromiumoxide` upgraded from `0.7` → `=0.9.1` (pinned exact version)
- `stygian-graph`: `JobberPlugin` promoted from unit struct to a struct with an injectable `token` field; `default_auth` now reads from `self.token` set at construction time instead of accessing the environment on every call; new `with_token(impl Into<String>)` constructor bypasses env lookup entirely — eliminates all unsafe env mutations from the test suite (closes [#5](https://github.com/greysquirr3l/stygian/pull/5), [#6](https://github.com/greysquirr3l/stygian/pull/6))

## [0.1.6] - 2026-03-02

### Security

- `stygian-browser`: added `#![deny(unsafe_code)]` — unsafe code is now a compile error in library code; test helpers that require it carry an explicit `#[allow(unsafe_code)]`
- `stygian-graph`: env-var mutations in `jobber` plugin tests are now serialised with `ENV_LOCK` (eliminates a potential race when tests run in parallel)

### Documentation

- `page-operations.md`: added `status_code()` to the "Reading page content" snippet and the complete usage example

## [0.1.5] - 2026-03-02

### Added

- `stygian-browser`: `PageHandle::url()` — returns the post-navigation, post-redirect page URL via CDP `Target.getTargetInfo` ([#1](https://github.com/greysquirr3l/stygian/issues/1))
- `stygian-browser`: `PageHandle::status_code()` — returns the HTTP status code of the most recent main-frame navigation, captured atomically from `Network.responseReceived` before `goto()` is called; returns `None` for `file://` navigations or before `navigate()` is called ([#1](https://github.com/greysquirr3l/stygian/issues/1))

## [0.1.4] - 2026-03-02

### Fixed

- **Security**: `--no-sandbox` was passed unconditionally to Chrome, disabling Chromium's built-in renderer sandbox on bare-metal hosts. It is now only passed when running inside a container (auto-detected via `/.dockerenv` / `/proc/1/cgroup` on Linux) or when explicitly set via `STYGIAN_DISABLE_SANDBOX=true`. macOS and Windows are never affected (their native sandbox mechanisms differ). (`stygian-browser`)
- Fixed fmt CI failures: import ordering across 23 files in `stygian-graph`

### Changed

- `BrowserConfig` gains a `disable_sandbox: bool` field and `.disable_sandbox()` builder method
- `STYGIAN_DISABLE_SANDBOX` env var overrides auto-detection
- Updated `stygian-browser` README: `STYGIAN_DISABLE_SANDBOX` added to env-var table; FAQ expanded with sandbox guidance
- Repository homepage updated from `greysquirr3l.github.io/mycelium` to `greysquirr3l.github.io/stygian`
- Documentation links point to GitHub Pages instead of docs.rs (crates not yet on crates.io)

## [0.1.3] - 2026-03-02

### Changed

- Relicensed from `MIT OR Apache-2.0` to `AGPL-3.0-only`
- Replaced `LICENSE-MIT` and `LICENSE-APACHE` with a single `LICENSE` (AGPL-3.0)
- Updated license badges and license sections in all README files
- Updated `deny.toml` to reflect AGPL-3.0-only as the project license

## [0.1.2] - 2026-03-01

### Changed

- Renamed all `MYCELIUM_*` environment variables to `STYGIAN_*` across both crates, docs, examples, and plan files
- Updated JSON schema `$id` URIs from `https://mycelium/schemas/...` to `https://stygian/schemas/...`
- Updated `.gitattributes` workspace comment to reference stygian

### Fixed

- Clippy `redundant_closure_for_method_calls`: replaced `|e| e.into_inner()` with `PoisonError::into_inner` in `config.rs`
- Clippy `default_constructed_unit_structs`: replaced `PipelineExecutor::default()` with direct struct literal in test
- Clippy `useless_format`: replaced `format!(r#"..."#)` with `.to_string()` in `pipeline_parser.rs` test
- Added `#[allow(clippy::missing_const_for_fn)]` with explanation to `rss_bytes()` — non-Linux targets see only `{ 0 }` but the Linux branch uses file I/O, making `const fn` impossible cross-platform

## [0.1.1] - 2026-03-01

### Added

- GitHub Actions CI/CD workflows: `ci.yml`, `docs.yml`, `release.yml`, `scorecard.yml`, `security.yml`
- `deny.toml` cargo-deny configuration for supply-chain security (advisories, licenses, bans)
- mdBook documentation site (`book/`) with 17 chapters across three parts: Graph Engine, Browser Automation, and Reference
- GitHub Pages deployment via `docs.yml` — mdBook at `/` with rustdoc API reference merged at `/api/`
- Additional unit tests for `FileStorage`, `Config`, `Executor`, and `Cli` modules (storage cross-pipeline retrieve, default config assertions, executor concurrency, CLI parse coverage)
- OpenAI, Gemini, and GitHub Copilot AI adapter test coverage improvements

### Changed

- README files updated with CI status badges and coverage reporting
- `.gitignore` extended to exclude `book/book/` build output and Coraline artifacts

### Added

#### stygian-graph

- Initial release of graph-based scraping engine
- Hexagonal architecture with ports and adapters pattern
- HTTP adapter with configurable timeouts and retries
- Browser adapter with JavaScript rendering support (via stygian-browser)
- AI extraction adapters: Claude, OpenAI, Gemini, GitHub Copilot, Ollama
- Multi-modal support for images, PDFs, videos
- Distributed execution via work queue abstraction
- Local work queue implementation for single-node deployments
- Pipeline validation: cycle detection, reachability checks, node validation
- Idempotency system for safe retries
- Circuit breaker pattern for graceful degradation
- Prometheus metrics collection
- Comprehensive test suite (280+ tests)

#### stygian-browser

- Initial release of anti-detection browser automation library
- Browser pool with warm instance reuse (<100ms acquisition)
- CDP-based automation via chromiumoxide
- Stealth features: navigator spoofing, canvas noise, WebGL randomization
- Human behavior simulation: Bézier mouse paths, realistic typing
- Configurable wait strategies: DOM loaded, network idle, selector waits
- Page lifecycle management with automatic cleanup
- Resource management and memory monitoring
- Comprehensive test suite (80+ tests)

### Changed

- N/A (initial release)

### Deprecated

- N/A (initial release)

### Removed

- N/A (initial release)

### Fixed

- N/A (initial release)

### Security

- N/A (initial release)

---

## [0.1.0] - 2026-02-28

Initial pre-release for testing and validation.

### Notes

This is a pre-1.0 release. Breaking changes may occur between minor versions until 1.0.0 is reached.

Both crates are functional and well-tested, but APIs may evolve based on community feedback.

---

[Unreleased]: https://github.com/greysquirr3l/stygian/compare/v0.1.9...HEAD
[0.1.9]: https://github.com/greysquirr3l/stygian/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/greysquirr3l/stygian/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/greysquirr3l/stygian/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/greysquirr3l/stygian/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/greysquirr3l/stygian/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/greysquirr3l/stygian/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/greysquirr3l/stygian/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/greysquirr3l/stygian/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/greysquirr3l/stygian/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/greysquirr3l/stygian/releases/tag/v0.1.0
