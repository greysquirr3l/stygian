# Performance Tuning and Optimization Guide

This guide covers strategies for maximizing Mycelium's throughput and efficiency across worker pools, caching, memory, networking, and observability.

## Table of Contents

- [Worker Pool Sizing](#worker-pool-sizing)
- [Channel Sizing](#channel-sizing)
- [Memory Optimization](#memory-optimization)
- [Cache Tuning](#cache-tuning)
- [Rayon vs Tokio](#rayon-vs-tokio)
- [Profiling](#profiling)
- [Database Query Optimization](#database-query-optimization)
- [Network Optimization](#network-optimization)
- [Checklist](#performance-checklist)

---

## Worker Pool Sizing

`WorkerPool` bounds concurrency for a set of services. Choosing the right size depends on whether work is I/O-bound or CPU-bound.

### I/O-bound services (HTTP, browser, AI APIs)

Network operations spend most of their time waiting for remote responses. A large pool hides latency with concurrency:

```rust
// Rule of thumb: 10–100× logical CPU count
let concurrency = num_cpus::get() * 50;
let queue_depth  = concurrency * 4;   // back-pressure buffer

let pool = WorkerPool::new(concurrency, queue_depth);
```

Start with `50×` and adjust based on:

- Target API rate limits (reduce if hitting 429s)
- Available file descriptors (`ulimit -n`)
- Memory pressure (each in-flight request buffers a response body)

### CPU-bound services (parsing, transforms, NLP)

CPU work cannot overlap on the same core. Match the pool to physical cores to avoid context-switch overhead:

```rust
// Rule of thumb: 1–2× logical CPU count
let concurrency = num_cpus::get();
let queue_depth  = concurrency * 2;

let pool = WorkerPool::new(concurrency, queue_depth);
```

### Mixed workloads

When a pipeline mixes HTTP fetching and CPU-heavy extraction, create **separate pools** per service type and register them independently:

```rust
let http_pool = WorkerPool::new(num_cpus::get() * 40, 512);
let cpu_pool  = WorkerPool::new(num_cpus::get(),       32);
```

### Back-pressure

`queue_depth` controls how many tasks accumulate before callers block. A shallow queue applies
back-pressure sooner, limiting memory use. A deep queue smooths bursts but can hide overload. A
ratio of 4–8× concurrency is a reasonable starting point.

---

## Channel Sizing

Mycelium uses bounded `tokio::sync::mpsc` channels internally. Their capacity directly affects latency and throughput.

### Throughput vs latency trade-off

| Channel depth | Throughput | Latency | Memory |
| --------------- | ------------ | --------- | -------- |
| 1 | Low | Low | Minimal |
| 64 (default) | Medium | Medium | Low |
| 512 | High | Higher | Moderate |
| Unbounded | Max | Variable | Uncapped |

Choose bounded channels everywhere in library code. Unbounded channels are appropriate only for finite, known-small producer rates (e.g., a timer tick).

### Practical guidelines

```rust
// Fast producer → slow consumer: larger buffer to absorb bursts
let (tx, rx) = tokio::sync::mpsc::channel::<ServiceOutput>(256);

// Slow producer → fast consumer: small buffer, latency first
let (tx, rx) = tokio::sync::mpsc::channel::<ServiceOutput>(8);

// Equal rates: match expected batch size
let (tx, rx) = tokio::sync::mpsc::channel::<ServiceOutput>(batch_size * 2);
```

### Detecting channel pressure

Monitor `channel_capacity - channel_len` at runtime. When the gap consistently approaches
zero, the downstream consumer is saturated — scale it up or increase the channel depth to
buffer bursts.

---

## Memory Optimization

### Buffer pooling

Allocating a fresh `Vec<u8>` for every HTTP response body is expensive for high-throughput pipelines. Use a pool of reusable buffers:

```rust
use std::sync::Arc;
use tokio::sync::Mutex;

struct BufferPool {
    pool: Arc<Mutex<Vec<Vec<u8>>>>,
    buf_size: usize,
}

impl BufferPool {
    fn new(capacity: usize, buf_size: usize) -> Self {
        let pool = (0..capacity).map(|_| Vec::with_capacity(buf_size)).collect();
        Self { pool: Arc::new(Mutex::new(pool)), buf_size }
    }

    async fn acquire(&self) -> Vec<u8> {
        let mut pool = self.pool.lock().await;
        pool.pop().unwrap_or_else(|| Vec::with_capacity(self.buf_size))
    }

    async fn release(&self, mut buf: Vec<u8>) {
        buf.clear();
        let mut pool = self.pool.lock().await;
        pool.push(buf);
    }
}
```

### Arena allocation for batch processing

When processing a batch of nodes in a single wave, allocate all intermediate data from
one arena and free it in one shot after the wave completes. The `bumpalo` crate provides
a fast bump allocator:

```toml
# Cargo.toml
bumpalo = "3"
```

```rust
use bumpalo::Bump;

async fn process_wave(inputs: &[ServiceInput]) {
    let arena = Bump::new();                          // one allocation
    let scratch: Vec<&str> = inputs
        .iter()
        .map(|i| arena.alloc_str(&i.url))
        .collect();
    // arena dropped at end of scope — all scratch memory freed at once
}
```

### Limit response body size

Unbounded response buffering is a memory leak waiting to happen. Always cap body reads:

```rust
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024; // 8 MiB

let body = response
    .bytes()
    .await?;

if body.len() > MAX_BODY_BYTES {
    return Err(ScrapingError::ResponseTooLarge(body.len()));
}
```

---

## Cache Tuning

### Hit rate first

A cache that rarely hits wastes memory and adds lookup overhead. Measure hit rate before tuning capacity:

```rust
use std::sync::atomic::{AtomicU64, Ordering};

struct CacheMetrics {
    hits:   AtomicU64,
    misses: AtomicU64,
}

impl CacheMetrics {
    fn hit_rate(&self) -> f64 {
        let h = self.hits.load(Ordering::Relaxed) as f64;
        let m = self.misses.load(Ordering::Relaxed) as f64;
        if h + m == 0.0 { return 0.0; }
        h / (h + m)
    }
}
```

Aim for ≥ 80% hit rate before increasing cache size. If hit rate is low despite large
capacity, review key construction — keys that are too specific (e.g., include timestamps)
will never match.

### Eviction policies

| Policy | Best for | `moka` feature |
| -------- | ---------- | ---------------- |
| LRU (Least Recently Used) | Temporal locality | Default |
| LFU (Least Frequently Used) | Repeated popular keys | `moka` TinyLFU |
| TTI (Time-to-Idle) | Session-like data | `time_to_idle` |
| TTL (Time-to-Live) | Freshness guarantees | `time_to_live` |

Mycelium uses `moka` for async-safe caching. Configure both TTL and capacity to avoid unbounded growth:

```rust
use moka::future::Cache;
use std::time::Duration;

let cache: Cache<String, ServiceOutput> = Cache::builder()
    .max_capacity(10_000)
    .time_to_live(Duration::from_secs(300))   // hard expiry
    .time_to_idle(Duration::from_secs(60))    // evict inactive entries sooner
    .build();
```

### TTL selection

| Data type | Recommended TTL |
| ----------- | ---------------- |
| Static assets (CSS, JS) | 24 h |
| Article / blog content | 1–4 h |
| Live pricing / inventory | 30–120 s |
| API tokens / auth | token expiry − 30 s |
| AI extraction results | 15–60 min (expensive to recompute) |

### DashMap for hot-path counters

Use `DashMap` instead of `Mutex<HashMap>` for high-concurrency counters or read-heavy maps — it shards internally and rarely contends:

```rust
use dashmap::DashMap;

let rate_counters: DashMap<String, u64> = DashMap::new();
*rate_counters.entry("domain.com".into()).or_insert(0) += 1;
```

---

## Rayon vs Tokio

### The fundamental rule

| Work type | Use |
| ----------- | ----- |
| I/O-bound (network, disk, DB) | `tokio::spawn` |
| CPU-bound (parsing, encoding, hashing) | `rayon::spawn` or `spawn_blocking` |

Mixing CPU work into a Tokio task starves other tasks because the thread is occupied and cannot drive `.await` points for other futures.

### `spawn_blocking` for ad-hoc CPU work

For one-off synchronous CPU operations inside an async context:

```rust
use tokio::task;

async fn parse_html(raw: String) -> Result<Document, ScrapingError> {
    task::spawn_blocking(move || {
        // runs on a dedicated blocking thread pool, not the async pool
        scraper::Html::parse_document(&raw)
    })
    .await
    .map_err(|e| ScrapingError::ParseError(e.to_string()))
}
```

### Rayon for data-parallel CPU work

When processing a large slice of items in parallel (e.g., scoring 10 000 scraped records):

```rust
use rayon::prelude::*;

fn rank_results(mut results: Vec<ScrapedRecord>) -> Vec<ScrapedRecord> {
    results.par_iter_mut().for_each(|r| r.score = compute_score(r));
    results.sort_unstable_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    results
}
```

To call Rayon from async code, use `spawn_blocking`:

```rust
let ranked = task::spawn_blocking(move || rank_results(results)).await?;
```

### Thread pool sizes

```rust
// Tokio: default is num_cpus, which is correct for I/O-bound
let rt = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(num_cpus::get())
    .enable_all()
    .build()?;

// Rayon: match physical (not logical) cores to reduce hyperthreading noise
rayon::ThreadPoolBuilder::new()
    .num_threads(num_cpus::get_physical())
    .build_global()
    .ok();
```

---

## Profiling

### CPU flamegraphs

Install the `flamegraph` subcommand and use `perf` (Linux) or `dtrace` (macOS):

```bash
cargo install flamegraph

# macOS — requires sudo for dtrace
sudo cargo flamegraph --bin mycelium-graph -- --config pipeline.toml

# Open the generated flamegraph.svg in a browser
open flamegraph.svg
```

Look for wide boxes in the flamegraph — these indicate hot functions consuming disproportionate CPU time.

### Criterion benchmarks

Add micro-benchmarks for hot paths using Criterion:

```toml
# Cargo.toml
[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }

[[bench]]
name = "pipeline_execution"
harness = false
```

```rust
// benches/pipeline_execution.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_dag_executor(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("dag_executor_10_nodes", |b| {
        b.to_async(&rt).iter(|| async {
            let pipeline = build_test_pipeline(10);
            black_box(pipeline.execute().await.unwrap())
        });
    });
}

criterion_group!(benches, bench_dag_executor);
criterion_main!(benches);
```

```bash
cargo bench --bench pipeline_execution
# HTML reports at target/criterion/pipeline_execution/report/index.html
```

### Memory profiling

**heaptrack** (Linux — lowest overhead):

```bash
sudo apt-get install heaptrack heaptrack-gui
heaptrack cargo test --release
heaptrack_gui heaptrack.mycelium-graph.*.gz
```

**valgrind/massif** (Linux — detailed but slow):

```bash
cargo build --release
valgrind --tool=massif --pages-as-heap=yes ./target/release/mycelium-graph
ms_print massif.out.* | head -50
```

**Instruments** (macOS):

```bash
# Build with debug symbols in release mode
RUSTFLAGS="-g" cargo build --release
# Open Instruments.app → Allocations template → attach to process
```

### Async task profiling with tokio-console

`tokio-console` reveals tasks that are slow to wake, stall on locks, or accumulate in channels:

```toml
# Cargo.toml
[dependencies]
console-subscriber = "0.4"
```

```rust
// main.rs — enable only in debug builds
#[cfg(debug_assertions)]
console_subscriber::init();
```

```bash
cargo install tokio-console
tokio-console  # connects to the running process on port 6669
```

---

## Database Query Optimization

### Connection pooling

Never open a raw connection per query. Use `sqlx::PgPool` with tuned pool parameters:

```rust
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

let pool = PgPoolOptions::new()
    .max_connections(32)
    .min_connections(4)
    .acquire_timeout(Duration::from_secs(5))
    .idle_timeout(Duration::from_secs(600))
    .connect(&database_url)
    .await?;
```

Set `max_connections` ≤ `PostgreSQL max_connections / number-of-replicas`. Exceeding this blocks acquisition and defeats the purpose of the pool.

### Prepared statements

`sqlx` prepares statements automatically when you use the `query!` macro. For dynamic queries built at runtime, cache prepared statements explicitly:

```rust
use sqlx::postgres::PgStatement;

let stmt: PgStatement = sqlx::query("SELECT url, content FROM pages WHERE domain = $1")
    .prepare(&pool)
    .await?;

// Reuse the same compiled plan on every call
let rows = stmt.query().bind(&domain).fetch_all(&pool).await?;
```

### Batch inserts

Inserting rows one at a time has O(n) round-trip overhead. Use unnest for multi-row inserts:

```rust
let urls:     Vec<String> = records.iter().map(|r| r.url.clone()).collect();
let contents: Vec<String> = records.iter().map(|r| r.content.clone()).collect();

sqlx::query!(
    r#"
    INSERT INTO pages (url, content)
    SELECT * FROM UNNEST($1::text[], $2::text[])
    ON CONFLICT (url) DO UPDATE SET content = EXCLUDED.content
    "#,
    &urls,
    &contents,
)
.execute(&pool)
.await?;
```

### Indexes

Add indexes on columns used in `WHERE` clauses and `JOIN` conditions. For Mycelium workloads:

```sql
-- Filter by domain and crawl status frequently
CREATE INDEX CONCURRENTLY idx_pages_domain_status ON pages (domain, crawl_status);

-- Full-text search on extracted content
CREATE INDEX CONCURRENTLY idx_pages_content_fts ON pages USING gin(to_tsvector('english', content));
```

---

## Network Optimization

### HTTP/2

`reqwest` with the `http2` feature enables HTTP/2 multiplexing — multiple requests share one TCP connection, eliminating head-of-line blocking:

```toml
# Cargo.toml
[dependencies]
reqwest = { version = "0.13", features = ["http2", "gzip", "deflate"] }
```

```rust
let client = reqwest::Client::builder()
    .http2_prior_knowledge()      // force HTTP/2 for known H/2 endpoints
    .http2_keep_alive_interval(Duration::from_secs(30))
    .http2_keep_alive_timeout(Duration::from_secs(10))
    .build()?;
```

HTTP/2 is most effective against APIs and CDNs that support it. Test with `curl --http2-prior-knowledge` to verify server support before forcing it.

### Connection reuse

`reqwest::Client` is a connection pool. Create **one instance** per adapter and reuse it for the lifetime of the adapter:

```rust
// GOOD — one client, pool of keep-alive connections
struct HttpAdapter {
    client: reqwest::Client,
}

// BAD — new TCP handshake on every request
async fn fetch(url: &str) -> Bytes {
    reqwest::get(url).await.unwrap().bytes().await.unwrap()
}
```

### Compression

Enable gzip and brotli decompression to reduce transfer size:

```rust
let client = reqwest::Client::builder()
    .gzip(true)
    .brotli(true)
    .build()?;
```

Always send `Accept-Encoding: gzip, br` in request headers. Most modern web servers will compress responses automatically.

### DNS caching and TCP tuning

```rust
let client = reqwest::Client::builder()
    .tcp_keepalive(Duration::from_secs(60))
    .tcp_nodelay(true)                      // disable Nagle's algorithm for low-latency sends
    .connection_verbose(false)
    .pool_max_idle_per_host(20)             // keep more connections warm per origin
    .pool_idle_timeout(Duration::from_secs(90))
    .build()?;
```

For workloads that hammer a small set of hosts (e.g., repeated calls to the same API),
increase `pool_max_idle_per_host`. For workloads that fan out across thousands of distinct
domains, reduce it to avoid exhausting file descriptors.

### Rate limiting

Apply per-domain rate limiting at the adapter level to avoid triggering anti-bot defences
and reduce server load:

```rust
use std::time::Duration;
use governor::{Quota, RateLimiter};
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use nonzero_ext::nonzero;

let limiter = RateLimiter::<NotKeyed, InMemoryState, DefaultClock>::direct(
    Quota::per_second(nonzero!(10u32)),          // 10 req/s
);

// Before each request:
limiter.until_ready().await;
```

---

## Performance Checklist

Use this checklist before marking a pipeline as production-ready:

### Worker pools

- [ ] I/O-bound services use `concurrency = num_cpus × 20–50`
- [ ] CPU-bound services use `concurrency = num_cpus`
- [ ] Separate pools exist for I/O and CPU workloads
- [ ] `queue_depth` is 4–8× `concurrency` for back-pressure

### Channels

- [ ] All channels are bounded
- [ ] Channel depth matches producer/consumer rate ratio
- [ ] Channel saturation is monitored at runtime

### Memory

- [ ] Response body size is capped (`MAX_BODY_BYTES`)
- [ ] Buffer pools are used for large, frequent allocations
- [ ] No `Vec::clone` in hot paths — prefer `Arc` or slice references

### Caching

- [ ] Cache hit rate ≥ 80% measured and logged
- [ ] TTL matches data freshness requirements
- [ ] Cache capacity has an upper bound (`max_capacity`)
- [ ] `DashMap` used for concurrent counters instead of `Mutex<HashMap>`

### CPU / async

- [ ] No blocking calls inside `tokio::spawn` tasks
- [ ] CPU-intensive work uses `spawn_blocking` or `rayon`
- [ ] Rayon thread pool is sized to physical (not logical) CPUs

### Performance Profiling

- [ ] Flamegraph generated and hot paths reviewed
- [ ] Criterion benchmarks exist for pipelines with SLAs
- [ ] Memory profile run — no unbounded growth under load

### Database

- [ ] `PgPool` with `max_connections ≤ server_limit / replicas`
- [ ] Batch inserts via `UNNEST` for > 10 rows
- [ ] Indexes exist on `WHERE` / `JOIN` columns
- [ ] No N+1 query patterns

### Network

- [ ] Single `reqwest::Client` instance per adapter (connection pool)
- [ ] HTTP/2 enabled for supported endpoints
- [ ] Compression (`gzip`, `brotli`) enabled
- [ ] Per-domain rate limiter applied
- [ ] `tcp_keepalive` set to prevent stale connection reuse

---

*See [architecture.md](architecture.md) for the hexagonal layer model and [custom-adapters.md](custom-adapters.md) for implementing new service adapters.*
