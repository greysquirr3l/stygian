//! WebGL parameter spoofing and readPixels noise injection.
//!
//! Overrides `WebGL1` and `WebGL2` APIs to present a coherent, session-unique GPU
//! identity and apply deterministic noise to `readPixels()` output.
//!
//! # Example
//!
//! ```
//! use stygian_browser::webgl_noise::{webgl_noise_script, WebGlProfile};
//! use stygian_browser::noise::{NoiseEngine, NoiseSeed};
//!
//! let engine = NoiseEngine::new(NoiseSeed::from(42_u64));
//! let js = webgl_noise_script(&WebGlProfile::nvidia_rtx_3060(), &engine);
//! assert!(js.contains("RTX 3060"));
//! assert!(js.contains("__stygian_webgl_noise"));
//! ```

use serde::{Deserialize, Serialize};

use crate::noise::NoiseEngine;

// ---------------------------------------------------------------------------
// ShaderPrecisionProfile
// ---------------------------------------------------------------------------

/// Shader precision format values for a GPU profile.
///
/// Matches the structure returned by `getShaderPrecisionFormat()`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShaderPrecisionProfile {
    /// High-float range exponent bits.
    pub high_float_range_min: i32,
    /// High-float range exponent bits.
    pub high_float_range_max: i32,
    /// High-float precision bits.
    pub high_float_precision: i32,
    /// Medium-float precision bits.
    pub medium_float_precision: i32,
    /// Low-float precision bits.
    pub low_float_precision: i32,
    /// High-int precision bits.
    pub high_int_precision: i32,
}

impl Default for ShaderPrecisionProfile {
    fn default() -> Self {
        Self {
            high_float_range_min: 127,
            high_float_range_max: 127,
            high_float_precision: 23,
            medium_float_precision: 23,
            low_float_precision: 23,
            high_int_precision: 31,
        }
    }
}

// ---------------------------------------------------------------------------
// ContextAttributes
// ---------------------------------------------------------------------------

/// WebGL context attributes returned by `getContextAttributes()`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextAttributes {
    /// Alpha channel enabled.
    pub alpha: bool,
    /// Anti-aliasing enabled.
    pub antialias: bool,
    /// Depth buffer enabled.
    pub depth: bool,
    /// Fail if major performance caveat.
    pub fail_if_major_performance_caveat: bool,
    /// Power preference.
    pub power_preference: String,
    /// Premultiplied alpha.
    pub premultiplied_alpha: bool,
    /// Preserve drawing buffer.
    pub preserve_drawing_buffer: bool,
    /// Stencil buffer.
    pub stencil: bool,
    /// Desynchronized.
    pub desynchronized: bool,
}

impl Default for ContextAttributes {
    fn default() -> Self {
        Self {
            alpha: true,
            antialias: true,
            depth: true,
            fail_if_major_performance_caveat: false,
            power_preference: "default".to_string(),
            premultiplied_alpha: true,
            preserve_drawing_buffer: false,
            stencil: false,
            desynchronized: false,
        }
    }
}

// ---------------------------------------------------------------------------
// WebGlProfile
// ---------------------------------------------------------------------------

/// A complete WebGL device identity profile.
///
/// Used to present a consistent, plausible GPU identity to fingerprinting scripts.
///
/// # Example
///
/// ```
/// use stygian_browser::webgl_noise::WebGlProfile;
///
/// let profile = WebGlProfile::nvidia_rtx_3060();
/// assert!(profile.renderer.contains("RTX 3060"));
/// assert!(profile.max_texture_size >= 16384);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebGlProfile {
    /// `UNMASKED_VENDOR_WEBGL` / `getParameter(GL_VENDOR)` value.
    pub vendor: String,
    /// `UNMASKED_RENDERER_WEBGL` / `getParameter(GL_RENDERER)` value.
    pub renderer: String,
    /// `MAX_TEXTURE_SIZE` in pixels.
    pub max_texture_size: u32,
    /// `MAX_VIEWPORT_DIMS` as `[width, height]`.
    pub max_viewport_dims: (u32, u32),
    /// `MAX_RENDERBUFFER_SIZE`.
    pub max_renderbuffer_size: u32,
    /// `MAX_VERTEX_ATTRIBS`.
    pub max_vertex_attribs: u32,
    /// `MAX_VARYING_VECTORS`.
    pub max_varying_vectors: u32,
    /// `MAX_FRAGMENT_UNIFORM_VECTORS`.
    pub max_fragment_uniform_vectors: u32,
    /// `MAX_VERTEX_UNIFORM_VECTORS`.
    pub max_vertex_uniform_vectors: u32,
    /// Ordered list of supported WebGL extensions.
    pub extensions: Vec<String>,
    /// Shader precision format.
    pub shader_precision: ShaderPrecisionProfile,
    /// Context attributes.
    pub context_attributes: ContextAttributes,
}

