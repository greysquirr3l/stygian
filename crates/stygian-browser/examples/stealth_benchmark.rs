//! Run anti-bot stealth benchmark targets and write report artifacts.
//!
//! Usage:
//! `cargo run --example stealth_benchmark --all-features -- [--all] [--targets t1,t2] [--timeout 45] [--out-json path] [--out-md path]`

#[cfg(feature = "stealth")]
use std::fs;
#[cfg(feature = "stealth")]
use std::path::PathBuf;
#[cfg(feature = "stealth")]
use std::time::Duration;

#[cfg(feature = "stealth")]
use stygian_browser::config::StealthLevel;
#[cfg(feature = "stealth")]
use stygian_browser::validation::benchmark::{BenchmarkConfig, StealthBenchmark};
#[cfg(feature = "stealth")]
use stygian_browser::{BrowserConfig, BrowserPool};

#[cfg(feature = "stealth")]
struct Args {
    tier1_only: bool,
    targets: Vec<String>,
    timeout_secs: Option<u64>,
    out_json: PathBuf,
    out_md: PathBuf,
}

#[cfg(feature = "stealth")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args(&std::env::args().skip(1).collect::<Vec<_>>())?;

    let config = BrowserConfig::builder()
        .headless(true)
        .stealth_level(StealthLevel::Advanced)
        .build();
    let pool = BrowserPool::new(config).await?;

    let selected_targets = BenchmarkConfig::parse_target_names(&args.targets);
    let benchmark_config = BenchmarkConfig {
        targets: selected_targets,
        tier1_only: args.tier1_only,
        continue_on_error: true,
        timeout_override: args.timeout_secs.map(Duration::from_secs),
    };

    let report = StealthBenchmark::run(&pool, &benchmark_config).await;

    let json = report.to_json_pretty()?;
    let markdown = report.to_markdown();

    fs::write(&args.out_json, json.as_bytes())?;
    fs::write(&args.out_md, markdown.as_bytes())?;

    println!(
        "wrote benchmark reports: json={} markdown={} passed={} failed={}",
        args.out_json.display(),
        args.out_md.display(),
        report.passed,
        report.failed
    );

    Ok(())
}

#[cfg(feature = "stealth")]
fn parse_args(args: &[String]) -> Result<Args, Box<dyn std::error::Error>> {
    let mut tier1_only = true;
    let mut targets = Vec::new();
    let mut timeout_secs = None;
    let mut out_json = PathBuf::from("target/stealth-benchmark.json");
    let mut out_md = PathBuf::from("target/stealth-benchmark.md");

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--all" {
            tier1_only = false;
        } else if let Some(val) = arg.strip_prefix("--targets=") {
            targets.extend(
                val.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string),
            );
        } else if arg == "--targets" {
            let val = iter
                .next()
                .ok_or("--targets requires a comma separated value")?;
            targets.extend(
                val.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string),
            );
        } else if let Some(val) = arg.strip_prefix("--timeout=") {
            timeout_secs = Some(
                val.parse::<u64>()
                    .map_err(|_| "--timeout must be an integer")?,
            );
        } else if arg == "--timeout" {
            let val = iter.next().ok_or("--timeout requires a value")?;
            timeout_secs = Some(
                val.parse::<u64>()
                    .map_err(|_| "--timeout must be an integer")?,
            );
        } else if let Some(val) = arg.strip_prefix("--out-json=") {
            out_json = PathBuf::from(val);
        } else if arg == "--out-json" {
            let val = iter.next().ok_or("--out-json requires a value")?;
            out_json = PathBuf::from(val);
        } else if let Some(val) = arg.strip_prefix("--out-md=") {
            out_md = PathBuf::from(val);
        } else if arg == "--out-md" {
            let val = iter.next().ok_or("--out-md requires a value")?;
            out_md = PathBuf::from(val);
        } else {
            return Err(format!("unknown argument: {arg}").into());
        }
    }

    Ok(Args {
        tier1_only,
        targets,
        timeout_secs,
        out_json,
        out_md,
    })
}

#[cfg(not(feature = "stealth"))]
fn main() {
    eprintln!("stealth_benchmark example requires --features stealth");
}
