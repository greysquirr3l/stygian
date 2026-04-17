//! `ClientRects` and `TextMetrics` fingerprint noise injection.
//!
//! Overrides `getBoundingClientRect`, `getClientRects`, `Range` equivalents, and
//! `CanvasRenderingContext2D.measureText` to inject deterministic sub-pixel noise
//! that breaks font/layout fingerprinting while preserving `DOMRect` consistency.
//!
//! # Example
//!
//! ```
//! use stygian_browser::rects_noise::rects_noise_script;
//! use stygian_browser::noise::{NoiseEngine, NoiseSeed};
//!
//! let engine = NoiseEngine::new(NoiseSeed::from(42_u64));
//! let js = rects_noise_script(&engine);
//! assert!(js.contains("getBoundingClientRect"));
//! assert!(js.contains("measureText"));
//! ```

use crate::noise::NoiseEngine;

/// Generate the `ClientRects` and `TextMetrics` noise injection script.
///
/// Must be injected via `Page.addScriptToEvaluateOnNewDocument`.
///
/// Noise preserves `DOMRect` internal consistency: `right = x + width`,
/// `bottom = y + height`.
///
/// # Example
///
/// ```
/// use stygian_browser::rects_noise::rects_noise_script;
/// use stygian_browser::noise::{NoiseEngine, NoiseSeed};
///
/// let js = rects_noise_script(&NoiseEngine::new(NoiseSeed::from(1_u64)));
/// assert!(js.contains("getBoundingClientRect"));
/// assert!(js.contains("getClientRects"));
/// assert!(js.contains("measureText"));
/// assert!(js.contains("__stygian_rect_noise"));
/// ```
#[must_use]
pub fn rects_noise_script(engine: &NoiseEngine) -> String {
    let noise_fn = engine.js_noise_fn();
    format!(
        r"(function() {{
  'use strict';

  // ── Noise helpers ──────────────────────────────────────────────────────
  {noise_fn}

  // ── Spoof toString ─────────────────────────────────────────────────────
  function _nts(name) {{ return function toString() {{ return 'function ' + name + '() {{ [native code] }}'; }}; }}
  function _def(obj, prop, fn) {{
    fn.toString = _nts(prop);
    Object.defineProperty(obj, prop, {{ value: fn, writable: false, configurable: false, enumerable: false }});
  }}

  // ── Element hash (stable, no DOM dependencies) ─────────────────────────
  // Combines tagName, className, and textContent length into a u32 index
  // for use as the noise key coordinate.
  function _elemHash(el) {{
    let h = 0;
    const tag = (el.tagName || '');
    const cls = (el.className && typeof el.className === 'string' ? el.className : '');
    const tlen = (el.textContent ? el.textContent.length : 0);
    for (let i = 0; i < tag.length; i++) h = ((h * 31) + tag.charCodeAt(i)) & 0xFFFFFFFF;
    for (let i = 0; i < cls.length; i++) h = ((h * 31) + cls.charCodeAt(i)) & 0xFFFFFFFF;
    h = ((h * 31) + tlen) & 0xFFFFFFFF;
    return h >>> 0;
  }}

  // ── Noise a DOMRect preserving consistency ──────────────────────────────
  function _noiseRect(rect, key, idx) {{
    const [dx, dy, dw, dh] = __stygian_rect_noise(key, idx);
    const x = rect.x + dx;
    const y = rect.y + dy;
    const w = rect.width  + dw;
    const h = rect.height + dh;
    return {{
      x: x, y: y, width: w, height: h,
      left: x, top: y, right: x + w, bottom: y + h,
      toJSON: function() {{
        return {{ x: x, y: y, width: w, height: h, left: x, top: y, right: x + w, bottom: y + h }};
      }}
    }};
  }}

  // ── Element.getBoundingClientRect ───────────────────────────────────────
  if (typeof Element !== 'undefined') {{
    const _origGBCR = Element.prototype.getBoundingClientRect;
    _def(Element.prototype, 'getBoundingClientRect', function getBoundingClientRect() {{
      const r = _origGBCR.call(this);
      return _noiseRect(r, 'rects.getBCR', _elemHash(this));
    }});

    // Element.getClientRects
    const _origGCR = Element.prototype.getClientRects;
    _def(Element.prototype, 'getClientRects', function getClientRects() {{
      const list = _origGCR.call(this);
      const h = _elemHash(this);
      const result = [];
      for (let i = 0; i < list.length; i++) {{
        result.push(_noiseRect(list[i], 'rects.getCR', (h + i) >>> 0));
      }}
      result[Symbol.iterator] = Array.prototype[Symbol.iterator];
      result.item = function(idx) {{ return result[idx] || null; }};
      result.length = result.length;
      return result;
    }});
  }}

  // ── Range.getBoundingClientRect ─────────────────────────────────────────
  if (typeof Range !== 'undefined') {{
    const _origRGBCR = Range.prototype.getBoundingClientRect;
    _def(Range.prototype, 'getBoundingClientRect', function getBoundingClientRect() {{
      const r = _origRGBCR.call(this);
      return _noiseRect(r, 'rects.rangeBCR', 0);
    }});

    const _origRGCR = Range.prototype.getClientRects;
    _def(Range.prototype, 'getClientRects', function getClientRects() {{
      const list = _origRGCR.call(this);
      const result = [];
      for (let i = 0; i < list.length; i++) {{
        result.push(_noiseRect(list[i], 'rects.rangeCR', i));
      }}
      result[Symbol.iterator] = Array.prototype[Symbol.iterator];
      result.item = function(idx) {{ return result[idx] || null; }};
      result.length = result.length;
      return result;
    }});
  }}

  // ── CanvasRenderingContext2D.measureText ────────────────────────────────
  if (typeof CanvasRenderingContext2D !== 'undefined') {{
    const _origMT = CanvasRenderingContext2D.prototype.measureText;
    _def(CanvasRenderingContext2D.prototype, 'measureText', function measureText(text) {{
      const m = _origMT.call(this, text);
      // Hash the text for a stable noise index
      let th = 0;
      for (let i = 0; i < text.length; i++) th = ((th * 31) + text.charCodeAt(i)) & 0xFFFFFFFF;
      const [dx, , dw, ] = __stygian_rect_noise('rects.measureText', th >>> 0);
      const scale = 0.01; // ±0.001..0.01 pixels
      return {{
        width:                    m.width                    + dx * scale,
        actualBoundingBoxLeft:    m.actualBoundingBoxLeft    + dx * scale,
        actualBoundingBoxRight:   m.actualBoundingBoxRight   + dw * scale,
        actualBoundingBoxAscent:  m.actualBoundingBoxAscent  + dx * scale,
        actualBoundingBoxDescent: m.actualBoundingBoxDescent + dw * scale,
        fontBoundingBoxAscent:    m.fontBoundingBoxAscent    + dx * scale,
        fontBoundingBoxDescent:   m.fontBoundingBoxDescent   + dw * scale,
        emHeightAscent:           m.emHeightAscent           + dx * scale,
        emHeightDescent:          m.emHeightDescent          + dw * scale,
        hangingBaseline:          m.hangingBaseline          + dx * scale,
        alphabeticBaseline:       m.alphabeticBaseline       + dx * scale,
        ideographicBaseline:      m.ideographicBaseline      + dx * scale,
      }};
    }});
  }}

}})();
"
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::{NoiseEngine, NoiseSeed};

    fn eng(seed: u64) -> NoiseEngine {
        NoiseEngine::new(NoiseSeed::from(seed))
    }

    #[test]
    fn script_overrides_all_methods() {
        let js = rects_noise_script(&eng(1));
        assert!(
            js.contains("getBoundingClientRect"),
            "missing getBoundingClientRect"
        );
        assert!(js.contains("getClientRects"), "missing getClientRects");
        assert!(js.contains("Range"), "missing Range overrides");
        assert!(js.contains("measureText"), "missing measureText");
    }

    #[test]
    fn script_preserves_domrect_consistency() {
        let js = rects_noise_script(&eng(1));
        // The _noiseRect helper must set right = x + w and bottom = y + h
        assert!(
            js.contains("right: x + w"),
            "DOMRect right not derived from x+w"
        );
        assert!(
            js.contains("bottom: y + h"),
            "DOMRect bottom not derived from y+h"
        );
    }

    #[test]
    fn text_metrics_covers_8_properties() {
        let js = rects_noise_script(&eng(1));
        let required = [
            "width",
            "actualBoundingBoxLeft",
            "actualBoundingBoxRight",
            "actualBoundingBoxAscent",
            "actualBoundingBoxDescent",
            "fontBoundingBoxAscent",
            "fontBoundingBoxDescent",
            "emHeightAscent",
        ];
        for prop in &required {
            assert!(js.contains(prop), "TextMetrics missing {prop}");
        }
    }

    #[test]
    fn script_contains_rect_noise_fn() {
        let js = rects_noise_script(&eng(1));
        assert!(js.contains("__stygian_rect_noise"), "missing rect noise fn");
    }

    #[test]
    fn element_hash_handles_null_classname() {
        let js = rects_noise_script(&eng(1));
        // JS handles missing className gracefully
        assert!(
            js.contains("typeof el.className === 'string'"),
            "className guard missing"
        );
    }

    #[test]
    fn script_contains_native_tostring() {
        let js = rects_noise_script(&eng(1));
        assert!(js.contains("[native code]"), "missing toString spoof");
    }
}