impl WebGlProfile {
    /// Return the NVIDIA RTX 3060 profile with all fields populated.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::webgl_noise::WebGlProfile;
    /// let p = WebGlProfile::nvidia_rtx_3060();
    /// assert!(p.renderer.contains("RTX 3060"));
    /// assert_eq!(p.max_texture_size, 16384);
    /// ```
    #[must_use]
    pub fn nvidia_rtx_3060() -> Self {
        Self {
            vendor: "Google Inc. (NVIDIA)".to_string(),
            renderer: "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)"
                .to_string(),
            max_texture_size: 16384,
            max_viewport_dims: (32768, 32768),
            max_renderbuffer_size: 16384,
            max_vertex_attribs: 16,
            max_varying_vectors: 15,
            max_fragment_uniform_vectors: 1024,
            max_vertex_uniform_vectors: 4096,
            extensions: default_extensions(),
            shader_precision: ShaderPrecisionProfile::default(),
            context_attributes: ContextAttributes::default(),
        }
    }

    /// Return the NVIDIA GTX 1660 profile.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::webgl_noise::WebGlProfile;
    /// let p = WebGlProfile::nvidia_gtx_1660();
    /// assert!(p.renderer.contains("GTX 1660"));
    /// ```
    #[must_use]
    pub fn nvidia_gtx_1660() -> Self {
        Self {
            vendor: "Google Inc. (NVIDIA)".to_string(),
            renderer:
                "ANGLE (NVIDIA, NVIDIA GeForce GTX 1660 SUPER Direct3D11 vs_5_0 ps_5_0, D3D11)"
                    .to_string(),
            max_texture_size: 16384,
            max_viewport_dims: (32768, 32768),
            max_renderbuffer_size: 16384,
            max_vertex_attribs: 16,
            max_varying_vectors: 15,
            max_fragment_uniform_vectors: 1024,
            max_vertex_uniform_vectors: 4096,
            extensions: default_extensions(),
            shader_precision: ShaderPrecisionProfile::default(),
            context_attributes: ContextAttributes::default(),
        }
    }

    /// Return the AMD RX 6700 profile.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::webgl_noise::WebGlProfile;
    /// let p = WebGlProfile::amd_rx_6700();
    /// assert!(p.renderer.contains("RX 6700"));
    /// ```
    #[must_use]
    pub fn amd_rx_6700() -> Self {
        Self {
            vendor: "Google Inc. (AMD)".to_string(),
            renderer: "ANGLE (AMD, AMD Radeon RX 6700 XT Direct3D11 vs_5_0 ps_5_0, D3D11)"
                .to_string(),
            max_texture_size: 16384,
            max_viewport_dims: (32768, 32768),
            max_renderbuffer_size: 16384,
            max_vertex_attribs: 16,
            max_varying_vectors: 15,
            max_fragment_uniform_vectors: 1024,
            max_vertex_uniform_vectors: 4096,
            extensions: default_extensions(),
            shader_precision: ShaderPrecisionProfile::default(),
            context_attributes: ContextAttributes::default(),
        }
    }

    /// Return the Intel UHD 630 profile (integrated graphics).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::webgl_noise::WebGlProfile;
    /// let p = WebGlProfile::intel_uhd_630();
    /// assert!(p.renderer.contains("UHD Graphics 630"));
    /// ```
    #[must_use]
    pub fn intel_uhd_630() -> Self {
        Self {
            vendor: "Google Inc. (Intel)".to_string(),
            renderer: "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)"
                .to_string(),
            max_texture_size: 8192,
            max_viewport_dims: (16384, 16384),
            max_renderbuffer_size: 8192,
            max_vertex_attribs: 16,
            max_varying_vectors: 15,
            max_fragment_uniform_vectors: 1024,
            max_vertex_uniform_vectors: 4096,
            extensions: default_extensions(),
            shader_precision: ShaderPrecisionProfile::default(),
            context_attributes: ContextAttributes::default(),
        }
    }

