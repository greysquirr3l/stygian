use std::collections::{BTreeMap, BTreeSet};

use crate::classifier::classify_transaction;
use crate::har;
use crate::types::{
    AdapterStrategy, AntiBotProvider, AntiBotRequirement, Detection, HarRequestSummary,
    HostSummary, IntegrationRecommendation, InvestigationDiff, InvestigationReport, MarkerCount,
    RequirementLevel, RequirementsProfile,
};

/// Build an investigation report from a HAR payload.
///
/// # Errors
///
/// Returns [`har::HarError`] when the HAR payload is invalid or malformed.
pub fn investigate_har(har_json: &str) -> Result<InvestigationReport, har::HarError> {
    let parsed = har::parse_har_transactions(har_json)?;

    let mut status_histogram: BTreeMap<u16, u64> = BTreeMap::new();
    let mut resource_type_histogram: BTreeMap<String, u64> = BTreeMap::new();
    let mut provider_histogram: BTreeMap<AntiBotProvider, u64> = BTreeMap::new();
    let mut marker_histogram: BTreeMap<String, u64> = BTreeMap::new();
    let mut host_accumulator: BTreeMap<String, HostSummary> = BTreeMap::new();

    let mut blocked_requests = 0_u64;
    let mut all_requests: Vec<HarRequestSummary> = Vec::new();
    let mut suspicious_requests: Vec<HarRequestSummary> = Vec::new();

    for req in parsed.requests {
        let detection = classify_transaction(&req.transaction);

        let summary = HarRequestSummary {
            url: req.transaction.url.clone(),
            status: req.transaction.status,
            resource_type: req.resource_type.clone(),
            detection,
        };

        let status_entry = status_histogram.entry(summary.status).or_insert(0);
        *status_entry = status_entry.saturating_add(1);

        let resource_label = summary
            .resource_type
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let resource_entry = resource_type_histogram.entry(resource_label).or_insert(0);
        *resource_entry = resource_entry.saturating_add(1);

        let provider_entry = provider_histogram
            .entry(summary.detection.provider)
            .or_insert(0);
        *provider_entry = provider_entry.saturating_add(1);

        for marker in &summary.detection.markers {
            let marker_entry = marker_histogram.entry(marker.clone()).or_insert(0);
            *marker_entry = marker_entry.saturating_add(1);
        }

        let is_blocked = summary.status == 403 || summary.status == 429;
        if is_blocked {
            blocked_requests = blocked_requests.saturating_add(1);
        }

        let host = extract_host(&summary.url);
        let host_summary = host_accumulator.entry(host.clone()).or_insert(HostSummary {
            host,
            total_requests: 0,
            blocked_requests: 0,
        });
        host_summary.total_requests = host_summary.total_requests.saturating_add(1);
        if is_blocked {
            host_summary.blocked_requests = host_summary.blocked_requests.saturating_add(1);
        }

        let is_suspicious = is_blocked || summary.detection.provider != AntiBotProvider::Unknown;
        if is_suspicious {
            suspicious_requests.push(summary.clone());
        }

        all_requests.push(summary);
    }

    let total_requests = u64::try_from(all_requests.len()).unwrap_or(u64::MAX);

    let aggregate = aggregate_detection(&all_requests);

    let mut top_markers = marker_histogram
        .iter()
        .map(|(marker, count)| MarkerCount {
            marker: marker.clone(),
            count: *count,
        })
        .collect::<Vec<_>>();
    top_markers.sort_by_key(|marker| std::cmp::Reverse(marker.count));
    if top_markers.len() > 25 {
        top_markers.truncate(25);
    }

    let mut hosts = host_accumulator.into_values().collect::<Vec<_>>();
    hosts.sort_by_key(|host| std::cmp::Reverse(host.total_requests));

    suspicious_requests.sort_by_key(|req| std::cmp::Reverse(req.status));
    if suspicious_requests.len() > 200 {
        suspicious_requests.truncate(200);
    }

    Ok(InvestigationReport {
        page_title: parsed.page_title,
        total_requests,
        blocked_requests,
        status_histogram,
        resource_type_histogram,
        provider_histogram,
        marker_histogram,
        top_markers,
        hosts,
        suspicious_requests,
        aggregate,
    })
}

