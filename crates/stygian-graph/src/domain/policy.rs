//! Single robots policy value type — closes guide failure mode #9.
//!
//! Recon and production paths must agree on the policy applied to every
//! URL they touch. The guide calls this out as the ninth failure mode of
//! agent-built scrapers: _"Two robots.txt policies, never reconciled.
//! Exploration ignores it; the deliverable obeys it."_
//!
//! This module exposes a single [`RobotsPolicy`] enum plus its decision
//! type [`RobotsDecision`]. Both recon and production consume the same
//! value through [`RobotsPolicyGuard`] (in `ports/robots_policy.rs`), so
//! the policy choice can be asserted at pipeline-build time and again at
//! execute time.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::error::StygianError;

/// How a pipeline treats `robots.txt` across recon and production.
///
/// The default is [`RobotsPolicy::Obey`] — honouring robots.txt is the
/// only choice that's safe to ship without a written authorisation.
///
/// # Example
///
/// ```
/// use stygian_graph::domain::policy::RobotsPolicy;
/// use std::str::FromStr;
///
/// let p: RobotsPolicy = "obey".parse().unwrap();
/// assert_eq!(p, RobotsPolicy::Obey);
/// assert_eq!(p.to_string(), "obey");
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RobotsPolicy {
    /// Honour `robots.txt`. Refuse to fetch URLs the guard forbids.
    ///
    /// The canonical choice — anything else requires explicit operator
    /// consent and audit.
    #[default]
    Obey,

    /// Ignore `robots.txt` but record every ignored URL with the
    /// reason. Surfaced through the pipeline run report.
    IgnoreWithAudit,

    /// Ignore `robots.txt` without record. Escape hatch only.
    IgnoreSilently,
}

impl fmt::Display for RobotsPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Obey => "obey",
            Self::IgnoreWithAudit => "ignore_with_audit",
            Self::IgnoreSilently => "ignore_silently",
        };
        f.write_str(s)
    }
}

impl FromStr for RobotsPolicy {
    type Err = StygianError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "obey" | "Obey" | "OBEY" => Ok(Self::Obey),
            "ignore_with_audit" | "IgnoreWithAudit" => Ok(Self::IgnoreWithAudit),
            "ignore_silently" | "IgnoreSilently" => Ok(Self::IgnoreSilently),
            other => Err(StygianError::Config(
                super::error::ConfigError::InvalidValue {
                    key: "robots_policy".to_string(),
                    reason: format!(
                        "unknown variant '{other}' (expected one of: \
                         obey, ignore_with_audit, ignore_silently)"
                    ),
                },
            )),
        }
    }
}

/// The verdict returned by a [`RobotsPolicyGuard`](crate::ports::robots_policy::RobotsPolicyGuard)
/// for a single URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RobotsDecision {
    /// The guard has no opinion — caller should treat this as "no signal".
    ///
    /// A real guard returns this for URLs outside any rule it tracks
    /// (e.g. a non-HTTP scheme, or a host the guard has no data for).
    Unknown,

    /// The URL is permitted under the current policy.
    Allow {
        /// Human-readable reason — usually the matched rule, or
        /// "no rule matched" when the guard has no robots.txt data.
        reason: String,
    },

    /// The URL is forbidden under the current policy.
    Forbid {
        /// Human-readable reason — usually the matched `Disallow` rule.
        reason: String,
    },
}

impl RobotsDecision {
    /// `true` when the guard is explicitly permitting the URL.
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow { .. })
    }

    /// `true` when the guard is explicitly forbidding the URL.
    #[must_use]
    pub const fn is_forbidden(&self) -> bool {
        matches!(self, Self::Forbid { .. })
    }

    /// `true` when the guard has no opinion either way.
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn default_is_obey() {
        assert_eq!(RobotsPolicy::default(), RobotsPolicy::Obey);
    }

    #[test]
    fn round_trip_display_fromstr() {
        for policy in [
            RobotsPolicy::Obey,
            RobotsPolicy::IgnoreWithAudit,
            RobotsPolicy::IgnoreSilently,
        ] {
            assert_eq!(RobotsPolicy::from_str(&policy.to_string()).unwrap(), policy);
        }
    }

    #[test]
    fn fromstr_accepts_camel_case_aliases() {
        assert_eq!(
            RobotsPolicy::from_str("IgnoreWithAudit").unwrap(),
            RobotsPolicy::IgnoreWithAudit
        );
        assert_eq!(
            RobotsPolicy::from_str("IgnoreSilently").unwrap(),
            RobotsPolicy::IgnoreSilently
        );
    }

    #[test]
    fn fromstr_rejects_unknown_variant() {
        let err = RobotsPolicy::from_str("always_obey").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("unknown variant"),
            "msg should explain the unknown variant: {msg}"
        );
        assert!(
            msg.contains("always_obey"),
            "msg should echo the bad input: {msg}"
        );
    }

    #[test]
    fn serialize_round_trip_json() {
        let json = serde_json::to_string(&RobotsPolicy::IgnoreWithAudit).unwrap();
        assert_eq!(json, "\"ignore_with_audit\"");
        let back: RobotsPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, RobotsPolicy::IgnoreWithAudit);
    }

    #[test]
    fn decision_helpers_classify_correctly() {
        let allow = RobotsDecision::Allow {
            reason: "no rule matched".to_string(),
        };
        assert!(allow.is_allowed());
        assert!(!allow.is_forbidden());
        assert!(!allow.is_unknown());

        let forbid = RobotsDecision::Forbid {
            reason: "Disallow: /private".to_string(),
        };
        assert!(forbid.is_forbidden());
        assert!(!forbid.is_allowed());

        assert!(RobotsDecision::Unknown.is_unknown());
    }
}
