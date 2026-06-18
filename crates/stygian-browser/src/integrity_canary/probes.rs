//! Integrity probe catalogue.
//!
//! Defines the [`IntegrityProbe`] record, the [`IntegrityProbeId`]
//! taxonomy, the [`IntegrityProbeOutcome`] severity, and the
//! per-probe mitigation hint text.
//!
//! Probes are pure data (no I/O). Their JavaScript `script` fields
//! are designed to be sent verbatim to CDP `Runtime.evaluate` by
//! the consumer (browser automation), and the resulting JSON
//! output is decoded by [`IntegrityProbe::parse_output`] into a
//! [`ProbeFinding`].
//!
//! All scripts use the broadest-compatibility form (old-style
//! `var` / `function()` / `Array.prototype.slice.call`, no arrow
//! functions or template literals) so they run on the same browser
//! engines the existing stealth injection targets.

use serde::{Deserialize, Serialize};

/// Stable identifier for a built-in integrity probe.
///
/// New variants are additive — existing variants keep their
/// discriminant and `serde` label across releases so consumers can
/// safely branch on the wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityProbeId {
    /// `Object.getOwnPropertyDescriptor(Navigator.prototype, "webdriver")`
    /// must be an accessor (getter), not a data property. Patched
    /// stealth implementations frequently redefine `webdriver` as a
    /// data property.
    WebDriverDescriptorNative,
    /// `Function.prototype.toString` applied to a known native
    /// method must report `[native code]`. Patched natives that
    /// override `toString` without preserving the native code
    /// marker leak patch source.
    FunctionToStringNative,
    /// `(function(){}).toString()` must report `function anonymous() {
    /// [native code] }` — a clean function literal must look native.
    /// Patched frames occasionally return the wrapped function body
    /// instead of `[native code]`.
    ErrorToStringNative,
    /// `Intl.DateTimeFormat.prototype.format.toString()` must
    /// contain `[native code]`.
    IntlDateTimeFormatNative,
    /// `RegExp.prototype.test.toString()` must contain `[native code]`.
    RegExpTestNative,
    /// `CanvasRenderingContext2D.prototype.getImageData.toString()`
    /// must contain `[native code]`.
    CanvasGetImageDataNative,
    /// `performance.now()` resolution should be plausibly
    /// microsecond-scale and not quantized (e.g. values forced to
    /// 0.1 ms ticks).
    PerformanceNowResolution,
    /// `new Proxy({}, { ownKeys: () => [] })` must report no own
    /// keys — patched natives that trap `[[OwnPropertyKeys]]`
    /// through a `Proxy` leak the trap to detection scripts.
    ProxyTrapObservable,
}

impl IntegrityProbeId {
    /// Stable `snake_case` label used in telemetry.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::WebDriverDescriptorNative => "webdriver_descriptor_native",
            Self::FunctionToStringNative => "function_to_string_native",
            Self::ErrorToStringNative => "error_to_string_native",
            Self::IntlDateTimeFormatNative => "intl_date_time_format_native",
            Self::RegExpTestNative => "regexp_test_native",
            Self::CanvasGetImageDataNative => "canvas_get_image_data_native",
            Self::PerformanceNowResolution => "performance_now_resolution",
            Self::ProxyTrapObservable => "proxy_trap_observable",
        }
    }
}

impl std::fmt::Display for IntegrityProbeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Severity outcome of an integrity probe.
///
/// `Skipped` is **not** an error — it marks a probe that could not
/// run (the relevant API was not exposed on the page, e.g. an
/// opaque-origin iframe). Skipped probes contribute **zero** risk to
/// the aggregate score and are excluded from the denominator (so
/// partial probe coverage does not pull the score toward zero).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityProbeOutcome {
    /// Probe ran and the surface looks native / unpatched.
    Clean,
    /// Probe ran and the surface shows an ambiguous anomaly that
    /// is consistent with either a stealth patch or a browser
    /// polyfill.
    TrapSuspected,
    /// Probe ran and the surface shows a deterministic patch
    /// artefact (e.g. `webdriver` is a data property on
    /// `Navigator.prototype`).
    TrapConfirmed,
    /// Probe could not run (API not exposed, exception thrown,
    /// etc.). No panic, no risk contribution.
    Skipped,
}

