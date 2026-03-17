# Request Signing

The `SigningPort` trait decouples request signing from the adapters that make HTTP calls.
Any adapter that needs signed outbound requests holds an `Arc<dyn ErasedSigningPort>` and
calls `sign()` with the request material before dispatching it.

This separation means the calling adapter never knows or cares *how* requests are signed —
whether by a Frida RPC bridge, an AWS SDK, a pure-Rust HMAC function, or a lightweight
Python sidecar.

---

## When to use `SigningPort`

| Signing scheme | Typical use |
| --- | --- |
| **Frida RPC bridge** | Hook native `.so` signing code inside a running mobile app (Tinder, Snapchat, …) via a thin HTTP sidecar |
| **AWS Signature V4** | Sign S3 / API Gateway requests; keep IAM credentials out of the graph pipeline |
| **OAuth 1.0a** | Generate per-request `oauth_signature` for Twitter/X API v1 endpoints |
| **Custom HMAC** | Add `X-Request-Signature` + `X-Signed-At` headers required by trading or payment APIs |
| **Timestamp + nonce** | Anti-replay headers for any API that validates request freshness |
| **Device attestation** | Attach Play Integrity / Apple DeviceCheck tokens to every request |
| **mTLS client credentials** | Surface a client certificate thumbprint as a header when TLS termination is upstream |

---

## Input and output types

### `SigningInput`

The request material passed to the signer:

```rust
use stygian_graph::ports::signing::SigningInput;
use serde_json::json;

let input = SigningInput {
    method:  "POST".to_string(),
    url:     "https://api.example.com/v2/messages".to_string(),
    headers: Default::default(),   // headers already present before signing
    body:    Some(b"{\"text\":\"hello\"}".to_vec()),
    context: json!({ "nonce_seed": 42 }),  // arbitrary caller data
};
```

| Field | Type | Description |
| --- | --- | --- |
| `method` | `String` | HTTP method (`"GET"`, `"POST"`, …) |
| `url` | `String` | Fully-qualified target URL |
| `headers` | `HashMap<String, String>` | Headers already present on the request |
| `body` | `Option<Vec<u8>>` | Raw request body; `None` for bodyless methods |
| `context` | `serde_json::Value` | Caller-supplied metadata (nonce seeds, session tokens, …) |

### `SigningOutput`

The material to merge into the request:

```rust
use stygian_graph::ports::signing::SigningOutput;
use std::collections::HashMap;

let mut headers = HashMap::new();
headers.insert("Authorization".to_string(), "HMAC-SHA256 sig=abc123".to_string());
headers.insert("X-Signed-At".to_string(), "1710676800000".to_string());

let output = SigningOutput {
    headers,
    query_params:  vec![],
    body_override: None,
};
```

| Field | Type | Description |
| --- | --- | --- |
| `headers` | `HashMap<String, String>` | Headers to add or override on the request |
| `query_params` | `Vec<(String, String)>` | Query parameters to append to the URL |
| `body_override` | `Option<Vec<u8>>` | If `Some`, replaces the request body (for digest-in-body schemes) |

All fields default to empty — a default `SigningOutput` is a valid no-op.

---

## Built-in adapters

### `NoopSigningAdapter`

Passes requests through unsigned. Use as a default when signing is
optional, or to disable signing in tests:

```rust
use stygian_graph::adapters::signing::NoopSigningAdapter;
use stygian_graph::ports::signing::{SigningPort, SigningInput};
use serde_json::json;

# tokio::runtime::Runtime::new().unwrap().block_on(async {
let signer = NoopSigningAdapter;

let output = signer.sign(SigningInput {
    method:  "GET".to_string(),
    url:     "https://example.com".to_string(),
    headers: Default::default(),
    body:    None,
    context: json!({}),
}).await.unwrap();

assert!(output.headers.is_empty());
# });
```

---

### `HttpSigningAdapter`

Delegates signing to any external HTTP sidecar. The sidecar receives a JSON payload describing the request and returns the headers / query params /
body override to apply.

```rust
use stygian_graph::adapters::signing::{HttpSigningAdapter, HttpSigningConfig};
use std::time::Duration;

let signer = HttpSigningAdapter::new(HttpSigningConfig {
    endpoint:     "http://localhost:27042/sign".to_string(),
    timeout:      Duration::from_secs(5),
    bearer_token: Some("sidecar-secret".to_string()),
    ..Default::default()
});
```

#### Config fields

| Field | Default | Description |
| --- | --- | --- |
| `endpoint` | `"http://localhost:27042/sign"` | Full URL of the sidecar's sign endpoint |
| `timeout` | 10 s | Per-request timeout when calling the sidecar |
| `bearer_token` | `None` | Bearer token used to authenticate with the sidecar |
| `extra_headers` | `{}` | Static headers forwarded on every sidecar call |

#### Sidecar wire format

The sidecar receives a `POST` with this JSON body:

```json
{
  "method":   "GET",
  "url":      "https://api.tinder.com/v2/profile",
  "headers":  { "Content-Type": "application/json" },
  "body_b64": null,
  "context":  {}
}
```

