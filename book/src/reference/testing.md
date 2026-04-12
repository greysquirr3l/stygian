# Testing & Coverage

---

## Running tests

```bash
# All workspace tests (no Chrome required)
cargo test --workspace

# All features including browser integration tests
cargo test --workspace --all-features

# Run only the tests that need Chrome (previously ignored)
cargo test --workspace --all-features -- --include-ignored

# Specific crate
cargo test -p stygian-graph
cargo test -p stygian-browser
```

---

## Test organisation

### stygian-graph

Tests live alongside their module in `src/` (unit tests) and in `crates/stygian-graph/tests/`
(integration tests).

| Layer | Location | Approach |
| --- | --- | --- |
| Domain | `src/domain/*/tests` | Pure Rust, no I/O |
| Adapters | `src/adapters/*/tests` | Mock port implementations |
| Application | `src/application/*/tests` | In-process service registry |
| Integration | `tests/` | End-to-end with real HTTP (httpbin) |

All tests in the graph crate pass without any external services.

### stygian-browser

Browser tests fall into two categories:

| Category | Attribute | Runs in CI |
| --- | --- | --- |
| Pure-logic tests (config, stealth scripts, math) | none | ✅ always |
| Integration tests (real Chrome required) | `#[ignore = "requires Chrome"]` | ❌ opt-in |

To run integration tests locally:

```bash
# Ensure Chrome 120+ is on PATH, then:
cargo test -p stygian-browser --all-features -- --include-ignored
```

---

## Coverage

Coverage is measured with
[`cargo-tarpaulin`](https://github.com/xd009642/tarpaulin).

### Install

```bash
cargo install cargo-tarpaulin
```

### Measure

```bash
# Workspace summary (excludes Chrome-gated tests)
cargo tarpaulin --workspace --all-features --ignore-tests --out Lcov

# stygian-graph only
cargo tarpaulin -p stygian-graph --all-features --ignore-tests --out Lcov

# stygian-browser logic-only (no Chrome)
cargo tarpaulin -p stygian-browser --lib --ignore-tests --out Lcov
```

### Current numbers

| Scope | Line coverage | Notes |
| --- | --- | --- |
| **Workspace** | **65.74 %** | 2 882 / 4 384 lines · 1639 tests |
| `stygian-graph` | ~72 % | All unit and integration logic covered |
| `stygian-browser` | structurally bounded | Chrome-gated tests excluded from CI |

High-coverage modules in `stygian-graph`:

| Module | Coverage |
| --- | --- |
| `application/config.rs` | ~100 % |
| `application/executor.rs` | ~100 % |
| `domain/idempotency.rs` | ~100 % |
| `application/registry.rs` | ~100 % |
| `adapters/claude.rs` | ~95 % |
| `adapters/openai.rs` | ~95 % |
| `adapters/gemini.rs` | ~95 % |

### Why browser coverage is bounded

Every test that launches a real Chrome instance is annotated:

```rust
#[tokio::test]
#[ignore = "requires Chrome"]
async fn pool_acquire_release() {
    // ...
}
```

This keeps `cargo test` fast and green in CI environments without a Chrome binary.
Pure-logic coverage (fingerprint generation, stealth script math, config validation,
simulator algorithms) is high; only the CDP I/O paths are excluded.

---

## Adding tests

### Unit test pattern (graph)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn my_adapter_returns_error_on_empty_url() {
        let adapter = MyAdapter::default();
        let result  = adapter.execute(ServiceInput::default()).await;
        assert!(result.is_err());
    }
}
```

### Browser test pattern (logic only)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stealth_level_debug_display() {
        assert_eq!(format!("{:?}", StealthLevel::Advanced), "Advanced");
    }

    #[tokio::test]
    #[ignore = "requires Chrome"]
    async fn pool_warm_start() {
        let pool = BrowserPool::new(BrowserConfig::default()).await.unwrap();
        assert!(pool.stats().available >= 1);
        pool.shutdown().await;
    }
}
```

---

## CI test matrix

The GitHub Actions CI workflow (`.github/workflows/ci.yml`) runs:

| Job | Runs on | Command |
| --- | --- | --- |
| `test` | `ubuntu-latest` | `cargo test --workspace --all-features` |
| `clippy` | `ubuntu-latest` | `cargo clippy --workspace --all-features -- -D warnings` |
| `fmt` | `ubuntu-latest` | `cargo fmt --check` |
| `docs` | `ubuntu-latest` | `cargo doc --workspace --no-deps --all-features` |
| `msrv` | `ubuntu-latest` | `cargo +1.94.0 check --workspace` |
| `cross-platform` | `windows-latest`, `macos-latest` | `cargo test --workspace` |

Browser integration tests (`--include-ignored`) are **not** run in CI — they require a
Chrome binary and a display server which are not available in the default runner images.