impl IntegrityProbeOutcome {
    /// Stable `snake_case` label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::TrapSuspected => "trap_suspected",
            Self::TrapConfirmed => "trap_confirmed",
            Self::Skipped => "skipped",
        }
    }

    /// Severity multiplier used by the score formula:
    ///
    /// - `Clean`     → `0.0` (no risk contribution)
    /// - `TrapSuspected` → `0.5` (ambiguous signal)
    /// - `TrapConfirmed` → `1.0` (deterministic regression)
    /// - `Skipped`   → `0.0` AND excluded from the denominator
    ///
    /// Exposing this as a constant helper keeps the scoring formula
    /// in one place so future risk weighting stays consistent.
    #[must_use]
    pub const fn severity(self) -> f64 {
        match self {
            Self::Clean | Self::Skipped => 0.0,
            Self::TrapSuspected => 0.5,
            Self::TrapConfirmed => 1.0,
        }
    }

    /// `true` when the outcome counts toward the aggregate score
    /// (i.e. the probe actually ran and produced a verdict).
    #[must_use]
    pub const fn contributes(self) -> bool {
        !matches!(self, Self::Skipped)
    }
}

impl std::fmt::Display for IntegrityProbeOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Single integrity probe definition: stable id, weight, JS
/// evaluation script, description, and mitigation hint.
///
/// The catalogue lives in [`all_probes`]. The struct is `Clone` so
/// callers can build modified copies for testing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityProbe {
    /// Stable probe identifier.
    pub id: IntegrityProbeId,
    /// Weight in `[0.0, 1.0]` used by the aggregate score.
    pub weight: f64,
    /// Human-readable description (one short paragraph).
    pub description: &'static str,
    /// JavaScript evaluation script (verbatim). Returns a JSON
    /// string with shape
    /// `'{"outcome":"clean|trap_suspected|trap_confirmed|skipped","evidence":"..."}'`.
    pub script: &'static str,
    /// Actionable mitigation hint emitted in the diagnostic
    /// payload when the probe fires.
    pub mitigation_hint: &'static str,
}

impl IntegrityProbe {
    /// Build a finding representing a confirmed trap (test helper).
    #[must_use]
    pub fn confirmed_finding(
        id: impl Into<String>,
        weight: f64,
        evidence: impl Into<String>,
    ) -> ProbeFinding {
        ProbeFinding {
            id: id.into(),
            outcome: IntegrityProbeOutcome::TrapConfirmed,
            weight,
            evidence: evidence.into(),
            mitigation_hint: String::new(),
        }
    }

    /// Parse the JSON string returned by the probe's `script`.
    ///
    /// Returns a [`ProbeFinding`] with the recorded outcome and
    /// evidence. When JSON parsing fails, the result is mapped to a
    /// [`IntegrityProbeOutcome::Skipped`] finding carrying the raw
    /// output as evidence — this is the conservative fallback that
    /// keeps the report deterministic under parse failures.
    #[must_use]
    pub fn parse_output(&self, json: &str) -> ProbeFinding {
        #[derive(Deserialize)]
        struct Output {
            outcome: String,
            #[serde(default)]
            evidence: String,
        }
        match serde_json::from_str::<Output>(json) {
            Ok(o) => {
                let outcome = match o.outcome.as_str() {
                    "clean" => IntegrityProbeOutcome::Clean,
                    "trap_suspected" => IntegrityProbeOutcome::TrapSuspected,
                    "trap_confirmed" => IntegrityProbeOutcome::TrapConfirmed,
                    _ => IntegrityProbeOutcome::Skipped,
                };
                ProbeFinding {
                    id: self.id.label().to_string(),
                    outcome,
                    weight: self.weight,
                    evidence: o.evidence,
                    mitigation_hint: self.mitigation_hint.to_string(),
                }
            }
            Err(err) => ProbeFinding {
                id: self.id.label().to_string(),
                outcome: IntegrityProbeOutcome::Skipped,
                weight: self.weight,
                evidence: format!("parse error: {err} | raw: {json}"),
                mitigation_hint: self.mitigation_hint.to_string(),
            },
        }
    }
}

