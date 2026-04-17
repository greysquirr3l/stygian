//! Stealth benchmark harness for anti-bot validation targets.
//!
//! The harness executes one or more [`ValidationTarget`] validators and emits
//! deterministic JSON/Markdown reports for local runs and CI artifacts.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::pool::BrowserPool;

use super::{ValidationResult, ValidationSuite, ValidationTarget};

/// Benchmark target category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkCategory {
    /// Browser fingerprint observatories/scanners.
    Fingerprint,
    /// Challenge-heavy bot-protection pages.
    Challenge,
    /// Network/IP/WebRTC leak checks.
    NetworkLeak,
}

impl fmt::Display for BenchmarkCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Fingerprint => "fingerprint",
            Self::Challenge => "challenge",
            Self::NetworkLeak => "network_leak",
        };
        f.write_str(value)
    }
}

/// Static benchmark metadata for a validation target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BenchmarkTarget {
    /// Validation target enum key.
    pub target: ValidationTarget,
    /// Human readable target name.
    pub name: &'static str,
    /// Target URL.
    pub url: &'static str,
    /// Benchmark category.
    pub category: BenchmarkCategory,
    /// Per-target timeout for execution.
    #[serde(with = "duration_secs")]
    pub timeout: Duration,
}

impl BenchmarkTarget {
    /// Build benchmark metadata from a [`ValidationTarget`].
    #[must_use]
    pub const fn from_validation_target(target: ValidationTarget) -> Self {
        match target {
            ValidationTarget::CreepJs => Self {
                target,
                name: "CreepJS",
                url: target.url(),
                category: BenchmarkCategory::Fingerprint,
                timeout: Duration::from_secs(45),
            },
            ValidationTarget::BrowserScan => Self {
                target,
                name: "BrowserScan",
                url: target.url(),
                category: BenchmarkCategory::NetworkLeak,
                timeout: Duration::from_secs(45),
            },
            ValidationTarget::FingerprintJs => Self {
                target,
                name: "FingerprintJS",
                url: target.url(),
                category: BenchmarkCategory::Fingerprint,
                timeout: Duration::from_secs(45),
            },
            ValidationTarget::Kasada => Self {
                target,
                name: "Kasada",
                url: target.url(),
                category: BenchmarkCategory::Challenge,
                timeout: Duration::from_secs(45),
            },
            ValidationTarget::Cloudflare => Self {
                target,
                name: "Cloudflare",
                url: target.url(),
                category: BenchmarkCategory::Challenge,
                timeout: Duration::from_secs(45),
            },
            ValidationTarget::Akamai => Self {
                target,
                name: "Akamai",
                url: target.url(),
                category: BenchmarkCategory::Challenge,
                timeout: Duration::from_secs(45),
            },
            ValidationTarget::DataDome => Self {
                target,
                name: "DataDome",
                url: target.url(),
                category: BenchmarkCategory::Challenge,
                timeout: Duration::from_secs(45),
            },
            ValidationTarget::PerimeterX => Self {
                target,
                name: "PerimeterX",
                url: target.url(),
                category: BenchmarkCategory::Challenge,
                timeout: Duration::from_secs(45),
            },
        }
    }
}

/// Benchmark runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BenchmarkConfig {
    /// Targets to execute in order.
    pub targets: Vec<ValidationTarget>,
    /// Restrict execution to Tier-1 CI-safe targets.
    pub tier1_only: bool,
    /// Continue running remaining targets after one fails.
    pub continue_on_error: bool,
    /// Optional override for every target timeout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_override: Option<Duration>,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            targets: ValidationTarget::tier1().to_vec(),
            tier1_only: true,
            continue_on_error: true,
            timeout_override: None,
        }
    }
}

impl BenchmarkConfig {
    /// Resolve the effective target list from config flags.
    #[must_use]
    pub fn resolved_targets(&self) -> Vec<ValidationTarget> {
        if self.tier1_only {
            return ValidationTarget::tier1().to_vec();
        }

        if self.targets.is_empty() {
            return ValidationTarget::tier1().to_vec();
        }

        self.targets.clone()
    }