/// Compare a baseline and candidate investigation report.
#[must_use]
pub fn compare_reports(
    baseline: &InvestigationReport,
    candidate: &InvestigationReport,
) -> InvestigationDiff {
    let baseline_ratio = blocked_ratio(baseline.blocked_requests, baseline.total_requests);
    let candidate_ratio = blocked_ratio(candidate.blocked_requests, candidate.total_requests);
    let blocked_ratio_delta = candidate_ratio - baseline_ratio;

    let mut provider_delta: BTreeMap<AntiBotProvider, i64> = BTreeMap::new();
    let all_providers =
        collect_provider_keys(&baseline.provider_histogram, &candidate.provider_histogram);
    for provider in all_providers {
        let base = baseline
            .provider_histogram
            .get(&provider)
            .copied()
            .unwrap_or(0);
        let cand = candidate
            .provider_histogram
            .get(&provider)
            .copied()
            .unwrap_or(0);

        let cand_i64 = i64::try_from(cand).unwrap_or(i64::MAX);
        let base_i64 = i64::try_from(base).unwrap_or(i64::MAX);

        let _ = provider_delta.insert(provider, cand_i64.saturating_sub(base_i64));
    }

    let baseline_markers = baseline
        .marker_histogram
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let candidate_markers = candidate
        .marker_histogram
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let new_markers = candidate_markers
        .difference(&baseline_markers)
        .cloned()
        .collect::<Vec<_>>();

    InvestigationDiff {
        baseline_total_requests: baseline.total_requests,
        candidate_total_requests: candidate.total_requests,
        baseline_blocked_requests: baseline.blocked_requests,
        candidate_blocked_requests: candidate.blocked_requests,
        blocked_ratio_delta,
        likely_regression: blocked_ratio_delta >= 0.02,
        provider_delta,
        new_markers,
    }
}

/// Infer operational requirements and adapter strategy from an investigation report.
#[must_use]
pub fn infer_requirements(report: &InvestigationReport) -> RequirementsProfile {
    let mut requirements = Vec::new();

    let blocked_ratio = blocked_ratio(report.blocked_requests, report.total_requests);
    let marker_set = report
        .top_markers
        .iter()
        .map(|marker| marker.marker.to_lowercase())
        .collect::<BTreeSet<_>>();

    let has_cloudflare = marker_set.iter().any(|m| {
        m.contains("cf-ray") || m.contains("__cf_bm") || m.contains("cdn-cgi/challenge-platform")
    });
    let has_datadome = marker_set.iter().any(|m| {
        m.contains("x-datadome")
            || m.contains("x-dd-b")
            || m.contains("datadome=")
            || m.contains("captcha-delivery.com")
    });

    if has_cloudflare {
        requirements.push(AntiBotRequirement {
            id: "js_runtime_and_cookie_lifecycle".to_string(),
            title: "Maintain JS-capable session flow".to_string(),
            why: "Challenge markers indicate server-side scoring that expects browser-like session progression.".to_string(),
            evidence: select_marker_evidence(&marker_set, &["cf-ray", "__cf_bm", "cdn-cgi/challenge-platform"]),
            level: RequirementLevel::High,
        });
    }

    if has_datadome {
        requirements.push(AntiBotRequirement {
            id: "fingerprint_and_identity_consistency".to_string(),
            title: "Keep request identity consistent".to_string(),
            why: "DataDome markers commonly correlate with strict consistency checks across headers, cookies, and connection profile.".to_string(),
            evidence: select_marker_evidence(&marker_set, &["x-datadome", "x-dd-b", "datadome=", "captcha-delivery.com"]),
            level: RequirementLevel::High,
        });
    }

    if blocked_ratio >= 0.10 {
        requirements.push(AntiBotRequirement {
            id: "adaptive_rate_and_retry_budget".to_string(),
            title: "Apply adaptive pacing and bounded retries".to_string(),
            why: "Elevated block ratio suggests aggressive concurrency or retry behavior is increasing risk scoring.".to_string(),
            evidence: vec![format!("blocked_ratio={blocked_ratio:.4}")],
            level: RequirementLevel::High,
        });
    }

    let status_429 = report.status_histogram.get(&429).copied().unwrap_or(0);
    if status_429 > 0 {
        requirements.push(AntiBotRequirement {
            id: "rate_limit_backoff".to_string(),
            title: "Honor explicit rate limits".to_string(),
            why: "Observed HTTP 429 responses indicate throttling pressure.".to_string(),
            evidence: vec![format!("status_429={status_429}")],
            level: RequirementLevel::Medium,
        });
    }

    let preflight_count = report
        .resource_type_histogram
        .get("preflight")
        .copied()
        .unwrap_or(0);
    if preflight_count > 0 {
        requirements.push(AntiBotRequirement {
            id: "cors_and_header_fidelity".to_string(),
            title: "Preserve browser-like CORS/header flow".to_string(),
            why: "Preflight-heavy traffic can fail if adapter behavior diverges from browser request choreography.".to_string(),
            evidence: vec![format!("preflight_requests={preflight_count}")],
            level: RequirementLevel::Medium,
        });
    }

    let recommendation = recommend_strategy(
        report.aggregate.provider,
        blocked_ratio,
        has_cloudflare,
        has_datadome,
        &requirements,
    );

    RequirementsProfile {
        provider: report.aggregate.provider,
        confidence: report.aggregate.confidence,
        requirements,
        recommendation,
    }
}

