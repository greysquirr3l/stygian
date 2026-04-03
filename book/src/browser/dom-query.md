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

// Full outer HTML
let html: String = node.outer_html().await?;

// Inner HTML only (children, not the element itself)
let inner: String = node.inner_html().await?;

// Tag name — always lowercase
let tag: String = node.tag_name().await?; // e.g. "div", "a", "li"

// CSS classes as a Vec
let cls: Vec<String> = node.classes().await?;
```

### Reading attributes

```rust,no_run
// Returns the attribute value or an empty string if absent
let href:    String = node.attr("href").await?;
let data_id: String = node.attr("data-id").await?;
```

---

## DOM traversal

`NodeHandle` supports element-level traversal (skipping text and comment nodes).

### `parent()`

Returns the direct parent element, or `None` if the node is `<body>` or detached.

```rust,no_run
if let Some(parent) = node.parent().await? {
    println!("parent tag: {}", parent.tag_name().await?);
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

---

## Adaptive similarity search

The `find_similar` feature (`similarity` cargo feature) locates elements that are
structurally similar to a reference node even when class names, depth, or IDs have
changed across page versions.

### Cargo feature

```toml
stygian-browser = { version = "0.8", features = ["similarity"] }
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
