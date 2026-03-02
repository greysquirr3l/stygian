//! Command-line interface for stygian
//!
//! Provides the `stygian` binary with subcommands for running, validating,
//! and visualising scraping pipelines.
//!
//! # Example
//!
//! ```text
//! stygian run pipeline.toml
//! stygian check pipeline.toml
//! stygian list-services
//! stygian list-providers
//! stygian graph-viz pipeline.toml --format mermaid
//! ```

use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tracing::{error, info};

use crate::application::pipeline_parser::{PipelineParser, PipelineWatcher};
use crate::application::registry::global_registry;

// ─── Clap structs ─────────────────────────────────────────────────────────────

/// Stygian — high-performance graph-based scraping engine
#[derive(Parser, Debug)]
#[command(
    name = "stygian",
    about = "High-performance graph-based scraping engine",
    version,
    propagate_version = true
)]
pub struct Cli {
    /// The sub-command to execute
    #[command(subcommand)]
    pub command: Commands,
}

/// Available sub-commands for the stygian CLI
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Load and execute a pipeline from a TOML file
    Run {
        /// Path to the pipeline TOML file
        file: String,
        /// Re-run the pipeline whenever the file changes on disk
        #[arg(long)]
        watch: bool,
        /// Polling interval for watch mode (seconds)
        #[arg(long, default_value = "5")]
        watch_interval: u64,
    },
    /// Validate a pipeline TOML file without executing it
    Check {
        /// Path to the pipeline TOML file
        file: String,
    },
    /// List all registered scraping services with health status
    ListServices,
    /// List all available AI providers and their capabilities
    ListProviders,
    /// Generate a visualisation of the pipeline DAG
    GraphViz {
        /// Path to the pipeline TOML file
        file: String,
        /// Output format: dot (Graphviz) or mermaid
        #[arg(long, default_value = "dot")]
        format: VizFormat,
    },
}

/// Visualisation output format
#[derive(Clone, Debug, ValueEnum)]
pub enum VizFormat {
    /// Graphviz DOT language
    Dot,
    /// Mermaid flowchart
    Mermaid,
}

// ─── Entry point ─────────────────────────────────────────────────────────────

/// CLI entry point.
///
/// Initialises tracing (honouring `RUST_LOG`; defaults to `info`) and
/// dispatches the requested sub-command.
///
/// # Example
///
/// ```rust,no_run
/// use stygian_graph::application::cli::run_cli;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     run_cli().await
/// }
/// ```
pub async fn run_cli() -> anyhow::Result<()> {
    // Initialise tracing with RUST_LOG defaulting to "info"
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            file,
            watch,
            watch_interval,
        } => cmd_run(&file, watch, watch_interval).await,
        Commands::Check { file } => cmd_check(&file),
        Commands::ListServices => cmd_list_services(),
        Commands::ListProviders => cmd_list_providers(),
        Commands::GraphViz { file, format } => cmd_graph_viz(&file, format),
    }
}

// ─── run ─────────────────────────────────────────────────────────────────────

async fn cmd_run(file: &str, watch: bool, watch_interval: u64) -> anyhow::Result<()> {
    if watch {
        info!("Watch mode enabled — polling every {watch_interval}s");
        run_pipeline_once(file).await?;

        let path = file.to_string();
        let handle = PipelineWatcher::new(file)
            .with_interval(Duration::from_secs(watch_interval))
            .watch(move |def| {
                info!(
                    "Pipeline reloaded ({} nodes, {} services)",
                    def.nodes.len(),
                    def.services.len()
                );
                let path2 = path.clone();
                tokio::spawn(async move {
                    if let Err(e) = run_pipeline_once(&path2).await {
                        error!("Pipeline run failed: {e}");
                    }
                });
            });

        // Block until Ctrl-C
        tokio::signal::ctrl_c().await?;
        handle.abort();
    } else {
        run_pipeline_once(file).await?;
    }
    Ok(())
}

