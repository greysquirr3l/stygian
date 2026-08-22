//! Robots-policy guard port — closes guide failure mode #9.
//!
//! `RobotsPolicyGuard` is the consumer-owned port trait that both recon
//! and production paths use to ask "may I fetch this URL?". The
//! [`RobotsPolicy`](crate::domain::policy::RobotsPolicy) value type
//! lives in the domain layer and is shared; only the guard implementation
//! (the part that actually fetches + parses `robots.txt`) is swappable.
//!
//! Default adapters shipped by `stygian-graph`:
//!
//! - [`PermissiveRobotsGuard`] — returns `Allow` for every URL. Used
//!   when the operator has explicitly opted in to `IgnoreSilently` or
//!   the guard has not been wired up. Safe default for unit tests.
//! - (Reserved for future adapters) — `CachedRobotsGuard` backed by a
//!   real `robots.txt` fetch + parse.

use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::domain::error::{GraphError, Result, StygianError};
use crate::domain::policy::{RobotsDecision, RobotsPolicy};

/// Reason captured in the [`RobotsDecision::Forbid`] variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForbidReason {
    /// `robots.txt` disallows this URL.
    Disallow,
    /// `robots.txt` rate-limits this URL (e.g. `Crawl-delay` > configured max).
    RateLimited,
    /// `robots.txt` is missing but the operator has set
    /// [`RobotsPolicy::Obey`] — refuse by default rather than silently
    /// passing through.
    MissingRobotsTxt,
}

/// Port trait: ask a guard whether a URL is permitted under the
/// configured [`RobotsPolicy`].
///
/// Adapters must be `Send + Sync` so they can be stored in an `Arc` and
/// shared between the recon path and the production path. The guard is
/// invoked at pipeline-build time (so forbidden URLs surface before any
/// production traffic) and again at execute time (so policy drift
/// between spec-build and run is caught).
#[async_trait]
pub trait RobotsPolicyGuard: Send + Sync {
    /// Stable name for diagnostics — usually the upstream source
    /// (`"manual"`, `"robots_txt"`, `"cached"`).
    fn name(&self) -> &'static str;

    /// Decide whether a single URL may be fetched.
    ///
    /// # Errors
    ///
    /// - [`GraphError::ServiceUnavailable`] if the guard cannot reach
    ///   its data source (e.g. network failure during a live
    ///   `robots.txt` lookup).
    async fn decide(&self, url: &str) -> Result<RobotsDecision>;
}

/// Apply a [`RobotsPolicy`] to a [`RobotsDecision`]. The single source
/// of truth for how a decision maps to a concrete action.
///
/// This helper exists so both recon and production use the *same*
/// reducer. Without it, two callers could each implement their own
/// `match` and reintroduce the very bug T111 exists to prevent.
#[must_use]
pub fn apply_policy(policy: RobotsPolicy, decision: RobotsDecision) -> PolicyOutcome {
    use RobotsDecision as D;
    use RobotsPolicy as P;

    match (policy, decision) {
        // Obey + Forbid → refuse with the guard's reason.
        (P::Obey, D::Forbid { reason }) => PolicyOutcome::Refuse { reason },
        // Obey + Unknown → refuse with a default reason. We refuse
        // (rather than allow) because the guide's rule of thumb is
        // "spec is built from pages the shipped spider may be forbidden
        // to fetch" — Unknown under Obey is the dangerous case.
        (P::Obey, D::Unknown) => PolicyOutcome::Refuse {
            reason: "robots guard returned Unknown under Obey policy".to_string(),
        },

        // IgnoreWithAudit + Forbid → fetch + audit (the only arm that
        // emits a non-default outcome).
        (P::IgnoreWithAudit, D::Forbid { reason }) => PolicyOutcome::FetchWithAudit { reason },

        // Every other (policy, decision) pair maps to plain `Fetch`:
        //   Obey + Allow,
        //   IgnoreWithAudit + Allow,
        //   IgnoreWithAudit + Unknown,
        //   IgnoreSilently + Allow | Forbid | Unknown.
        // The `Fetch` arm intentionally collapses these so the
        // reducer stays a single source of truth.
        _ => PolicyOutcome::Fetch,
    }
}

/// The reducer's output — what the caller should do for this URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyOutcome {
    /// Fetch the URL.
    Fetch,
    /// Fetch the URL and record the guard's reason as an audit event.
    FetchWithAudit {
        /// Reason supplied by the guard — usually the matched rule.
        reason: String,
    },
    /// Do not fetch the URL.
    Refuse {
        /// Reason supplied by the guard — usually the matched rule.
        reason: String,
    },
}

impl fmt::Display for PolicyOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fetch => f.write_str("fetch"),
            Self::FetchWithAudit { reason } => write!(f, "fetch_with_audit({reason})"),
            Self::Refuse { reason } => write!(f, "refuse({reason})"),
        }
    }
}

/// No-op guard: returns [`RobotsDecision::Allow`] for every URL.
///
/// This is the default adapter shipped with `stygian-graph`. It exists
/// so pipelines can wire a guard in place without taking on a real
/// `robots.txt` fetcher. Real implementations are out of scope for
/// T111 — see the module docs for the planned `CachedRobotsGuard`.
///
/// When the operator sets [`RobotsPolicy::Obey`] and the guard returns
/// `Allow`, the outcome is `Fetch` — i.e. the guard is the single
/// source of truth on which URLs are permitted. With
/// `PermissiveRobotsGuard`, *every* URL is permitted, so Obey behaves
/// as if `robots.txt` has been explicitly fetched and parsed to allow
/// everything.
#[derive(Debug, Clone, Default)]
pub struct PermissiveRobotsGuard;

