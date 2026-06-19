# DOM Query API

`stygian-browser` provides a live DOM query API that operates directly over the Chrome
DevTools Protocol (CDP), bypassing the `page.content()` + HTML-parse round-trip.

---

## `query_selector_all`

Query all matching elements and get back lightweight `NodeHandle` values.

```rust,no_run
let nodes: Vec<NodeHandle> = page.query_selector_all("article.post").await?;
println!("{} posts found", nodes.len());
```

Returns an empty `Vec` (not an error) when no elements match — consistent with the JS
`querySelectorAll` contract.

---

## `NodeHandle`

A `NodeHandle` wraps a CDP `RemoteObjectId` and provides typed accessors. All operations
are lazy and execute over the open WebSocket connection; no HTML serialisation occurs until
you explicitly call a method.

### Reading content

```rust,no_run
let node = &nodes[0]; // NodeHandle

// Inner text (JS textContent)
let text: String = node.text_content().await?;

// Full outer HTML — the default `OuterHtmlStrategy::Current` strategy.
let html: String = node.outer_html().await?;

// Inner HTML only (children, not the element itself)
let inner: String = node.inner_html().await?;

// All attributes as a HashMap<name, value> in one CDP round-trip
let attrs = node.attr_map().await?;

// CSS class string (split on whitespace to get individual classes)
let class_str = node.attr("class").await?.unwrap_or_default();
let classes: Vec<&str> = class_str.split_whitespace().collect();

// Ancestor tag names as a Vec (nearest first: ["li", "ul", "nav", "body"])
let ancestors: Vec<String> = node.ancestors().await?;
```

### Reading attributes

```rust,no_run
// Returns the attribute value or an empty string if absent
let href:    String = node.attr("href").await?;
let data_id: String = node.attr("data-id").await?;
```

---

## Deep outerHTML resolution (`outer_html_with_strategy`)

`NodeHandle::outer_html()` uses the legacy `OuterHtmlStrategy::Current`
strategy by default — a Chromium element-level JS evaluation of
`this.outerHTML` with a `XMLSerializer` fallback when the primary call
returns an empty payload. That covers most pages, but highly dynamic
sites (notably Wix Studio / Editor X meshes, large SPAs, and pages with
deep shadow-DOM subtrees) intermittently return truncated or empty
payloads from the JS-side `outerHTML` accessor.

For those cases use `outer_html_with_strategy(OuterHtmlStrategy::Recursive)`
directly. The `Recursive` strategy prefers the dedicated CDP command
`DOM.getOuterHTML` — a single round-trip that performs the serialisation
**inside the browser** with shadow-DOM roots included by default — and
falls back to a Rust-side walk that calls `DOM.describeNode(nodeId, depth=-1)`
and serialises the resulting `Node` tree locally.

```rust,no_run
use stygian_browser::OuterHtmlStrategy;

let result = node.outer_html_with_strategy(OuterHtmlStrategy::Recursive).await?;
match result {
    OuterHtmlResult::Content(html) => println!("got {} bytes", html.len()),
    OuterHtmlResult::Empty         => println!("both backends returned empty"),
    OuterHtmlResult::Failed { backends } => {
        eprintln!("all backends failed: {}", backends.join(", "));
    }
}
```

### `OuterHtmlStrategy` variants

| Variant | Backend | When to use |
| --- | --- | --- |
| `Current` *(default)* | Element-level JS eval `this.outerHTML` → `XMLSerializer` fallback | The historical default; works for most pages |
| `Recursive` | CDP `DOM.getOuterHTML` (single round-trip) → Rust-side `DOM.describeNode` walk fallback | Wix Studio / Editor X meshes, large SPAs, deep shadow-DOM subtrees, pages where the JS-side `outerHTML` accessor intermittently returns empty |

### `OuterHtmlResult` variants

| Variant | Meaning |
| --- | --- |
| `Content(String)` | Successfully serialised outer markup |
| `Empty` | Every backend the strategy tried returned an empty payload (page may still be rendering or the node has been detached) |
| `Failed { backends: Vec<&'static str> }` | Every backend errored; `backends` lists them in attempt order for diagnostics |

`OuterHtmlResult` implements `Serialize` so the outcome can be emitted in
structured logs and per-request reports. `Display` returns
`"Empty"`, `"Content(N bytes)"`, or `"Failed(a, b)"` for log lines.

### Why "Recursive" works generically (not Wix-specific)

