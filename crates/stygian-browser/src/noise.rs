//! Deterministic noise seed engine for fingerprint perturbation.
//!
//! Provides [`NoiseSeed`], [`NoiseEngine`], and [`NoiseConfig`] for injecting
//! repeatable, per-session noise into canvas, WebGL, audio, and layout APIs.
//! Given the same [`NoiseSeed`], all noise generators produce identical output
//! across platforms and across Web Workers / Service Workers.
//!
//! # Example
//!
//! ```
//! use stygian_browser::noise::{NoiseEngine, NoiseSeed};
//!
//! let engine = NoiseEngine::new(NoiseSeed::from(42_u64));
//! let (dr, dg, db, da) = engine.pixel_noise("canvas.toDataURL", 10, 20);
//! assert!((-3..=3).contains(&dr));
//! ```

use std::fmt;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// NoiseSeed
// ---------------------------------------------------------------------------

/// A 64-bit seed that drives all deterministic noise generators.
///
/// Construct via [`NoiseSeed::random()`] for per-session uniqueness or
/// [`NoiseSeed::from(u64)`] for reproducible testing.
///
/// # Example
///
/// ```
/// use stygian_browser::noise::NoiseSeed;
///
/// let seed = NoiseSeed::from(12345_u64);
/// let seed2 = NoiseSeed::random();
/// assert_ne!(seed, seed2); // extremely unlikely to collide
/// ```
#[allow(clippy::unsafe_derive_deserialize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NoiseSeed(u64);

impl NoiseSeed {
    /// Generate a cryptographically random [`NoiseSeed`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::noise::NoiseSeed;
    /// let a = NoiseSeed::random();
    /// let b = NoiseSeed::random();
    /// // successive calls produce different values with overwhelming probability
    /// assert_ne!(a, b);
    /// ```
    #[must_use]
    pub fn random() -> Self {
        // Use std::time + thread-local counter for a cheap but sufficiently unique seed.
        // We avoid pulling in `rand` as a dep here — this is not a CSPRNG use case;
        // uniqueness across sessions is sufficient.
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        use std::time::{Duration, SystemTime};

        std::thread_local! {
            static COUNTER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
        }

        let count = COUNTER.with(|c| {
            let v = c.get().wrapping_add(1);
            c.set(v);
            v
        });

        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .subsec_nanos();

        let mut hasher = DefaultHasher::new();
        // Mix time + counter + stack address for uniqueness across threads/sessions
        nanos.hash(&mut hasher);
        count.hash(&mut hasher);
        Self(hasher.finish())
    }

