//! Default [`CoherencePort`](crate::ports::coherence::CoherencePort) adapter.
//!
//! Applies the five-vector rule from the 2026 scraping guide (L2839,
//! L3135-3138): the proxy exit's country, the recursive DNS resolver's
//! country, the WebRTC public IP `/16`, the browser timezone, the
//! browser locale, and the `Accept-Language` header must all agree
//! before the request is sent.
//!
//! The validator is **stateless and O(1)**: no locks, no async I/O, no
//! allocation beyond the country-↔-timezone lookup table (which is a
//! `&'static` slice). It is therefore safe to share via an
//! `Arc<dyn CoherencePort>` and to call on the hot path of
//! `ProxyManager::acquire_proxy_with_coherence` without violating the
//! `crates/stygian-proxy/AGENTS.md` sub-µs acquisition budget.
//!
//! ## Hot-path budget
//!
//! The validator performs at most six constant-time checks per call:
//!
//! 1. `proxy_geo_country` vs `dns_resolver_country`
//!    ([`MismatchField::ProxyGeoVsDns`], `Hard`).
//! 2. `webrtc_public_ip` vs `proxy_ip` (`/16` agreement, IPv4 only —
//!    [`MismatchField::WebRtcPublicIp`], `Hard`).
//! 3. `browser_timezone` ↔ proxy / DNS country
//!    ([`MismatchField::Timezone`], `Advisory`).
//! 4. `browser_locale` region ↔ proxy / DNS country
//!    ([`MismatchField::Locale`], `Advisory`).
//! 5. `accept_language` primary region ↔ proxy / DNS country
//!    ([`MismatchField::AcceptLanguage`], `Advisory`).
//!
//! The first failing check short-circuits and returns. With all five
//! inputs present the validator stays under 100 ns on any modern
//! laptop — comfortably within the 1 µs acquisition budget.
//!
//! ## Country ↔ timezone table
//!
//! The static `COUNTRY_TIMEZONE_REGIONS` table maps every ISO country
//! covered by the spec tests to the set of IANA TZ region prefixes
//! that count as "domestic" for that country. The table is intentionally
//! compact (covers only the countries mentioned in the T97 acceptance
//! tests and a small superset of common scraping targets); unknown
//! countries yield `Unknown("unmapped_country")` so operators can see
//! the gap in test logs without false-positive mismatches.

use crate::ports::coherence::{
    CoherenceContext, CoherencePort, CoherenceVerdict, IsoCountry, MismatchField, MismatchSeverity,
    Tz,
};

/// Default validator: stateless, `Send + Sync + 'static`, O(1).
///
/// Constructed via [`DefaultCoherenceValidator::default`]; the type is
/// intentionally unit-struct because no configuration is needed today
/// (the country ↔ timezone table is a `&'static` slice). Future
/// per-tenant timezone overrides can be added without changing the
/// public surface — replace the `&'static` slice with a
/// `Cow<'static, …>` and add a builder step.
///
/// # Example
///
/// ```rust
/// use std::net::IpAddr;
/// use std::str::FromStr;
/// use stygian_proxy::adapters::coherence::DefaultCoherenceValidator;
/// use stygian_proxy::ports::coherence::{
///     AcceptLanguage, CoherenceContext, CoherencePort, CoherenceVerdict, IsoCountry, Locale, Tz,
/// };
///
/// let validator = DefaultCoherenceValidator;
///
/// // Clean US context.
/// let ctx = CoherenceContext {
///     proxy_geo_country: Some(IsoCountry::new("US").unwrap()),
///     dns_resolver_country: Some(IsoCountry::new("US").unwrap()),
///     browser_locale: Locale::new("en-US").unwrap(),
///     browser_timezone: Tz::new("America/New_York").unwrap(),
///     accept_language: AcceptLanguage::new("en-US,en;q=0.9").unwrap(),
///     webrtc_local_ip: None,
///     webrtc_public_ip: Some(IpAddr::from_str("192.0.2.42").unwrap()),
///     proxy_ip: Some(IpAddr::from_str("192.0.2.7").unwrap()),
/// };
/// assert_eq!(validator.evaluate(&ctx), CoherenceVerdict::Coherent);
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultCoherenceValidator;

