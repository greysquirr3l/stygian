//! Engine-keyed memory identity (T110).
//!
//! The [`EngineKey`] is the **durable identity** of a scraping
//! target: the anti-bot engine family, its version, the target
//! class, and the TLS profile used to talk to it. It deliberately
//! does **not** carry the URL — a self-healing patch recorded
//! against one URL on one engine should heal every URL on that
//! engine, not just the URL it was discovered on.
//!
//! The principle being encoded is the platform-keyed-memory
//! instinct from the source guide: *"the durable asset is not the
//! record you extracted, it is the route you learned to it."* A
//! URL is volatile (it rotates, it expires, it points at one
//! record among many); the engine is durable (it is the same vendor
//! behind every URL on the same platform).
//!
//! All four fields participate in the [`EngineKey::hash`] /
//! [`EngineKey::cmp`] equivalence so two entries recorded under
//! different `version` or `tls_profile` values are **never** the
//! same key. The four guard tests in the parent `memory` module
//! exercise this property — re-keying a vendor version or changing
//! the TLS profile deliberately produces a fresh memory slot.
//!
//! # Wire format
//!
//! [`EngineKey`] implements `Display`, `FromStr` (via
//! `TryFrom<&str>`), and `serde::Serialize` /
//! `serde::Deserialize`. The wire form is
//! `engine[/version][+tls_profile]:target_class`, e.g.:
//!
//! - `cloudflare:api` — bare engine + target class
//! - `cloudflare/bot-manager-v3:api` — with vendor version
//! - `cloudflare+chrome136:api` — with TLS profile
//! - `cloudflare/bot-manager-v3+chrome136:api` — fully specified
//!
//! `Display` round-trips through `TryFrom<&str>` and the JSON
//! form is a 4-field object. The `Default` value is the
//! `VendorId::Unknown` engine at `TargetClass::Unknown` with no
//! version and no TLS profile.

use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::types::TargetClass;
use crate::vendor_classifier::VendorId;

/// Error returned when an [`EngineKey`] cannot be parsed from a
/// string slice.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineKeyParseError {
    /// The supplied string was empty.
    #[error("engine key is empty")]
    Empty,
    /// The string was missing the `:` separator that delimits the
    /// engine (and optional `version` / `tls_profile`) from the
    /// target class.
    #[error("engine key is missing ':' separator between engine and target class")]
    MissingTargetClassSeparator,
    /// The engine component did not match any [`VendorId`] label.
    #[error("unknown engine label: {0}")]
    UnknownEngine(String),
    /// The target class component did not match any
    /// [`TargetClass`] label.
    #[error("unknown target class label: {0}")]
    UnknownTargetClass(String),
    /// The TLS profile or version component was empty after the
    /// `+` or `/` separator.
    #[error("engine key has empty {0} component")]
    EmptyModifier(&'static str),
}

/// Durable identity of a scraping target's anti-bot engine.
///
/// Two `EngineKey` values are equal iff **every** field is equal
/// — including the optional `version` and `tls_profile`. This is
/// deliberate: an Akamai Bot Manager v3 patch must not silently
/// apply to a v4 deployment, and a Chrome-136 TLS-profile patch
/// must not silently apply to a Firefox-130 profile.
///
/// The field layout matches the T110 spec: `engine` is the
/// anti-bot engine family, `version` is the vendor's revision
/// (e.g. `bot-manager-v3`), `target_class` is the site's posture,
/// and `tls_profile` is the client-side TLS fingerprint (e.g.
/// `chrome136`).
///
/// # Example
///
/// ```
/// use stygian_charon::challenge_feedback::EngineKey;
/// use stygian_charon::types::TargetClass;
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let key = EngineKey {
///     engine: VendorId::Cloudflare,
///     version: Some("bot-manager-v3".to_string()),
///     target_class: TargetClass::Api,
///     tls_profile: Some("chrome136".to_string()),
/// };
/// assert_eq!(key.engine, VendorId::Cloudflare);
/// assert_eq!(key.version.as_deref(), Some("bot-manager-v3"));
/// ```
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EngineKey {
    /// Anti-bot engine family. Use `VendorId::Unknown` only when
    /// no engine could be classified.
    pub engine: VendorId,
    /// Optional vendor version (e.g. `"bot-manager-v3"`).
    /// `None` means the version is unknown or unversioned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Target class (api / `content_site` / `high_security` / unknown).
    pub target_class: TargetClass,
    /// Optional client TLS profile (e.g. `"chrome136"`).
    /// `None` means the TLS profile was not recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_profile: Option<String>,
}

impl EngineKey {
    /// Engine component of the key, used for display and stable
    /// hashing. Returns [`VendorId::label`].
    #[must_use]
    pub const fn engine_label(&self) -> &'static str {
        self.engine.label()
    }

    /// Validate the key has no empty optional fields. Returns
    /// `Ok(())` for well-formed keys; otherwise a
    /// [`EngineKeyParseError::EmptyModifier`].
    ///
    /// This is primarily a helper for `FromStr` but is also
    /// useful when constructing keys from user-supplied config.
    ///
    /// # Errors
    ///
    /// Returns [`EngineKeyParseError::EmptyModifier`] if either
    /// optional component is an empty string.
    pub fn validate(&self) -> Result<(), EngineKeyParseError> {
        if let Some(version) = self.version.as_deref()
            && version.is_empty()
        {
            return Err(EngineKeyParseError::EmptyModifier("version"));
        }
        if let Some(tls_profile) = self.tls_profile.as_deref()
            && tls_profile.is_empty()
        {
            return Err(EngineKeyParseError::EmptyModifier("tls_profile"));
        }
        Ok(())
    }
}

