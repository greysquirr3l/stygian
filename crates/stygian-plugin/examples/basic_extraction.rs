//! Basic extraction example

use serde_json::json;
use stygian_plugin::{
    adapters::ExtractionEngine,
    domain::{ExtractionRequest, ExtractionTemplate, Region, Selector, Transformation},
    ports::PluginExtractionPort,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a simple extraction template
    let template = ExtractionTemplate::new("Basic Example")
        .with_description("Extract title and price from a product page")
        .with_region(
            Region::new(
                "title",
                Selector::css("h1.product-title"),
                json!({"type": "string"}),
            )
            .with_transformation(Transformation::Trim),
        )
        .with_region(
            Region::new(
                "price",
                Selector::css(".product-price"),
                json!({"type": "string"}),
            )
            .with_transformation(Transformation::Trim),
        );

    // Sample HTML
    let html = r#"
        <html>
            <h1 class="product-title">  Widget 3000  </h1>
            <span class="product-price">$99.99</span>
        </html>
    "#;

    // Create extraction request
    let request = ExtractionRequest::new(template, "https://example.com/product", html);

    // Execute extraction using the engine
    let engine = ExtractionEngine;
    let result = engine.execute(&request).await?;

    println!("Extraction Result:");
    println!("  Success: {}", result.is_fully_successful());
    println!("  Elapsed: {}ms", result.metadata.elapsed_ms);
    println!("  Data: {:#?}", result.data);

    Ok(())
}