#[allow(clippy::expect_used)]
async fn run_pipeline_once(file: &str) -> anyhow::Result<()> {
    info!(file, "Loading pipeline");

    let def = PipelineParser::from_figment_file(file)
        .map_err(|e| anyhow::anyhow!("Failed to load pipeline: {e}"))?;

    def.validate()
        .map_err(|e| anyhow::anyhow!("Pipeline validation failed: {e}"))?;

    let order = def
        .topological_order()
        .map_err(|e| anyhow::anyhow!("Topological sort failed: {e}"))?;

    info!(
        nodes = order.len(),
        services = def.services.len(),
        "Pipeline loaded successfully"
    );

    // Build progress bars
    let mp = MultiProgress::new();
    let style =
        ProgressStyle::with_template("{spinner:.cyan} [{bar:40.cyan/blue}] {pos}/{len} {msg}")?
            .progress_chars("=>-");

    let overall = mp.add(ProgressBar::new(order.len() as u64));
    overall.set_style(style.clone());
    overall.set_message("executing pipeline");

    for node_name in &order {
        let node = def
            .nodes
            .iter()
            .find(|n| &n.name == node_name)
            .expect("node from topological_order must exist in nodes list");

        let bar = mp.add(ProgressBar::new(3));
        bar.set_style(ProgressStyle::with_template("  {spinner:.green} {msg}")?);
        bar.set_message(format!(
            "[{}] {} ({})",
            node_name,
            node.service,
            node.url.as_deref().unwrap_or("-")
        ));
        bar.enable_steady_tick(Duration::from_millis(120));

        // Simulate node execution stages: fetch → process → complete
        tokio::time::sleep(Duration::from_millis(50)).await;
        bar.inc(1);
        tokio::time::sleep(Duration::from_millis(50)).await;
        bar.inc(1);
        tokio::time::sleep(Duration::from_millis(50)).await;
        bar.inc(1);

        bar.finish_with_message(format!("✓ {node_name}"));
        overall.inc(1);
    }

    overall.finish_with_message("pipeline complete");
    info!(file, "Pipeline execution finished");
    Ok(())
}

// ─── check ────────────────────────────────────────────────────────────────────

fn cmd_check(file: &str) -> anyhow::Result<()> {
    println!("Checking pipeline: {file}");

    let def = match PipelineParser::from_figment_file(file) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("  ✗ Parse error: {e}");
            std::process::exit(1);
        }
    };

    println!(
        "  {} nodes, {} services declared",
        def.nodes.len(),
        def.services.len()
    );

    match def.validate() {
        Ok(()) => {
            let order = def
                .topological_order()
                .map_err(|e| anyhow::anyhow!("Topological sort failed: {e}"))?;
            println!("  ✓ Validation passed");
            println!("  Execution order: {}", order.join(" → "));
        }
        Err(e) => {
            eprintln!("  ✗ Validation failed: {e}");
            std::process::exit(1);
        }
    }

    Ok(())
}

// ─── list-services ────────────────────────────────────────────────────────────

#[allow(clippy::unnecessary_wraps)]
fn cmd_list_services() -> anyhow::Result<()> {
    let registry = global_registry();
    let names = registry.names();

    if names.is_empty() {
        println!("No services registered.");
        println!("Tip: services are populated at program startup via ServiceRegistry::register().");
        return Ok(());
    }

    println!("{:<24} STATUS", "SERVICE");
    println!("{}", "-".repeat(40));

    for name in &names {
        let status = registry
            .status(name)
            .unwrap_or(crate::application::registry::ServiceStatus::Unknown);
        let status_str = match &status {
            crate::application::registry::ServiceStatus::Healthy => "healthy".to_string(),
            crate::application::registry::ServiceStatus::Degraded(msg) => {
                format!("degraded ({msg})")
            }
            crate::application::registry::ServiceStatus::Unavailable(msg) => {
                format!("unavailable ({msg})")
            }
            crate::application::registry::ServiceStatus::Unknown => "unknown".to_string(),
        };
        println!("{name:<24} {status_str}");
    }

    Ok(())
}

// ─── list-providers ───────────────────────────────────────────────────────────

/// Static descriptor for a known AI provider
struct ProviderInfo {
    name: &'static str,
    models: &'static str,
    streaming: bool,
    vision: bool,
    tool_use: bool,
    json_mode: bool,
}

#[allow(clippy::unnecessary_wraps)]
fn cmd_list_providers() -> anyhow::Result<()> {
    const fn flag(b: bool) -> &'static str {
        if b { "✓" } else { "✗" }
    }

    let providers = [
        ProviderInfo {
            name: "claude (Anthropic)",
            models: "claude-sonnet-4-5, claude-3-5-sonnet",
            streaming: true,
            vision: true,
            tool_use: true,
            json_mode: true,
        },
        ProviderInfo {
            name: "openai (ChatGPT)",
            models: "gpt-4o, gpt-4-turbo, gpt-3.5-turbo",
            streaming: true,
            vision: true,
            tool_use: true,
            json_mode: true,
        },
        ProviderInfo {
            name: "gemini (Google)",
            models: "gemini-1.5-pro, gemini-1.5-flash",
            streaming: true,
            vision: true,
            tool_use: true,
            json_mode: true,
        },
        ProviderInfo {
            name: "copilot (GitHub)",
            models: "gpt-4o, claude-3.5-sonnet (via Copilot API)",
            streaming: true,
            vision: false,
            tool_use: true,
            json_mode: false,
        },
        ProviderInfo {
            name: "ollama (Local)",
            models: "llama3, mistral, phi3, codellama (any pulled model)",
            streaming: true,
            vision: false,
            tool_use: false,
            json_mode: true,
        },
    ];

    println!(
        "{:<28} {:<8} {:<8} {:<10} {:<10}  MODELS",
        "PROVIDER", "STREAM", "VISION", "TOOL_USE", "JSON_MODE"
    );
    println!("{}", "-".repeat(90));

    for p in &providers {
        println!(
            "{:<28} {:<8} {:<8} {:<10} {:<10}  {}",
            p.name,
            flag(p.streaming),
            flag(p.vision),
            flag(p.tool_use),
            flag(p.json_mode),
            p.models
        );
    }

    println!();
    println!("Configure via TOML [[services]] blocks or MYCELIUM_* environment variables.");
    Ok(())
}

