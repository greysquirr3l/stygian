# Environment Variables

All configuration values for both crates can be overridden at runtime via environment
variables. No recompilation required.

---

## stygian-browser

| Variable | Default | Description |
|---|---|---|
| `MYCELIUM_CHROME_PATH` | auto-detect | Absolute path to Chrome or Chromium binary |
| `MYCELIUM_HEADLESS` | `true` | `false` for headed mode (displays browser window) |
| `MYCELIUM_STEALTH_LEVEL` | `advanced` | `none`, `basic`, or `advanced` |
| `MYCELIUM_POOL_MIN` | `2` | Minimum warm browser instances |
| `MYCELIUM_POOL_MAX` | `10` | Maximum concurrent browser instances |
| `MYCELIUM_POOL_ACQUIRE_TIMEOUT_SECS` | `30` | Seconds to wait for a pool slot before error |
| `MYCELIUM_CDP_FIX_MODE` | `addBinding` | `addBinding`, `isolatedworld`, or `enabledisable` |
| `MYCELIUM_PROXY` | — | Proxy URL (`http://`, `https://`, or `socks5://`) |
| `MYCELIUM_WINDOW_WIDTH` | `1920` | Browser viewport width in pixels |
| `MYCELIUM_WINDOW_HEIGHT` | `1080` | Browser viewport height in pixels |

### Chrome binary auto-detection order

When `MYCELIUM_CHROME_PATH` is not set, the library searches:

1. `google-chrome` on `$PATH`
2. `chromium` on `$PATH`
3. `/usr/bin/google-chrome`
4. `/usr/bin/chromium-browser`
5. `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome` (macOS)
6. `C:\Program Files\Google\Chrome\Application\chrome.exe` (Windows)

---

## stygian-graph

| Variable | Default | Description |
|---|---|---|
| `MYCELIUM_LOG` | `info` | Alias for `RUST_LOG` if set; otherwise defers to `RUST_LOG` |
| `MYCELIUM_WORKERS` | `num_cpus * 4` | Default worker pool concurrency |
| `MYCELIUM_QUEUE_DEPTH` | `workers * 4` | Worker pool channel depth |
| `MYCELIUM_CACHE_CAPACITY` | `10000` | Default LRU cache entry limit |
| `MYCELIUM_CACHE_TTL_SECS` | `300` | Default DashMap cache TTL in seconds |
| `MYCELIUM_HTTP_TIMEOUT_SECS` | `30` | HTTP adapter request timeout |
| `MYCELIUM_HTTP_MAX_REDIRECTS` | `10` | HTTP redirect chain limit |

### AI provider variables

| Variable | Used by |
|---|---|
| `ANTHROPIC_API_KEY` | `ClaudeAdapter` |
| `OPENAI_API_KEY` | `OpenAiAdapter` |
| `GOOGLE_API_KEY` | `GeminiAdapter` |
| `GITHUB_TOKEN` | `CopilotAdapter` |
| `OLLAMA_BASE_URL` | `OllamaAdapter` (default: `http://localhost:11434`) |

### Distributed execution variables

| Variable | Default | Description |
|---|---|---|
| `REDIS_URL` | `redis://localhost:6379` | Redis/Valkey connection URL |
| `REDIS_MAX_CONNECTIONS` | `20` | Redis connection pool size |
| `MYCELIUM_QUEUE_NAME` | `stygian:work` | Default work queue key |
| `MYCELIUM_VISIBILITY_TIMEOUT_SECS` | `60` | Task visibility timeout for in-flight items |

---

## Tracing and logging

| Variable | Example | Description |
|---|---|---|
| `RUST_LOG` | `stygian_graph=debug,stygian_browser=info` | Log level per crate |
| `RUST_LOG` | `trace` | Enable all tracing (very verbose) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP trace export endpoint |
| `OTEL_SERVICE_NAME` | `stygian-scraper` | Service name in trace metadata |