The request body (if any) is base64-encoded in `body_b64`. The sidecar must
respond with a JSON object:

```json
{
  "headers":      { "X-Auth-Token": "abc123", "X-Signed-At": "1710676800000" },
  "query_params": [],
  "body_b64":     null
}
```

All response fields are optional — omit any field that your scheme does not use.

---

## Frida RPC bridge

The most common use of `HttpSigningAdapter` is hooking a mobile app's native signing function via [Frida](https://frida.re/) and exposing it
through a thin HTTP sidecar.

```
┌─────────── Your machine ────────────────────────────────┐
│                                                         │
│  stygian-graph pipeline                                 │
│    └─ HttpSigningAdapter → POST http://localhost:27042  │─ adb forward ─►┐
│                                                         │                │
└─────────────────────────────────────────────────────────┘                │
                                                                           ▼
                                                          ┌──── Android device / emulator ────┐
                                                          │                                   │
                                                          │  Frida sidecar (Python/Flask)     │
                                                          │    └─ frida.attach("com.example") │
                                                          │       └─ libauth.so!computeHMAC() │
                                                          │                                   │
                                                          └───────────────────────────────────┘
```

**Example Python sidecar** (`frida_sidecar.py`):

```python
import frida
from flask import Flask, request, jsonify
import base64

session = frida.get_usb_device().attach("com.example.app")
script = session.create_script("""
    const computeHmac = new NativeFunction(
        Module.findExportByName("libauth.so", "_Z11computeHMACPKcS0_"),  // gitleaks:allow
        'pointer', ['pointer', 'pointer']
    );
    rpc.exports.sign = (url, body) =>
        computeHmac(
            Memory.allocUtf8String(url),
            Memory.allocUtf8String(body)
        ).readUtf8String();
""")
script.load()

app = Flask(__name__)

@app.route("/sign", methods=["POST"])
def sign():
    req = request.json
    body = base64.b64decode(req["body_b64"]).decode() if req.get("body_b64") else ""
    sig = script.exports_sync.sign(req["url"], body)
    return jsonify({"headers": {"X-Request-Signature": sig}})

app.run(host="0.0.0.0", port=27042)
```

Forward the port and point the adapter at it:

```bash
adb forward tcp:27042 tcp:27042
```

```rust
use stygian_graph::adapters::signing::{HttpSigningAdapter, HttpSigningConfig};

let signer = HttpSigningAdapter::new(HttpSigningConfig {
    endpoint: "http://localhost:27042/sign".to_string(),
    ..Default::default()
});
```

---

## Implementing a custom `SigningPort`

For pure-Rust schemes, implement `SigningPort` directly — no sidecar needed:

```rust
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use stygian_graph::ports::signing::{SigningError, SigningInput, SigningOutput, SigningPort};

pub struct TimestampNonceAdapter {
    secret: Vec<u8>,
}

impl SigningPort for TimestampNonceAdapter {
    async fn sign(&self, _input: SigningInput) -> Result<SigningOutput, SigningError> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| SigningError::Other(e.to_string()))?
            .as_millis()
            .to_string();

        let mut headers = HashMap::new();
        headers.insert("X-Timestamp".to_string(), ts);
        headers.insert("X-Nonce".to_string(), uuid::Uuid::new_v4().to_string());
        // Add HMAC over (method + url + ts) using self.secret here …

        Ok(SigningOutput { headers, ..Default::default() })
    }
}
```

Follow the [Custom Adapters](./custom-adapters.md) guide for the full checklist.

---

## Wiring into a pipeline

Use `Arc<dyn ErasedSigningPort>` to hold any signer at runtime:

```rust
use std::sync::Arc;
use stygian_graph::adapters::signing::{HttpSigningAdapter, HttpSigningConfig};
use stygian_graph::ports::signing::ErasedSigningPort;

let signer: Arc<dyn ErasedSigningPort> = Arc::new(
    HttpSigningAdapter::new(HttpSigningConfig {
        endpoint: "http://localhost:27042/sign".to_string(),
        ..Default::default()
    })
);

// Pass `signer` to any adapter or service that accepts `Arc<dyn ErasedSigningPort>`
```

`ErasedSigningPort` is the object-safe version of `SigningPort` that enables dynamic
dispatch. The blanket `impl<T: SigningPort> ErasedSigningPort for T` means any concrete
`SigningPort` implementation can be erased without additional boilerplate.

---

## Error handling

`SigningError` converts to `StygianError::Service(ServiceError::AuthenticationFailed)`
via the `From` trait, so signing failures surface as authentication errors in the pipeline.

| Variant | Meaning |
| --- | --- |
| `BackendUnavailable(msg)` | Sidecar is unreachable (network error, DNS failure) |
| `InvalidResponse(msg)` | Sidecar returned an unexpected HTTP status or malformed JSON |
| `CredentialsMissing(msg)` | Signing key or secret was not configured |
| `Timeout(ms)` | Sidecar did not respond within the configured timeout |
| `Other(msg)` | Catch-all for any other signing failure |
