# Performance Tuning

This guide covers strategies for maximising throughput and efficiency: worker pool sizing,
channel depth, memory management, cache tuning, and profiling.

---

## Worker pool sizing

`WorkerPool` bounds concurrency for a set of services. The optimal size depends on whether
work is I/O-bound or CPU-bound.

### I/O-bound services (HTTP, browser, AI APIs)

Network operations spend most of their time waiting for remote responses.
A large pool hides latency with concurrency:

```rust
// Rule of thumb: 10–100× logical CPU count
let concurrency = num_cpus::get() * 50;
let queue_depth  = concurrency * 4;   // back-pressure buffer

let pool = WorkerPool::new(concurrency, queue_depth);
```

Start with `50×` and adjust based on:

- Target API rate limits (reduce if hitting 429s)
- Available file descriptors (`ulimit -n`)
- Memory pressure (every in-flight request buffers a response body)

### CPU-bound services (parsing, transforms, NLP)

CPU work cannot overlap on the same core. Match the pool to physical cores to avoid
context-switch overhead:

```rust
// Rule of thumb: 1–2× logical CPU count
let concurrency = num_cpus::get();
let queue_depth  = concurrency * 2;

let pool = WorkerPool::new(concurrency, queue_depth);
```

### Mixed workloads

When a pipeline mixes HTTP fetching and CPU-heavy extraction,
use **separate pools** per service type:

```rust
let http_pool = WorkerPool::new(num_cpus::get() * 40, 512);
let cpu_pool  = WorkerPool::new(num_cpus::get(),        32);
```

### Back-pressure

`queue_depth` controls how many tasks accumulate before callers block.
A shallow queue applies back-pressure sooner, limiting memory. A deep queue
smooths bursts but can hide overload. A ratio of 4–8× concurrency is a
good starting point.

---

## Channel sizing

Stygian uses bounded `tokio::sync::mpsc` channels internally.

| Channel depth | Throughput | Latency | Memory |
| --- | --- | --- | --- |
| 1 | Low | Low | Minimal |
| 64 (default) | Medium | Medium | Low |
| 512 | High | Higher | Moderate |
| Unbounded | Max | Variable | Uncapped |

Always use bounded channels in library code. Unbounded channels are only appropriate
for producers with a known-small, finite rate (e.g. a timer).

Detect saturation at runtime: when `capacity - len` consistently approaches zero,
the downstream consumer is a bottleneck — either scale it out or increase depth.

---

## Memory optimisation

### Limit response body size

Unbounded response buffering will exhaust memory on large responses:

```rust
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024; // 8 MiB

let body = response.bytes().await?;
if body.len() > MAX_BODY_BYTES {
    return Err(ScrapingError::ResponseTooLarge(body.len()));
}
```

### Arena allocation for wave processing

Allocate all intermediate data for a wave from a single arena and free it in one shot:

```toml
# Cargo.toml
bumpalo = "3"
```

```rust
use bumpalo::Bump;

async fn process_wave(inputs: &[ServiceInput]) {
    let arena = Bump::new();
    let urls: Vec<&str> = inputs
        .iter()
        .map(|i| arena.alloc_str(&i.url))
        .collect();
    // arena dropped here — all scratch freed in one operation
}
```

### Buffer pooling

Avoid allocating a fresh `Vec<u8>` for every HTTP response — reuse from a pool:

```rust
use tokio::sync::Mutex;

struct BufferPool {
    pool:     Mutex<Vec<Vec<u8>>>,
    buf_size: usize,
}

impl BufferPool {
    async fn acquire(&self) -> Vec<u8> {
        let mut p = self.pool.lock().await;
        p.pop().unwrap_or_else(|| Vec::with_capacity(self.buf_size))
    }

    async fn release(&self, mut buf: Vec<u8>) {
        buf.clear();
        self.pool.lock().await.push(buf);
    }
}
```

---

## Cache tuning

### Measure hit rate first

A cache that rarely hits wastes memory and adds lookup overhead:

```rust
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

struct CacheMetrics { hits: AtomicU64, misses: AtomicU64 }

fn hit_rate(m: &CacheMetrics) -> f64 {
    let h = m.hits.load(Relaxed)   as f64;
    let m = m.misses.load(Relaxed) as f64;
    if h + m == 0.0 { return 0.0; }
    h / (h + m)
}
```

Target ≥ 0.80 for URL-keyed caches before increasing capacity.

### LRU vs DashMap

| | `BoundedLruCache` | `DashMapCache` |
| --- | --- | --- |
| Eviction policy | LRU (capacity-based) | TTL-based (background task) |
| Best for | Fixed working set | Time-sensitive data |
| Concurrency overhead | Higher (LRU list mutex) | Lower (sharded map) |

For AI extraction results with a 24 h freshness requirement, use `DashMapCache` with
`ttl = Duration::from_secs(86_400)`. For deduplication of seen URLs, `BoundedLruCache`
is the right choice.

---

## Rayon vs Tokio

| | Tokio | Rayon |
| --- | --- | --- |
| Use for | I/O-bound work (network, disk) | CPU-bound work (parsing, transforms) |
| Blocking? | No — async tasks never block threads | Yes — tasks block Rayon threads |
| Thread pool | Shared Tokio runtime | Separate Rayon thread pool |

**Rule**: never call synchronous CPU-heavy work directly in a Tokio task. Offload with
`tokio::task::spawn_blocking` or `rayon::spawn`:

```rust
// CPU-heavy HTML parsing — do NOT do this in an async fn directly
let html = response.text().await?;
let extracted = tokio::task::spawn_blocking(move || {
    heavy_parsing(html)
}).await??;
```

---

## Profiling

### Flamegraph with cargo-flamegraph

```bash
cargo install flamegraph
# Profile a benchmark
cargo flamegraph --bench dag_executor -- --bench
```

### Criterion benchmarks

The `stygian-graph` crate ships Criterion benchmarks in `benches/`:

```bash
cargo bench                       # run all benchmarks
cargo bench dag_executor          # run a specific group
cargo bench -- --save-baseline v0 # save baseline for comparison
cargo bench -- --load-baseline v0 # compare against baseline
```

Key benchmark targets:

| Benchmark | What it measures |
| --- | --- |
| `dag_executor/wave_10` | 10-node wave execution overhead |
| `dag_executor/wave_100` | 100-node wave execution overhead |
| `http_adapter/single` | Single HTTP request round-trip |
| `cache/lru_hit` | LRU cache read under contention |

---

## Benchmarks (Apple M4 Pro)

| Operation | Latency |
| --- | --- |
| DAG executor overhead per wave | ~50 µs |
| HTTP adapter (cached DNS) | ~2 ms |
| Browser acquisition (warm pool) | <100 ms |
| LRU cache read (1 M ops/s) | ~1 µs |
