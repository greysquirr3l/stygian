# stygian-extract-derive

Proc-macro backend for [`stygian-browser`](../stygian-browser)'s `#[derive(Extract)]`.

**Do not add this crate to your `Cargo.toml` directly.**
Use `stygian-browser` with the `extract` feature instead:

```toml
stygian-browser = { version = "*", features = ["extract"] }
```

Then annotate your struct:

```rust
use stygian_browser::extract::Extract;

#[derive(Extract)]
struct Product {
    #[selector("h2.title")]
    name: String,

    #[selector("button[data-sku]", attr = "data-sku")]
    sku: String,

    #[selector("img.hero", attr = "src")]
    image_url: Option<String>,
}
```