fn aggregate_detection(requests: &[HarRequestSummary]) -> Detection {
    let mut provider_counts: BTreeMap<AntiBotProvider, u64> = BTreeMap::new();
    let mut markers: Vec<String> = Vec::new();

    for req in requests {
        if req.detection.provider != AntiBotProvider::Unknown {
            let entry = provider_counts.entry(req.detection.provider).or_insert(0);
            *entry = entry.saturating_add(1);
        }
        markers.extend(req.detection.markers.iter().cloned());
    }

    if provider_counts.is_empty() {
        return Detection {
            provider: AntiBotProvider::Unknown,
            confidence: 0.0,
            markers: Vec::new(),
        };
    }

    let mut ordered = provider_counts.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

    if let Some((provider, top_count)) = ordered.first().copied() {
        let second_count = ordered.get(1).map_or(0, |pair| pair.1);
        let confidence = if top_count + second_count == 0 {
            0.0
        } else {
            to_f64(top_count) / to_f64(top_count + second_count)
        };

        Detection {
            provider,
            confidence,
            markers,
        }
    } else {
        Detection {
            provider: AntiBotProvider::Unknown,
            confidence: 0.0,
            markers,
        }
    }
}

fn blocked_ratio(blocked: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        to_f64(blocked) / to_f64(total)
    }
}

#[allow(clippy::cast_precision_loss)]
const fn to_f64(value: u64) -> f64 {
    value as f64
}

fn collect_provider_keys(
    left: &BTreeMap<AntiBotProvider, u64>,
    right: &BTreeMap<AntiBotProvider, u64>,
) -> BTreeSet<AntiBotProvider> {
    left.keys().chain(right.keys()).copied().collect()
}

fn extract_host(url: &str) -> String {
    if let Some((_, rest)) = url.split_once("://") {
        let before_path = rest.split('/').next().unwrap_or(rest);
        let without_auth = before_path.split('@').next_back().unwrap_or(before_path);
        without_auth.to_string()
    } else {
        url.split('/').next().unwrap_or(url).to_string()
    }
}

fn select_marker_evidence(marker_set: &BTreeSet<String>, needles: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for marker in marker_set {
        if needles.iter().any(|needle| marker.contains(needle)) {
            out.push(marker.clone());
        }
    }
    out
}

