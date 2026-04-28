#[cfg(feature = "live-validation")]
use std::collections::BTreeMap;
use std::env;
use std::fs;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use stygian_charon::{
    BlockedRatioSlo, RequirementLevel, RequirementsProfile, TargetClass, build_runtime_policy,
    infer_requirements_with_target_class, investigate_har,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum StealthLevel {
    Low,
    Medium,
    High,
}

impl StealthLevel {
    fn from_cli(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            _ => Err(format!(
                "invalid stealth level '{value}', expected one of: low, medium, high"
            )),
        }
    }

    #[cfg(feature = "live-validation")]
    const fn timeout(self) -> std::time::Duration {
        match self {
            Self::Low => std::time::Duration::from_secs(8),
            Self::Medium => std::time::Duration::from_secs(15),
            Self::High => std::time::Duration::from_secs(25),
        }
    }

    #[cfg(feature = "live-validation")]
    const fn user_agent(self) -> &'static str {
        match self {
            Self::Low => "stygian-charon-live-validator/0.11",
            Self::Medium => {
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
            }
            Self::High => {
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
            }
        }
    }
}

#[derive(Debug)]
struct Config {
    url: Option<String>,
    har_path: Option<String>,
    target_class: TargetClass,
    stealth_level: StealthLevel,
    baseline_out: Option<String>,
    baseline_compare: Option<String>,
}

#[derive(Debug, Serialize)]
struct SloAssessment {
    acceptable: bool,
    warning: bool,
    critical: bool,
    blocked_ratio: f64,
    thresholds: BlockedRatioSlo,
}

#[derive(Debug, Serialize)]
struct Output {
    input_url: Option<String>,
    har_source: String,
    target_class: TargetClass,
    stealth_level: StealthLevel,
    slo_assessment: SloAssessment,
    escalation_level: String,
    report: stygian_charon::InvestigationReport,
    requirements: RequirementsProfile,
    policy: stygian_charon::RuntimePolicy,
    baseline_comparison: Option<BaselineComparison>,
    har: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BaselineArtifact {
    target_class: TargetClass,
    stealth_level: StealthLevel,
    blocked_ratio: f64,
    escalation_level: String,
    risk_score: f64,
    execution_mode: stygian_charon::ExecutionMode,
    session_mode: stygian_charon::SessionMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BaselineComparison {
    baseline_path: String,
    blocked_ratio_delta: f64,
    risk_score_delta: f64,
    escalation_changed: bool,
    recommended_action: String,
}

fn parse_target_class(value: &str) -> Result<TargetClass, String> {
    match value.to_ascii_lowercase().as_str() {
        "api" => Ok(TargetClass::Api),
        "contentsite" | "content-site" | "content_site" | "content" => Ok(TargetClass::ContentSite),
        "highsecurity" | "high-security" | "high_security" | "high" => {
            Ok(TargetClass::HighSecurity)
        }
        "unknown" => Ok(TargetClass::Unknown),
        _ => Err(format!(
            "invalid target class '{value}', expected one of: api, content-site, high-security, unknown"
        )),
    }
}

fn parse_args() -> Result<Config, String> {
    let mut args = env::args().skip(1);

    let mut url: Option<String> = None;
    let mut har_path: Option<String> = None;
    let mut target_class = TargetClass::Unknown;
    let mut stealth_level = StealthLevel::Medium;
    let mut baseline_out: Option<String> = None;
    let mut baseline_compare: Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--url" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --url".to_string())?;
                url = Some(value);
            }
            "--har-path" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --har-path".to_string())?;
                har_path = Some(value);
            }
            "--target-class" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --target-class".to_string())?;
                target_class = parse_target_class(&value)?;
            }
            "--stealth-level" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --stealth-level".to_string())?;
                stealth_level = StealthLevel::from_cli(&value)?;
            }
            "--baseline-out" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --baseline-out".to_string())?;
                baseline_out = Some(value);
            }
            "--baseline-compare" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --baseline-compare".to_string())?;
                baseline_compare = Some(value);
            }
            "--help" | "-h" => {
                return Err(usage());
            }
            other => {
                return Err(format!("unknown argument '{other}'\n\n{}", usage()));
            }
        }
    }

    if url.is_none() && har_path.is_none() {
        return Err(format!(
            "either --url or --har-path is required\n\n{}",
            usage()
        ));
    }

    Ok(Config {
        url,
        har_path,
        target_class,
        stealth_level,
        baseline_out,
        baseline_compare,
    })
}

