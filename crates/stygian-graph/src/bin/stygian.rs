//! Stygian CLI binary entry point

use stygian_graph::application::cli::run_cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run_cli().await
}