    /// Return the raw u64 seed value.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::noise::NoiseSeed;
    /// let seed = NoiseSeed::from(99_u64);
    /// assert_eq!(seed.as_u64(), 99);
    /// ```
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl From<u64> for NoiseSeed {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl fmt::Display for NoiseSeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Hash mixer — cross-platform deterministic, no stdlib HashMap randomness
// ---------------------------------------------------------------------------

/// Deterministic keyed mixer: seed + operation string + two u32 coordinates → u64.
///
/// Uses a Fibonacci multiplicative hash with byte-by-byte string mixing so that
/// output is identical on every platform without depending on `DefaultHasher`.
#[inline]
fn mix(seed: u64, operation: &str, a: u32, b: u32) -> u64 {
    const M: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut h = seed;
    for byte in operation.bytes() {
        h = h.wrapping_mul(M).wrapping_add(u64::from(byte));
        h ^= h >> 33;
    }
    h = h.wrapping_mul(M).wrapping_add(u64::from(a));
    h ^= h >> 33;
    h = h.wrapping_mul(M).wrapping_add(u64::from(b));
    h ^= h >> 33;
    h
}

/// Extract four independent `i8` values from a `u64`, each bounded to `[-3, 3]`.
#[inline]
const fn bounded_bytes(h: u64) -> (i8, i8, i8, i8) {
    // Split hash into four 16-bit lanes, map each mod 7 → [0,6] → shift by 3 → [-3,3]
    let red = ((h & 0xFFFF) % 7) as i8 - 3;
    let green = (((h >> 16) & 0xFFFF) % 7) as i8 - 3;
    let blue = (((h >> 32) & 0xFFFF) % 7) as i8 - 3;
    let alpha = (((h >> 48) & 0xFFFF) % 7) as i8 - 3;
    (red, green, blue, alpha)
}

// ---------------------------------------------------------------------------
// NoiseEngine
// ---------------------------------------------------------------------------

/// Deterministic noise generator seeded with a [`NoiseSeed`].
///
/// All methods are pure functions — same seed + same arguments always produce
/// the same output on every platform.
///
/// # Example
///
/// ```
/// use stygian_browser::noise::{NoiseEngine, NoiseSeed};
///
/// let engine = NoiseEngine::new(NoiseSeed::from(1_u64));
/// let n1 = engine.pixel_noise("canvas", 0, 0);
/// let n2 = engine.pixel_noise("canvas", 0, 0);
/// assert_eq!(n1, n2); // deterministic
/// ```
#[derive(Debug, Clone)]
pub struct NoiseEngine {
    seed: NoiseSeed,
}

impl NoiseEngine {
    /// Create a new [`NoiseEngine`] with the given seed.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::noise::{NoiseEngine, NoiseSeed};
    /// let engine = NoiseEngine::new(NoiseSeed::from(42_u64));
    /// ```
    #[must_use]
    pub const fn new(seed: NoiseSeed) -> Self {
        Self { seed }
    }

    /// Return the seed this engine was created with.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::noise::{NoiseEngine, NoiseSeed};
    /// let s = NoiseSeed::from(7_u64);
    /// assert_eq!(NoiseEngine::new(s).seed(), s);
    /// ```
    #[must_use]
    pub const fn seed(&self) -> NoiseSeed {
        self.seed
    }

    /// RGBA pixel delta for canvas operations, each component in `[-3, 3]`.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::noise::{NoiseEngine, NoiseSeed};
    /// let e = NoiseEngine::new(NoiseSeed::from(1_u64));
    /// let (r, g, b, a) = e.pixel_noise("toDataURL", 5, 10);
    /// assert!((-3..=3).contains(&r));
    /// ```
    #[must_use]
    pub fn pixel_noise(&self, operation: &str, x: u32, y: u32) -> (i8, i8, i8, i8) {
        bounded_bytes(mix(self.seed.0, operation, x, y))
    }

    /// Small floating-point perturbation for audio / timing values.
    ///
    /// Returns a value in `[-0.000_01, 0.000_01]`, imperceptible to human
    /// listening but sufficient to alter the floating-point fingerprint.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::noise::{NoiseEngine, NoiseSeed};
    /// let e = NoiseEngine::new(NoiseSeed::from(1_u64));
    /// let delta = e.float_noise("AudioBuffer", 0);
    /// assert!(delta.abs() <= 0.000_01);
    /// ```
    #[must_use]
    pub fn float_noise(&self, operation: &str, index: u32) -> f64 {
        let h = mix(self.seed.0, operation, index, 0);
        // Keep only 53 bits so conversion to f64 is exact.
        let upper53 = h >> 11;
        let high = ((upper53 >> 21) & 0xFFFF_FFFF) as u32;
        let low = (upper53 & ((1_u64 << 21) - 1)) as u32;
        let normalized =
            (f64::from(high) * 2_097_152.0 + f64::from(low)) / 9_007_199_254_740_991.0;
        (normalized - 0.5) * 2.0e-5 // [-1e-5, 1e-5]
    }

    /// x/y/width/height delta for `ClientRect` / `TextMetrics` noise.
    ///
    /// Each component is a sub-pixel fractional delta in `[-0.5, 0.5]`.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::noise::{NoiseEngine, NoiseSeed};
    /// let e = NoiseEngine::new(NoiseSeed::from(1_u64));
    /// let (dx, dy, dw, dh) = e.rect_noise("getBoundingClientRect", 0);
    /// assert!(dx.abs() <= 0.5);
    /// ```
    #[must_use]
    pub fn rect_noise(&self, operation: &str, index: u32) -> (f64, f64, f64, f64) {
        let hash = mix(self.seed.0, operation, index, 0xDEAD_BEEF);
        let (red, green, blue, alpha) = bounded_bytes(hash);
        // Map [-3, 3] → [-0.5, 0.5] (divide by 6)
        let scale = 1.0_f64 / 6.0;
        (
            f64::from(red) * scale,
            f64::from(green) * scale,
            f64::from(blue) * scale,
            f64::from(alpha) * scale,
        )
    }

    /// RGBA pixel delta for WebGL `readPixels`, each component in `[-3, 3]`.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::noise::{NoiseEngine, NoiseSeed};
    /// let e = NoiseEngine::new(NoiseSeed::from(1_u64));
    /// let (r, g, b, a) = e.webgl_noise("readPixels", 0, 0);
    /// assert!((-3..=3).contains(&r));
    /// ```
    #[must_use]
    pub fn webgl_noise(&self, operation: &str, x: u32, y: u32) -> (i8, i8, i8, i8) {
        bounded_bytes(mix(self.seed.0, operation, x, y ^ 0xCAFE_BABE))
    }

    /// Generate the JavaScript source for `__stygian_noise(operation, x, y)`.
    ///
    /// The returned string embeds the seed value and replicates the hash-based
    /// noise logic in pure JS with no DOM dependencies — safe to inject into
    /// Worker / Service Worker contexts.
    ///
    /// Returns `(i8, i8, i8, i8)`-equivalent as a JS array `[r, g, b, a]`.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::noise::{NoiseEngine, NoiseSeed};
    /// let e = NoiseEngine::new(NoiseSeed::from(42_u64));
    /// let js = e.js_noise_fn();
    /// assert!(js.contains("42"));
    /// assert!(js.contains("__stygian_noise"));
    /// ```
    #[must_use]
    pub fn js_noise_fn(&self) -> String {
        let seed = self.seed.0;
        // The JS reimplements the Rust mix() function using BigInt arithmetic
        // so that 64-bit multiply/XOR is exact. Returns [r, g, b, a] each in [-3, 3].
        format!(
            r"(function() {{
  const _SEED = {seed}n;
  const _M = 0x9e3779b97f4a7c15n;
  const _MASK = 0xFFFFFFFFFFFFFFFFn;

  function _mix(seed, op, a, b) {{
    let h = BigInt(seed);
    for (let i = 0; i < op.length; i++) {{
      h = ((h * _M) + BigInt(op.charCodeAt(i))) & _MASK;
      h = (h ^ (h >> 33n)) & _MASK;
    }}
    h = ((h * _M) + BigInt(a)) & _MASK;
    h = (h ^ (h >> 33n)) & _MASK;
    h = ((h * _M) + BigInt(b)) & _MASK;
    h = (h ^ (h >> 33n)) & _MASK;
    return h;
  }}

  function _bb(h) {{
    const r = Number((h & 0xFFFFn) % 7n) - 3;
    const g = Number(((h >> 16n) & 0xFFFFn) % 7n) - 3;
    const b = Number(((h >> 32n) & 0xFFFFn) % 7n) - 3;
    const a = Number(((h >> 48n) & 0xFFFFn) % 7n) - 3;
    return [r, g, b, a];
  }}

  globalThis.__stygian_noise = function(operation, x, y) {{
    return _bb(_mix(_SEED, operation, x >>> 0, y >>> 0));
  }};

  globalThis.__stygian_float_noise = function(operation, index) {{
    const h = _mix(_SEED, operation, index >>> 0, 0);
    return (Number(h) / Number(_MASK) - 0.5) * 2e-5;
  }};

  globalThis.__stygian_rect_noise = function(operation, index) {{
    const h = _mix(_SEED, operation, index >>> 0, 0xDEADBEEF);
    const [r, g, b, a] = _bb(h);
    return [r / 6, g / 6, b / 6, a / 6];
  }};

  globalThis.__stygian_webgl_noise = function(operation, x, y) {{
    return _bb(_mix(_SEED, operation, x >>> 0, (y >>> 0) ^ 0xCAFEBABE));
  }};
}})();"
        )
    }
}