fn usage() -> String {
    [
        "usage:",
        "  cargo run -p stygian-charon --example live_slo_validator --features live-validation -- \\",
        "    --url <target-url> --target-class <api|content-site|high-security|unknown> --stealth-level <low|medium|high> [--baseline-out <path>] [--baseline-compare <path>]",
        "",
        "or:",
        "  cargo run -p stygian-charon --example live_slo_validator --features live-validation -- \\",
        "    --har-path <path-to.har> --target-class <api|content-site|high-security|unknown> --stealth-level <low|medium|high> [--baseline-out <path>] [--baseline-compare <path>]",
    ]
    .join("\n")
}

#[cfg(feature = "live-validation")]
fn parse_headers(headers: &reqwest::header::HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|parsed| (name.as_str().to_string(), parsed.to_string()))
        })
        .collect()
}

#[cfg(feature = "live-validation")]
fn build_har_for_single_request(
    url: &str,
    status: u16,
    headers: &BTreeMap<String, String>,
) -> String {
    let response_headers = headers
        .iter()
        .map(|(name, value)| json!({"name": name, "value": value}))
        .collect::<Vec<_>>();

    json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "stygian-charon-live-validator", "version": "0.11"},
            "pages": [{
                "id": "page_1",
                "title": url,
                "startedDateTime": "2026-01-01T00:00:00.000Z",
                "pageTimings": {"onLoad": 0}
            }],
            "entries": [{
                "pageref": "page_1",
                "startedDateTime": "2026-01-01T00:00:00.000Z",
                "time": 0,
                "request": {
                    "method": "GET",
                    "url": url,
                    "httpVersion": "HTTP/2",
                    "headers": [],
                    "queryString": [],
                    "cookies": [],
                    "headersSize": -1,
                    "bodySize": 0
                },
                "response": {
                    "status": status,
                    "statusText": "live-capture",
                    "httpVersion": "HTTP/2",
                    "headers": response_headers,
                    "cookies": [],
                    "content": {"size": 0, "mimeType": "text/html", "text": ""},
                    "redirectURL": "",
                    "headersSize": -1,
                    "bodySize": 0
                },
                "cache": {},
                "timings": {
                    "blocked": 0,
                    "dns": 0,
                    "connect": 0,
                    "send": 0,
                    "wait": 0,
                    "receive": 0,
                    "ssl": 0
                }
            }]
        }
    })
    .to_string()
}

#[cfg(feature = "live-validation")]
async fn capture_har_from_url(url: &str, stealth_level: StealthLevel) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(stealth_level.timeout())
        .user_agent(stealth_level.user_agent())
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))?;

    let response = client
        .get(url)
        .header(
            "accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("accept-language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|error| format!("failed to fetch URL '{url}': {error}"))?;

    let status = response.status().as_u16();
    let headers = parse_headers(response.headers());

    Ok(build_har_for_single_request(url, status, &headers))
}

fn resolve_escalation_level(requirements: &RequirementsProfile) -> String {
    let adaptive = requirements
        .requirements
        .iter()
        .find(|requirement| requirement.id == "adaptive_rate_and_retry_budget");

    match adaptive.map(|requirement| requirement.level) {
        Some(RequirementLevel::High) => "High".to_string(),
        Some(RequirementLevel::Medium) => "Medium".to_string(),
        _ => "Acceptable".to_string(),
    }
}

fn build_baseline_artifact(
    config: &Config,
    slo_assessment: &SloAssessment,
    escalation_level: &str,
    policy: &stygian_charon::RuntimePolicy,
) -> BaselineArtifact {
    BaselineArtifact {
        target_class: config.target_class,
        stealth_level: config.stealth_level,
        blocked_ratio: slo_assessment.blocked_ratio,
        escalation_level: escalation_level.to_string(),
        risk_score: policy.risk_score,
        execution_mode: policy.execution_mode,
        session_mode: policy.session_mode,
    }
}

