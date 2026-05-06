use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::classifier::classify_transaction;
use crate::types::{AntiBotProvider, Detection, TransactionView};

/// Classification of how a probe exercises the detection system.
///
/// Used to group expected behaviour in regression runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ProbeCategory {
    /// Clean, everyday traffic with no bot-protection signals.
    Benign,
    /// Partial or ambiguous signals that may or may not trigger detection.
    Suspicious,
    /// Full adversarial signals; a well-tuned analyzer must return the expected provider.
    Adversarial,
    /// Edge cases that exercise boundary conditions (empty headers, unusual status codes, etc.).
    EdgeCase,
}

/// Expected detection outcome for a probe.
///
/// A probe passes when the classified provider matches `expected_provider` and
/// the confidence is at least `min_confidence`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeExpectation {
    /// Provider the probe expects to detect.
    pub expected_provider: AntiBotProvider,
    /// Minimum confidence threshold in `[0.0, 1.0]`.  `0.0` accepts any confidence.
    pub min_confidence: f64,
}

/// A single challenge-style probe with its input, expected outcome, and metadata.
///
/// # Example
///
/// ```rust
/// use stygian_charon::probe::{ChallengeProbe, ProbeCategory, ProbeExpectation};
/// use stygian_charon::types::{AntiBotProvider, TransactionView};
/// use std::collections::BTreeMap;
///
/// let probe = ChallengeProbe {
///     name: "cf-ray-header".to_string(),
///     description: "Cloudflare CF-Ray header present".to_string(),
///     category: ProbeCategory::Adversarial,
///     transaction: TransactionView {
///         url: "https://example.com/".to_string(),
///         status: 403,
///         response_headers: {
///             let mut h = BTreeMap::new();
///             h.insert("cf-ray".to_string(), "abc123-LHR".to_string());
///             h
///         },
///         response_body_snippet: None,
///     },
///     expectation: ProbeExpectation {
///         expected_provider: AntiBotProvider::Cloudflare,
///         min_confidence: 0.5,
///     },
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChallengeProbe {
    /// Short identifier used in reports.
    pub name: String,
    /// Human-readable description of what this probe covers.
    pub description: String,
    /// Probe category.
    pub category: ProbeCategory,
    /// Synthetic transaction to classify.
    pub transaction: TransactionView,
    /// Expected outcome.
    pub expectation: ProbeExpectation,
}

/// Outcome of running a single probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeRunResult {
    /// Probe name.
    pub name: String,
    /// Probe category.
    pub category: ProbeCategory,
    /// Actual detection produced by the classifier.
    pub actual: Detection,
    /// Expected outcome.
    pub expectation: ProbeExpectation,
    /// Whether the probe passed.
    pub passed: bool,
    /// Failure reason when `!passed`.
    pub failure_reason: Option<String>,
}

/// Summary report produced by running a full probe pack.
///
/// # Example
///
/// ```rust
/// use stygian_charon::probe::{run_probe_pack, challenge_probe_pack};
///
/// let report = run_probe_pack(&challenge_probe_pack());
/// assert_eq!(report.total, report.passed + report.failed);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbePackReport {
    /// Total probes run.
    pub total: usize,
    /// Probes that passed.
    pub passed: usize,
    /// Probes that failed.
    pub failed: usize,
    /// Individual results (sorted: failures first, then by category, then by name).
    pub results: Vec<ProbeRunResult>,
    /// Whether the full pack passed with no failures.
    pub all_passed: bool,
}

impl ProbePackReport {
    /// Returns only the failed results.
    #[must_use]
    pub fn failures(&self) -> Vec<&ProbeRunResult> {
        self.results.iter().filter(|r| !r.passed).collect()
    }
}