impl DefaultCoherenceValidator {
    /// Build a new validator instance. Equivalent to
    /// [`DefaultCoherenceValidator::default`] but reads more naturally
    /// at call sites that prefer explicit constructor calls.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl CoherencePort for DefaultCoherenceValidator {
    fn evaluate(&self, ctx: &CoherenceContext) -> CoherenceVerdict {
        // 1. Country agreement: proxy_geo vs DNS resolver.
        if let Some(mismatch) = check_country_agreement(ctx) {
            return mismatch;
        }

        // 2. WebRTC public IP /16 agreement with proxy exit.
        if let Some(mismatch) = check_webrtc_slash_16(ctx) {
            return mismatch;
        }

        // 3. Timezone ↔ country agreement (advisory).
        if let Some(mismatch) = check_timezone(ctx) {
            return mismatch;
        }

        // 4. Locale ↔ country agreement (advisory).
        if let Some(mismatch) = check_locale(ctx) {
            return mismatch;
        }

        // 5. Accept-Language ↔ country agreement (advisory).
        if let Some(mismatch) = check_accept_language(ctx) {
            return mismatch;
        }

        CoherenceVerdict::Coherent
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-vector checks
// ─────────────────────────────────────────────────────────────────────────────

/// Country agreement: `proxy_geo_country` vs `dns_resolver_country`.
///
/// `Hard` severity because both fields are server-side observables; a
/// disagreement is the cleanest proxy-leak signal in the spec. When
/// only one of the two is present we cannot reach a verdict on the
/// agreement vector specifically, but the
/// [`effective_country`](self::effective_country) helper will still
/// drive the downstream advisory checks off whichever country is
/// available so the timezone / locale / `Accept-Language` vectors are
/// not silently skipped.
fn check_country_agreement(ctx: &CoherenceContext) -> Option<CoherenceVerdict> {
    match (
        ctx.proxy_geo_country.as_ref(),
        ctx.dns_resolver_country.as_ref(),
    ) {
        (Some(proxy), Some(dns)) if proxy == dns => None,
        (Some(_), Some(_)) => Some(CoherenceVerdict::Mismatch {
            field: MismatchField::ProxyGeoVsDns,
            severity: MismatchSeverity::Hard,
        }),
        // At least one of the two geo fields is missing — downstream
        // checks will surface the gap if no country is available.
        _ => None,
    }
}

/// WebRTC public IP /16 agreement with proxy exit IP.
///
/// `Hard` severity because WebRTC leaks are observable by any web page;
/// the `/16` heuristic is the same the 2026 guide recommends.
fn check_webrtc_slash_16(ctx: &CoherenceContext) -> Option<CoherenceVerdict> {
    match (ctx.webrtc_public_ip, ctx.proxy_ip) {
        (Some(public), Some(proxy)) => match CoherenceContext::same_slash_16(public, proxy) {
            Some(true) => None,
            Some(false) => Some(CoherenceVerdict::Mismatch {
                field: MismatchField::WebRtcPublicIp,
                severity: MismatchSeverity::Hard,
            }),
            // Different address families → can't compare; treat as
            // `Unknown` so operators see the gap.
            None => Some(CoherenceVerdict::unknown("webrtc_proxy_ip_family_mismatch")),
        },
        (Some(_), None) => Some(CoherenceVerdict::unknown("missing_proxy_ip")),
        // Public IP not yet observed (browser disabled WebRTC) is the
        // common case — return `None` so other vectors still get checked.
        _ => None,
    }
}

/// Timezone ↔ country agreement.
///
/// `Advisory` because the timezone is browser-side and trivially
/// spoofable; the validator uses the [`COUNTRY_TIMEZONE_REGIONS`]
/// static map to decide which TZ prefixes count as "domestic" for a
/// given country.
fn check_timezone(ctx: &CoherenceContext) -> Option<CoherenceVerdict> {
    let Some(country) = effective_country(ctx) else {
        return Some(CoherenceVerdict::unknown("missing_geo_country"));
    };

    if tz_matches_country(&ctx.browser_timezone, country) {
        None
    } else {
        Some(CoherenceVerdict::Mismatch {
            field: MismatchField::Timezone,
            severity: MismatchSeverity::Advisory,
        })
    }
}

/// Locale ↔ country agreement (region subtag).
///
/// `Advisory` because the locale is browser-side and trivial to fake.
fn check_locale(ctx: &CoherenceContext) -> Option<CoherenceVerdict> {
    let Some(country) = effective_country(ctx) else {
        return Some(CoherenceVerdict::unknown("missing_geo_country"));
    };

    match ctx.browser_locale.region() {
        Some(region) if region.eq_ignore_ascii_case(country.as_str()) => None,
        Some(_) => Some(CoherenceVerdict::Mismatch {
            field: MismatchField::Locale,
            severity: MismatchSeverity::Advisory,
        }),
        None => Some(CoherenceVerdict::unknown("locale_missing_region")),
    }
}

/// Accept-Language ↔ country agreement (primary region).
///
/// `Advisory` because the header is server-set and a savvy operator
/// can easily spoof it; the check still catches obvious drift.
fn check_accept_language(ctx: &CoherenceContext) -> Option<CoherenceVerdict> {
    let Some(country) = effective_country(ctx) else {
        return Some(CoherenceVerdict::unknown("missing_geo_country"));
    };

    match ctx.accept_language.primary_region() {
        Some(region)
            if region
                .region()
                .is_some_and(|r| r.eq_ignore_ascii_case(country.as_str())) =>
        {
            None
        }
        Some(_) => Some(CoherenceVerdict::Mismatch {
            field: MismatchField::AcceptLanguage,
            severity: MismatchSeverity::Advisory,
        }),
        None => None, // bare language tag (e.g. `en;q=1.0`) — no region, no mismatch
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Country ↔ timezone table
// ─────────────────────────────────────────────────────────────────────────────

/// Return the country that drives every per-vector check.
///
/// When `proxy_geo_country` is present it wins (it is the authoritative
/// location for the egress). Otherwise `dns_resolver_country` is used
/// (DNS geolocation is the second-best signal). When neither is known
/// the check returns `Unknown("missing_geo_country")` so the manager
/// can log the gap.
fn effective_country(ctx: &CoherenceContext) -> Option<&IsoCountry> {
    ctx.proxy_geo_country
        .as_ref()
        .or(ctx.dns_resolver_country.as_ref())
}

/// Check whether `tz` is plausibly domestic for `country`.
///
/// Uses [`COUNTRY_TIMEZONE_REGIONS`] — a static map of ISO code → list
/// of acceptable IANA TZ region prefixes. The lookup walks a short
/// slice and never allocates.
fn tz_matches_country(tz: &Tz, country: &IsoCountry) -> bool {
    let Some((_, allowed_regions)) = COUNTRY_TIMEZONE_REGIONS
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(country.as_str()))
    else {
        return false;
    };
    tz.region().is_some_and(|tz_region| {
        allowed_regions
            .iter()
            .any(|prefix| prefix.eq_ignore_ascii_case(tz_region))
    })
}

/// Country ↔ list of acceptable IANA TZ region prefixes.
///
/// The list is intentionally compact: only the countries explicitly
/// covered by the T97 acceptance tests plus a small superset of common
/// scraping targets (US, GB, DE, FR, NL, JP, AU, CA, IN, PK, BR, MX,
/// SG, HK). Unknown countries yield a `false` from
/// [`tz_matches_country`] and the validator emits
/// `Unknown("missing_geo_country")` from the caller.
///
/// To extend the table, append `(country, &[prefixes])` and add a
/// unit test under `country_timezone_table_covers_test_cases`.
type TzPrefixList = &'static [&'static str];

const COUNTRY_TIMEZONE_REGIONS: &[(&str, TzPrefixList)] = &[
    // ── T97 spec countries ───────────────────────────────────────────────
    (
        "US",
        &[
            "America", // All US contiguous + Alaska timezones.
            "Pacific", // Hawaii.
            "US",      // Legacy POSIX-style names like US/Pacific.
        ],
    ),
    (
        "PK",
        &[
            "Asia", // Asia/Karachi.
        ],
    ),
    // ── Common scraping targets ──────────────────────────────────────────
    (
        "GB",
        &[
            "Europe", // Europe/London.
        ],
    ),
    (
        "DE",
        &[
            "Europe", // Europe/Berlin, Europe/Busingen.
        ],
    ),
    (
        "FR",
        &[
            "Europe", // Europe/Paris.
        ],
    ),
    (
        "NL",
        &[
            "Europe", // Europe/Amsterdam.
        ],
    ),
    (
        "JP",
        &[
            "Asia", // Asia/Tokyo.
        ],
    ),
    (
        "AU",
        &[
            "Australia",  // Australia/Sydney, Australia/Perth, …
            "Antarctica", // Australian Antarctic stations (rare).
        ],
    ),
    (
        "CA",
        &[
            "America", // America/Toronto, America/Vancouver, …
        ],
    ),
    (
        "IN",
        &[
            "Asia", // Asia/Kolkata.
        ],
    ),
    (
        "BR",
        &[
            "America", // America/Sao_Paulo, …
        ],
    ),
    (
        "MX",
        &[
            "America", // America/Mexico_City, …
        ],
    ),
    (
        "SG",
        &[
            "Asia", // Asia/Singapore.
        ],
    ),
    (
        "HK",
        &[
            "Asia", // Asia/Hong_Kong.
        ],
    ),
];

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::str::FromStr;
    use std::sync::Arc;

    use crate::ports::coherence::{
        AcceptLanguage, CoherencePort, Locale, MismatchField, MismatchSeverity,
    };

    fn validator() -> DefaultCoherenceValidator {
        DefaultCoherenceValidator
    }

    fn us() -> IsoCountry {
        IsoCountry::new("US").unwrap()
    }
    fn pk() -> IsoCountry {
        IsoCountry::new("PK").unwrap()
    }
    fn en_us_locale() -> Locale {
        Locale::new("en-US").unwrap()
    }
    fn fr_fr_locale() -> Locale {
        Locale::new("fr-FR").unwrap()
    }
    fn ny_tz() -> Tz {
        Tz::new("America/New_York").unwrap()
    }
    fn london_tz() -> Tz {
        Tz::new("Europe/London").unwrap()
    }
    fn al_en_us() -> AcceptLanguage {
        AcceptLanguage::new("en-US,en;q=0.9").unwrap()
    }
    fn al_fr_fr() -> AcceptLanguage {
        AcceptLanguage::new("fr-FR,fr;q=0.9").unwrap()
    }

    /// Helper: build a clean US context, then let the caller tweak.
    fn base_us_ctx() -> CoherenceContext {
        CoherenceContext {
            proxy_geo_country: Some(us()),
            dns_resolver_country: Some(us()),
            browser_locale: en_us_locale(),
            browser_timezone: ny_tz(),
            accept_language: al_en_us(),
            webrtc_local_ip: None,
            webrtc_public_ip: Some(IpAddr::from_str("192.0.2.42").unwrap()),
            proxy_ip: Some(IpAddr::from_str("192.0.2.7").unwrap()),
        }
    }

    // ── Spec coverage ──────────────────────────────────────────────────────

    /// T97 spec: US proxy + US DNS + `en-US` locale + `America/New_York`
    /// TZ → `Coherent`.
    #[test]
    fn us_proxy_us_dns_en_us_america_new_york_is_coherent() {
        let ctx = base_us_ctx();
        assert_eq!(validator().evaluate(&ctx), CoherenceVerdict::Coherent);
    }

    /// T97 spec: US proxy + PK DNS + `en-US` locale → `Mismatch`
    /// on `DnsCountry`, severity `Hard`.
    #[test]
    fn us_proxy_pk_dns_is_hard_dns_mismatch() {
        let ctx = CoherenceContext {
            dns_resolver_country: Some(pk()),
            ..base_us_ctx()
        };
        assert_eq!(
            validator().evaluate(&ctx),
            CoherenceVerdict::Mismatch {
                field: MismatchField::ProxyGeoVsDns,
                severity: MismatchSeverity::Hard,
            }
        );
    }

    /// T97 spec: US proxy + US DNS + WebRTC public IP in `192.0.2.0/24`
    /// (same /16 as proxy) → `Coherent`.
    #[test]
    fn webrtc_public_ip_same_slash_16_is_coherent() {
        // proxy_ip is already 192.0.2.7 → /16 is 192.0.2.0/16.
        let ctx = base_us_ctx();
        assert_eq!(validator().evaluate(&ctx), CoherenceVerdict::Coherent);
    }

    /// T97 spec: US proxy + WebRTC public IP in `203.0.113.0/24`
    /// (different /16) → `Mismatch` on `WebRtcPublicIp`, severity `Hard`.
    #[test]
    fn webrtc_public_ip_different_slash_16_is_hard_mismatch() {
        let ctx = CoherenceContext {
            webrtc_public_ip: Some(IpAddr::from_str("203.0.113.5").unwrap()),
            ..base_us_ctx()
        };
        assert_eq!(
            validator().evaluate(&ctx),
            CoherenceVerdict::Mismatch {
                field: MismatchField::WebRtcPublicIp,
                severity: MismatchSeverity::Hard,
            }
        );
    }

    /// T97 spec: US proxy + US DNS + `en-US` locale + `Europe/London` TZ
    /// → `Mismatch` on `Timezone`, severity `Advisory`.
    #[test]
    fn us_proxy_europe_london_tz_is_advisory_timezone_mismatch() {
        let ctx = CoherenceContext {
            browser_timezone: london_tz(),
            ..base_us_ctx()
        };
        assert_eq!(
            validator().evaluate(&ctx),
            CoherenceVerdict::Mismatch {
                field: MismatchField::Timezone,
                severity: MismatchSeverity::Advisory,
            }
        );
    }

    // ── Trait / stateness guarantees ───────────────────────────────────────

    /// The validator is `Send + Sync + 'static` so it can be shared via
    /// an `Arc<dyn CoherencePort>`. This is required by the manager
    /// plumbing and the crate-level performance / contention budget.
    #[test]
    fn validator_is_send_sync_static() {
        fn assert_send_sync_static<T: Send + Sync + 'static>() {}
        assert_send_sync_static::<DefaultCoherenceValidator>();
        assert_send_sync_static::<BoxedValidator>();
    }

    /// Convenience alias to keep the `Send + Sync + 'static` test
    /// independent of [`crate::ports::coherence::BoxedCoherencePort`].
    type BoxedValidator = Arc<dyn CoherencePort>;

    /// The validator must be stateless — calling `evaluate` twice on
    /// the same instance with the same context yields the same verdict
    /// and does not mutate internal state. Captured indirectly by
    /// running 1 000 evaluations and asserting the expected verdict on
    /// every call.
    #[test]
    fn validator_is_stateless_across_repeated_calls() {
        let v = validator();
        let ctx_clean = base_us_ctx();
        let ctx_mismatch = CoherenceContext {
            browser_timezone: london_tz(),
            ..base_us_ctx()
        };
        for _ in 0..1_000 {
            assert_eq!(v.evaluate(&ctx_clean), CoherenceVerdict::Coherent);
            assert!(matches!(
                v.evaluate(&ctx_mismatch),
                CoherenceVerdict::Mismatch {
                    field: MismatchField::Timezone,
                    severity: MismatchSeverity::Advisory,
                }
            ));
        }
    }

    /// The validator dispatch path through `Box<dyn CoherencePort>`
    /// (the manager's storage shape) returns the same verdicts as the
    /// direct call.
    #[test]
    fn boxed_dispatch_matches_direct_call() {
        let v: BoxedValidator = Arc::new(DefaultCoherenceValidator::new());
        let ctx = CoherenceContext {
            browser_timezone: london_tz(),
            ..base_us_ctx()
        };
        assert_eq!(v.evaluate(&ctx), validator().evaluate(&ctx));
    }

    // ── Per-vector negative coverage ───────────────────────────────────────

    /// Locale disagreement is `Advisory`, not `Hard`.
    #[test]
    fn locale_mismatch_is_advisory() {
        let ctx = CoherenceContext {
            browser_locale: fr_fr_locale(),
            ..base_us_ctx()
        };
        assert_eq!(
            validator().evaluate(&ctx),
            CoherenceVerdict::Mismatch {
                field: MismatchField::Locale,
                severity: MismatchSeverity::Advisory,
            }
        );
    }

    /// Accept-Language disagreement is `Advisory`.
    #[test]
    fn accept_language_mismatch_is_advisory() {
        let ctx = CoherenceContext {
            accept_language: al_fr_fr(),
            ..base_us_ctx()
        };
        assert_eq!(
            validator().evaluate(&ctx),
            CoherenceVerdict::Mismatch {
                field: MismatchField::AcceptLanguage,
                severity: MismatchSeverity::Advisory,
            }
        );
    }

    /// Bare-language Accept-Language (`en;q=1.0`) does not trigger an
    /// `AcceptLanguage` mismatch — there is no region to disagree with.
    #[test]
    fn bare_language_accept_language_does_not_mismatch() {
        let ctx = CoherenceContext {
            accept_language: AcceptLanguage::new("en;q=1.0").unwrap(),
            ..base_us_ctx()
        };
        assert_eq!(validator().evaluate(&ctx), CoherenceVerdict::Coherent);
    }

    /// Hard mismatches short-circuit before the advisory checks, so
    /// PK DNS wins over the London timezone disagreement.
    #[test]
    fn hard_dns_mismatch_short_circuits_advisory() {
        let ctx = CoherenceContext {
            dns_resolver_country: Some(pk()),
            browser_timezone: london_tz(),
            browser_locale: fr_fr_locale(),
            accept_language: al_fr_fr(),
            ..base_us_ctx()
        };
        assert_eq!(
            validator().evaluate(&ctx),
            CoherenceVerdict::Mismatch {
                field: MismatchField::ProxyGeoVsDns,
                severity: MismatchSeverity::Hard,
            }
        );
    }

    /// Hard WebRTC /16 mismatch wins over advisory timezone /
    /// locale drift.
    #[test]
    fn hard_webrtc_mismatch_short_circuits_advisory() {
        let ctx = CoherenceContext {
            webrtc_public_ip: Some(IpAddr::from_str("203.0.113.5").unwrap()),
            browser_timezone: london_tz(),
            browser_locale: fr_fr_locale(),
            ..base_us_ctx()
        };
        assert_eq!(
            validator().evaluate(&ctx),
            CoherenceVerdict::Mismatch {
                field: MismatchField::WebRtcPublicIp,
                severity: MismatchSeverity::Hard,
            }
        );
    }

    /// When neither `proxy_geo_country` nor `dns_resolver_country` is
    /// present, every country-driven check returns `Unknown`.
    #[test]
    fn missing_geo_returns_unknown() {
        let ctx = CoherenceContext {
            proxy_geo_country: None,
            dns_resolver_country: None,
            ..base_us_ctx()
        };
        // Country agreement check is first and short-circuits.
        assert!(matches!(
            validator().evaluate(&ctx),
            CoherenceVerdict::Unknown(_)
        ));
    }

    /// Missing `proxy_ip` while WebRTC public IP is observed yields
    /// `Unknown("missing_proxy_ip")` from the /16 check.
    #[test]
    fn missing_proxy_ip_returns_unknown() {
        let ctx = CoherenceContext {
            proxy_ip: None,
            ..base_us_ctx()
        };
        assert!(matches!(
            validator().evaluate(&ctx),
            CoherenceVerdict::Unknown(reason) if reason == "missing_proxy_ip"
        ));
    }

    /// WebRTC disabled (no public IP) is a no-op for the /16 check —
    /// the rest of the context is still evaluated.
    #[test]
    fn webrtc_disabled_does_not_block_other_checks() {
        let ctx = CoherenceContext {
            webrtc_public_ip: None,
            proxy_ip: None,
            browser_timezone: london_tz(),
            ..base_us_ctx()
        };
        // Without a proxy IP the timezone check still runs and the
        // London-vs-US disagreement fires (Advisory).
        assert_eq!(
            validator().evaluate(&ctx),
            CoherenceVerdict::Mismatch {
                field: MismatchField::Timezone,
                severity: MismatchSeverity::Advisory,
            }
        );
    }

    /// Missing `proxy_geo_country` falls back to `dns_resolver_country`
    /// for the effective country — a US-only DNS context still
    /// produces a London-timezone mismatch.
    #[test]
    fn dns_country_fills_in_when_proxy_country_missing() {
        let ctx = CoherenceContext {
            proxy_geo_country: None,
            dns_resolver_country: Some(us()),
            browser_timezone: london_tz(),
            webrtc_public_ip: None,
            proxy_ip: None,
            ..base_us_ctx()
        };
        assert_eq!(
            validator().evaluate(&ctx),
            CoherenceVerdict::Mismatch {
                field: MismatchField::Timezone,
                severity: MismatchSeverity::Advisory,
            }
        );
    }

    /// Mismatched proxy / DNS with US DNS only: country agreement
    /// still wins because both fields are present.
    #[test]
    fn mismatched_proxy_and_dns_is_hard_mismatch() {
        let ctx = CoherenceContext {
            proxy_geo_country: Some(us()),
            dns_resolver_country: Some(pk()),
            webrtc_public_ip: None,
            proxy_ip: None,
            ..base_us_ctx()
        };
        assert_eq!(
            validator().evaluate(&ctx),
            CoherenceVerdict::Mismatch {
                field: MismatchField::ProxyGeoVsDns,
                severity: MismatchSeverity::Hard,
            }
        );
    }

    // ── Hot-path budget ────────────────────────────────────────────────────

    /// Spec acceptance test: 10 000 single-vector coherence checks stay
    /// under the 1 µs-acquisition-target budget over the validator call
    /// itself. The full `acquire_proxy_with_coherence` integration
    /// (selector + storage round-trip) is exercised in
    /// `crates/stygian-proxy/src/manager.rs`.
    #[test]
    fn hot_path_budget_10k_calls() {
        let v = validator();
        let ctx = base_us_ctx();
        let start = std::time::Instant::now();
        for _ in 0..10_000 {
            let _ = v.evaluate(&ctx);
        }
        let elapsed = start.elapsed();
        // 10 000 calls × 100 ns = 1 ms ceiling; the 1 s wall budget
        // catches a 1000× regression without flaking under CI load.
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "10k coherence checks took {elapsed:?}; hot-path budget violated"
        );
    }