    /// Parse user-facing target names into enum values.
    #[must_use]
    pub fn parse_target_names(names: &[String]) -> Vec<ValidationTarget> {
        names
            .iter()
            .filter_map(|name| match name.trim().to_ascii_lowercase().as_str() {
                "creepjs" => Some(ValidationTarget::CreepJs),
                "browserscan" => Some(ValidationTarget::BrowserScan),
                "fingerprintjs" | "fingerprint_js" => Some(ValidationTarget::FingerprintJs),
                "kasada" => Some(ValidationTarget::Kasada),
                "cloudflare" => Some(ValidationTarget::Cloudflare),
                "akamai" => Some(ValidationTarget::Akamai),
                "datadome" | "data_dome" => Some(ValidationTarget::DataDome),
                "perimeterx" | "perimeter_x" => Some(ValidationTarget::PerimeterX),
                _ => None,
            })
            .collect()
    }
}

/// One benchmark execution item.
#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkItem {
    /// Target metadata.
    pub target: BenchmarkTarget,
    /// Validator output.
    pub result: ValidationResult,
}

/// Full benchmark report.
#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkReport {
    /// Unix timestamp (seconds) for run start.
    pub started_at_epoch_secs: u64,
    /// Number of passed validations.
    pub passed: usize,
    /// Number of failed validations.
    pub failed: usize,
    /// Result entries in execution order.
    pub results: Vec<BenchmarkItem>,
}

impl BenchmarkReport {
    /// Serialize the report as deterministic pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        let mut root = Map::new();
        root.insert(
            "started_at_epoch_secs".to_string(),
            Value::from(self.started_at_epoch_secs),
        );
        root.insert("passed".to_string(), Value::from(self.passed));
        root.insert("failed".to_string(), Value::from(self.failed));

        let mut results = Vec::with_capacity(self.results.len());
        for item in &self.results {
            let mut item_obj = Map::new();

            let mut target_obj = Map::new();
            target_obj.insert(
                "target".to_string(),
                serde_json::to_value(item.target.target)?,
            );
            target_obj.insert("name".to_string(), Value::from(item.target.name));
            target_obj.insert("url".to_string(), Value::from(item.target.url));
            target_obj.insert(
                "category".to_string(),
                Value::from(item.target.category.to_string()),
            );
            target_obj.insert(
                "timeout_secs".to_string(),
                Value::from(item.target.timeout.as_secs_f64()),
            );

            let mut result_obj = Map::new();
            result_obj.insert(
                "target".to_string(),
                serde_json::to_value(item.result.target)?,
            );
            result_obj.insert("passed".to_string(), Value::from(item.result.passed));
            if let Some(score) = item.result.score {
                result_obj.insert("score".to_string(), Value::from(score));
            } else {
                result_obj.insert("score".to_string(), Value::Null);
            }
            result_obj.insert(
                "elapsed_secs".to_string(),
                Value::from(item.result.elapsed.as_secs_f64()),
            );
            result_obj.insert(
                "screenshot".to_string(),
                if item.result.screenshot.is_some() {
                    Value::from("present")
                } else {
                    Value::Null
                },
            );

            let detail_map: BTreeMap<String, String> = item
                .result
                .details
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            result_obj.insert("details".to_string(), serde_json::to_value(detail_map)?);

            item_obj.insert("target".to_string(), Value::Object(target_obj));
            item_obj.insert("result".to_string(), Value::Object(result_obj));
            results.push(Value::Object(item_obj));
        }

        root.insert("results".to_string(), Value::Array(results));
        serde_json::to_string_pretty(&Value::Object(root))
    }

    /// Generate a markdown summary table and per-target detail section.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Stealth Benchmark Report\n\n");
        let _ = writeln!(
            out,
            "- started_at_epoch_secs: {}",
            self.started_at_epoch_secs
        );
        let _ = writeln!(out, "- passed: {}", self.passed);
        let _ = writeln!(out, "- failed: {}", self.failed);
        out.push('\n');

        out.push_str("| Target | Category | Passed | Score | Elapsed (s) |\n");
        out.push_str("|---|---|---:|---:|---:|\n");
        for item in &self.results {
            let score = item
                .result
                .score
                .map_or_else(|| "-".to_string(), |v| format!("{v:.3}"));
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {:.3} |",
                item.target.name,
                item.target.category,
                if item.result.passed { "yes" } else { "no" },
                score,
                item.result.elapsed.as_secs_f64()
            );
        }

        out.push_str("\n## Details\n\n");
        for item in &self.results {
            let _ = writeln!(out, "### {} ({})", item.target.name, item.target.url);
            out.push('\n');
            let _ = writeln!(out, "- passed: {}", item.result.passed);
            let _ = writeln!(out, "- elapsed_s: {:.3}", item.result.elapsed.as_secs_f64());
            if let Some(score) = item.result.score {
                let _ = writeln!(out, "- score: {score:.3}");
            }
            if !item.result.details.is_empty() {
                out.push_str("- details:\n");
                for (k, v) in sorted_details(&item.result.details) {
                    let _ = writeln!(out, "  - {k}: {v}");
                }
            }
            out.push('\n');
        }

        out
    }
}

