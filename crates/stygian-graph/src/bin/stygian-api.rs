//! stygian-api binary — REST API server for pipeline management
//!
//! Reads the following environment variables:
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | `MYCELIUM_API_KEY` | `"dev-key"` | API key for `/pipelines` routes |
//! | `MYCELIUM_BIND` | `"0.0.0.0:8080"` | TCP address to listen on |
//! | `RUST_LOG` | `"info"` | Tracing filter |
//!
//! # Usage
//!
//! ```bash
//! MYCELIUM_API_KEY=secret cargo run --bin stygian-api
//! curl -H "X-Api-Key: secret" http://localhost:8080/pipelines
//! ```

use stygian_graph::application::api_server::ApiServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialise tracing
    let log_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(log_filter)
        .compact()
        .init();

    let bind = std::env::var("MYCELIUM_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    ApiServer::from_env().run(&bind).await
}
