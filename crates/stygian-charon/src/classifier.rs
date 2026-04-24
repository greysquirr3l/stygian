use std::cmp::Reverse;
use std::collections::BTreeMap;

use crate::har;
use crate::types::{
    AntiBotProvider, Detection, HarClassificationReport, HarRequestSummary, ProviderScore,
    TransactionView,
};

#[derive(Debug, Clone, Copy)]
struct Signature {
    needle: &'static str,
    provider: AntiBotProvider,
    weight: u32,
}

const SIGNATURES: &[Signature] = &[
    Signature {
        needle: "x-datadome",
        provider: AntiBotProvider::DataDome,
        weight: 5,
    },
    Signature {
        needle: "x-datadome-cid",
        provider: AntiBotProvider::DataDome,
        weight: 5,
    },
    Signature {
        needle: "x-dd-b",
        provider: AntiBotProvider::DataDome,
        weight: 4,
    },
    Signature {
        needle: "datadome=",
        provider: AntiBotProvider::DataDome,
        weight: 4,
    },
    Signature {
        needle: "captcha-delivery.com",
        provider: AntiBotProvider::DataDome,
        weight: 4,
    },
    Signature {
        needle: "server:cloudflare",
        provider: AntiBotProvider::Cloudflare,
        weight: 3,
    },
    Signature {
        needle: "cf-ray",
        provider: AntiBotProvider::Cloudflare,
        weight: 5,
    },
    Signature {
        needle: "__cf_bm",
        provider: AntiBotProvider::Cloudflare,
        weight: 4,
    },
    Signature {
        needle: "cdn-cgi/challenge-platform",
        provider: AntiBotProvider::Cloudflare,
        weight: 4,
    },
    Signature {
        needle: "attention required! | cloudflare",
        provider: AntiBotProvider::Cloudflare,
        weight: 4,
    },
    Signature {
        needle: "_abck",
        provider: AntiBotProvider::Akamai,
        weight: 5,
    },
    Signature {
        needle: "bm_sv",
        provider: AntiBotProvider::Akamai,
        weight: 5,
    },
    Signature {
        needle: "akamai",
        provider: AntiBotProvider::Akamai,
        weight: 2,
    },
    Signature {
        needle: "_px",
        provider: AntiBotProvider::PerimeterX,
        weight: 5,
    },
    Signature {
        needle: "perimeterx",
        provider: AntiBotProvider::PerimeterX,
        weight: 4,
    },
    Signature {
        needle: "humansecurity",
        provider: AntiBotProvider::PerimeterX,
        weight: 3,
    },
    Signature {
        needle: "x-kpsdk",
        provider: AntiBotProvider::Kasada,
        weight: 5,
    },
    Signature {
        needle: "kasada",
        provider: AntiBotProvider::Kasada,
        weight: 4,
    },
    Signature {
        needle: "x-fpjs",
        provider: AntiBotProvider::FingerprintCom,
        weight: 4,
    },
    Signature {
        needle: "fingerprint.com",
        provider: AntiBotProvider::FingerprintCom,
        weight: 3,
    },
];

/// Classify a transaction view into a likely anti-bot provider.
#[must_use]
pub fn classify_transaction(tx: &TransactionView) -> Detection {
    let mut scores: BTreeMap<AntiBotProvider, ProviderScore> = BTreeMap::new();

    for provider in [
        AntiBotProvider::DataDome,
        AntiBotProvider::Cloudflare,
        AntiBotProvider::Akamai,
        AntiBotProvider::PerimeterX,
        AntiBotProvider::Kasada,
        AntiBotProvider::FingerprintCom,
    ] {
        let _prev = scores.insert(
            provider,
            ProviderScore {
                provider,
                score: 0,
                markers: Vec::new(),
            },
        );
    }

    let normalized_headers = normalize_headers(&tx.response_headers);
    let body = tx
        .response_body_snippet
        .as_ref()
        .map_or_else(String::new, |s| s.to_lowercase());

    let mut haystacks = String::new();
    haystacks.push_str(&tx.url.to_lowercase());
    haystacks.push('\n');
    haystacks.push_str(&normalized_headers);
    haystacks.push('\n');
    haystacks.push_str(&body);

    for sig in SIGNATURES {
        if haystacks.contains(sig.needle)
            && let Some(score) = scores.get_mut(&sig.provider)
        {
            score.score = score.score.saturating_add(sig.weight);
            score.markers.push(sig.needle.to_string());
        }
    }

    // 403/429 can increase confidence but does not imply a specific vendor.
    if tx.status == 403 || tx.status == 429 {
        for provider in [AntiBotProvider::DataDome, AntiBotProvider::Cloudflare] {
            if let Some(score) = scores.get_mut(&provider)
                && score.score > 0
            {
                score.score = score.score.saturating_add(1);
                score.markers.push(format!("status:{}", tx.status));
            }
        }
    }

    let mut ordered: Vec<ProviderScore> = scores.into_values().collect();
    ordered.sort_by_key(|score| Reverse(score.score));

    let top = ordered.first();
    let second = ordered.get(1);

    match (top, second) {
        (Some(primary), Some(secondary)) if primary.score > 0 => {
            let denom = primary.score + secondary.score;
            let confidence = if denom == 0 {
                0.0
            } else {
                f64::from(primary.score) / f64::from(denom)
            };
            Detection {
                provider: primary.provider,
                confidence,
                markers: primary.markers.clone(),
            }
        }
        (Some(primary), _) if primary.score > 0 => Detection {
            provider: primary.provider,
            confidence: 1.0,
            markers: primary.markers.clone(),
        },
        _ => Detection {
            provider: AntiBotProvider::Unknown,
            confidence: 0.0,
            markers: Vec::new(),
        },
    }
}