/// Run a probe pack against the default `V1` classifier and return a report.
///
/// # Example
///
/// ```rust
/// use stygian_charon::probe::{run_probe_pack, challenge_probe_pack};
///
/// let report = run_probe_pack(&challenge_probe_pack());
/// assert!(report.all_passed, "probe pack regressions: {:?}", report.failures());
/// ```
#[must_use]
pub fn run_probe_pack(probes: &[ChallengeProbe]) -> ProbePackReport {
    let mut results: Vec<ProbeRunResult> = probes.iter().map(run_one_probe).collect();

    results.sort_by(|a, b| {
        b.passed
            .cmp(&a.passed)
            .then(a.category.cmp(&b.category))
            .then(a.name.cmp(&b.name))
    });

    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.len() - passed;
    let total = results.len();

    ProbePackReport {
        total,
        passed,
        failed,
        all_passed: failed == 0,
        results,
    }
}

fn run_one_probe(probe: &ChallengeProbe) -> ProbeRunResult {
    let actual = classify_transaction(&probe.transaction);

    let passed = actual.provider == probe.expectation.expected_provider
        && actual.confidence >= probe.expectation.min_confidence;

    let failure_reason = if passed {
        None
    } else {
        let mut reasons = Vec::new();
        if actual.provider != probe.expectation.expected_provider {
            reasons.push(format!(
                "provider: expected {:?}, got {:?}",
                probe.expectation.expected_provider, actual.provider
            ));
        }
        if actual.confidence < probe.expectation.min_confidence {
            reasons.push(format!(
                "confidence: expected >= {:.2}, got {:.2}",
                probe.expectation.min_confidence, actual.confidence
            ));
        }
        Some(reasons.join("; "))
    };

    ProbeRunResult {
        name: probe.name.clone(),
        category: probe.category,
        actual,
        expectation: probe.expectation.clone(),
        passed,
        failure_reason,
    }
}

/// Build the canonical challenge probe pack.
///
/// Returns the built-in set of benign, suspicious, adversarial, and edge-case probes.
///
/// # Example
///
/// ```rust
/// use stygian_charon::probe::challenge_probe_pack;
///
/// let probes = challenge_probe_pack();
/// assert!(!probes.is_empty());
/// ```
#[must_use]
pub fn challenge_probe_pack() -> Vec<ChallengeProbe> {
    let mut probes = Vec::new();
    probes.extend(build_benign_probes());
    probes.extend(build_suspicious_probes());
    probes.extend(build_adversarial_probes());
    probes.extend(build_edge_case_probes());
    probes
}

fn build_benign_probes() -> Vec<ChallengeProbe> {
    vec![
        ChallengeProbe {
            name: "benign-200-ok".to_string(),
            description: "Plain 200 OK response with no anti-bot headers".to_string(),
            category: ProbeCategory::Benign,
            transaction: TransactionView {
                url: "https://example.com/page".to_string(),
                status: 200,
                response_headers: BTreeMap::new(),
                response_body_snippet: None,
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::Unknown,
                min_confidence: 0.0,
            },
        },
        ChallengeProbe {
            name: "benign-cdn-headers".to_string(),
            description: "Standard CDN headers that share no anti-bot signals".to_string(),
            category: ProbeCategory::Benign,
            transaction: TransactionView {
                url: "https://example.com/api/v1/data".to_string(),
                status: 200,
                response_headers: {
                    let mut h = BTreeMap::new();
                    h.insert("content-type".to_string(), "application/json".to_string());
                    h.insert(
                        "cache-control".to_string(),
                        "public, max-age=3600".to_string(),
                    );
                    h.insert("x-cache".to_string(), "HIT".to_string());
                    h
                },
                response_body_snippet: None,
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::Unknown,
                min_confidence: 0.0,
            },
        },
    ]
}