/// Captured result of a single integrity probe evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeFinding {
    /// Probe identifier (`snake_case` label).
    pub id: String,
    /// Resolved outcome.
    pub outcome: IntegrityProbeOutcome,
    /// Recorded weight at evaluation time.
    pub weight: f64,
    /// Free-form evidence returned by the JavaScript evaluation
    /// (e.g. `"function webdriver() {}"` for a patched `webdriver`
    /// accessor).
    pub evidence: String,
    /// Per-probe mitigation hint copied from the catalogue. Empty
    /// when the catalogue entry has no hint.
    pub mitigation_hint: String,
}

impl ProbeFinding {
    /// `true` when the probe fired with a [`IntegrityProbeOutcome::TrapSuspected`]
    /// or [`IntegrityProbeOutcome::TrapConfirmed`] outcome.
    #[must_use]
    pub const fn is_trap(&self) -> bool {
        matches!(
            self.outcome,
            IntegrityProbeOutcome::TrapSuspected | IntegrityProbeOutcome::TrapConfirmed
        )
    }

    /// `true` when the probe reported a confirmed trap.
    #[must_use]
    pub const fn is_confirmed(&self) -> bool {
        matches!(self.outcome, IntegrityProbeOutcome::TrapConfirmed)
    }

    /// Numeric contribution of this finding to the aggregate risk
    /// score (weight × severity). Returns `0.0` for skipped findings.
    #[must_use]
    pub fn contribution(&self) -> f64 {
        if !self.outcome.contributes() {
            return 0.0;
        }
        self.weight * self.outcome.severity()
    }
}

// ─── Built-in probe scripts ──────────────────────────────────────────────────

const SCRIPT_WEBDRIVER_DESCRIPTOR: &str = concat!(
    "(function(){",
    "var d=Object.getOwnPropertyDescriptor(Navigator.prototype,'webdriver');",
    "if(typeof d==='undefined'){return JSON.stringify({outcome:'clean',evidence:'no descriptor'});}",
    "if(typeof d.get!=='function'||typeof d.set!=='undefined'){",
    "return JSON.stringify({outcome:'trap_confirmed',",
    "evidence:'descriptor is data property (get='+typeof d.get+', set='+typeof d.set+')'});",
    "}",
    "if(d.configurable===false&&d.enumerable===false){",
    "return JSON.stringify({outcome:'trap_suspected',",
    "evidence:'accessor present but non-configurable (configurable='+String(d.configurable)+')'});",
    "}",
    "return JSON.stringify({outcome:'clean',evidence:'accessor present and configurable'});",
    "})()"
);

const SCRIPT_FUNCTION_TO_STRING: &str = concat!(
    "(function(){",
    "var native='function () { [native code] }';",
    "var keys=['getTime','now','random','test','format'];",
    "var host=typeof Intl!=='undefined'?Intl.DateTimeFormat.prototype:null;",
    "if(!host||typeof host.format!=='function'){",
    "return JSON.stringify({outcome:'skipped',evidence:'Intl.DateTimeFormat.format unavailable'});",
    "}",
    "var s=Function.prototype.toString.call(host.format);",
    "if(s.indexOf('[native code]')===-1){",
    "return JSON.stringify({outcome:'trap_confirmed',evidence:s.substring(0,80)});",
    "}",
    "return JSON.stringify({outcome:'clean',evidence:'[native code] marker present'});",
    "})()"
);