/// Benchmark harness entrypoint.
pub struct StealthBenchmark;

impl StealthBenchmark {
    /// Run benchmark according to config.
    pub async fn run(pool: &Arc<BrowserPool>, config: &BenchmarkConfig) -> BenchmarkReport {
        let started_at_epoch_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();

        let targets = config.resolved_targets();
        let mut results = Vec::with_capacity(targets.len());

        for target in targets {
            let benchmark_target = BenchmarkTarget::from_validation_target(target);
            let timeout = config.timeout_override.unwrap_or(benchmark_target.timeout);

            let run = tokio::time::timeout(timeout, ValidationSuite::run_one(pool, target)).await;
            let result = run.unwrap_or_else(|_| {
                ValidationResult::failed(
                    target,
                    &format!("benchmark timeout after {}s", timeout.as_secs()),
                )
            });

            let passed = result.passed;
            results.push(BenchmarkItem {
                target: benchmark_target,
                result,
            });

            if !config.continue_on_error && !passed {
                break;
            }
        }

        let passed = results.iter().filter(|r| r.result.passed).count();
        let failed = results.len().saturating_sub(passed);

        BenchmarkReport {
            started_at_epoch_secs,
            passed,
            failed,
            results,
        }
    }
}

fn sorted_details(details: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut ordered: Vec<(String, String)> = details
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0));
    ordered
}

mod duration_secs {
    use std::time::Duration;

    use serde::Serializer;

    pub(super) fn serialize<S>(d: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(d.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parsing_and_filtering() {
        let names = vec![
            "creepjs".to_string(),
            "cloudflare".to_string(),
            "invalid".to_string(),
        ];
        let parsed = BenchmarkConfig::parse_target_names(&names);
        assert_eq!(
            parsed,
            vec![ValidationTarget::CreepJs, ValidationTarget::Cloudflare]
        );

        let cfg = BenchmarkConfig {
            targets: parsed,
            tier1_only: true,
            continue_on_error: true,
            timeout_override: None,
        };
        assert_eq!(cfg.resolved_targets(), ValidationTarget::tier1().to_vec());
    }

    #[test]
    fn report_json_is_deterministic_for_same_input() {
        let report = BenchmarkReport {
            started_at_epoch_secs: 123,
            passed: 1,
            failed: 1,
            results: vec![
                BenchmarkItem {
                    target: BenchmarkTarget::from_validation_target(ValidationTarget::CreepJs),
                    result: ValidationResult {
                        target: ValidationTarget::CreepJs,
                        passed: true,
                        score: Some(0.95),
                        details: HashMap::from([
                            ("b".to_string(), "2".to_string()),
                            ("a".to_string(), "1".to_string()),
                        ]),
                        screenshot: None,
                        elapsed: Duration::from_secs(1),
                    },
                },
                BenchmarkItem {
                    target: BenchmarkTarget::from_validation_target(ValidationTarget::BrowserScan),
                    result: ValidationResult::failed(ValidationTarget::BrowserScan, "blocked"),
                },
            ],
        };

        let first = report.to_json_pretty();
        let second = report.to_json_pretty();

        assert!(first.is_ok());
        assert!(second.is_ok());

        let first_json = first.unwrap_or_default();
        let second_json = second.unwrap_or_default();
        assert_eq!(first_json, second_json);
    }

    #[cfg(feature = "stealth")]
    #[tokio::test]
    #[ignore = "requires live network and browser runtime"]
    async fn live_target_schema_completeness() {
        use crate::BrowserConfig;

        let pool_result = crate::pool::BrowserPool::new(BrowserConfig::default()).await;
        let Ok(pool) = pool_result else {
            // Environment-dependent ignored test: no panic path for strict clippy.
            return;
        };

        let config = BenchmarkConfig {
            targets: vec![ValidationTarget::CreepJs],
            tier1_only: false,
            continue_on_error: true,
            timeout_override: Some(Duration::from_secs(15)),
        };

        let report = StealthBenchmark::run(&pool, &config).await;
        assert_eq!(report.results.len(), 1);
        let item = report.results.first();
        assert!(item.is_some());
        let Some(item) = item else {
            return;
        };
        assert_eq!(item.target.target, ValidationTarget::CreepJs);
        assert!(!item.target.url.is_empty());
        assert!(!item.target.name.is_empty());
    }
}
