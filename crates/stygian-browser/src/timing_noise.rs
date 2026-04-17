//! Performance timing noise injection.
//!
//! Injects deterministic jitter into `performance.now()`, `performance.timeOrigin`,
//! `Date.now()`, and `performance.getEntries*()` to break hardware-speed and
//! headless-detection timing fingerprints.
//!
//! Monotonicity of `performance.now()` is preserved — the wrapped function never
//! returns a value lower than its previous call.
//!
//! # Example
//!
//! ```
//! use stygian_browser::timing_noise::{timing_noise_script, TimingNoiseConfig};
//! use stygian_browser::noise::NoiseSeed;
//!
//! let cfg = TimingNoiseConfig { enabled: true, jitter_ms: 0.3, seed: NoiseSeed::from(1_u64) };
//! let js = timing_noise_script(&cfg);
//! assert!(js.contains("performance.now"));
//! assert!(js.contains("__stygian_time_offset"));
//! ```

use serde::{Deserialize, Serialize};

use crate::noise::{NoiseEngine, NoiseSeed};

// ---------------------------------------------------------------------------
// TimingNoiseConfig
// ---------------------------------------------------------------------------

/// Configuration for performance timing noise.
///
/// # Example
///
/// ```
/// use stygian_browser::timing_noise::TimingNoiseConfig;
/// use stygian_browser::noise::NoiseSeed;
///
/// let cfg = TimingNoiseConfig::default();
/// assert!(cfg.enabled);
/// assert!(cfg.jitter_ms > 0.0 && cfg.jitter_ms <= 1.0);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimingNoiseConfig {
    /// Whether timing noise is injected at all.
    pub enabled: bool,
    /// Maximum timing jitter in milliseconds (recommended: 0.1–0.5).
    pub jitter_ms: f64,
    /// Noise seed used to derive timing noise values.
    pub seed: NoiseSeed,
}

impl Default for TimingNoiseConfig {
    /// Enabled with 0.3 ms max jitter and a random seed.
    fn default() -> Self {
        Self {
            enabled: true,
            jitter_ms: 0.3,
            seed: NoiseSeed::random(),
        }
    }
}

// ---------------------------------------------------------------------------
// Script generator
// ---------------------------------------------------------------------------