/// Classify all entries in a HAR JSON payload.
///
/// # Errors
///
/// Returns [`har::HarError`] when the input is not valid HAR JSON
/// or is missing required HAR fields.
pub fn classify_har(har_json: &str) -> Result<HarClassificationReport, har::HarError> {
    let parsed = har::parse_har_transactions(har_json)?;

    let requests = parsed
        .requests
        .into_iter()
        .map(|req| HarRequestSummary {
            url: req.transaction.url.clone(),
            status: req.transaction.status,
            resource_type: req.resource_type,
            detection: classify_transaction(&req.transaction),
        })
        .collect::<Vec<_>>();

    let aggregate = aggregate_detection(&requests);

    Ok(HarClassificationReport {
        page_title: parsed.page_title,
        aggregate,
        requests,
    })
}

fn aggregate_detection(requests: &[HarRequestSummary]) -> Detection {
    let mut provider_counts: BTreeMap<AntiBotProvider, u32> = BTreeMap::new();
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

    let mut ordered: Vec<(AntiBotProvider, u32)> = provider_counts.into_iter().collect();
    ordered.sort_by_key(|(_, count)| Reverse(*count));

    if let Some((provider, top_count)) = ordered.first().copied() {
        let second_count = ordered.get(1).map_or(0, |x| x.1);
        let confidence = if top_count + second_count == 0 {
            0.0
        } else {
            f64::from(top_count) / f64::from(top_count + second_count)
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

fn normalize_headers(headers: &BTreeMap<String, String>) -> String {
    let mut normalized = String::new();
    for (key, value) in headers {
        normalized.push_str(&key.to_lowercase());
        normalized.push(':');
        normalized.push_str(&value.to_lowercase());
        normalized.push('\n');
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn classifies_datadome_from_headers() {
        let mut headers = BTreeMap::new();
        let _ = headers.insert("x-datadome".to_string(), "protected".to_string());
        let _ = headers.insert("x-datadome-cid".to_string(), "abc".to_string());
        let _ = headers.insert("set-cookie".to_string(), "datadome=xyz; Path=/".to_string());

        let tx = TransactionView {
            url: "https://www.g2.com/".to_string(),
            status: 403,
            response_headers: headers,
            response_body_snippet: Some("Please enable JS".to_string()),
        };

        let detection = classify_transaction(&tx);

        assert_eq!(detection.provider, AntiBotProvider::DataDome);
        assert!(detection.confidence > 0.5);
    }

    #[test]
    fn classifies_cloudflare_from_body_and_headers() {
        let mut headers = BTreeMap::new();
        let _ = headers.insert("server".to_string(), "cloudflare".to_string());
        let _ = headers.insert("cf-ray".to_string(), "123-ORD".to_string());

        let tx = TransactionView {
            url: "https://www.capterra.com/".to_string(),
            status: 403,
            response_headers: headers,
            response_body_snippet: Some("Attention Required! | Cloudflare".to_string()),
        };

        let detection = classify_transaction(&tx);

        assert_eq!(detection.provider, AntiBotProvider::Cloudflare);
        assert!(detection.confidence > 0.5);
    }
}