fn build_suspicious_probes() -> Vec<ChallengeProbe> {
    vec![
        ChallengeProbe {
            name: "suspicious-akamai-partial".to_string(),
            description: "Single low-weight Akamai marker; should detect Akamai".to_string(),
            category: ProbeCategory::Suspicious,
            transaction: TransactionView {
                url: "https://example.com/product".to_string(),
                status: 200,
                response_headers: {
                    let mut h = BTreeMap::new();
                    h.insert("server".to_string(), "AkamaiGHost".to_string());
                    h
                },
                response_body_snippet: Some("akamai".to_string()),
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::Akamai,
                min_confidence: 0.0,
            },
        },
        ChallengeProbe {
            name: "suspicious-fingerprint-partial".to_string(),
            description: "One FingerprintJS URL reference in body".to_string(),
            category: ProbeCategory::Suspicious,
            transaction: TransactionView {
                url: "https://example.com/checkout".to_string(),
                status: 200,
                response_headers: BTreeMap::new(),
                response_body_snippet: Some("fingerprint.com/v3/agent".to_string()),
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::FingerprintCom,
                min_confidence: 0.0,
            },
        },
    ]
}

fn build_adversarial_probes() -> Vec<ChallengeProbe> {
    let mut probes = Vec::new();
    probes.extend(build_adversarial_probes_part_one());
    probes.extend(build_adversarial_probes_part_two());
    probes
}

fn build_adversarial_probes_part_one() -> Vec<ChallengeProbe> {
    vec![
        ChallengeProbe {
            name: "adversarial-datadome-full".to_string(),
            description: "Full DataDome challenge: x-datadome + cookie + captcha URL".to_string(),
            category: ProbeCategory::Adversarial,
            transaction: TransactionView {
                url: "https://target.com/page".to_string(),
                status: 403,
                response_headers: {
                    let mut h = BTreeMap::new();
                    h.insert("x-datadome".to_string(), "1".to_string());
                    h.insert("x-datadome-cid".to_string(), "abc123".to_string());
                    h.insert(
                        "set-cookie".to_string(),
                        "datadome=xyz; Domain=.target.com".to_string(),
                    );
                    h
                },
                response_body_snippet: Some(
                    "Redirecting to captcha-delivery.com/captcha".to_string(),
                ),
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::DataDome,
                min_confidence: 0.5,
            },
        },
        ChallengeProbe {
            name: "adversarial-cloudflare-challenge".to_string(),
            description: "Cloudflare challenge page: CF-Ray + __cf_bm cookie + server header"
                .to_string(),
            category: ProbeCategory::Adversarial,
            transaction: TransactionView {
                url: "https://target.com/".to_string(),
                status: 403,
                response_headers: {
                    let mut h = BTreeMap::new();
                    h.insert("cf-ray".to_string(), "7a1b2c3d4e5f-LHR".to_string());
                    h.insert("server".to_string(), "cloudflare".to_string());
                    h.insert(
                        "set-cookie".to_string(),
                        "__cf_bm=token; SameSite=None".to_string(),
                    );
                    h
                },
                response_body_snippet: Some("Attention Required! | Cloudflare".to_string()),
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::Cloudflare,
                min_confidence: 0.5,
            },
        },
        ChallengeProbe {
            name: "adversarial-akamai-bot-manager".to_string(),
            description: "Akamai Bot Manager: _abck + bm_sv cookies".to_string(),
            category: ProbeCategory::Adversarial,
            transaction: TransactionView {
                url: "https://target.com/cart".to_string(),
                status: 200,
                response_headers: {
                    let mut h = BTreeMap::new();
                    h.insert(
                        "set-cookie".to_string(),
                        "_abck=sensor_data; bm_sv=session_token".to_string(),
                    );
                    h
                },
                response_body_snippet: None,
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::Akamai,
                min_confidence: 0.5,
            },
        },
    ]
}

