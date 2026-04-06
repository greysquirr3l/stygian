# Browser Pool

The pool maintains a configurable number of warm browser instances, enforces a maximum
concurrency limit, and applies backpressure when all slots are occupied.

---

## How it works

```text
BrowserPool
├── [Browser 0] — idle       ← returned immediately on acquire()
├── [Browser 1] — active     ← in use by a caller
└── [Browser 2] — idle       ← available

acquire()  →  lease one idle browser  →  returns BrowserHandle
release()  →  return browser to pool  →  health check, keep or discard
```

When all browsers are active and the pool is at `max_size`, callers block in
`acquire()` until one becomes available or `acquire_timeout` expires.

---

## Creating a pool

```rust,no_run
use stygian_browser::{BrowserConfig, BrowserPool};
use stygian_browser::config::PoolConfig;
use std::time::Duration;

let config = BrowserConfig::builder()
    .pool(PoolConfig {
        min_size:        2,    // launch 2 browsers immediately
        max_size:        8,    // cap at 8 concurrent browsers
        idle_timeout:    Duration::from_secs(300),
        acquire_timeout: Duration::from_secs(10),
    })
    .build();

let pool = BrowserPool::new(config).await?;
```

`BrowserPool::new()` launches `min_size` browsers in parallel and returns once they are
all ready. Subsequent calls to `acquire()` return warm instances with no launch overhead.

---

## Acquiring and releasing

```rust,no_run
// Acquire — blocks if pool is saturated
let handle = pool.acquire().await?;

// Do work on the browser
let mut page = handle.browser().expect("browser is available").new_page().await?;
page.navigate("https://example.com", WaitUntil::Load, Duration::from_secs(30)).await?;

// Release — returns browser to pool; discards if health check fails
handle.release().await;
```

`BrowserHandle` also implements `Drop`: if you forget to call `release()` the browser
is returned to the pool automatically, though doing so inside an async context is
preferred.

---

## Pool stats

```rust,no_run
let stats = pool.stats();

println!("idle      : {}", stats.idle);       // warm browsers ready to use immediately
println!("active    : {}", stats.active);     // total managed (idle + in-use)
println!("available : {}", stats.available);  // free semaphore slots (max - active)
println!("max       : {}", stats.max);        // pool capacity
```

---

## Health checks

When a browser is released, the pool runs a lightweight health check:

1. Check the CDP connection is still open (no round-trip required).
2. Verify that the browser process is alive.
3. If either fails, discard the browser and spawn a replacement asynchronously.

Discarded browsers are replaced in the background — the pool never drops below `min_size`
for long, and callers are never blocked waiting for a replace that will never arrive.

---

## Idle eviction

Browsers that have been idle longer than `idle_timeout` are gracefully closed and removed
from the pool. This reclaims system memory when scraping activity is low.

If eviction would drop the pool below `min_size`, eviction is skipped for those browsers
(they stay warm).

Eviction applies to both the shared queue and all per-context queues. Empty context
queues are pruned automatically.

---

## Context segregation

When multiple bots or tenants share a single pool, use `acquire_for()` to keep their
browser instances isolated. Browsers acquired for one context are never returned to a
different context.

```rust,no_run
// Bot A and Bot B use the same pool, but their browsers never mix
let a = pool.acquire_for("bot-a").await?;
let b = pool.acquire_for("bot-b").await?;

// Each handle knows its context
assert_eq!(a.context_id(), Some("bot-a"));
assert_eq!(b.context_id(), Some("bot-b"));

a.release().await;
b.release().await;
```

The global `max_size` still governs total capacity across every context. If the pool is
full, `acquire_for()` blocks just like `acquire()`.

### Releasing a context

When a bot or tenant is deprovisioned, drain its idle browsers:

```rust,no_run
let shut_down = pool.release_context("bot-a").await;
println!("Closed {shut_down} browsers for bot-a");
```

Active handles for that context are unaffected; they will be disposed normally when
released or dropped.

### Listing contexts

```rust,no_run
let ids = pool.context_ids().await;
println!("Active contexts with idle browsers: {ids:?}");
```

### Shared vs scoped

| Method | Queue | Reuse scope |
| ------- | ------- | ------------- |
| `acquire()` | shared | any `acquire()` caller |
| `acquire_for("x")` | scoped to `"x"` | only `acquire_for("x")` |

Both paths share the same semaphore and `max_size`, so global backpressure
is applied regardless of how browsers were acquired.

---

## Cold start behaviour

If `acquire()` is called when the pool is empty (e.g. on first call before `min_size`
browsers have launched) or when all browsers are active and `total < max_size`, a
new browser is launched on demand. Cold starts take < 2 s on modern hardware.

---

## Graceful shutdown

```rust,no_run
// Closes all browsers gracefully — waits for active handles to be released first
pool.shutdown().await;
```

`shutdown()` signals the pool to stop accepting new `acquire()` calls, waits for all
active handles to be released (or times out after `acquire_timeout`), then closes every
browser.
