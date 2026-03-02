# Changelog

All notable changes to the stygian workspace will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/greysquirr3l/stygian/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/greysquirr3l/stygian/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/greysquirr3l/stygian/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/greysquirr3l/stygian/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/greysquirr3l/stygian/releases/tag/v0.1.0