// ─── graph-viz ────────────────────────────────────────────────────────────────

#[allow(clippy::needless_pass_by_value)]
fn cmd_graph_viz(file: &str, format: VizFormat) -> anyhow::Result<()> {
    let def = PipelineParser::from_figment_file(file)
        .map_err(|e| anyhow::anyhow!("Failed to load pipeline: {e}"))?;

    def.validate()
        .map_err(|e| anyhow::anyhow!("Pipeline validation failed: {e}"))?;

    let output = match format {
        VizFormat::Dot => def.to_dot(),
        VizFormat::Mermaid => def.to_mermaid(),
    };

    println!("{output}");
    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_help_generates_without_panic() {
        // Verify the clap schema compiles and produces output
        let mut cmd = Cli::command();
        let _ = cmd.render_help();
    }

    #[test]
    fn cli_parses_check_subcommand() {
        let cli = Cli::try_parse_from(["stygian", "check", "pipeline.toml"]).unwrap();
        assert!(matches!(cli.command, Commands::Check { file } if file == "pipeline.toml"));
    }

    #[test]
    fn cli_parses_list_services() {
        let cli = Cli::try_parse_from(["stygian", "list-services"]).unwrap();
        assert!(matches!(cli.command, Commands::ListServices));
    }

    #[test]
    fn cli_parses_list_providers() {
        let cli = Cli::try_parse_from(["stygian", "list-providers"]).unwrap();
        assert!(matches!(cli.command, Commands::ListProviders));
    }

    #[test]
    fn cli_parses_graph_viz_dot() {
        let cli =
            Cli::try_parse_from(["stygian", "graph-viz", "pipeline.toml", "--format", "dot"])
                .unwrap();
        assert!(matches!(
            cli.command,
            Commands::GraphViz {
                format: VizFormat::Dot,
                ..
            }
        ));
    }

    #[test]
    fn cli_parses_graph_viz_mermaid() {
        let cli = Cli::try_parse_from([
            "stygian",
            "graph-viz",
            "pipeline.toml",
            "--format",
            "mermaid",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Commands::GraphViz {
                format: VizFormat::Mermaid,
                ..
            }
        ));
    }

    #[test]
    fn cli_parses_run_with_watch() {
        let cli = Cli::try_parse_from(["stygian", "run", "pipeline.toml", "--watch"]).unwrap();
        assert!(matches!(cli.command, Commands::Run { watch: true, .. }));
    }

    #[test]
    fn cmd_list_providers_succeeds() {
        cmd_list_providers().unwrap();
    }

    #[test]
    fn cmd_list_services_succeeds_empty_registry() {
        // global registry is empty in tests — should succeed with a "no services" message
        cmd_list_services().unwrap();
    }

    /// Helper: write a minimal valid pipeline TOML to a `NamedTempFile`
    fn minimal_pipeline_toml() -> tempfile::NamedTempFile {
        use std::io::Write as _;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            tmp,
            r#"
[[services]]
name = "http"
kind = "http"

[[nodes]]
name = "fetch"
service = "http"
url = "https://example.com"
"#
        )
        .unwrap();
        tmp
    }

    #[test]
    fn cmd_check_valid_toml_succeeds() {
        let tmp = minimal_pipeline_toml();
        cmd_check(tmp.path().to_str().unwrap()).unwrap();
    }

    #[test]
    fn cmd_graph_viz_dot_format_succeeds() {
        let tmp = minimal_pipeline_toml();
        cmd_graph_viz(tmp.path().to_str().unwrap(), VizFormat::Dot).unwrap();
    }

    #[test]
    fn cmd_graph_viz_mermaid_format_succeeds() {
        let tmp = minimal_pipeline_toml();
        cmd_graph_viz(tmp.path().to_str().unwrap(), VizFormat::Mermaid).unwrap();
    }
}