const SCRIPT_ERROR_TO_STRING: &str = concat!(
    "(function(){",
    "var s=Function.prototype.toString.call(function(){});",
    "if(s.indexOf('[native code]')!==-1){",
    "return JSON.stringify({outcome:'clean',evidence:'[native code] marker present'});",
    "}",
    "if(s.substring(0,8)==='function'){",
    "return JSON.stringify({outcome:'trap_suspected',evidence:s.substring(0,80)});",
    "}",
    "return JSON.stringify({outcome:'trap_confirmed',evidence:s.substring(0,80)});",
    "})()"
);

const SCRIPT_INTL_DATE_TIME_FORMAT: &str = concat!(
    "(function(){",
    "if(typeof Intl==='undefined'||typeof Intl.DateTimeFormat==='undefined'){",
    "return JSON.stringify({outcome:'skipped',evidence:'Intl.DateTimeFormat unavailable'});",
    "}",
    "var s=Function.prototype.toString.call(Intl.DateTimeFormat.prototype.format);",
    "if(s.indexOf('[native code]')===-1){",
    "return JSON.stringify({outcome:'trap_confirmed',evidence:s.substring(0,80)});",
    "}",
    "return JSON.stringify({outcome:'clean',evidence:'[native code] marker present'});",
    "})()"
);

const SCRIPT_REGEXP_TEST: &str = concat!(
    "(function(){",
    "var s=Function.prototype.toString.call(RegExp.prototype.test);",
    "if(s.indexOf('[native code]')===-1){",
    "return JSON.stringify({outcome:'trap_confirmed',evidence:s.substring(0,80)});",
    "}",
    "return JSON.stringify({outcome:'clean',evidence:'[native code] marker present'});",
    "})()"
);

const SCRIPT_CANVAS_GET_IMAGE_DATA: &str = concat!(
    "(function(){",
    "var s=Function.prototype.toString.call(CanvasRenderingContext2D.prototype.getImageData);",
    "if(s.indexOf('[native code]')===-1){",
    "return JSON.stringify({outcome:'trap_confirmed',evidence:s.substring(0,80)});",
    "}",
    "return JSON.stringify({outcome:'clean',evidence:'[native code] marker present'});",
    "})()"
);

const SCRIPT_PERFORMANCE_NOW_RESOLUTION: &str = concat!(
    "(function(){",
    "if(typeof performance==='undefined'||typeof performance.now!=='function'){",
    "return JSON.stringify({outcome:'skipped',evidence:'performance.now unavailable'});",
    "}",
    "var samples=[];",
    "for(var i=0;i<20;i++){samples.push(performance.now());}",
    "var deltas=[];",
    "for(var j=1;j<samples.length;j++){deltas.push(samples[j]-samples[j-1]);}",
    "deltas.sort(function(a,b){return a-b;});",
    "var median=deltas[Math.floor(deltas.length/2)];",
    "if(median<=0){return JSON.stringify({outcome:'skipped',evidence:'zero deltas'});}",
    "var ratio=median/Math.round(median);",
    "if(Math.abs(ratio-1)>0.05&&median>0.05){",
    "return JSON.stringify({outcome:'trap_suspected',",
    "evidence:'median delta='+median.toFixed(4)+'ms deviates from a clean tick'});",
    "}",
    "if(median>=5){",
    "return JSON.stringify({outcome:'trap_suspected',",
    "evidence:'median delta='+median.toFixed(4)+'ms looks quantized'});",
    "}",
    "return JSON.stringify({outcome:'clean',evidence:'median delta='+median.toFixed(4)+'ms'});",
    "})()"
);

