# Changelog

All notable changes to the mycelium workspace will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
- `.gitignore` extended to exclude `book/book/` build output and Coraline artefacts

### Added

#### mycelium-graph

- Initial release of graph-based scraping engine
- Hexagonal architecture with ports and adapters pattern
- HTTP adapter with configurable timeouts and retries
- Browser adapter with JavaScript rendering support (via mycelium-browser)
- AI extraction adapters: Claude, OpenAI, Gemini, GitHub Copilot, Ollama
- Multi-modal support for images, PDFs, videos
- Distributed execution via work queue abstraction
- Local work queue implementation for single-node deployments
- Pipeline validation: cycle detection, reachability checks, node validation
- Idempotency system for safe retries
- Circuit breaker pattern for graceful degradation
- Prometheus metrics collection
- Comprehensive test suite (280+ tests)

#### mycelium-browser

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

[Unreleased]: https://github.com/greysquirr3l/mycelium/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/greysquirr3l/mycelium/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/greysquirr3l/mycelium/releases/tag/v0.1.0