/// Generate the timing noise injection script for `config`.
///
/// Returns an empty string when `config.enabled` is false.
///
/// # Example
///
/// ```
/// use stygian_browser::timing_noise::{timing_noise_script, TimingNoiseConfig};
/// use stygian_browser::noise::NoiseSeed;
///
/// let cfg = TimingNoiseConfig { enabled: false, jitter_ms: 0.3, seed: NoiseSeed::from(1_u64) };
/// assert!(timing_noise_script(&cfg).is_empty());
///
/// let cfg2 = TimingNoiseConfig { enabled: true, jitter_ms: 0.3, seed: NoiseSeed::from(1_u64) };
/// let js = timing_noise_script(&cfg2);
/// assert!(js.contains("performance.now"));
/// ```
#[must_use]
pub fn timing_noise_script(config: &TimingNoiseConfig) -> String {
    if !config.enabled {
        return String::new();
    }

    let engine = NoiseEngine::new(config.seed);
    let noise_fn = engine.js_noise_fn();
    let jitter_ms = config.jitter_ms;

    // A fixed time-origin shift derived from the seed: between ±10 ms.
    // This prevents cross-tab correlation via performance.timeOrigin.
    let origin_shift = {
        let h = engine.float_noise("timing.origin", 0);
        // float_noise is in [-1e-5, 1e-5]; scale to [-10, 10] ms
        h * 1_000_000_000.0 // * 1e9 → [-10, 10] ms ballpark (capped below)
    };
    // Clamp to [-10, 10]
    let origin_shift_ms = origin_shift.clamp(-10.0, 10.0);

    format!(
        r"(function() {{
  'use strict';

  // ── Noise helpers ──────────────────────────────────────────────────────
  {noise_fn}

  const _JITTER_MS = {jitter_ms};
  // Fixed origin shift for this session (±{origin_shift_ms:.4} ms)
  const _ORIGIN_SHIFT = {origin_shift_ms:.6};

  // ── performance.now() — monotonic jitter accumulator ──────────────────
  let __stygian_time_offset = 0.0;
  let __stygian_pnow_counter = 0;
  let __stygian_pnow_last = 0.0;

  const _origPerfNow = performance.now.bind(performance);

  Object.defineProperty(performance, 'now', {{
    value: function now() {{
      const base = _origPerfNow();
      const noiseFraction = __stygian_float_noise('timing.now', __stygian_pnow_counter++);
      // noiseFraction is in [-1e-5, 1e-5]; scale to [-jitter_ms/2, jitter_ms/2]
      const delta = noiseFraction * (_JITTER_MS * 50000.0);
      // Accumulate only positive deltas to keep monotonicity
      const positive = Math.max(0.0, delta);
      __stygian_time_offset += positive;
      const result = Math.max(__stygian_pnow_last, base + __stygian_time_offset);
      __stygian_pnow_last = result;
      return result;
    }},
    writable: false,
    configurable: false,
    enumerable: true,
  }});

  // ── performance.timeOrigin — fixed per-session shift ──────────────────
  const _origTimeOrigin = performance.timeOrigin;
  Object.defineProperty(performance, 'timeOrigin', {{
    get: function() {{ return _origTimeOrigin + _ORIGIN_SHIFT; }},
    configurable: false,
    enumerable: true,
  }});

  // ── Date.now() — apply same origin shift ─────────────────────────────
  const _origDateNow = Date.now.bind(Date);
  (function() {{
    const shifted = function now() {{
      return _origDateNow() + _ORIGIN_SHIFT;
    }};
    shifted.toString = function toString() {{ return 'function now() {{ [native code] }}'; }};
    try {{
      Date.now = shifted;
    }} catch(e) {{
      Object.defineProperty(Date, 'now', {{
        value: shifted, writable: false, configurable: false, enumerable: false
      }});
    }}
  }})();

  // ── performance.getEntries* — noise on timing fields ─────────────────
  function _noiseEntry(entry, idx) {{
    const delta = __stygian_float_noise('timing.entry', idx) * (_JITTER_MS * 50000.0);
    // Preserve ordering: only add positive deltas
    const d = Math.abs(delta);
    // Build a plain-object copy with shifted timings; preserve startTime ordering
    return {{
      name: entry.name,
      entryType: entry.entryType,
      startTime: entry.startTime + d,
      duration: entry.duration,
      // Resource / Navigation fields (may be undefined on other entry types)
      // We only copy defined fields to avoid breaking typed PerformanceEntry comparisons
      toJSON: function() {{
        const j = entry.toJSON ? entry.toJSON() : {{}};
        j.startTime = entry.startTime + d;
        return j;
      }},
    }};
  }}

  const _origGetEntries = performance.getEntries.bind(performance);
  Object.defineProperty(performance, 'getEntries', {{
    value: function getEntries() {{
      return _origGetEntries().map(function(e, i) {{ return _noiseEntry(e, i); }});
    }},
    writable: false, configurable: false, enumerable: true,
  }});

  const _origGetEntriesByType = performance.getEntriesByType.bind(performance);
  Object.defineProperty(performance, 'getEntriesByType', {{
    value: function getEntriesByType(type) {{
      return _origGetEntriesByType(type).map(function(e, i) {{ return _noiseEntry(e, i); }});
    }},
    writable: false, configurable: false, enumerable: true,
  }});

  const _origGetEntriesByName = performance.getEntriesByName.bind(performance);
  Object.defineProperty(performance, 'getEntriesByName', {{
    value: function getEntriesByName(name, type) {{
      const args = type !== undefined ? [name, type] : [name];
      return _origGetEntriesByName.apply(performance, args)
        .map(function(e, i) {{ return _noiseEntry(e, i); }});
    }},
    writable: false, configurable: false, enumerable: true,
  }});

}})();
",
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::NoiseSeed;

    fn cfg(enabled: bool, jitter: f64, seed: u64) -> TimingNoiseConfig {
        TimingNoiseConfig {
            enabled,
            jitter_ms: jitter,
            seed: NoiseSeed::from(seed),
        }
    }

    #[test]
    fn disabled_returns_empty() {
        assert!(timing_noise_script(&cfg(false, 0.3, 1)).is_empty());
    }

    #[test]
    fn script_overrides_perf_now() {
        let js = timing_noise_script(&cfg(true, 0.3, 1));
        assert!(
            js.contains("performance.now"),
            "missing performance.now override"
        );
    }

    #[test]
    fn script_overrides_time_origin() {
        let js = timing_noise_script(&cfg(true, 0.3, 1));
        assert!(js.contains("timeOrigin"), "missing timeOrigin override");
    }

    #[test]
    fn script_overrides_date_now() {
        let js = timing_noise_script(&cfg(true, 0.3, 1));
        assert!(js.contains("Date.now"), "missing Date.now override");
    }

    #[test]
    fn script_overrides_get_entries() {
        let js = timing_noise_script(&cfg(true, 0.3, 1));
        assert!(js.contains("getEntries"), "missing getEntries override");
        assert!(js.contains("getEntriesByType"), "missing getEntriesByType");
        assert!(js.contains("getEntriesByName"), "missing getEntriesByName");
    }

    #[test]
    fn script_has_monotonicity_accumulator() {
        let js = timing_noise_script(&cfg(true, 0.3, 1));
        assert!(
            js.contains("__stygian_time_offset"),
            "missing monotonicity accumulator"
        );
    }

    #[test]
    fn default_jitter_in_reasonable_range() {
        let c = TimingNoiseConfig::default();
        assert!(
            c.jitter_ms >= 0.01 && c.jitter_ms <= 1.0,
            "jitter_ms out of range"
        );
    }

    #[test]
    fn serde_round_trip() {
        let c = TimingNoiseConfig {
            enabled: true,
            jitter_ms: 0.25,
            seed: NoiseSeed::from(98765_u64),
        };
        let json_result = serde_json::to_string(&c);
        assert!(json_result.is_ok(), "serialize failed: {json_result:?}");
        let Ok(json) = json_result else {
            return;
        };
        let cfg_result: Result<TimingNoiseConfig, _> = serde_json::from_str(&json);
        assert!(cfg_result.is_ok(), "deserialize failed: {cfg_result:?}");
        let Ok(c2) = cfg_result else {
            return;
        };
        assert_eq!(c2.enabled, c.enabled);
        assert!((c2.jitter_ms - c.jitter_ms).abs() < f64::EPSILON);
        assert_eq!(c2.seed.as_u64(), c.seed.as_u64());
    }

    #[test]
    fn different_seeds_produce_different_scripts() {
        let js1 = timing_noise_script(&cfg(true, 0.3, 1));
        let js2 = timing_noise_script(&cfg(true, 0.3, 2));
        assert_ne!(js1, js2);
    }
}