const SCRIPT_PROXY_TRAP_OBSERVABLE: &str = concat!(
    "(function(){",
    "var keys;",
    "try{",
    "keys=Object.keys(new Proxy({},{ownKeys:function(){return ['__patched__'];}}));",
    "}catch(e){",
    "return JSON.stringify({outcome:'skipped',evidence:'Proxy unavailable: '+String(e)});",
    "}",
    "if(keys.length===0){",
    "return JSON.stringify({outcome:'clean',evidence:'proxy ownKeys trap observable (expected)'});",
    "}",
    "if(keys.indexOf('__patched__')!==-1){",
    "return JSON.stringify({outcome:'trap_suspected',",
    "evidence:'proxy ownKeys trap returns custom keys: '+JSON.stringify(keys)});",
    "}",
    "return JSON.stringify({outcome:'clean',evidence:'proxy keys='+JSON.stringify(keys)});",
    "})()"
);

// ─── Probe catalogue ─────────────────────────────────────────────────────────

/// Built-in integrity probe catalogue.
///
/// Each probe has a stable id, a weight in `[0.0, 1.0]`, a
/// human-readable description, a self-contained JavaScript
/// evaluation script, and a per-probe mitigation hint. The total
/// weight of the catalogue is `1.0` so the aggregate score is a
/// weighted average in `[0.0, 1.0]`.
pub static PROBES: &[IntegrityProbe] = &[
    IntegrityProbe {
        id: IntegrityProbeId::WebDriverDescriptorNative,
        weight: 0.20,
        description: "Navigator.prototype.webdriver must be an accessor descriptor (getter + no setter)",
        script: SCRIPT_WEBDRIVER_DESCRIPTOR,
        mitigation_hint: "Re-define navigator.webdriver via Object.defineProperty with a native-shaped accessor (configurable: true, enumerable: false, getter returns false). Avoid data-property overrides that leak the patch artefact.",
    },
    IntegrityProbe {
        id: IntegrityProbeId::FunctionToStringNative,
        weight: 0.18,
        description: "Function.prototype.toString must report [native code] for native methods",
        script: SCRIPT_FUNCTION_TO_STRING,
        mitigation_hint: "Preserve the [native code] marker on patched prototype methods (use Object.defineProperty with the native function as the value, not a wrapper). If a polyfill is unavoidable, override toString to return the canonical 'function name() { [native code] }' shape.",
    },
    IntegrityProbe {
        id: IntegrityProbeId::ErrorToStringNative,
        weight: 0.08,
        description: "(function(){}).toString() must look native",
        script: SCRIPT_ERROR_TO_STRING,
        mitigation_hint: "Avoid wrapping function literals in proxy / decorator chains that intercept toString. Browser vendors intentionally return 'function () { [native code] }' for empty literals.",
    },
    IntegrityProbe {
        id: IntegrityProbeId::IntlDateTimeFormatNative,
        weight: 0.10,
        description: "Intl.DateTimeFormat.prototype.format must be a native function",
        script: SCRIPT_INTL_DATE_TIME_FORMAT,
        mitigation_hint: "Do not monkey-patch Intl.DateTimeFormat.prototype.format — anti-bot scripts probe this surface explicitly. Override at the call site instead (use a wrapper around Date.prototype.toLocaleString).",
    },
    IntegrityProbe {
        id: IntegrityProbeId::RegExpTestNative,
        weight: 0.08,
        description: "RegExp.prototype.test must be a native function",
        script: SCRIPT_REGEXP_TEST,
        mitigation_hint: "Avoid replacing RegExp.prototype.test with a wrapper. If pattern instrumentation is required, do it via a custom regex helper function rather than prototype mutation.",
    },
    IntegrityProbe {
        id: IntegrityProbeId::CanvasGetImageDataNative,
        weight: 0.10,
        description: "CanvasRenderingContext2D.prototype.getImageData must be a native function",
        script: SCRIPT_CANVAS_GET_IMAGE_DATA,
        mitigation_hint: "Apply canvas fingerprint noise at the pixel-data layer (post getImageData) rather than by overriding getImageData itself. Vendors fingerprint the descriptor shape first.",
    },
    IntegrityProbe {
        id: IntegrityProbeId::PerformanceNowResolution,
        weight: 0.14,
        description: "performance.now() resolution must look plausible (microsecond-scale, not quantized)",
        script: SCRIPT_PERFORMANCE_NOW_RESOLUTION,
        mitigation_hint: "Replace timing-noise quantization with a continuous distribution (Gaussian jitter, stddev ~5-25 µs) or apply noise at the consumer layer (performance.now callers) rather than patching performance.now itself.",
    },
    IntegrityProbe {
        id: IntegrityProbeId::ProxyTrapObservable,
        weight: 0.12,
        description: "Proxy ownKeys trap must not leak surface state on patched natives",
        script: SCRIPT_PROXY_TRAP_OBSERVABLE,
        mitigation_hint: "When wrapping native objects, return the canonical ownKeys list ([] for empty wrappers) and never expose a 'patched' sentinel key through the trap. Detection scripts diff the trap output against the real underlying object.",
    },
];

