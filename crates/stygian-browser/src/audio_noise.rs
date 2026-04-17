//! Audio fingerprint noise injection.
//!
//! Overrides `AudioBuffer`, `AnalyserNode`, and `OfflineAudioContext` APIs to
//! inject deterministic per-session noise that breaks audio fingerprinting
//! while remaining inaudible.
//!
//! # Example
//!
//! ```
//! use stygian_browser::audio_noise::audio_noise_script;
//! use stygian_browser::noise::{NoiseEngine, NoiseSeed};
//!
//! let engine = NoiseEngine::new(NoiseSeed::from(42_u64));
//! let js = audio_noise_script(&engine);
//! assert!(js.contains("getChannelData"));
//! assert!(js.contains("__stygian_float_noise"));
//! ```

use crate::noise::NoiseEngine;

/// Generate the audio noise injection script for a given [`NoiseEngine`].
///
/// Must be injected via `Page.addScriptToEvaluateOnNewDocument`. Works in
/// Worker contexts where `OfflineAudioContext` is available.
///
/// Returns an empty string if audio noise is not needed (callers should
/// check [`crate::noise::NoiseConfig::audio_enabled`]).
///
/// # Example
///
/// ```
/// use stygian_browser::audio_noise::audio_noise_script;
/// use stygian_browser::noise::{NoiseEngine, NoiseSeed};
///
/// let js = audio_noise_script(&NoiseEngine::new(NoiseSeed::from(1_u64)));
/// assert!(js.contains("AudioBuffer"));
/// assert!(js.contains("getChannelData"));
/// assert!(js.contains("copyFromChannel"));
/// assert!(js.contains("getFloatFrequencyData"));
/// assert!(js.contains("startRendering"));
/// ```
#[must_use]
pub fn audio_noise_script(engine: &NoiseEngine) -> String {
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

  // ── AudioBuffer.getChannelData ───────────────────────────────────────
  if (typeof AudioBuffer !== 'undefined') {{
    const _origGCD = AudioBuffer.prototype.getChannelData;
    _def(AudioBuffer.prototype, 'getChannelData', function getChannelData(channel) {{
      const data = _origGCD.call(this, channel);
      const key = 'audio.getChannelData.' + channel;
      for (let i = 0; i < data.length; i++) {{
        data[i] += __stygian_float_noise(key, i);
      }}
      return data;
    }});

    // AudioBuffer.copyFromChannel
    const _origCFC = AudioBuffer.prototype.copyFromChannel;
    _def(AudioBuffer.prototype, 'copyFromChannel', function copyFromChannel(dest, channelNumber, startInChannel) {{
      const off = startInChannel || 0;
      _origCFC.call(this, dest, channelNumber, off);
      const key = 'audio.copyFromChannel.' + channelNumber;
      for (let i = 0; i < dest.length; i++) {{
        dest[i] += __stygian_float_noise(key, off + i);
      }}
    }});
  }}

  // ── AnalyserNode frequency/time domain ──────────────────────────────
  if (typeof AnalyserNode !== 'undefined') {{
    const _origGFFD = AnalyserNode.prototype.getFloatFrequencyData;
    _def(AnalyserNode.prototype, 'getFloatFrequencyData', function getFloatFrequencyData(arr) {{
      _origGFFD.call(this, arr);
      for (let i = 0; i < arr.length; i++) {{
        arr[i] += __stygian_float_noise('audio.floatFreq', i);
      }}
    }});

    const _origGBFD = AnalyserNode.prototype.getByteFrequencyData;
    _def(AnalyserNode.prototype, 'getByteFrequencyData', function getByteFrequencyData(arr) {{
      _origGBFD.call(this, arr);
      for (let i = 0; i < arr.length; i++) {{
        const delta = (__stygian_float_noise('audio.byteFreq', i) * 1e5) | 0;
        arr[i] = Math.max(0, Math.min(255, arr[i] + delta));
      }}
    }});

    const _origGFTD = AnalyserNode.prototype.getFloatTimeDomainData;
    _def(AnalyserNode.prototype, 'getFloatTimeDomainData', function getFloatTimeDomainData(arr) {{
      _origGFTD.call(this, arr);
      for (let i = 0; i < arr.length; i++) {{
        arr[i] += __stygian_float_noise('audio.floatTime', i);
      }}
    }});

    const _origGBTD = AnalyserNode.prototype.getByteTimeDomainData;
    _def(AnalyserNode.prototype, 'getByteTimeDomainData', function getByteTimeDomainData(arr) {{
      _origGBTD.call(this, arr);
      for (let i = 0; i < arr.length; i++) {{
        const delta = (__stygian_float_noise('audio.byteTime', i) * 1e5) | 0;
        arr[i] = Math.max(0, Math.min(255, arr[i] + delta));
      }}
    }});
  }}

  // ── OfflineAudioContext.startRendering ───────────────────────────────
  if (typeof OfflineAudioContext !== 'undefined') {{
    const _origSR = OfflineAudioContext.prototype.startRendering;
    _def(OfflineAudioContext.prototype, 'startRendering', function startRendering() {{
      return _origSR.call(this).then(function(buffer) {{
        const nCh = buffer.numberOfChannels;
        for (let c = 0; c < nCh; c++) {{
          const data = buffer.getChannelData(c);
          const key = 'audio.offline.' + c;
          for (let i = 0; i < data.length; i++) {{
            data[i] += __stygian_float_noise(key, i);
          }}
        }}
        return buffer;
      }});
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
        let js = audio_noise_script(&eng(1));
        assert!(js.contains("getChannelData"), "missing getChannelData");
        assert!(js.contains("copyFromChannel"), "missing copyFromChannel");
        assert!(
            js.contains("getFloatFrequencyData"),
            "missing getFloatFrequencyData"
        );
        assert!(
            js.contains("getByteFrequencyData"),
            "missing getByteFrequencyData"
        );
        assert!(
            js.contains("getFloatTimeDomainData"),
            "missing getFloatTimeDomainData"
        );
        assert!(
            js.contains("getByteTimeDomainData"),
            "missing getByteTimeDomainData"
        );
        assert!(js.contains("startRendering"), "missing startRendering");
    }

    #[test]
    fn script_contains_float_noise_fn() {
        let js = audio_noise_script(&eng(1));
        assert!(
            js.contains("__stygian_float_noise"),
            "missing __stygian_float_noise"
        );
    }

    #[test]
    fn script_contains_native_tostring() {
        let js = audio_noise_script(&eng(1));
        assert!(js.contains("[native code]"), "missing toString spoof");
    }

    #[test]
    fn script_contains_seed() {
        let js = audio_noise_script(&eng(54321));
        assert!(js.contains("54321"), "seed not embedded");
    }

    #[test]
    fn different_seeds_differ() {
        let js1 = audio_noise_script(&eng(1));
        let js2 = audio_noise_script(&eng(2));
        assert_ne!(js1, js2);
    }
}