impl Display for EngineKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.engine.label())?;
        if let Some(version) = self.version.as_deref() {
            write!(f, "/{version}")?;
        }
        if let Some(tls_profile) = self.tls_profile.as_deref() {
            write!(f, "+{tls_profile}")?;
        }
        let target_class = match self.target_class {
            TargetClass::Api => "api",
            TargetClass::ContentSite => "content_site",
            TargetClass::HighSecurity => "high_security",
            TargetClass::Unknown => "unknown",
        };
        write!(f, ":{target_class}")
    }
}

impl FromStr for EngineKey {
    type Err = EngineKeyParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(EngineKeyParseError::Empty);
        }

        let (head, target_class_str) = s
            .rsplit_once(':')
            .ok_or(EngineKeyParseError::MissingTargetClassSeparator)?;
        let target_class = match target_class_str {
            "api" => TargetClass::Api,
            "content_site" => TargetClass::ContentSite,
            "high_security" => TargetClass::HighSecurity,
            "unknown" => TargetClass::Unknown,
            other => return Err(EngineKeyParseError::UnknownTargetClass(other.to_string())),
        };

        // Head: `engine[/version][+tls_profile]`
        let mut version: Option<String> = None;
        let mut tls_profile: Option<String> = None;
        let engine_str;

        if let Some((pre_tls, tls)) = head.split_once('+') {
            if tls.is_empty() {
                return Err(EngineKeyParseError::EmptyModifier("tls_profile"));
            }
            tls_profile = Some(tls.to_string());
            if let Some((pre_ver, ver)) = pre_tls.split_once('/') {
                if ver.is_empty() {
                    return Err(EngineKeyParseError::EmptyModifier("version"));
                }
                version = Some(ver.to_string());
                engine_str = pre_ver;
            } else {
                engine_str = pre_tls;
            }
        } else if let Some((pre_ver, ver)) = head.split_once('/') {
            if ver.is_empty() {
                return Err(EngineKeyParseError::EmptyModifier("version"));
            }
            version = Some(ver.to_string());
            engine_str = pre_ver;
        } else {
            engine_str = head;
        }

        let engine = VendorId::from_label(engine_str)
            .ok_or_else(|| EngineKeyParseError::UnknownEngine(engine_str.to_string()))?;

        Ok(Self {
            engine,
            version,
            target_class,
            tls_profile,
        })
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

    fn cloudflare_api() -> EngineKey {
        EngineKey {
            engine: VendorId::Cloudflare,
            version: None,
            target_class: TargetClass::Api,
            tls_profile: None,
        }
    }

    #[test]
    fn display_round_trips_through_from_str() {
        for key in [
            cloudflare_api(),
            EngineKey {
                engine: VendorId::Akamai,
                version: Some("bot-manager-v3".to_string()),
                target_class: TargetClass::HighSecurity,
                tls_profile: None,
            },
            EngineKey {
                engine: VendorId::Cloudflare,
                version: None,
                target_class: TargetClass::ContentSite,
                tls_profile: Some("chrome136".to_string()),
            },
            EngineKey {
                engine: VendorId::DataDome,
                version: Some("v2".to_string()),
                target_class: TargetClass::Api,
                tls_profile: Some("firefox130".to_string()),
            },
        ] {
            let rendered = key.to_string();
            let parsed: EngineKey = rendered.parse().expect("round-trip parse");
            assert_eq!(parsed, key, "round-trip mismatch for {rendered}");
        }
    }

    #[test]
    fn from_str_rejects_empty_input() {
        assert_eq!(
            EngineKey::from_str(""),
            Err(EngineKeyParseError::Empty),
            "empty input must be rejected"
        );
    }

    #[test]
    fn from_str_rejects_missing_target_class_separator() {
        assert_eq!(
            EngineKey::from_str("cloudflare"),
            Err(EngineKeyParseError::MissingTargetClassSeparator),
            "missing ':' separator must be rejected"
        );
    }

    #[test]
    fn from_str_rejects_unknown_engine_label() {
        assert_eq!(
            EngineKey::from_str("not_a_vendor:api"),
            Err(EngineKeyParseError::UnknownEngine(
                "not_a_vendor".to_string()
            )),
            "unknown engine label must be rejected"
        );
    }

    #[test]
    fn from_str_rejects_unknown_target_class_label() {
        assert_eq!(
            EngineKey::from_str("cloudflare:not_a_class"),
            Err(EngineKeyParseError::UnknownTargetClass(
                "not_a_class".to_string()
            )),
            "unknown target class must be rejected"
        );
    }

    #[test]
    fn from_str_rejects_empty_modifier_components() {
        assert_eq!(
            EngineKey::from_str("cloudflare/:api"),
            Err(EngineKeyParseError::EmptyModifier("version")),
            "empty version must be rejected"
        );
        assert_eq!(
            EngineKey::from_str("cloudflare+:api"),
            Err(EngineKeyParseError::EmptyModifier("tls_profile")),
            "empty tls_profile must be rejected"
        );
    }

    #[test]
    fn keys_differing_only_by_target_class_are_not_equal() {
        let a = cloudflare_api();
        let b = EngineKey {
            target_class: TargetClass::ContentSite,
            ..a.clone()
        };
        assert_ne!(a, b, "target_class must participate in equality");
    }

    #[test]
    fn keys_differing_only_by_tls_profile_are_not_equal() {
        let a = cloudflare_api();
        let b = EngineKey {
            tls_profile: Some("chrome136".to_string()),
            ..a.clone()
        };
        assert_ne!(a, b, "tls_profile must participate in equality");
    }

    #[test]
    fn keys_differing_only_by_version_are_not_equal() {
        let a = EngineKey {
            version: Some("v3".to_string()),
            ..cloudflare_api()
        };
        let b = EngineKey {
            version: Some("v4".to_string()),
            ..a.clone()
        };
        assert_ne!(a, b, "version must participate in equality");
    }

    #[test]
    fn serde_json_round_trips_through_engine_key() {
        let key = EngineKey {
            engine: VendorId::PerimeterX,
            version: Some("human-v1".to_string()),
            target_class: TargetClass::HighSecurity,
            tls_profile: Some("chrome136".to_string()),
        };
        let json = serde_json::to_string(&key).expect("serialize");
        let parsed: EngineKey = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, key, "JSON round-trip mismatch");
    }

    #[test]
    fn from_str_uses_rsplit_once_to_locate_target_class() {
        // Multiple `:` characters in the input: only the last one is
        // the engine/target-class separator (`rsplit_once` semantics).
        // The trailing "extra" segment is what gets parsed as the
        // target class, so it must surface as `UnknownTargetClass`
        // (no VendorId label contains a `:`).
        match EngineKey::from_str("cloudflare:api:extra") {
            Err(EngineKeyParseError::UnknownTargetClass(s)) => assert_eq!(s, "extra"),
            other => panic!(
                "extra colons must surface as UnknownTargetClass on the suffix, got {other:?}"
            ),
        }

        // Sanity: a single trailing colon leaves the target class
        // empty, which is a parse error.
        assert_eq!(
            EngineKey::from_str("cloudflare:"),
            Err(EngineKeyParseError::UnknownTargetClass(String::new()))
        );
    }

    #[test]
    fn validate_rejects_empty_optional_components() {
        let bad_version = EngineKey {
            version: Some(String::new()),
            ..cloudflare_api()
        };
        assert_eq!(
            bad_version.validate(),
            Err(EngineKeyParseError::EmptyModifier("version"))
        );

        let bad_tls = EngineKey {
            tls_profile: Some(String::new()),
            ..cloudflare_api()
        };
        assert_eq!(
            bad_tls.validate(),
            Err(EngineKeyParseError::EmptyModifier("tls_profile"))
        );

        assert_eq!(cloudflare_api().validate(), Ok(()));
    }
}