fn recommended_action(
    blocked_ratio_delta: f64,
    risk_score_delta: f64,
    escalation_changed: bool,
) -> String {
    if escalation_changed || blocked_ratio_delta >= 0.05 || risk_score_delta >= 0.10 {
        "investigate_regression".to_string()
    } else if blocked_ratio_delta <= -0.05 && risk_score_delta <= -0.10 {
        "improved_stability".to_string()
    } else {
        "monitor".to_string()
    }
}

fn compare_to_baseline(
    baseline_path: &str,
    current: &BaselineArtifact,
) -> Result<BaselineComparison, Box<dyn std::error::Error>> {
    let baseline_json = fs::read_to_string(baseline_path)?;
    let baseline: BaselineArtifact = serde_json::from_str(&baseline_json)?;

    let blocked_ratio_delta = current.blocked_ratio - baseline.blocked_ratio;
    let risk_score_delta = current.risk_score - baseline.risk_score;
    let escalation_changed = current.escalation_level != baseline.escalation_level;

    Ok(BaselineComparison {
        baseline_path: baseline_path.to_string(),
        blocked_ratio_delta,
        risk_score_delta,
        escalation_changed,
        recommended_action: recommended_action(
            blocked_ratio_delta,
            risk_score_delta,
            escalation_changed,
        ),
    })
}

fn run_core(
    config: &Config,
    har_json: &str,
    har_source: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let report = investigate_har(har_json)?;
    let requirements = infer_requirements_with_target_class(&report, config.target_class);
    let policy = build_runtime_policy(&report, &requirements);

    let blocked_ratio = if report.total_requests == 0 {
        0.0
    } else {
        to_f64(report.blocked_requests) / to_f64(report.total_requests)
    };

    let thresholds = BlockedRatioSlo::for_class(config.target_class);
    let (acceptable, warning, critical) = thresholds.assess(blocked_ratio);

    let escalation_level = resolve_escalation_level(&requirements);

    let slo_assessment = SloAssessment {
        acceptable,
        warning,
        critical,
        blocked_ratio,
        thresholds,
    };

    let baseline_artifact =
        build_baseline_artifact(config, &slo_assessment, &escalation_level, &policy);

    if let Some(path) = &config.baseline_out {
        let baseline_json = serde_json::to_string_pretty(&baseline_artifact)?;
        fs::write(path, baseline_json)?;
    }

    let baseline_comparison = match &config.baseline_compare {
        Some(path) => Some(compare_to_baseline(path, &baseline_artifact)?),
        None => None,
    };

    let output = Output {
        input_url: config.url.clone(),
        har_source,
        target_class: config.target_class,
        stealth_level: config.stealth_level,
        slo_assessment,
        escalation_level,
        report,
        requirements,
        policy,
        baseline_comparison,
        har: serde_json::from_str(har_json).unwrap_or_else(|_| json!({"raw": har_json})),
    };

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

#[cfg(feature = "live-validation")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = match parse_args() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };

    let (har_json, har_source) = if let Some(path) = &config.har_path {
        (
            fs::read_to_string(path).map_err(|error| {
                let message = format!("failed to read HAR file '{path}': {error}");
                std::io::Error::other(message)
            })?,
            format!("file:{path}"),
        )
    } else if let Some(url) = &config.url {
        (
            capture_har_from_url(url, config.stealth_level)
                .await
                .map_err(std::io::Error::other)?,
            "live:reqwest".to_string(),
        )
    } else {
        let message = "internal argument parsing error: no URL or HAR source provided".to_string();
        return Err(std::io::Error::other(message).into());
    };

    run_core(&config, &har_json, har_source)
}

#[cfg(not(feature = "live-validation"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = match parse_args() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };

    let Some(path) = config.har_path.clone() else {
        let message = "--url mode requires --features live-validation".to_string();
        return Err(std::io::Error::other(message).into());
    };

    let har_json = fs::read_to_string(&path).map_err(|error| {
        let message = format!("failed to read HAR file '{path}': {error}");
        std::io::Error::other(message)
    })?;

    run_core(&config, &har_json, format!("file:{path}"))
}

#[allow(clippy::cast_precision_loss)]
const fn to_f64(value: u64) -> f64 {
    value as f64
}
