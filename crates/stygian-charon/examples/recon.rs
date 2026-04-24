use std::{env, fs};

use stygian_charon::{InvestigationBundle, analyze_and_plan, investigate_har, plan_from_report};

fn print_bundle(bundle: &InvestigationBundle) {
    println!(
        "page_title={}",
        bundle.report.page_title.as_deref().unwrap_or("<none>")
    );
    println!("total_requests={}", bundle.report.total_requests);
    println!("blocked_requests={}", bundle.report.blocked_requests);
    println!("aggregate_provider={:?}", bundle.report.aggregate.provider);
    println!(
        "aggregate_confidence={:.3}",
        bundle.report.aggregate.confidence
    );
    println!(
        "recommendation_strategy={:?}",
        bundle.requirements.recommendation.strategy
    );
    println!("risk_score={:.3}", bundle.policy.risk_score);
    println!("provider_histogram={:?}", bundle.report.provider_histogram);

    println!("top_markers_count={}", bundle.report.top_markers.len());
    for marker in bundle.report.top_markers.iter().take(10) {
        println!("top_marker.{}={}", marker.marker, marker.count);
    }

    println!(
        "suspicious_requests_count={}",
        bundle.report.suspicious_requests.len()
    );
    for request in bundle.report.suspicious_requests.iter().take(5) {
        println!(
            "suspicious_request.status={} provider={:?} url={}",
            request.status, request.detection.provider, request.url
        );
    }

    for (key, value) in &bundle.requirements.recommendation.config_hints {
        println!("config_hint.{key}={value}");
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args();
    let _bin = args.next();
    let Some(har_path) = args.next() else {
        eprintln!("usage: cargo run --example recon -p stygian-charon -- <path-to.har>");
        std::process::exit(2);
    };

    let har_json = fs::read_to_string(&har_path)?;

    let bundle = analyze_and_plan(&har_json)?;
    print_bundle(&bundle);

    let report = investigate_har(&har_json)?;
    let bundle_from_report = plan_from_report(report);
    eprintln!(
        "verified plan_from_report strategy={:?}",
        bundle_from_report.requirements.recommendation.strategy
    );

    Ok(())
}