/// Return the full built-in integrity probe catalogue.
#[must_use]
pub fn all_probes() -> &'static [IntegrityProbe] {
    PROBES
}

/// Look up a probe by its stable identifier.
///
/// Returns `None` when the id is unknown — this lets callers branch
/// safely on `serde_json::from_str` results without panicking.
#[must_use]
pub fn probe_by_id(id: IntegrityProbeId) -> Option<&'static IntegrityProbe> {
    PROBES.iter().find(|p| p.id == id)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn probe_ids_have_stable_labels() {
        assert_eq!(
            IntegrityProbeId::WebDriverDescriptorNative.label(),
            "webdriver_descriptor_native"
        );
        assert_eq!(
            IntegrityProbeId::PerformanceNowResolution.label(),
            "performance_now_resolution"
        );
    }

    #[test]
    fn outcome_labels_are_stable() {
        assert_eq!(IntegrityProbeOutcome::Clean.label(), "clean");
        assert_eq!(IntegrityProbeOutcome::TrapSuspected.label(), "trap_suspected");
        assert_eq!(IntegrityProbeOutcome::TrapConfirmed.label(), "trap_confirmed");
        assert_eq!(IntegrityProbeOutcome::Skipped.label(), "skipped");
    }

    #[test]
    fn outcome_severity_matches_documented_formula() {
        assert!(approx_eq(IntegrityProbeOutcome::Clean.severity(), 0.0));
        assert!(approx_eq(IntegrityProbeOutcome::TrapSuspected.severity(), 0.5));
        assert!(approx_eq(IntegrityProbeOutcome::TrapConfirmed.severity(), 1.0));
        assert!(approx_eq(IntegrityProbeOutcome::Skipped.severity(), 0.0));
    }

    #[test]
    fn outcome_contributes_excludes_skipped() {
        assert!(IntegrityProbeOutcome::Clean.contributes());
        assert!(IntegrityProbeOutcome::TrapSuspected.contributes());
        assert!(IntegrityProbeOutcome::TrapConfirmed.contributes());
        assert!(!IntegrityProbeOutcome::Skipped.contributes());
    }

    #[test]
    fn probe_catalogue_has_eight_entries() {
        assert_eq!(all_probes().len(), 8);
    }

    #[test]
    fn probe_catalogue_has_unique_ids() {
        let mut seen = std::collections::HashSet::new();
        for probe in all_probes() {
            assert!(seen.insert(probe.id), "duplicate probe id: {:?}", probe.id);
        }
    }

    #[test]
    fn probe_weights_sum_to_one() {
        let total: f64 = all_probes().iter().map(|p| p.weight).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "total probe weight must be 1.0, got: {total}"
        );
    }

    #[test]
    fn probe_weights_are_in_unit_interval() {
        for probe in all_probes() {
            assert!(
                (0.0..=1.0).contains(&probe.weight),
                "probe {:?} weight {} outside [0.0, 1.0]",
                probe.id,
                probe.weight
            );
        }
    }

    #[test]
    fn probe_scripts_are_non_empty_and_emit_json() {
        for probe in all_probes() {
            assert!(!probe.script.is_empty());
            assert!(probe.script.contains("JSON.stringify"));
        }
    }

    #[test]
    fn probe_mitigation_hints_are_non_empty() {
        for probe in all_probes() {
            assert!(
                !probe.mitigation_hint.is_empty(),
                "probe {:?} has empty mitigation_hint",
                probe.id
            );
            assert!(
                probe.mitigation_hint.len() >= 40,
                "probe {:?} mitigation_hint is suspiciously short",
                probe.id
            );
        }
    }

    #[test]
    fn parse_output_clean_passing_json() {
        let probe = &all_probes()[0]; // WebDriverDescriptorNative
        let finding = probe.parse_output(
            r#"{"outcome":"clean","evidence":"accessor present and configurable"}"#,
        );
        assert_eq!(finding.id, "webdriver_descriptor_native");
        assert_eq!(finding.outcome, IntegrityProbeOutcome::Clean);
        assert_eq!(finding.evidence, "accessor present and configurable");
        assert!(!finding.is_trap());
        assert!(!finding.is_confirmed());
    }

    #[test]
    fn parse_output_confirmed_trap_json() {
        let probe = &all_probes()[0];
        let finding = probe.parse_output(
            r#"{"outcome":"trap_confirmed","evidence":"descriptor is data property"}"#,
        );
        assert_eq!(finding.outcome, IntegrityProbeOutcome::TrapConfirmed);
        assert!(finding.is_trap());
        assert!(finding.is_confirmed());
        assert!(approx_eq(finding.contribution(), probe.weight));
    }

    #[test]
    fn parse_output_suspected_trap_json() {
        let probe = &all_probes()[0];
        let finding = probe.parse_output(r#"{"outcome":"trap_suspected","evidence":"unclear"}"#);
        assert_eq!(finding.outcome, IntegrityProbeOutcome::TrapSuspected);
        assert!(finding.is_trap());
        assert!(!finding.is_confirmed());
        assert!((finding.contribution() - probe.weight * 0.5).abs() < 1e-9);
    }

    #[test]
    fn parse_output_skipped_json() {
        let probe = &all_probes()[0];
        let finding = probe.parse_output(r#"{"outcome":"skipped","evidence":"unavailable"}"#);
        assert_eq!(finding.outcome, IntegrityProbeOutcome::Skipped);
        assert!(!finding.is_trap());
        assert!(approx_eq(finding.contribution(), 0.0));
    }

    #[test]
    fn parse_output_invalid_json_returns_skipped_with_raw() {
        let probe = &all_probes()[0];
        let finding = probe.parse_output("not json at all");
        assert_eq!(finding.outcome, IntegrityProbeOutcome::Skipped);
        assert!(finding.evidence.contains("parse error"));
        assert!(finding.evidence.contains("not json at all"));
    }

    #[test]
    fn parse_output_unknown_outcome_label_returns_skipped() {
        let probe = &all_probes()[0];
        let finding = probe.parse_output(r#"{"outcome":"mystery","evidence":"?"}"#);
        assert_eq!(finding.outcome, IntegrityProbeOutcome::Skipped);
    }

    #[test]
    fn parse_output_copies_mitigation_hint_from_catalogue() {
        let probe = &all_probes()[0];
        let finding = probe.parse_output(r#"{"outcome":"trap_confirmed","evidence":"x"}"#);
        assert_eq!(finding.mitigation_hint, probe.mitigation_hint);
    }

    #[test]
    fn probe_by_id_resolves_known_ids() {
        for probe in all_probes() {
            assert_eq!(probe_by_id(probe.id).map(|p| p.id), Some(probe.id));
        }
    }

    #[test]
    fn confirmed_finding_helper_uses_provided_weight() {
        let f = IntegrityProbe::confirmed_finding("test_probe", 0.25, "evidence");
        assert_eq!(f.id, "test_probe");
        assert!(approx_eq(f.weight, 0.25));
        assert_eq!(f.outcome, IntegrityProbeOutcome::TrapConfirmed);
        assert!(f.is_confirmed());
    }

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }
}