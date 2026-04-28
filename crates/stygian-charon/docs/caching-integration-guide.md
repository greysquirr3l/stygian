# Caching Integration Guide

This guide covers the CHR-014 investigation report cache layer in `stygian-charon`.

## Features

Enable the in-memory cache API:

```toml
[dependencies]
stygian-charon = { path = "crates/stygian-charon", features = ["caching"] }
```

Enable the optional Redis-backed cache:

```toml
[dependencies]
stygian-charon = { path = "crates/stygian-charon", features = ["redis-cache"] }
```

`redis-cache` implies `caching`.

## Available APIs

- `InvestigationReportCache`
- `MemoryInvestigationCache`
- `RedisInvestigationCache` (with `redis-cache`)
- `investigation_cache_key`
- `investigate_har_cached`
- `investigate_har_cached_with_target_class`

## Memory Cache Example

```rust
use std::num::NonZeroUsize;
use std::time::Duration;

use stygian_charon::{
    MemoryInvestigationCache, TargetClass, investigate_har_cached_with_target_class,
};

let cache = MemoryInvestigationCache::new(
    NonZeroUsize::new(128).expect("cache capacity must be non-zero"),
    Duration::from_secs(300),
);

let report = investigate_har_cached_with_target_class(
    har_json,
    TargetClass::ContentSite,
    &cache,
)?;

assert_eq!(report.target_class, Some(TargetClass::ContentSite));
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Redis Cache Example

```rust
use std::time::Duration;

use stygian_charon::{
    RedisInvestigationCache, TargetClass, investigate_har_cached_with_target_class,
};

let cache = RedisInvestigationCache::new("redis://127.0.0.1/", Duration::from_secs(300))?;
let report = investigate_har_cached_with_target_class(
    har_json,
    TargetClass::Api,
    &cache,
)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Cache Key Behavior

The cache key includes:

- HAR payload content
- explicit target class

That keeps cached results isolated across target-class-specific assessment flows.

## Invalidation

All cache implementations support:

- `invalidate(key)` for single-entry removal
- `clear()` for full cache reset

Generate a matching key with `investigation_cache_key(har_json, target_class)`.

## Notes

- Cache hits bypass HAR parsing and investigation reconstruction.
- `MemoryInvestigationCache` uses LRU eviction and TTL expiry on read.
- `RedisInvestigationCache` stores serialized `InvestigationReport` JSON using `SETEX`.
- Cached wrappers set `report.target_class` so downstream SLO inference stays explicit.
