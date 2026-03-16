# Environment Variables

All configuration values for both crates can be overridden at runtime via environment
variables. No recompilation required.

---

## stygian-browser

| Variable | Default | Description |
| --- | --- | --- |
| `STYGIAN_CHROME_PATH` | auto-detect | Absolute path to Chrome or Chromium binary |
| `STYGIAN_HEADLESS` | `true` | `false` for headed mode (displays browser window) |
| `STYGIAN_HEADLESS_MODE` | `new` | `new` (`--headless=new`) or `legacy` (classic `--headless` for Chromium < 112) |
| `STYGIAN_STEALTH_LEVEL` | `advanced` | `none`, `basic`, or `advanced` |
| `STYGIAN_POOL_MIN` | `2` | Minimum warm browser instances |
| `STYGIAN_POOL_MAX` | `10` | Maximum concurrent browser instances |
| `STYGIAN_POOL_IDLE_SECS` | `300` | Idle timeout before a browser is evicted |
| `STYGIAN_POOL_ACQUIRE_SECS` | `5` | Seconds to wait for a pool slot before error |
| `STYGIAN_LAUNCH_TIMEOUT_SECS` | `10` | Browser launch timeout |
| `STYGIAN_CDP_TIMEOUT_SECS` | `30` | Per-operation CDP command timeout |
| `STYGIAN_CDP_FIX_MODE` | `addBinding` | `addBinding`, `isolatedworld`, or `enabledisable` |
| `STYGIAN_PROXY` | — | Proxy URL (`http://`, `https://`, or `socks5://`) |
| `STYGIAN_PROXY_BYPASS` | — | Comma-separated proxy bypass list (e.g. `<local>,localhost`) |
| `STYGIAN_DISABLE_SANDBOX` | auto-detect | `true` inside containers, `false` on bare metal |

### Chrome binary auto-detection order

When `STYGIAN_CHROME_PATH` is not set, the library searches:

1. `google-chrome` on `$PATH`
2. `chromium` on `$PATH`
3. `/usr/bin/google-chrome`
4. `/usr/bin/chromium-browser`
5. `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome` (macOS)
6. `C:\Program Files\Google\Chrome\Application\chrome.exe` (Windows)

---

## stygian-proxy

`stygian-proxy` is configured by constructing `ProxyConfig` directly in code.
There are no environment variable overrides; pass the struct to
`ProxyManager::with_round_robin` or `ProxyManager::with_strategy`.

| Field | Default | Description |
| --- | --- | --- |
| `health_check_url` | `https://httpbin.org/ip` | URL probed to verify proxy liveness |
| `health_check_interval` | `60 s` | How often the background task runs |
| `health_check_timeout` | `5 s` | Per-probe HTTP timeout |
| `circuit_open_threshold` | `5` | Consecutive failures before circuit opens |
| `circuit_half_open_after` | `30 s` | Cooldown before attempting recovery |

---

## stygian-graph

| Variable | Default | Description |
| --- | --- | --- |
| `STYGIAN_LOG` | `info` | Alias for `RUST_LOG` if set; otherwise defers to `RUST_LOG` |
| `STYGIAN_WORKERS` | `num_cpus * 4` | Default worker pool concurrency |
| `STYGIAN_QUEUE_DEPTH` | `workers * 4` | Worker pool channel depth |
| `STYGIAN_CACHE_CAPACITY` | `10000` | Default LRU cache entry limit |
| `STYGIAN_CACHE_TTL_SECS` | `300` | Default DashMap cache TTL in seconds |
| `STYGIAN_HTTP_TIMEOUT_SECS` | `30` | HTTP adapter request timeout |
| `STYGIAN_HTTP_MAX_REDIRECTS` | `10` | HTTP redirect chain limit |

### AI provider variables

| Variable | Used by |
| --- | --- |
| `ANTHROPIC_API_KEY` | `ClaudeAdapter` |
| `OPENAI_API_KEY` | `OpenAiAdapter` |
| `GOOGLE_API_KEY` | `GeminiAdapter` |
| `GITHUB_TOKEN` | `CopilotAdapter` |
| `OLLAMA_BASE_URL` | `OllamaAdapter` (default: `http://localhost:11434`) |

### Distributed execution variables

| Variable | Default | Description |
| --- | --- | --- |
| `REDIS_URL` | `redis://localhost:6379` | Redis/Valkey connection URL |
| `REDIS_MAX_CONNECTIONS` | `20` | Redis connection pool size |
| `STYGIAN_QUEUE_NAME` | `stygian:work` | Default work queue key |
| `STYGIAN_VISIBILITY_TIMEOUT_SECS` | `60` | Task visibility timeout for in-flight items |

---

## Tracing and logging

| Variable | Example | Description |
| --- | --- | --- |
| `RUST_LOG` | `stygian_graph=debug,stygian_browser=info` | Log level per crate |
| `RUST_LOG` | `trace` | Enable all tracing (very verbose) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP trace export endpoint |
| `OTEL_SERVICE_NAME` | `stygian-scraper` | Service name in trace metadata |
