# Structured Extraction — `#[derive(Extract)]`

`stygian-extract-derive` provides a procedural macro that maps a CSS selector spec directly
onto a Rust struct, letting you express your scraping schema as types rather than
imperative loops.

---

## Dependency

Add `stygian-extract-derive` to your `Cargo.toml`:

```toml
stygian-extract-derive = { version = "*" }
stygian-browser         = { version = "*" }
```

---

## Quick start

```rust,no_run
use stygian_extract_derive::Extract;
use stygian_browser::PageHandle;

#[derive(Debug, Extract)]
struct Article {
    #[selector("h1.title")]
    title: String,

    #[selector("a.author", attr = "href")]
    author_url: String,

    #[selector("p.summary")]
    summary: Option<String>,
}

let handle = pool.acquire().await?;
let mut page = handle.browser().expect("browser is available").new_page().await?;
page.navigate("https://example.com", WaitUntil::Load, Duration::from_secs(30)).await?;
let article: Article = page.extract::<Article>(".article-body").await?;
println!("{:#?}", article);
```

---

## `#[selector]` attribute variants

### Text content — `#[selector("css")]`

Selects the first matching element and captures its `textContent`.

```rust,no_run
#[selector("span.price")]
price: String,
```

### Attribute value — `#[selector("css", attr = "name")]`

Selects the first matching element and reads the named attribute.

```rust,no_run
#[selector("a.profile-link", attr = "href")]
profile_url: String,

#[selector("img.avatar", attr = "src")]
avatar_src: String,
```

### Nested struct — `#[selector("css", nested)]`

Selects the first matching element and applies the field type's selector spec within
that element's subtree. The field's type must also `#[derive(Extract)]`.

```rust,no_run
#[derive(Debug, Extract)]
struct Author {
    #[selector("span.name")]
    name: String,

    #[selector("a.social", attr = "href")]
    social_url: String,
}

#[derive(Debug, Extract)]
struct Post {
    #[selector("h2.title")]
    title: String,

    #[selector("div.author-block", nested)]
    author: Author,
}
```

---

## Optional fields

Wrap a field's type in `Option<T>` to treat a missing element as `None` rather than an
error. Non-optional fields propagate an `ExtractionError::NotFound` when no match exists.

```rust,no_run
#[derive(Debug, Extract)]
struct Product {
    #[selector("h1.name")]
    name: String,               // required — error if absent

    #[selector("span.sale-price")]
    sale_price: Option<String>, // optional — None if not on sale
}
```

---

## Extracting a list

For pages with repeating items, call `page.extract_all::<T>(root_selector)`:

```rust,no_run
#[derive(Debug, Extract)]
struct SearchResult {
    #[selector("h3 a")]
    title: String,

    #[selector("h3 a", attr = "href")]
    url: String,

    #[selector("div.snippet")]
    snippet: Option<String>,
}

let results: Vec<SearchResult> =
    page.extract_all::<SearchResult>("div.g").await?;

for r in &results {
    println!("{}: {}", r.title, r.url);
}
```

Each element matching `div.g` acts as a scoped root for that item's selectors.

---

## Full example — news article

```rust,no_run
use stygian_extract_derive::Extract;
use stygian_browser::PageHandle;

#[derive(Debug, Extract)]
struct ByLine {
    #[selector("a.author-name")]
    name: String,

    #[selector("a.author-name", attr = "href")]
    profile: String,
}

#[derive(Debug, Extract)]
struct NewsArticle {
    #[selector("h1")]
    headline: String,

    #[selector("div.byline", nested)]
    by_line: ByLine,

    #[selector("time", attr = "datetime")]
    published_at: String,

    #[selector("div.article-body")]
    body: String,

    #[selector("ul.tags")]
    tags: Option<String>,
}

async fn scrape(page: &mut PageHandle) -> Result<NewsArticle, Box<dyn std::error::Error>> {
    let article = page.extract::<NewsArticle>("article.main").await?;
    Ok(article)
}
```
