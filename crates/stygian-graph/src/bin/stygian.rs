//! Stygian CLI binary entry point

use stygian_graph::application::cli::run_cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Run CLI
    run_cli().await
}