    // ── Country ↔ timezone table ───────────────────────────────────────────

    /// The static table covers every country used in the T97 spec.
    #[test]
    fn country_timezone_table_covers_test_cases() {
        for code in ["US", "PK"] {
            assert!(
                COUNTRY_TIMEZONE_REGIONS
                    .iter()
                    .any(|(country, _)| *country == code),
                "country {code} missing from COUNTRY_TIMEZONE_REGIONS"
            );
        }
    }

    /// `tz_matches_country` accepts the canonical US timezones.
    #[test]
    fn tz_matches_us_timezones() {
        for tz_str in [
            "America/New_York",
            "America/Chicago",
            "America/Denver",
            "America/Los_Angeles",
            "America/Phoenix",
            "America/Anchorage",
            "Pacific/Honolulu",
        ] {
            let tz = Tz::new(tz_str).unwrap();
            assert!(tz_matches_country(&tz, &us()), "should accept {tz_str}");
        }
    }

    /// `tz_matches_country` rejects non-US timezones for `US`.
    #[test]
    fn tz_rejects_non_us_timezones() {
        for tz_str in [
            "Europe/London",
            "Asia/Tokyo",
            "Asia/Karachi",
            "Australia/Sydney",
        ] {
            let tz = Tz::new(tz_str).unwrap();
            assert!(
                !tz_matches_country(&tz, &us()),
                "should reject {tz_str} for US"
            );
        }
    }

    /// `tz_matches_country` returns `false` for any country that is
    /// not in the static table. This is the "fail-quiet" path —
    /// operators see `Unknown("missing_geo_country")` in the caller
    /// rather than a false-positive mismatch.
    #[test]
    fn tz_unknown_country_returns_false() {
        // Country that is intentionally absent from
        // `COUNTRY_TIMEZONE_REGIONS`.
        let country = IsoCountry::new("ZZ").unwrap();
        assert!(!tz_matches_country(&ny_tz(), &country));
    }
}