fn recommend_strategy(
    provider: AntiBotProvider,
    blocked_ratio: f64,
    has_cloudflare: bool,
    has_datadome: bool,
    requirements: &[AntiBotRequirement],
) -> IntegrationRecommendation {
    let mut required_stygian_features = Vec::new();
    let mut config_hints = BTreeMap::new();

    let strategy = if has_datadome {
        required_stygian_features.push("stygian-browser".to_string());
        required_stygian_features.push("stygian-proxy".to_string());
        let _ = config_hints.insert("proxy.rotation".to_string(), "per-domain".to_string());
        let _ = config_hints.insert("session.sticky_ttl_secs".to_string(), "600".to_string());
        let _ = config_hints.insert(
            "webrtc.policy".to_string(),
            "disable_non_proxied_udp".to_string(),
        );
        AdapterStrategy::StickyProxy
    } else if has_cloudflare || blocked_ratio >= 0.05 {
        required_stygian_features.push("stygian-browser".to_string());
        let _ = config_hints.insert("request.rate_limit.rps".to_string(), "1-3".to_string());
        let _ = config_hints.insert(
            "retry.backoff".to_string(),
            "exponential+jitter".to_string(),
        );
        AdapterStrategy::BrowserStealth
    } else if provider == AntiBotProvider::Unknown && requirements.is_empty() {
        required_stygian_features.push("stygian-graph".to_string());
        AdapterStrategy::DirectHttp
    } else {
        required_stygian_features.push("stygian-graph".to_string());
        required_stygian_features.push("stygian-charon".to_string());
        AdapterStrategy::InvestigateOnly
    };

    let rationale = match strategy {
        AdapterStrategy::StickyProxy => {
            "Provider markers suggest identity/session continuity and proxy stickiness are primary requirements."
                .to_string()
        }
        AdapterStrategy::BrowserStealth => {
            "Challenge density indicates browser-backed execution with conservative pacing is required."
                .to_string()
        }
        AdapterStrategy::DirectHttp => {
            "No strong anti-bot markers were detected; direct HTTP path appears sufficient."
                .to_string()
        }
        AdapterStrategy::SessionWarmup => {
            "Session priming is recommended before collection workloads."
                .to_string()
        }
        AdapterStrategy::InvestigateOnly => {
            "Signals are mixed; keep adaptive telemetry enabled and gather additional baseline runs."
                .to_string()
        }
    };

    IntegrationRecommendation {
        strategy,
        rationale,
        required_stygian_features,
        config_hints,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_reports_flags_block_ratio_regression() {
        let baseline = InvestigationReport {
            page_title: None,
            total_requests: 100,
            blocked_requests: 5,
            status_histogram: BTreeMap::new(),
            resource_type_histogram: BTreeMap::new(),
            provider_histogram: BTreeMap::new(),
            marker_histogram: BTreeMap::new(),
            top_markers: Vec::new(),
            hosts: Vec::new(),
            suspicious_requests: Vec::new(),
            aggregate: Detection {
                provider: AntiBotProvider::Unknown,
                confidence: 0.0,
                markers: Vec::new(),
            },
        };

        let candidate = InvestigationReport {
            blocked_requests: 12,
            ..baseline.clone()
        };

        let diff = compare_reports(&baseline, &candidate);
        assert!(diff.blocked_ratio_delta > 0.02);
        assert!(diff.likely_regression);
    }

    #[test]
    fn infer_requirements_identifies_cloudflare_signals() {
        let mut status_histogram = BTreeMap::new();
        let _ = status_histogram.insert(403, 7);

        let mut resource_histogram = BTreeMap::new();
        let _ = resource_histogram.insert("document".to_string(), 10);

        let report = InvestigationReport {
            page_title: Some("https://example.com".to_string()),
            total_requests: 10,
            blocked_requests: 7,
            status_histogram,
            resource_type_histogram: resource_histogram,
            provider_histogram: BTreeMap::new(),
            marker_histogram: BTreeMap::from([
                ("cf-ray".to_string(), 5),
                ("__cf_bm".to_string(), 5),
            ]),
            top_markers: vec![
                MarkerCount {
                    marker: "cf-ray".to_string(),
                    count: 5,
                },
                MarkerCount {
                    marker: "__cf_bm".to_string(),
                    count: 5,
                },
            ],
            hosts: Vec::new(),
            suspicious_requests: Vec::new(),
            aggregate: Detection {
                provider: AntiBotProvider::Cloudflare,
                confidence: 0.9,
                markers: vec!["cf-ray".to_string()],
            },
        };

        let profile = infer_requirements(&report);
        assert_eq!(profile.provider, AntiBotProvider::Cloudflare);
        assert!(!profile.requirements.is_empty());
        assert_eq!(
            profile.recommendation.strategy,
            AdapterStrategy::BrowserStealth
        );
    }
}