#[async_trait]
impl RobotsPolicyGuard for PermissiveRobotsGuard {
    fn name(&self) -> &'static str {
        "permissive"
    }

    async fn decide(&self, _url: &str) -> Result<RobotsDecision> {
        Ok(RobotsDecision::Allow {
            reason: "permissive guard: no opinion".to_string(),
        })
    }
}

/// Wrap a [`PermissiveRobotsGuard`] so the result is always a fresh
/// `Arc` — convenient for callers that need a stable port-owned type.
#[must_use]
pub fn permissive_guard() -> std::sync::Arc<dyn RobotsPolicyGuard> {
    std::sync::Arc::new(PermissiveRobotsGuard)
}

/// Validate that the supplied [`RobotsPolicy`] + [`RobotsDecision`]
/// combination is internally consistent.
///
/// Returns `Err(StygianError::Graph(GraphError::InvalidPipeline(...)))`
/// if a pipeline tries to enforce `Obey` while shipping a no-op guard
/// that returns `Allow` for everything — that combination would mean
/// the pipeline claims to obey robots.txt but cannot, since it has no
/// data source to check against. The intent is to surface this at
/// pipeline-build time rather than silently passing every URL through.
pub fn validate_guard_pair(policy: RobotsPolicy, guard: &dyn RobotsPolicyGuard) -> Result<()> {
    if matches!(policy, RobotsPolicy::Obey) && guard.name() == "permissive" {
        return Err(StygianError::Graph(GraphError::InvalidPipeline(
            "robots_policy = Obey requires a non-permissive RobotsPolicyGuard; \
             ship a real robots.txt fetcher or downgrade the policy to \
             IgnoreSilently"
                .to_string(),
        )));
    }
    Ok(())
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
    fn obey_allow_fetches() {
        let out = apply_policy(
            RobotsPolicy::Obey,
            RobotsDecision::Allow {
                reason: "ok".to_string(),
            },
        );
        assert_eq!(out, PolicyOutcome::Fetch);
    }

    #[test]
    fn obey_forbid_refuses() {
        let out = apply_policy(
            RobotsPolicy::Obey,
            RobotsDecision::Forbid {
                reason: "Disallow: /private".to_string(),
            },
        );
        assert_eq!(
            out,
            PolicyOutcome::Refuse {
                reason: "Disallow: /private".to_string()
            }
        );
    }

    #[test]
    fn obey_unknown_refuses() {
        let out = apply_policy(RobotsPolicy::Obey, RobotsDecision::Unknown);
        match out {
            PolicyOutcome::Refuse { reason } => {
                assert!(reason.contains("Unknown"));
            }
            other => panic!("expected Refuse, got {other:?}"),
        }
    }

    #[test]
    fn ignore_with_audit_forbid_fetches_with_audit() {
        let out = apply_policy(
            RobotsPolicy::IgnoreWithAudit,
            RobotsDecision::Forbid {
                reason: "Disallow: /x".to_string(),
            },
        );
        assert_eq!(
            out,
            PolicyOutcome::FetchWithAudit {
                reason: "Disallow: /x".to_string()
            }
        );
    }

    #[test]
    fn ignore_with_audit_unknown_fetches() {
        let out = apply_policy(RobotsPolicy::IgnoreWithAudit, RobotsDecision::Unknown);
        assert_eq!(out, PolicyOutcome::Fetch);
    }

    #[test]
    fn ignore_silently_always_fetches() {
        for decision in [
            RobotsDecision::Allow {
                reason: "ok".to_string(),
            },
            RobotsDecision::Forbid {
                reason: "no".to_string(),
            },
            RobotsDecision::Unknown,
        ] {
            assert_eq!(
                apply_policy(RobotsPolicy::IgnoreSilently, decision),
                PolicyOutcome::Fetch
            );
        }
    }

    #[tokio::test]
    async fn permissive_guard_allows_everything() {
        let guard = PermissiveRobotsGuard;
        assert_eq!(guard.name(), "permissive");
        let d = guard.decide("https://example.com/anything").await.unwrap();
        assert!(d.is_allowed());
    }

    #[test]
    fn validate_guard_pair_rejects_obey_with_permissive() {
        let guard = PermissiveRobotsGuard;
        let err = validate_guard_pair(RobotsPolicy::Obey, &guard).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Obey"), "msg should mention policy: {msg}");
        assert!(
            msg.contains("permissive"),
            "msg should mention guard: {msg}"
        );
    }

    #[test]
    fn validate_guard_pair_accepts_ignore_silently_with_permissive() {
        let guard = PermissiveRobotsGuard;
        validate_guard_pair(RobotsPolicy::IgnoreSilently, &guard).unwrap();
        validate_guard_pair(RobotsPolicy::IgnoreWithAudit, &guard).unwrap();
    }

    #[test]
    fn policy_outcome_display_is_human_readable() {
        assert_eq!(PolicyOutcome::Fetch.to_string(), "fetch");
        assert_eq!(
            PolicyOutcome::FetchWithAudit {
                reason: "x".to_string()
            }
            .to_string(),
            "fetch_with_audit(x)"
        );
        assert_eq!(
            PolicyOutcome::Refuse {
                reason: "y".to_string()
            }
            .to_string(),
            "refuse(y)"
        );
    }
}