The `Recursive` strategy resolves the Wix Studio / Editor X empty-payload
case **without any Wix-specific selectors, attributes, or heuristics**.
It just selects a different CDP backend — `DOM.getOuterHTML` — that
already handles deeply nested subtrees, large SPAs, and shadow-DOM
trees correctly in a single browser-side pass. The Rust-side
`DOM.describeNode(depth=-1)` walk is the second-line fallback for the
rare cases where `DOM.getOuterHTML` itself fails.

### Existing `outer_html()` is unchanged

`NodeHandle::outer_html()` is a thin backwards-compatible wrapper:

- `Content(html)` → `Ok(html)` — same as before.
- `Empty` → `Ok(String::new())` — preserves the historical contract.
- `Failed { .. }` → `Ok(String::new())` — preserves the historical
  contract (the legacy method could not surface backend diagnostics, so
  empty string is the safe fallback).

Callers that need to distinguish Empty / Content / Failed, or that want
the deep-resolution path on Wix Studio / shadow-DOM pages, should call
`outer_html_with_strategy` directly.

---

## DOM traversal

`NodeHandle` supports element-level traversal (skipping text and comment nodes).

### `parent()`

Returns the direct parent element, or `None` if the node is `<body>` or detached.

```rust,no_run
if let Some(parent) = node.parent().await? {
    let html = parent.outer_html().await?;
    println!("parent: {}", &html[..html.len().min(80)]);
}
```

### `next_sibling()`

Returns the next element sibling, or `None` if this is the last child.

```rust,no_run
// Walk a list forward
let items = page.query_selector_all("li.step").await?;
let mut cur = items[0].next_sibling().await?;
while let Some(node) = cur {
    println!("{}", node.text_content().await?);
    cur = node.next_sibling().await?;
}
```

### `previous_sibling()`

Returns the previous element sibling, or `None` if this is the first child.

```rust,no_run
if let Some(prev) = node.previous_sibling().await? {
    println!("previous: {}", prev.text_content().await?);
}
```

> **Stale nodes:** If the page navigates or the element is removed from the DOM between
> acquiring a `NodeHandle` and calling a method on it, the call returns
> `BrowserError::StaleNode`. Handle this like a normal `?` error.
>
> For `outer_html_with_strategy` specifically, a stale node surfaces as
> `OuterHtmlResult::Failed { backends: vec!["DOM.getOuterHTML"] }` (or
> `"DOM.describeNode-walk"` if the fallback is the one that errored) —
> the strategy records which backend reported the failure rather than
> bubbling the error through `?`. Use `outer_html()` directly if you
> want the legacy `?` error path.

---

## Adaptive similarity search

The `find_similar` feature (`similarity` cargo feature) locates elements that are
structurally similar to a reference node even when class names, depth, or IDs have
changed across page versions.

### Cargo feature

```toml
stygian-browser = { version = "*", features = ["similarity"] }
```

### How it works

`NodeHandle::fingerprint()` captures a structural snapshot:

```rust,no_run
use stygian_browser::similarity::ElementFingerprint;

let fp: ElementFingerprint = node.fingerprint().await?;
// fp.tag        — lower-case tag name ("div", "a", ...)
// fp.classes    — sorted CSS class list
// fp.attr_names — sorted attribute name list (excluding "class" / "id")
// fp.depth      — distance from <body>
```

Similarity is scored using a weighted Jaccard coefficient:

| Component | Weight |
| --------- | ------ |
| Tag name match | 40 % |
| Class list Jaccard | 35 % |
| Attribute names Jaccard | 15 % |
| Depth proximity | 10 % |

### `find_similar`

```rust,no_run
use stygian_browser::similarity::{SimilarityConfig, SimilarMatch};

// Default config: threshold = 0.7, max_results = 10
let matches: Vec<SimilarMatch> =
    page.find_similar(&fp, SimilarityConfig::default()).await?;

for m in &matches {
    println!("score {:.2}: {}", m.score, m.node.outer_html().await?);
}
```

### Custom config

```rust,no_run
let matches = page
    .find_similar(
        &fp,
        SimilarityConfig { threshold: 0.5, max_results: 5 },
    )
    .await?;
```

### Persisting fingerprints

`ElementFingerprint` is `serde::Serialize + Deserialize`, so you can capture a reference
element in one session and reuse it later:

```rust,no_run
// Capture
let fp = node.fingerprint().await?;
let json = serde_json::to_string(&fp)?;
tokio::fs::write("fingerprint.json", &json).await?;

// Reuse in a later session
let json = tokio::fs::read_to_string("fingerprint.json").await?;
let fp: ElementFingerprint = serde_json::from_str(&json)?;
let matches = page.find_similar(&fp, SimilarityConfig::default()).await?;
```