    /// Assert basic internal consistency: texture size ≤ viewport dims, etc.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::webgl_noise::WebGlProfile;
    /// let p = WebGlProfile::nvidia_rtx_3060();
    /// p.assert_consistent();
    /// ```
    pub fn assert_consistent(&self) {
        assert!(
            self.max_texture_size <= self.max_viewport_dims.0,
            "max_texture_size must be <= max_viewport_dims.0"
        );
        assert!(
            self.max_texture_size <= self.max_viewport_dims.1,
            "max_texture_size must be <= max_viewport_dims.1"
        );
        assert!(
            self.max_renderbuffer_size <= self.max_texture_size,
            "max_renderbuffer_size must be <= max_texture_size"
        );
    }
}

fn default_extensions() -> Vec<String> {
    [
        "ANGLE_instanced_arrays",
        "EXT_blend_minmax",
        "EXT_clip_control",
        "EXT_color_buffer_half_float",
        "EXT_depth_clamp",
        "EXT_disjoint_timer_query",
        "EXT_float_blend",
        "EXT_frag_depth",
        "EXT_shader_texture_lod",
        "EXT_texture_compression_bptc",
        "EXT_texture_compression_rgtc",
        "EXT_texture_filter_anisotropic",
        "EXT_sRGB",
        "KHR_parallel_shader_compile",
        "OES_element_index_uint",
        "OES_fbo_render_mipmap",
        "OES_standard_derivatives",
        "OES_texture_float",
        "OES_texture_float_linear",
        "OES_texture_half_float",
        "OES_texture_half_float_linear",
        "OES_vertex_array_object",
        "WEBGL_color_buffer_float",
        "WEBGL_compressed_texture_s3tc",
        "WEBGL_compressed_texture_s3tc_srgb",
        "WEBGL_debug_renderer_info",
        "WEBGL_debug_shaders",
        "WEBGL_depth_texture",
        "WEBGL_draw_buffers",
        "WEBGL_lose_context",
        "WEBGL_multi_draw",
        "WEBGL_polygon_mode",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

// ---------------------------------------------------------------------------
// Script generation
// ---------------------------------------------------------------------------

/// Generate the WebGL noise injection script for a given profile and engine.
///
/// # Example
///
/// ```
/// use stygian_browser::webgl_noise::{webgl_noise_script, WebGlProfile};
/// use stygian_browser::noise::{NoiseEngine, NoiseSeed};
///
/// let js = webgl_noise_script(&WebGlProfile::nvidia_rtx_3060(), &NoiseEngine::new(NoiseSeed::from(1)));
/// assert!(js.contains("getParameter"));
/// assert!(js.contains("readPixels"));
/// ```
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn webgl_noise_script(profile: &WebGlProfile, engine: &NoiseEngine) -> String {
    let noise_fn = engine.js_noise_fn();
    let vendor = &profile.vendor;
    let renderer = &profile.renderer;
    let max_tex = profile.max_texture_size;
    let vp_w = profile.max_viewport_dims.0;
    let vp_h = profile.max_viewport_dims.1;
    let max_rb = profile.max_renderbuffer_size;
    let max_va = profile.max_vertex_attribs;
    let max_varying_vectors = profile.max_varying_vectors;
    let max_fragment_uniform_vectors = profile.max_fragment_uniform_vectors;
    let max_vertex_uniform_vectors = profile.max_vertex_uniform_vectors;

    let exts_json = {
        let items: Vec<String> = profile
            .extensions
            .iter()
            .map(|e| format!("{e:?}"))
            .collect();
        format!("[{}]", items.join(", "))
    };

    let sp = &profile.shader_precision;
    let ca = &profile.context_attributes;
    let ca_power = &ca.power_preference;

    format!(
        r"(function() {{
  'use strict';

  // ── Noise helpers ──────────────────────────────────────────────────────
  {noise_fn}

  // ── WebGL constants ────────────────────────────────────────────────────
  const _VENDOR   = 0x1F00;
  const _RENDERER = 0x1F01;
  const _UNMASKED_VENDOR   = 0x9245;
  const _UNMASKED_RENDERER = 0x9246;
  const _MAX_TEXTURE_SIZE            = 0x0D33;
  const _MAX_VIEWPORT_DIMS           = 0x0D3A;
  const _MAX_RENDERBUFFER_SIZE       = 0x84E8;
  const _MAX_VERTEX_ATTRIBS          = 0x8869;
  const _MAX_VARYING_VECTORS         = 0x8DFC;
  const _MAX_FRAGMENT_UNIFORM_VECTORS = 0x8DFD;
  const _MAX_VERTEX_UNIFORM_VECTORS  = 0x8DFB;

  const _PROFILE_VENDOR   = {vendor:?};
  const _PROFILE_RENDERER = {renderer:?};
  const _EXTENSIONS = {exts_json};

  // ── Spoof toString ─────────────────────────────────────────────────────
  function _nts(name) {{ return function toString() {{ return 'function ' + name + '() {{ [native code] }}'; }}; }}
  function _def(obj, prop, fn) {{
    fn.toString = _nts(prop);
    Object.defineProperty(obj, prop, {{ value: fn, writable: false, configurable: false, enumerable: false }});
  }}

  // ── Patch both WebGL1 and WebGL2 ────────────────────────────────────────
  [WebGLRenderingContext, (typeof WebGL2RenderingContext !== 'undefined' ? WebGL2RenderingContext : null)]
    .filter(Boolean)
    .forEach(function(Ctx) {{
      const proto = Ctx.prototype;

      // getParameter
      const _origGP = proto.getParameter;
      _def(proto, 'getParameter', function getParameter(pname) {{
        switch (pname) {{
          case _VENDOR:             return _PROFILE_VENDOR;
          case _RENDERER:           return _PROFILE_RENDERER;
          case _UNMASKED_VENDOR:    return _PROFILE_VENDOR;
          case _UNMASKED_RENDERER:  return _PROFILE_RENDERER;
          case _MAX_TEXTURE_SIZE:   return {max_tex};
          case _MAX_VIEWPORT_DIMS:  return new Int32Array([{vp_w}, {vp_h}]);
          case _MAX_RENDERBUFFER_SIZE: return {max_rb};
          case _MAX_VERTEX_ATTRIBS: return {max_va};
          case _MAX_VARYING_VECTORS: return {max_varying_vectors};
          case _MAX_FRAGMENT_UNIFORM_VECTORS: return {max_fragment_uniform_vectors};
          case _MAX_VERTEX_UNIFORM_VECTORS: return {max_vertex_uniform_vectors};
          default: return _origGP.call(this, pname);
        }}
      }});

      // getSupportedExtensions
      _def(proto, 'getSupportedExtensions', function getSupportedExtensions() {{
        return _EXTENSIONS.slice();
      }});

      // getExtension
      const _origGE = proto.getExtension;
      _def(proto, 'getExtension', function getExtension(name) {{
        if (!_EXTENSIONS.includes(name)) return null;
        return _origGE.call(this, name);
      }});

      // getShaderPrecisionFormat
      _def(proto, 'getShaderPrecisionFormat', function getShaderPrecisionFormat(shaderType, precisionType) {{
        // HIGH_FLOAT = 0x8DF2, MEDIUM_FLOAT = 0x8DF1, LOW_FLOAT = 0x8DF0
        // HIGH_INT = 0x8DF5, MEDIUM_INT = 0x8DF4, LOW_INT = 0x8DF3
        const HIGH_FLOAT = 0x8DF2, MEDIUM_FLOAT = 0x8DF1, HIGH_INT = 0x8DF5;
        if (precisionType === HIGH_FLOAT) {{
          return {{ rangeMin: {sp_hfrm}, rangeMax: {sp_hfrx}, precision: {sp_hfp} }};
        }} else if (precisionType === MEDIUM_FLOAT) {{
          return {{ rangeMin: 127, rangeMax: 127, precision: {sp_mfp} }};
        }} else if (precisionType === HIGH_INT) {{
          return {{ rangeMin: 31, rangeMax: 30, precision: {sp_hip} }};
        }}
        return {{ rangeMin: 1, rangeMax: 1, precision: 8 }};
      }});

      // getContextAttributes
      _def(proto, 'getContextAttributes', function getContextAttributes() {{
        return {{
          alpha: {ca_alpha},
          antialias: {ca_antialias},
          depth: {ca_depth},
          failIfMajorPerformanceCaveat: {ca_fail},
          powerPreference: {ca_power:?},
          premultipliedAlpha: {ca_pma},
          preserveDrawingBuffer: {ca_pdb},
          stencil: {ca_stencil},
          desynchronized: {ca_desync},
        }};
      }});

      // readPixels — apply webgl noise to output
      const _origRP = proto.readPixels;
      _def(proto, 'readPixels', function readPixels(x, y, width, height, format, type, pixels) {{
        _origRP.call(this, x, y, width, height, format, type, pixels);
        if (pixels instanceof Uint8Array || pixels instanceof Uint8ClampedArray) {{
          for (let i = 0; i < pixels.length; i += 4) {{
            const px = (x + ((i / 4) % width)) >>> 0;
            const py = (y + (((i / 4) / width) | 0)) >>> 0;
            if (pixels[i] === 0 && pixels[i+1] === 0 && pixels[i+2] === 0 && pixels[i+3] === 0) continue;
            const [dr, dg, db, da] = __stygian_webgl_noise('readPixels', px, py);
            pixels[i]   = Math.max(0, Math.min(255, pixels[i]   + dr));
            pixels[i+1] = Math.max(0, Math.min(255, pixels[i+1] + dg));
            pixels[i+2] = Math.max(0, Math.min(255, pixels[i+2] + db));
            pixels[i+3] = Math.max(0, Math.min(255, pixels[i+3] + da));
          }}
        }}
      }});
    }});
}})();
",
        noise_fn = noise_fn,
        vendor = vendor,
        renderer = renderer,
        exts_json = exts_json,
        max_tex = max_tex,
        vp_w = vp_w,
        vp_h = vp_h,
        max_rb = max_rb,
        max_va = max_va,
        max_varying_vectors = max_varying_vectors,
        max_fragment_uniform_vectors = max_fragment_uniform_vectors,
        max_vertex_uniform_vectors = max_vertex_uniform_vectors,
        sp_hfrm = sp.high_float_range_min,
        sp_hfrx = sp.high_float_range_max,
        sp_hfp = sp.high_float_precision,
        sp_mfp = sp.medium_float_precision,
        sp_hip = sp.high_int_precision,
        ca_alpha = ca.alpha,
        ca_antialias = ca.antialias,
        ca_depth = ca.depth,
        ca_fail = ca.fail_if_major_performance_caveat,
        ca_power = ca_power,
        ca_pma = ca.premultiplied_alpha,
        ca_pdb = ca.preserve_drawing_buffer,
        ca_stencil = ca.stencil,
        ca_desync = ca.desynchronized,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::{NoiseEngine, NoiseSeed};

    fn eng() -> NoiseEngine {
        NoiseEngine::new(NoiseSeed::from(1_u64))
    }

    #[test]
    fn all_profiles_consistent() {
        WebGlProfile::nvidia_rtx_3060().assert_consistent();
        WebGlProfile::nvidia_gtx_1660().assert_consistent();
        WebGlProfile::amd_rx_6700().assert_consistent();
        WebGlProfile::intel_uhd_630().assert_consistent();
    }

    #[test]
    fn script_contains_webgl_overrides() {
        let js = webgl_noise_script(&WebGlProfile::nvidia_rtx_3060(), &eng());
        assert!(js.contains("getParameter"), "missing getParameter");
        assert!(
            js.contains("getSupportedExtensions"),
            "missing getSupportedExtensions"
        );
        assert!(js.contains("getExtension"), "missing getExtension");
        assert!(
            js.contains("getShaderPrecisionFormat"),
            "missing getShaderPrecisionFormat"
        );
        assert!(
            js.contains("getContextAttributes"),
            "missing getContextAttributes"
        );
        assert!(js.contains("readPixels"), "missing readPixels");
    }

    #[test]
    fn script_contains_noise_reference() {
        let js = webgl_noise_script(&WebGlProfile::nvidia_rtx_3060(), &eng());
        assert!(
            js.contains("__stygian_webgl_noise"),
            "missing webgl noise fn"
        );
    }

    #[test]
    fn script_contains_native_tostring() {
        let js = webgl_noise_script(&WebGlProfile::nvidia_rtx_3060(), &eng());
        assert!(js.contains("[native code]"), "missing toString spoof");
    }

    #[test]
    fn profile_serde_round_trip() {
        let p = WebGlProfile::nvidia_rtx_3060();
        let json_result = serde_json::to_string(&p);
        assert!(json_result.is_ok(), "serialize failed: {json_result:?}");
        let Ok(json) = json_result else {
            return;
        };
        let back_result: Result<WebGlProfile, _> = serde_json::from_str(&json);
        assert!(back_result.is_ok(), "deserialize failed: {back_result:?}");
        let Ok(back) = back_result else {
            return;
        };
        assert_eq!(back.vendor, p.vendor);
        assert_eq!(back.renderer, p.renderer);
        assert_eq!(back.max_texture_size, p.max_texture_size);
        assert_eq!(back.extensions.len(), p.extensions.len());
    }

    #[test]
    fn script_contains_renderer_string() {
        let p = WebGlProfile::nvidia_rtx_3060();
        let js = webgl_noise_script(&p, &eng());
        assert!(js.contains("RTX 3060"), "renderer not in script");
    }
}
