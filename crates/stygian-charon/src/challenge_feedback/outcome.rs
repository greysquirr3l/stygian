use serde::{Deserialize, Serialize};

use crate::types::TargetClass;

/// Normalised label for the outcome of a single acquisition attempt.
///
/// The taxonomy is intentionally small and stable so that policy
/// planning, vendor classification (T89), and change-detection
/// feeds (T88) can all agree on a shared vocabulary. Each variant
/// carries a stable `snake_case` wire label and a per-outcome
/// [`risk_delta`][Self::risk_delta] that the feedback loop adds to
/// the next runtime policy's risk score (subject to the documented
/// [`MAX_RISK_DELTA`][crate::challenge_feedback::MAX_RISK_DELTA] clamp).
///
/// # Example
///
/// ```
/// use stygian_charon::challenge_feedback::ChallengeOutcome;
///
/// let outcome = ChallengeOutcome::HardChallenge;
/// assert_eq!(outcome.label(), "hard_challenge");
/// assert!(outcome.risk_delta() > 0.0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeOutcome {
    /// The request returned successfully (2xx) with no challenge
    /// artefact in the response body or headers.
    Pass,
    /// The request returned a soft challenge (e.g. Cloudflare
    /// "Just a moment…" interstitial, `403` with a JS challenge
    /// script, or a slow-down page) that the runner eventually
    /// solved without raising execution mode.
    SoftChallenge,
    /// The request returned a hard challenge (e.g. a `cf-chl-bypass`
    /// token, a `DataDome` interstitial, an Akamai Bot Manager
    /// challenge page) that required a browser-stealth strategy.
    HardChallenge,
    /// The request was blocked outright (e.g. `403`/`429` with no
    /// challenge artefact — IP-level or fingerprint-level
    /// rejection).
    Blocked,
    /// The request was served a CAPTCHA (reCAPTCHA, hCaptcha,
    /// `DataDome` `captcha-delivery`, etc.) that could not be
    /// solved automatically.
    Captcha,
}

impl ChallengeOutcome {
    /// Stable, human-readable label for telemetry / JSON output.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::challenge_feedback::ChallengeOutcome;
    ///
    /// assert_eq!(ChallengeOutcome::Pass.label(), "pass");
    /// assert_eq!(ChallengeOutcome::SoftChallenge.label(), "soft_challenge");
    /// assert_eq!(ChallengeOutcome::HardChallenge.label(), "hard_challenge");
    /// assert_eq!(ChallengeOutcome::Blocked.label(), "blocked");
    /// assert_eq!(ChallengeOutcome::Captcha.label(), "captcha");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::SoftChallenge => "soft_challenge",
            Self::HardChallenge => "hard_challenge",
            Self::Blocked => "blocked",
            Self::Captcha => "captcha",
        }
    }

    /// Per-outcome risk-score contribution before clamping.
    ///
    /// The values are bounded by
    /// [`MAX_RISK_DELTA`][crate::challenge_feedback::MAX_RISK_DELTA]
    /// so a single entry never overshoots the documented ceiling
    /// on its own. [`Pass`][Self::Pass] carries a small **negative**
    /// contribution to gently de-escalate after clean runs; every
    /// other outcome contributes a non-negative amount.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::challenge_feedback::ChallengeOutcome;
    ///
    /// assert!(ChallengeOutcome::Pass.risk_delta() < 0.0);
    /// assert!(ChallengeOutcome::SoftChallenge.risk_delta() > 0.0);
    /// assert!(ChallengeOutcome::HardChallenge.risk_delta() > 0.0);
    /// assert!(ChallengeOutcome::Blocked.risk_delta() > 0.0);
    /// assert!(ChallengeOutcome::Captcha.risk_delta() > 0.0);
    /// ```
    #[must_use]
    pub const fn risk_delta(self) -> f64 {
        match self {
            Self::Pass => -0.10,
            Self::SoftChallenge => 0.05,
            Self::HardChallenge => 0.15,
            Self::Blocked | Self::Captcha => 0.20,
        }
    }

    /// Per-`(outcome, target_class)` risk-score contribution.
    ///
    /// Different target classes interpret the same outcome
    /// differently: an API hitting a Captcha is anomalous (the
    /// runner should back off — sign flips to negative), while the
    /// same Captcha on a Content site is a normal anti-bot posture
    /// that warrants a higher risk score. The
    /// [`TargetClass::Unknown`] fallback returns `0.0` because the
    /// runner has no signal to act on.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::challenge_feedback::ChallengeOutcome;
    /// use stygian_charon::types::TargetClass;
    ///
    /// // API + Captcha is anomalous — sign flips to negative.
    /// let api_captcha = ChallengeOutcome::Captcha.risk_delta_for(TargetClass::Api);
    /// assert!(api_captcha < 0.0);
    ///
    /// // ContentSite + Captcha is a normal anti-bot posture — positive.
    /// let content_captcha = ChallengeOutcome::Captcha.risk_delta_for(TargetClass::ContentSite);
    /// assert!(content_captcha > 0.0);
    ///
    /// // Unknown class has no signal.
    /// let unknown = ChallengeOutcome::Captcha.risk_delta_for(TargetClass::Unknown);
    /// assert!(unknown == 0.0);
    /// ```
    #[must_use]
    pub const fn risk_delta_for(self, target_class: TargetClass) -> f64 {
        match target_class {
            // Machine-to-machine targets should always pass.
            // Any challenge is anomalous — flip the sign so positive
            // deltas (Captcha/Blocked/HardChallenge/SoftChallenge)
            // become negative ("back off — this is unusual"). The
            // incidental flip of Pass to positive is acceptable:
            // an API recording Pass is suspicious, not the baseline.
            TargetClass::Api => -self.risk_delta(),
            // Content sites and high-security targets both treat
            // challenges as expected signal — raw keeps the signal
            // flowing upward (negative for Pass, positive for any
            // challenge).
            TargetClass::ContentSite | TargetClass::HighSecurity => self.risk_delta(),
            // Unknown class has no signal to act on.
            TargetClass::Unknown => 0.0,
        }
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
    fn labels_are_stable() {
        assert_eq!(ChallengeOutcome::Pass.label(), "pass");
        assert_eq!(ChallengeOutcome::SoftChallenge.label(), "soft_challenge");
        assert_eq!(ChallengeOutcome::HardChallenge.label(), "hard_challenge");
        assert_eq!(ChallengeOutcome::Blocked.label(), "blocked");
        assert_eq!(ChallengeOutcome::Captcha.label(), "captcha");
    }

    #[test]
    fn risk_deltas_are_bounded() {
        for outcome in [
            ChallengeOutcome::Pass,
            ChallengeOutcome::SoftChallenge,
            ChallengeOutcome::HardChallenge,
            ChallengeOutcome::Blocked,
            ChallengeOutcome::Captcha,
        ] {
            let delta = outcome.risk_delta();
            assert!(
                (-0.20..=0.20).contains(&delta),
                "delta out of bounded range: {delta} for {outcome:?}"
            );
        }
    }

    #[test]
    fn serde_round_trip_is_stable() {
        for outcome in [
            ChallengeOutcome::Pass,
            ChallengeOutcome::SoftChallenge,
            ChallengeOutcome::HardChallenge,
            ChallengeOutcome::Blocked,
            ChallengeOutcome::Captcha,
        ] {
            let json = serde_json::to_string(&outcome).expect("serialize");
            let back: ChallengeOutcome = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(outcome, back);
            assert_eq!(json, format!("\"{}\"", outcome.label()));
        }
    }
}