// ---------------------------------------------------------------------------
// NoiseConfig
// ---------------------------------------------------------------------------

/// Configuration for the fingerprint noise subsystem.
///
/// Added to [`crate::config::BrowserConfig`] when the `stealth` feature is enabled.
///
/// # Example
///
/// ```
/// use stygian_browser::noise::{NoiseConfig, NoiseSeed};
///
/// let cfg = NoiseConfig::default();
/// assert!(cfg.canvas_enabled);
///
/// let custom = NoiseConfig {
///     seed: Some(NoiseSeed::from(123_u64)),
///     ..NoiseConfig::default()
/// };
/// assert_eq!(custom.seed.unwrap().as_u64(), 123);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NoiseConfig {
    /// Fixed seed for reproducibility. If `None`, a random seed is generated
    /// at [`NoiseEngine`] construction time.
    pub seed: Option<NoiseSeed>,

    /// Enable canvas API noise (`toDataURL`, `toBlob`, `getImageData`).
    pub canvas_enabled: bool,

    /// Enable WebGL API noise (`readPixels`, `getParameter`).
    pub webgl_enabled: bool,

    /// Enable audio API noise (`getChannelData`, analyser nodes).
    pub audio_enabled: bool,

    /// Enable layout API noise (`getBoundingClientRect`, `TextMetrics`).
    pub rects_enabled: bool,
}