fn build_adversarial_probes_part_two() -> Vec<ChallengeProbe> {
    vec![
        ChallengeProbe {
            name: "adversarial-perimeterx-block".to_string(),
            description: "PerimeterX / Human Security block page".to_string(),
            category: ProbeCategory::Adversarial,
            transaction: TransactionView {
                url: "https://target.com/search".to_string(),
                status: 403,
                response_headers: {
                    let mut h = BTreeMap::new();
                    h.insert("set-cookie".to_string(), "_px3=payload; Path=/".to_string());
                    h
                },
                response_body_snippet: Some("perimeterx access denied".to_string()),
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::PerimeterX,
                min_confidence: 0.5,
            },
        },
        ChallengeProbe {
            name: "adversarial-kasada-block".to_string(),
            description: "Kasada block with x-kpsdk header".to_string(),
            category: ProbeCategory::Adversarial,
            transaction: TransactionView {
                url: "https://target.com/api/checkout".to_string(),
                status: 429,
                response_headers: {
                    let mut h = BTreeMap::new();
                    h.insert("x-kpsdk-ct".to_string(), "kasada-token".to_string());
                    h.insert("x-kpsdk-cd".to_string(), "challenge-data".to_string());
                    h
                },
                response_body_snippet: Some("kasada protection active".to_string()),
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::Kasada,
                min_confidence: 0.5,
            },
        },
        ChallengeProbe {
            name: "adversarial-fingerprintcom-full".to_string(),
            description: "FingerprintJS Pro with x-fpjs header and body reference".to_string(),
            category: ProbeCategory::Adversarial,
            transaction: TransactionView {
                url: "https://target.com/auth".to_string(),
                status: 200,
                response_headers: {
                    let mut h = BTreeMap::new();
                    h.insert("x-fpjs-region".to_string(), "us-east-1".to_string());
                    h
                },
                response_body_snippet: Some(
                    "https://api.fingerprint.com/v3/agent?apiKey=xyz".to_string(),
                ),
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::FingerprintCom,
                min_confidence: 0.5,
            },
        },
    ]
}

fn build_edge_case_probes() -> Vec<ChallengeProbe> {
    vec![
        ChallengeProbe {
            name: "edge-empty-headers".to_string(),
            description: "Transaction with no headers and no body; must not panic".to_string(),
            category: ProbeCategory::EdgeCase,
            transaction: TransactionView {
                url: "https://example.com/".to_string(),
                status: 200,
                response_headers: BTreeMap::new(),
                response_body_snippet: None,
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::Unknown,
                min_confidence: 0.0,
            },
        },
        ChallengeProbe {
            name: "edge-status-0".to_string(),
            description: "Status code 0 (network error / timeout); must not panic".to_string(),
            category: ProbeCategory::EdgeCase,
            transaction: TransactionView {
                url: "https://example.com/".to_string(),
                status: 0,
                response_headers: BTreeMap::new(),
                response_body_snippet: None,
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::Unknown,
                min_confidence: 0.0,
            },
        },
        ChallengeProbe {
            name: "edge-mixed-case-header".to_string(),
            description: "CF-Ray header with mixed case; classifier should normalise".to_string(),
            category: ProbeCategory::EdgeCase,
            transaction: TransactionView {
                url: "https://target.com/".to_string(),
                status: 200,
                response_headers: {
                    let mut h = BTreeMap::new();
                    h.insert("CF-Ray".to_string(), "1234567890ab-SYD".to_string());
                    h
                },
                response_body_snippet: None,
            },
            expectation: ProbeExpectation {
                expected_provider: AntiBotProvider::Cloudflare,
                min_confidence: 0.5,
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_probe_pack_all_pass() {
        let probes = challenge_probe_pack();
        let report = run_probe_pack(&probes);
        assert!(
            report.all_passed,
            "probe pack regressions detected:\n{:#?}",
            report.failures()
        );
    }

    #[test]
    fn probe_pack_report_counts_are_consistent() {
        let probes = challenge_probe_pack();
        let report = run_probe_pack(&probes);
        assert_eq!(report.total, probes.len());
        assert_eq!(report.passed + report.failed, report.total);
    }

    #[test]
    fn probe_pack_has_all_categories() {
        let probes = challenge_probe_pack();
        let categories: std::collections::BTreeSet<_> = probes.iter().map(|p| p.category).collect();
        assert!(categories.contains(&ProbeCategory::Benign));
        assert!(categories.contains(&ProbeCategory::Suspicious));
        assert!(categories.contains(&ProbeCategory::Adversarial));
        assert!(categories.contains(&ProbeCategory::EdgeCase));
    }
}
