use serde::{Deserialize, Serialize};

use crate::har;
use crate::types::{Detection, HarClassificationReport, HarRequestSummary, TransactionView};

/// Version identifier for Charon provider analyzers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum AnalyzerVersion {
    /// Current signature analyzer.
    #[default]
    V1,
    /// Legacy signature analyzer retained for compatibility.
    V1Legacy,
}

impl AnalyzerVersion {
    /// Stable version id used in configs and reports.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::V1 => "v1",
            Self::V1Legacy => "v1-legacy",
        }
    }

    /// Parse a version id.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "v1" => Some(Self::V1),
            "v1-legacy" => Some(Self::V1Legacy),
            _ => None,
        }
    }

    /// Return `true` when this version is deprecated.
    #[must_use]
    pub const fn is_deprecated(self) -> bool {
        matches!(self, Self::V1Legacy)
    }

    /// Recommended migration target for deprecated versions.
    #[must_use]
    pub const fn migration_target(self) -> Option<Self> {
        match self {
            Self::V1Legacy => Some(Self::V1),
            Self::V1 => None,
        }
    }
}

/// Runtime profile selecting analyzer behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzerProfile {
    /// Profile identifier for logging and policy wiring.
    pub profile_id: String,
    /// Analyzer version used for classification.
    pub analyzer_version: AnalyzerVersion,
}

impl Default for AnalyzerProfile {
    fn default() -> Self {
        Self {
            profile_id: "default".to_string(),
            analyzer_version: AnalyzerVersion::V1,
        }
    }
}

/// Interface for versioned provider analyzers.
pub trait ProviderAnalyzer {
    /// Analyzer version identifier.
    fn version(&self) -> AnalyzerVersion;

    /// Classify one transaction into provider evidence.
    fn classify_transaction(&self, tx: &TransactionView) -> Detection;

    /// Classify all transactions in a HAR payload.
    ///
    /// # Errors
    ///
    /// Returns an error when HAR parsing fails.
    fn classify_har(&self, har_json: &str) -> Result<HarClassificationReport, har::HarError> {
        let parsed = har::parse_har_transactions(har_json)?;

        let requests = parsed
            .requests
            .into_iter()
            .map(|req| HarRequestSummary {
                url: req.transaction.url.clone(),
                status: req.transaction.status,
                resource_type: req.resource_type,
                detection: self.classify_transaction(&req.transaction),
            })
            .collect::<Vec<_>>();

        Ok(HarClassificationReport {
            page_title: parsed.page_title,
            aggregate: aggregate_detection(&requests),
            requests,
        })
    }
}

fn aggregate_detection(requests: &[HarRequestSummary]) -> Detection {
    let mut provider_counts: std::collections::BTreeMap<crate::types::AntiBotProvider, u32> =
        std::collections::BTreeMap::new();
    let mut markers: Vec<String> = Vec::new();

    for req in requests {
        if req.detection.provider != crate::types::AntiBotProvider::Unknown {
            let entry = provider_counts.entry(req.detection.provider).or_insert(0);
            *entry = entry.saturating_add(1);
        }
        markers.extend(req.detection.markers.iter().cloned());
    }

    if provider_counts.is_empty() {
        return Detection {
            provider: crate::types::AntiBotProvider::Unknown,
            confidence: 0.0,
            markers: Vec::new(),
        };
    }

    let mut ordered: Vec<(crate::types::AntiBotProvider, u32)> =
        provider_counts.into_iter().collect();
    ordered.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

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
            provider: crate::types::AntiBotProvider::Unknown,
            confidence: 0.0,
            markers: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyzer_version_migration_path_is_defined_for_legacy() {
        assert!(AnalyzerVersion::V1Legacy.is_deprecated());
        assert_eq!(
            AnalyzerVersion::V1Legacy.migration_target(),
            Some(AnalyzerVersion::V1)
        );
        assert_eq!(AnalyzerVersion::V1.migration_target(), None);
    }

    #[test]
    fn analyzer_version_parse_roundtrip() {
        let parsed = AnalyzerVersion::parse("v1");
        assert_eq!(parsed, Some(AnalyzerVersion::V1));
        assert_eq!(AnalyzerVersion::V1.id(), "v1");
    }
}
