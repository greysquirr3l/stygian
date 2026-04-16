//! Canvas fingerprint noise injection.
//!
//! Generates a CDP `Page.addScriptToEvaluateOnNewDocument` script that overrides
//! `CanvasRenderingContext2D` and `OffscreenCanvasRenderingContext2D` APIs to
//! inject deterministic per-session noise into all pixel-readback operations.
//!
//! The noise is driven by [`crate::noise::NoiseEngine`] (T37). Given the same
//! seed, every canvas read produces the same perturbation — enabling
//! cross-context consistency (main thread vs. OffscreenCanvas in a Worker).
//!
//! # Example
//!
//! ```
//! use stygian_browser::canvas_noise::canvas_noise_script;
//! use stygian_browser::noise::{NoiseEngine, NoiseSeed};
//!
//! let engine = NoiseEngine::new(NoiseSeed::from(42_u64));
//! let js = canvas_noise_script(&engine);
//! assert!(js.contains("__stygian_noise"));
//! assert!(js.contains("toDataURL"));
//! ```

use crate::noise::NoiseEngine;

/// Generate the canvas noise injection script for a given [`NoiseEngine`].
///
/// The script must be injected via `Page.addScriptToEvaluateOnNewDocument`
/// so it runs before any page JavaScript. It works in both the main thread
/// and Web Worker / OffscreenCanvas contexts.
///
/// Returns an empty string if canvas noise is not needed (callers should
/// check [`crate::noise::NoiseConfig::canvas_enabled`] before calling).
///
/// # Example
///
/// ```
/// use stygian_browser::canvas_noise::canvas_noise_script;
/// use stygian_browser::noise::{NoiseEngine, NoiseSeed};
///
/// let engine = NoiseEngine::new(NoiseSeed::from(1_u64));
/// let js = canvas_noise_script(&engine);
/// assert!(js.contains("toDataURL"));
/// assert!(js.contains("getImageData"));
/// assert!(js.contains("convertToBlob"));
/// ```
#[must_use]
pub fn canvas_noise_script(engine: &NoiseEngine) -> String {
    // First emit the seed-embedded noise helper (from T37), then the overrides.
    let noise_fn = engine.js_noise_fn();
    format!(
        r#"(function() {{
  'use strict';

  // ── Noise helpers (injected from T37 NoiseEngine) ──────────────────────
  {noise_fn}

  // ── Utility: apply pixel noise to an ImageData in-place ────────────────
  function _applyCanvasNoise(imageData, offsetX, offsetY, operation) {{
    const data = imageData.data;
    const w = imageData.width;
    for (let i = 0; i < data.length; i += 4) {{
      const pixelIdx = (i / 4) | 0;
      const px = (offsetX + (pixelIdx % w)) >>> 0;
      const py = (offsetY + ((pixelIdx / w) | 0)) >>> 0;
      // Only noise non-transparent, non-zero pixels to avoid noising blank canvases
      if (data[i] === 0 && data[i | 1] === 0 && data[i | 2] === 0 && data[i | 3] === 0) {{
        continue;
      }}
      const [dr, dg, db, da] = __stygian_noise(operation, px, py);
      data[i]     = Math.max(0, Math.min(255, data[i]     + dr));
      data[i | 1] = Math.max(0, Math.min(255, data[i | 1] + dg));
      data[i | 2] = Math.max(0, Math.min(255, data[i | 2] + db));
      data[i | 3] = Math.max(0, Math.min(255, data[i | 3] + da));
    }}
    return imageData;
  }}

  // ── Helper: copy canvas content to a temp canvas, noise it, return it ──
  function _noisedCanvas(src) {{
    const tmp = (typeof OffscreenCanvas !== 'undefined' && src instanceof OffscreenCanvas)
      ? new OffscreenCanvas(src.width, src.height)
      : document.createElement('canvas');
    tmp.width  = src.width;
    tmp.height = src.height;
    const ctx = tmp.getContext('2d');
    ctx.drawImage(src, 0, 0);
    const id = ctx.getImageData(0, 0, tmp.width, tmp.height);
    _applyCanvasNoise(id, 0, 0, 'canvas.toDataURL');
    ctx.putImageData(id, 0, 0);
    return tmp;
  }}

  // ── Spoof toString so overrides look like native functions ───────────────
  function _nativeToString(name) {{
    return function toString() {{ return 'function ' + name + '() {{ [native code] }}'; }};
  }}

  function _defineNative(obj, prop, fn) {{
    fn.toString = _nativeToString(prop);
    Object.defineProperty(obj, prop, {{
      value: fn,
      writable: false,
      configurable: false,
      enumerable: false,
    }});
  }}

  // ── CanvasRenderingContext2D.getImageData ────────────────────────────────
  (function() {{
    const ctx2d = CanvasRenderingContext2D.prototype;
    const _origGetImageData = ctx2d.getImageData;
    _defineNative(ctx2d, 'getImageData', function getImageData(sx, sy, sw, sh, settings) {{
      const id = settings !== undefined
        ? _origGetImageData.call(this, sx, sy, sw, sh, settings)
        : _origGetImageData.call(this, sx, sy, sw, sh);
      return _applyCanvasNoise(id, sx >>> 0, sy >>> 0, 'canvas.getImageData');
    }});
  }})();

  // ── HTMLCanvasElement.toDataURL ──────────────────────────────────────────
  (function() {{
    if (typeof HTMLCanvasElement === 'undefined') return;
    const _origToDataURL = HTMLCanvasElement.prototype.toDataURL;
    _defineNative(HTMLCanvasElement.prototype, 'toDataURL', function toDataURL(type, quality) {{
      const tmp = _noisedCanvas(this);
      return quality !== undefined
        ? _origToDataURL.call(tmp, type, quality)
        : type !== undefined
          ? _origToDataURL.call(tmp, type)
          : _origToDataURL.call(tmp);
    }});
  }})();

  // ── HTMLCanvasElement.toBlob ─────────────────────────────────────────────
  (function() {{
    if (typeof HTMLCanvasElement === 'undefined') return;
    const _origToBlob = HTMLCanvasElement.prototype.toBlob;
    _defineNative(HTMLCanvasElement.prototype, 'toBlob', function toBlob(callback, type, quality) {{
      const tmp = _noisedCanvas(this);
      if (quality !== undefined) {{
        _origToBlob.call(tmp, callback, type, quality);
      }} else if (type !== undefined) {{
        _origToBlob.call(tmp, callback, type);
      }} else {{
        _origToBlob.call(tmp, callback);
      }}
    }});
  }})();

  // ── OffscreenCanvasRenderingContext2D.getImageData ───────────────────────
  (function() {{
    if (typeof OffscreenCanvasRenderingContext2D === 'undefined') return;
    const octx2d = OffscreenCanvasRenderingContext2D.prototype;
    const _origOGID = octx2d.getImageData;
    _defineNative(octx2d, 'getImageData', function getImageData(sx, sy, sw, sh, settings) {{
      const id = settings !== undefined
        ? _origOGID.call(this, sx, sy, sw, sh, settings)
        : _origOGID.call(this, sx, sy, sw, sh);
      return _applyCanvasNoise(id, sx >>> 0, sy >>> 0, 'offscreencanvas.getImageData');
    }});
  }})();

  // ── OffscreenCanvas.convertToBlob ────────────────────────────────────────
  (function() {{
    if (typeof OffscreenCanvas === 'undefined') return;
    const _origCTB = OffscreenCanvas.prototype.convertToBlob;
    _defineNative(OffscreenCanvas.prototype, 'convertToBlob', async function convertToBlob(options) {{
      const tmp = new OffscreenCanvas(this.width, this.height);
      const ctx = tmp.getContext('2d');
      ctx.drawImage(this, 0, 0);
      const id = ctx.getImageData(0, 0, tmp.width, tmp.height);
      _applyCanvasNoise(id, 0, 0, 'offscreencanvas.convertToBlob');
      ctx.putImageData(id, 0, 0);
      return options !== undefined
        ? _origCTB.call(tmp, options)
        : _origCTB.call(tmp);
    }});
  }})();

}})();
"#,
        noise_fn = noise_fn
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::{NoiseEngine, NoiseSeed};

    fn engine(seed: u64) -> NoiseEngine {
        NoiseEngine::new(NoiseSeed::from(seed))
    }

    #[test]
    fn script_contains_seed() {
        let js = canvas_noise_script(&engine(12345));
        assert!(js.contains("12345"), "seed not embedded");
    }

    #[test]
    fn script_contains_all_five_overrides() {
        let js = canvas_noise_script(&engine(1));
        assert!(js.contains("getImageData"), "missing getImageData");
        assert!(js.contains("toDataURL"), "missing toDataURL");
        assert!(js.contains("toBlob"), "missing toBlob");
        // OffscreenCanvas variants
        assert!(
            js.contains("OffscreenCanvasRenderingContext2D"),
            "missing OffscreenCanvas getImageData"
        );
        assert!(js.contains("convertToBlob"), "missing convertToBlob");
    }

    #[test]
    fn script_contains_native_tostring_spoofing() {
        let js = canvas_noise_script(&engine(1));
        assert!(
            js.contains("[native code]"),
            "missing native code toString spoof"
        );
    }

    #[test]
    fn script_contains_noise_helper() {
        let js = canvas_noise_script(&engine(1));
        assert!(js.contains("__stygian_noise"), "missing __stygian_noise");
    }

    #[test]
    fn different_seeds_produce_different_seeds_in_script() {
        let js1 = canvas_noise_script(&engine(111));
        let js2 = canvas_noise_script(&engine(222));
        // The embedded seed values differ
        assert_ne!(js1, js2, "scripts should differ for different seeds");
    }
}