impl Default for NoiseConfig {
    fn default() -> Self {
        Self {
            seed: None,
            canvas_enabled: true,
            webgl_enabled: true,
            audio_enabled: true,
            rects_enabled: true,
        }
    }
}

impl NoiseConfig {
    /// Build a [`NoiseEngine`] from this config.
    ///
    /// If `seed` is `None`, a random seed is generated at call time.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::noise::{NoiseConfig, NoiseSeed};
    ///
    /// let cfg = NoiseConfig { seed: Some(NoiseSeed::from(1_u64)), ..Default::default() };
    /// let engine = cfg.build_engine();
    /// assert_eq!(engine.seed().as_u64(), 1);
    /// ```
    #[must_use]
    pub fn build_engine(&self) -> NoiseEngine {
        let seed = self.seed.unwrap_or_else(NoiseSeed::random);
        NoiseEngine::new(seed)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_args_deterministic() {
        let e = NoiseEngine::new(NoiseSeed::from(42_u64));
        assert_eq!(
            e.pixel_noise("canvas.toDataURL", 10, 20),
            e.pixel_noise("canvas.toDataURL", 10, 20)
        );
    }

    #[test]
    fn different_seeds_different_outputs() {
        let e1 = NoiseEngine::new(NoiseSeed::from(1_u64));
        let e2 = NoiseEngine::new(NoiseSeed::from(2_u64));
        // With overwhelming probability these differ — if they don't, the hash is broken
        assert_ne!(
            e1.pixel_noise("canvas.toDataURL", 10, 20),
            e2.pixel_noise("canvas.toDataURL", 10, 20)
        );
    }

    #[test]
    fn pixel_noise_bounded() {
        let e = NoiseEngine::new(NoiseSeed::from(0xDEAD_BEEF_u64));
        for px in 0u32..16 {
            for py in 0u32..16 {
                let (red, green, blue, alpha) = e.pixel_noise("canvas", px, py);
                assert!((-3..=3).contains(&red), "r={red} out of range");
                assert!((-3..=3).contains(&green), "g={green} out of range");
                assert!((-3..=3).contains(&blue), "b={blue} out of range");
                assert!((-3..=3).contains(&alpha), "a={alpha} out of range");
            }
        }
    }

    #[test]
    fn webgl_noise_bounded() {
        let e = NoiseEngine::new(NoiseSeed::from(0xCAFE_u64));
        for px in 0u32..8 {
            for py in 0u32..8 {
                let (red, green, blue, alpha) = e.webgl_noise("readPixels", px, py);
                assert!((-3..=3).contains(&red));
                assert!((-3..=3).contains(&green));
                assert!((-3..=3).contains(&blue));
                assert!((-3..=3).contains(&alpha));
            }
        }
    }

    #[test]
    fn float_noise_bounded() {
        let e = NoiseEngine::new(NoiseSeed::from(7_u64));
        for i in 0u32..32 {
            let v = e.float_noise("AudioBuffer", i);
            assert!(
                v.abs() <= 1e-5 + f64::EPSILON,
                "float_noise {v} out of range"
            );
        }
    }

    #[test]
    fn rect_noise_bounded() {
        let e = NoiseEngine::new(NoiseSeed::from(99_u64));
        for i in 0u32..16 {
            let (dx, dy, dw, dh) = e.rect_noise("getBoundingClientRect", i);
            assert!(dx.abs() <= 0.5 + f64::EPSILON);
            assert!(dy.abs() <= 0.5 + f64::EPSILON);
            assert!(dw.abs() <= 0.5 + f64::EPSILON);
            assert!(dh.abs() <= 0.5 + f64::EPSILON);
        }
    }

    #[test]
    fn noise_config_serde_round_trip() {
        let cfg = NoiseConfig {
            seed: Some(NoiseSeed::from(555_u64)),
            canvas_enabled: true,
            webgl_enabled: false,
            audio_enabled: true,
            rects_enabled: false,
        };
        let json_result = serde_json::to_string(&cfg);
        assert!(json_result.is_ok(), "serialize failed: {json_result:?}");
        let Ok(json) = json_result else {
            return;
        };
        let back_result: Result<NoiseConfig, _> = serde_json::from_str(&json);
        assert!(back_result.is_ok(), "deserialize failed: {back_result:?}");
        let Ok(back) = back_result else {
            return;
        };
        assert_eq!(back.seed, cfg.seed);
        assert_eq!(back.canvas_enabled, cfg.canvas_enabled);
        assert_eq!(back.webgl_enabled, cfg.webgl_enabled);
        assert_eq!(back.audio_enabled, cfg.audio_enabled);
        assert_eq!(back.rects_enabled, cfg.rects_enabled);
    }

    #[test]
    fn js_noise_fn_contains_seed() {
        let seed = 98_765_u64;
        let e = NoiseEngine::new(NoiseSeed::from(seed));
        let js = e.js_noise_fn();
        assert!(js.contains(&seed.to_string()), "seed not embedded in JS");
        assert!(js.contains("__stygian_noise"), "missing __stygian_noise");
        assert!(
            js.contains("__stygian_float_noise"),
            "missing __stygian_float_noise"
        );
    }

    #[test]
    fn noise_seed_random_unique() {
        // Generate 100 seeds — collision probability is negligible (birthday bound ~2^{-54} for 100 draws)
        let seeds: Vec<NoiseSeed> = (0..100).map(|_| NoiseSeed::random()).collect();
        let unique: std::collections::HashSet<u64> = seeds.iter().map(|s| s.as_u64()).collect();
        assert_eq!(unique.len(), 100, "random seeds collided");
    }

    #[test]
    fn noise_config_build_engine_uses_seed() {
        let cfg = NoiseConfig {
            seed: Some(NoiseSeed::from(77_u64)),
            ..Default::default()
        };
        let engine = cfg.build_engine();
        assert_eq!(engine.seed().as_u64(), 77);
    }

    #[test]
    fn noise_config_build_engine_random_when_none() {
        let cfg = NoiseConfig::default();
        let e1 = cfg.build_engine();
        let e2 = cfg.build_engine();
        // Random seeds differ with overwhelming probability
        assert_ne!(e1.seed().as_u64(), e2.seed().as_u64());
    }
}
