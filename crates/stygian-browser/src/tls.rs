//! TLS fingerprint profile types with JA3/JA4 representation.
//!
//! Provides domain types for modelling real browser TLS fingerprints so that
//! automated sessions present cipher-suite orderings, extension lists, and
//! ALPN preferences that match genuine browsers.
//!
//! # Built-in profiles
//!
//! Four static profiles ship with real-world TLS parameters:
//!
//! | Profile | Browser |
//! |---|---|
//! | [`CHROME_131`] | Google Chrome 131 |
//! | [`FIREFOX_133`] | Mozilla Firefox 133 |
//! | [`SAFARI_18`] | Apple Safari 18 |
//! | [`EDGE_131`] | Microsoft Edge 131 |
//!
//! # Example
//!
//! ```
//! use stygian_browser::tls::{CHROME_131, TlsProfile};
//!
//! let profile: &TlsProfile = &*CHROME_131;
//! assert_eq!(profile.name, "Chrome 131");
//!
//! let ja3 = profile.ja3();
//! assert!(!ja3.raw.is_empty());
//! assert!(!ja3.hash.is_empty());
//!
//! let ja4 = profile.ja4();
//! assert!(ja4.fingerprint.starts_with("t13"));
//! ```

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::LazyLock;

// ── entropy helper ───────────────────────────────────────────────────────────

/// Splitmix64-style hash — mixes `seed` with a `step` multiplier so every
/// call with a unique `step` produces an independent random-looking value.
pub(crate) const fn rng(seed: u64, step: u64) -> u64 {
    let x = seed.wrapping_add(step.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

// ── newtype wrappers ─────────────────────────────────────────────────────────

/// TLS cipher-suite identifier (IANA two-byte code point).
///
/// Order within a [`TlsProfile`] matters — anti-bot systems compare the
/// ordering against known browser fingerprints.
///
/// # Example
///
/// ```
/// use stygian_browser::tls::CipherSuiteId;
///
/// let aes128 = CipherSuiteId::TLS_AES_128_GCM_SHA256;
/// assert_eq!(aes128.0, 0x1301);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CipherSuiteId(pub u16);

impl CipherSuiteId {
    /// TLS 1.3 — AES-128-GCM with SHA-256.
    pub const TLS_AES_128_GCM_SHA256: Self = Self(0x1301);
    /// TLS 1.3 — AES-256-GCM with SHA-384.
    pub const TLS_AES_256_GCM_SHA384: Self = Self(0x1302);
    /// TLS 1.3 — ChaCha20-Poly1305 with SHA-256.
    pub const TLS_CHACHA20_POLY1305_SHA256: Self = Self(0x1303);
    /// TLS 1.2 — ECDHE-ECDSA-AES128-GCM-SHA256.
    pub const TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256: Self = Self(0xc02b);
    /// TLS 1.2 — ECDHE-RSA-AES128-GCM-SHA256.
    pub const TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256: Self = Self(0xc02f);
    /// TLS 1.2 — ECDHE-ECDSA-AES256-GCM-SHA384.
    pub const TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384: Self = Self(0xc02c);
    /// TLS 1.2 — ECDHE-RSA-AES256-GCM-SHA384.
    pub const TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384: Self = Self(0xc030);
    /// TLS 1.2 — ECDHE-ECDSA-CHACHA20-POLY1305.
    pub const TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256: Self = Self(0xcca9);
    /// TLS 1.2 — ECDHE-RSA-CHACHA20-POLY1305.
    pub const TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256: Self = Self(0xcca8);
    /// TLS 1.2 — ECDHE-RSA-AES128-SHA.
    pub const TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA: Self = Self(0xc013);
    /// TLS 1.2 — ECDHE-RSA-AES256-SHA.
    pub const TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA: Self = Self(0xc014);
    /// TLS 1.2 — RSA-AES128-GCM-SHA256.
    pub const TLS_RSA_WITH_AES_128_GCM_SHA256: Self = Self(0x009c);
    /// TLS 1.2 — RSA-AES256-GCM-SHA384.
    pub const TLS_RSA_WITH_AES_256_GCM_SHA384: Self = Self(0x009d);
    /// TLS 1.2 — RSA-AES128-SHA.
    pub const TLS_RSA_WITH_AES_128_CBC_SHA: Self = Self(0x002f);
    /// TLS 1.2 — RSA-AES256-SHA.
    pub const TLS_RSA_WITH_AES_256_CBC_SHA: Self = Self(0x0035);
}

impl fmt::Display for CipherSuiteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// TLS protocol version.
///
/// # Example
///
/// ```
/// use stygian_browser::tls::TlsVersion;
///
/// let v = TlsVersion::Tls13;
/// assert_eq!(v.iana_value(), 0x0304);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TlsVersion {
    /// TLS 1.2 (0x0303).
    Tls12,
    /// TLS 1.3 (0x0304).
    Tls13,
}

impl TlsVersion {
    /// Return the two-byte IANA protocol version number.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::tls::TlsVersion;
    ///
    /// assert_eq!(TlsVersion::Tls12.iana_value(), 0x0303);
    /// ```
    pub const fn iana_value(self) -> u16 {
        match self {
            Self::Tls12 => 0x0303,
            Self::Tls13 => 0x0304,
        }
    }
}

impl fmt::Display for TlsVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.iana_value())
    }
}

/// TLS extension identifier (IANA two-byte code point).
///
/// # Example
///
/// ```
/// use stygian_browser::tls::TlsExtensionId;
///
/// let sni = TlsExtensionId::SERVER_NAME;
/// assert_eq!(sni.0, 0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TlsExtensionId(pub u16);

impl TlsExtensionId {
    /// server_name (SNI).
    pub const SERVER_NAME: Self = Self(0);
    /// extended_master_secret.
    pub const EXTENDED_MASTER_SECRET: Self = Self(23);
    /// encrypt_then_mac.
    pub const ENCRYPT_THEN_MAC: Self = Self(22);
    /// session_ticket.
    pub const SESSION_TICKET: Self = Self(35);
    /// signature_algorithms.
    pub const SIGNATURE_ALGORITHMS: Self = Self(13);
    /// supported_versions.
    pub const SUPPORTED_VERSIONS: Self = Self(43);
    /// psk_key_exchange_modes.
    pub const PSK_KEY_EXCHANGE_MODES: Self = Self(45);
    /// key_share.
    pub const KEY_SHARE: Self = Self(51);
    /// supported_groups (a.k.a. elliptic_curves).
    pub const SUPPORTED_GROUPS: Self = Self(10);
    /// ec_point_formats.
    pub const EC_POINT_FORMATS: Self = Self(11);
    /// application_layer_protocol_negotiation.
    pub const ALPN: Self = Self(16);
    /// status_request (OCSP stapling).
    pub const STATUS_REQUEST: Self = Self(5);
    /// signed_certificate_timestamp.
    pub const SIGNED_CERTIFICATE_TIMESTAMP: Self = Self(18);
    /// compress_certificate.
    pub const COMPRESS_CERTIFICATE: Self = Self(27);
    /// application_settings (ALPS).
    pub const APPLICATION_SETTINGS: Self = Self(17513);
    /// renegotiation_info.
    pub const RENEGOTIATION_INFO: Self = Self(0xff01);
    /// delegated_credentials.
    pub const DELEGATED_CREDENTIALS: Self = Self(34);
    /// record_size_limit.
    pub const RECORD_SIZE_LIMIT: Self = Self(28);
    /// padding.
    pub const PADDING: Self = Self(21);
    /// pre_shared_key.
    pub const PRE_SHARED_KEY: Self = Self(41);
    /// post_handshake_auth.
    pub const POST_HANDSHAKE_AUTH: Self = Self(49);
}

impl fmt::Display for TlsExtensionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Named group (elliptic curve / key-exchange group) identifier.
///
/// # Example
///
/// ```
/// use stygian_browser::tls::SupportedGroup;
///
/// let x25519 = SupportedGroup::X25519;
/// assert_eq!(x25519.iana_value(), 0x001d);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SupportedGroup {
    /// X25519 Diffie-Hellman (0x001d).
    X25519,
    /// secp256r1 / P-256 (0x0017).
    SecP256r1,
    /// secp384r1 / P-384 (0x0018).
    SecP384r1,
    /// secp521r1 / P-521 (0x0019).
    SecP521r1,
    /// X25519Kyber768Draft00 — post-quantum hybrid (0x6399).
    X25519Kyber768,
    /// FFDHE2048 (0x0100).
    Ffdhe2048,
    /// FFDHE3072 (0x0101).
    Ffdhe3072,
}

impl SupportedGroup {
    /// Return the two-byte IANA named-group value.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::tls::SupportedGroup;
    ///
    /// assert_eq!(SupportedGroup::SecP256r1.iana_value(), 0x0017);
    /// ```
    pub const fn iana_value(self) -> u16 {
        match self {
            Self::X25519 => 0x001d,
            Self::SecP256r1 => 0x0017,
            Self::SecP384r1 => 0x0018,
            Self::SecP521r1 => 0x0019,
            Self::X25519Kyber768 => 0x6399,
            Self::Ffdhe2048 => 0x0100,
            Self::Ffdhe3072 => 0x0101,
        }
    }
}

impl fmt::Display for SupportedGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.iana_value())
    }
}

/// TLS signature algorithm identifier (IANA two-byte code point).
///
/// # Example
///
/// ```
/// use stygian_browser::tls::SignatureAlgorithm;
///
/// let ecdsa = SignatureAlgorithm::ECDSA_SECP256R1_SHA256;
/// assert_eq!(ecdsa.0, 0x0403);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SignatureAlgorithm(pub u16);

impl SignatureAlgorithm {
    /// ecdsa_secp256r1_sha256.
    pub const ECDSA_SECP256R1_SHA256: Self = Self(0x0403);
    /// rsa_pss_rsae_sha256.
    pub const RSA_PSS_RSAE_SHA256: Self = Self(0x0804);
    /// rsa_pkcs1_sha256.
    pub const RSA_PKCS1_SHA256: Self = Self(0x0401);
    /// ecdsa_secp384r1_sha384.
    pub const ECDSA_SECP384R1_SHA384: Self = Self(0x0503);
    /// rsa_pss_rsae_sha384.
    pub const RSA_PSS_RSAE_SHA384: Self = Self(0x0805);
    /// rsa_pkcs1_sha384.
    pub const RSA_PKCS1_SHA384: Self = Self(0x0501);
    /// rsa_pss_rsae_sha512.
    pub const RSA_PSS_RSAE_SHA512: Self = Self(0x0806);
    /// rsa_pkcs1_sha512.
    pub const RSA_PKCS1_SHA512: Self = Self(0x0601);
    /// ecdsa_secp521r1_sha512.
    pub const ECDSA_SECP521R1_SHA512: Self = Self(0x0603);
    /// rsa_pkcs1_sha1 (legacy).
    pub const RSA_PKCS1_SHA1: Self = Self(0x0201);
    /// ecdsa_sha1 (legacy).
    pub const ECDSA_SHA1: Self = Self(0x0203);
}

impl fmt::Display for SignatureAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// ALPN protocol identifier negotiated during the TLS handshake.
///
/// # Example
///
/// ```
/// use stygian_browser::tls::AlpnProtocol;
///
/// assert_eq!(AlpnProtocol::H2.as_str(), "h2");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AlpnProtocol {
    /// HTTP/2 (`h2`).
    H2,
    /// HTTP/1.1 (`http/1.1`).
    Http11,
}

impl AlpnProtocol {
    /// Return the ALPN wire-format string.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::tls::AlpnProtocol;
    ///
    /// assert_eq!(AlpnProtocol::Http11.as_str(), "http/1.1");
    /// ```
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::H2 => "h2",
            Self::Http11 => "http/1.1",
        }
    }
}

impl fmt::Display for AlpnProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── TLS profile ──────────────────────────────────────────────────────────────

/// A complete TLS fingerprint profile matching a real browser's ClientHello.
///
/// The ordering of cipher suites, extensions, and supported groups matters —
/// anti-bot systems compare these orderings against known browser signatures.
///
/// # Example
///
/// ```
/// use stygian_browser::tls::{CHROME_131, TlsProfile};
///
/// let profile: &TlsProfile = &*CHROME_131;
/// assert_eq!(profile.name, "Chrome 131");
/// assert!(!profile.cipher_suites.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TlsProfile {
    /// Human-readable profile name (e.g. `"Chrome 131"`).
    pub name: String,
    /// Ordered cipher-suite list from the ClientHello.
    pub cipher_suites: Vec<CipherSuiteId>,
    /// Supported TLS protocol versions.
    pub tls_versions: Vec<TlsVersion>,
    /// Ordered extension list from the ClientHello.
    pub extensions: Vec<TlsExtensionId>,
    /// Supported named groups (elliptic curves / key exchange).
    pub supported_groups: Vec<SupportedGroup>,
    /// Supported signature algorithms.
    pub signature_algorithms: Vec<SignatureAlgorithm>,
    /// ALPN protocol list.
    pub alpn_protocols: Vec<AlpnProtocol>,
}

// ── JA3 ──────────────────────────────────────────────────────────────────────

/// JA3 TLS fingerprint — raw descriptor string and its MD5 hash.
///
/// The JA3 format is:
/// `TLSVersion,Ciphers,Extensions,EllipticCurves,EcPointFormats`
///
/// Fields within each section are dash-separated.
///
/// # Example
///
/// ```
/// use stygian_browser::tls::CHROME_131;
///
/// let ja3 = CHROME_131.ja3();
/// assert!(ja3.raw.contains(','));
/// assert_eq!(ja3.hash.len(), 32); // MD5 hex digest
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ja3Hash {
    /// Comma-separated JA3 descriptor string.
    pub raw: String,
    /// MD5 hex digest of [`raw`](Ja3Hash::raw).
    pub hash: String,
}

impl fmt::Display for Ja3Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hash)
    }
}

/// Compute MD5 of `data` and return a 32-char lowercase hex string.
///
/// This is a minimal, self-contained MD5 implementation used only for JA3 hash
/// computation. It avoids pulling in an external crate for a single use-site.
fn md5_hex(data: &[u8]) -> String {
    // Per-round shift amounts.
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];

    // Pre-computed T[i] = floor(2^32 * |sin(i+1)|).
    const K: [u32; 64] = [
        0xd76a_a478,
        0xe8c7_b756,
        0x2420_70db,
        0xc1bd_ceee,
        0xf57c_0faf,
        0x4787_c62a,
        0xa830_4613,
        0xfd46_9501,
        0x6980_98d8,
        0x8b44_f7af,
        0xffff_5bb1,
        0x895c_d7be,
        0x6b90_1122,
        0xfd98_7193,
        0xa679_438e,
        0x49b4_0821,
        0xf61e_2562,
        0xc040_b340,
        0x265e_5a51,
        0xe9b6_c7aa,
        0xd62f_105d,
        0x0244_1453,
        0xd8a1_e681,
        0xe7d3_fbc8,
        0x21e1_cde6,
        0xc337_07d6,
        0xf4d5_0d87,
        0x455a_14ed,
        0xa9e3_e905,
        0xfcef_a3f8,
        0x676f_02d9,
        0x8d2a_4c8a,
        0xfffa_3942,
        0x8771_f681,
        0x6d9d_6122,
        0xfde5_380c,
        0xa4be_ea44,
        0x4bde_cfa9,
        0xf6bb_4b60,
        0xbebf_bc70,
        0x289b_7ec6,
        0xeaa1_27fa,
        0xd4ef_3085,
        0x0488_1d05,
        0xd9d4_d039,
        0xe6db_99e5,
        0x1fa2_7cf8,
        0xc4ac_5665,
        0xf429_2244,
        0x432a_ff97,
        0xab94_23a7,
        0xfc93_a039,
        0x655b_59c3,
        0x8f0c_cc92,
        0xffef_f47d,
        0x8584_5dd1,
        0x6fa8_7e4f,
        0xfe2c_e6e0,
        0xa301_4314,
        0x4e08_11a1,
        0xf753_7e82,
        0xbd3a_f235,
        0x2ad7_d2bb,
        0xeb86_d391,
    ];

    // Pre-processing: add padding.
    let orig_len_bits = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&orig_len_bits.to_le_bytes());

    let mut a0: u32 = 0x6745_2301;
    let mut b0: u32 = 0xefcd_ab89;
    let mut c0: u32 = 0x98ba_dcfe;
    let mut d0: u32 = 0x1032_5476;

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            let off = i * 4;
            *word =
                u32::from_le_bytes([chunk[off], chunk[off + 1], chunk[off + 2], chunk[off + 3]]);
        }

        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let digest = [
        a0.to_le_bytes(),
        b0.to_le_bytes(),
        c0.to_le_bytes(),
        d0.to_le_bytes(),
    ];
    let mut hex = String::with_capacity(32);
    for group in &digest {
        for &byte in group {
            use fmt::Write;
            let _ = write!(hex, "{byte:02x}");
        }
    }
    hex
}

// ── JA4 ──────────────────────────────────────────────────────────────────────

/// JA4 TLS fingerprint — the modern successor to JA3.
///
/// Format: `{proto}{version}{sni}{cipher_count}{ext_count}_{sorted_ciphers_hash}_{sorted_exts_hash}`
///
/// # Example
///
/// ```
/// use stygian_browser::tls::CHROME_131;
///
/// let ja4 = CHROME_131.ja4();
/// assert!(ja4.fingerprint.starts_with("t13"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ja4 {
    /// The full JA4 fingerprint string.
    pub fingerprint: String,
}

impl fmt::Display for Ja4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.fingerprint)
    }
}

// ── profile methods ──────────────────────────────────────────────────────────

/// GREASE values that must be ignored during JA3/JA4 computation.
const GREASE_VALUES: &[u16] = &[
    0x0a0a, 0x1a1a, 0x2a2a, 0x3a3a, 0x4a4a, 0x5a5a, 0x6a6a, 0x7a7a, 0x8a8a, 0x9a9a, 0xaaaa, 0xbaba,
    0xcaca, 0xdada, 0xeaea, 0xfafa,
];

/// Return `true` if `v` is a TLS GREASE value.
fn is_grease(v: u16) -> bool {
    GREASE_VALUES.contains(&v)
}

impl TlsProfile {
    /// Compute the JA3 fingerprint for this profile.
    ///
    /// JA3 format: `TLSVersion,Ciphers,Extensions,EllipticCurves,EcPointFormats`
    ///
    /// - GREASE values are stripped from all fields.
    /// - EC point formats default to `0` (uncompressed) when not otherwise
    ///   specified in the profile.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::tls::CHROME_131;
    ///
    /// let ja3 = CHROME_131.ja3();
    /// assert!(ja3.raw.starts_with("772,"));
    /// assert_eq!(ja3.hash.len(), 32);
    /// ```
    pub fn ja3(&self) -> Ja3Hash {
        // TLS version — use highest advertised.
        let tls_ver = self
            .tls_versions
            .iter()
            .map(|v| v.iana_value())
            .max()
            .unwrap_or(TlsVersion::Tls12.iana_value());

        // Ciphers (GREASE stripped).
        let ciphers: Vec<String> = self
            .cipher_suites
            .iter()
            .filter(|c| !is_grease(c.0))
            .map(|c| c.0.to_string())
            .collect();

        // Extensions (GREASE stripped).
        let extensions: Vec<String> = self
            .extensions
            .iter()
            .filter(|e| !is_grease(e.0))
            .map(|e| e.0.to_string())
            .collect();

        // Elliptic curves (GREASE stripped).
        let curves: Vec<String> = self
            .supported_groups
            .iter()
            .filter(|g| !is_grease(g.iana_value()))
            .map(|g| g.iana_value().to_string())
            .collect();

        // EC point formats — default to uncompressed (0).
        let ec_point_formats = "0";

        let raw = format!(
            "{tls_ver},{},{},{},{ec_point_formats}",
            ciphers.join("-"),
            extensions.join("-"),
            curves.join("-"),
        );

        let hash = md5_hex(raw.as_bytes());
        Ja3Hash { raw, hash }
    }

    /// Compute the JA4 fingerprint for this profile.
    ///
    /// JA4 format (JA4_a section):
    /// `{q}{version}{sni}{cipher_count:02}{ext_count:02}_{alpn}_{sorted_cipher_hash}_{sorted_ext_hash}`
    ///
    /// This implements the JA4_a (raw fingerprint) portion. Sorted cipher and
    /// extension hashes use the first 12 hex characters of the SHA-256 —
    /// approximated here by truncated MD5 since we already have that
    /// implementation and the goal is fingerprint *representation*, not
    /// cryptographic security.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::tls::CHROME_131;
    ///
    /// let ja4 = CHROME_131.ja4();
    /// assert!(ja4.fingerprint.starts_with("t13"));
    /// ```
    pub fn ja4(&self) -> Ja4 {
        // Protocol: 't' for TCP TLS.
        let proto = 't';

        // TLS version: highest advertised, mapped to two-char code.
        let version = if self.tls_versions.contains(&TlsVersion::Tls13) {
            "13"
        } else {
            "12"
        };

        // SNI: 'd' = domain (SNI present), 'i' = IP (no SNI). We assume SNI
        // is present for browser profiles.
        let sni = 'd';

        // Counts (GREASE stripped), capped at 99.
        let cipher_count = self
            .cipher_suites
            .iter()
            .filter(|c| !is_grease(c.0))
            .count()
            .min(99);
        let ext_count = self
            .extensions
            .iter()
            .filter(|e| !is_grease(e.0))
            .count()
            .min(99);

        // ALPN: first protocol letter ('h' for h2, 'h' for http/1.1 — JA4
        // uses first+last chars). '00' when empty.
        let alpn_tag = match self.alpn_protocols.first() {
            Some(AlpnProtocol::H2) => "h2",
            Some(AlpnProtocol::Http11) => "h1",
            None => "00",
        };

        // Section a (the short fingerprint before hashes).
        let section_a = format!("{proto}{version}{sni}{cipher_count:02}{ext_count:02}_{alpn_tag}",);

        // Section b: sorted cipher suites (GREASE stripped), comma-separated,
        // hashed, first 12 hex chars.
        let mut sorted_ciphers: Vec<u16> = self
            .cipher_suites
            .iter()
            .filter(|c| !is_grease(c.0))
            .map(|c| c.0)
            .collect();
        sorted_ciphers.sort_unstable();
        let cipher_str: String = sorted_ciphers
            .iter()
            .map(|c| format!("{c:04x}"))
            .collect::<Vec<_>>()
            .join(",");
        let cipher_hash = &md5_hex(cipher_str.as_bytes())[..12];

        // Section c: sorted extensions (GREASE + SNI + ALPN stripped),
        // comma-separated, hashed, first 12 hex chars.
        let mut sorted_exts: Vec<u16> = self
            .extensions
            .iter()
            .filter(|e| {
                !is_grease(e.0)
                    && e.0 != TlsExtensionId::SERVER_NAME.0
                    && e.0 != TlsExtensionId::ALPN.0
            })
            .map(|e| e.0)
            .collect();
        sorted_exts.sort_unstable();
        let ext_str: String = sorted_exts
            .iter()
            .map(|e| format!("{e:04x}"))
            .collect::<Vec<_>>()
            .join(",");
        let ext_hash = &md5_hex(ext_str.as_bytes())[..12];

        Ja4 {
            fingerprint: format!("{section_a}_{cipher_hash}_{ext_hash}"),
        }
    }

    /// Select a built-in TLS profile weighted by real browser market share.
    ///
    /// Distribution mirrors [`DeviceProfile`](super::fingerprint::DeviceProfile)
    /// and [`BrowserKind`](super::fingerprint::BrowserKind) weights:
    ///
    /// - Windows (70%): Chrome 65%, Edge 16%, Firefox 19%
    /// - macOS (20%): Chrome 56%, Safari 36%, Firefox 8%
    /// - Linux (10%): Chrome 65%, Edge 16%, Firefox 19%
    ///
    /// Edge 131 shares Chrome's Blink engine so its TLS stack is nearly
    /// identical; the profile uses [`EDGE_131`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::tls::TlsProfile;
    ///
    /// let profile = TlsProfile::random_weighted(42);
    /// assert!(!profile.name.is_empty());
    /// ```
    pub fn random_weighted(seed: u64) -> &'static TlsProfile {
        // Step 1: pick OS (Windows 70%, Mac 20%, Linux 10%).
        let os_roll = rng(seed, 97) % 100;

        // Step 2: pick browser within that OS.
        let browser_roll = rng(seed, 201) % 100;

        match os_roll {
            // Windows / Linux: Chrome 65%, Edge 16%, Firefox 19%.
            0..=69 | 90..=99 => match browser_roll {
                0..=64 => &CHROME_131,
                65..=80 => &EDGE_131,
                _ => &FIREFOX_133,
            },
            // macOS: Chrome 56%, Safari 36%, Firefox 8%.
            _ => match browser_roll {
                0..=55 => &CHROME_131,
                56..=91 => &SAFARI_18,
                _ => &FIREFOX_133,
            },
        }
    }
}

// ── built-in profiles ────────────────────────────────────────────────────────

/// Google Chrome 131 TLS fingerprint profile.
///
/// Cipher suites, extensions, and groups sourced from real Chrome 131
/// ClientHello captures.
///
/// # Example
///
/// ```
/// use stygian_browser::tls::CHROME_131;
///
/// assert_eq!(CHROME_131.name, "Chrome 131");
/// assert!(CHROME_131.tls_versions.contains(&stygian_browser::tls::TlsVersion::Tls13));
/// ```
pub static CHROME_131: LazyLock<TlsProfile> = LazyLock::new(|| TlsProfile {
    name: "Chrome 131".to_string(),
    cipher_suites: vec![
        CipherSuiteId::TLS_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_CHACHA20_POLY1305_SHA256,
        CipherSuiteId::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA,
        CipherSuiteId::TLS_RSA_WITH_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_RSA_WITH_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_RSA_WITH_AES_128_CBC_SHA,
        CipherSuiteId::TLS_RSA_WITH_AES_256_CBC_SHA,
    ],
    tls_versions: vec![TlsVersion::Tls12, TlsVersion::Tls13],
    extensions: vec![
        TlsExtensionId::SERVER_NAME,
        TlsExtensionId::EXTENDED_MASTER_SECRET,
        TlsExtensionId::RENEGOTIATION_INFO,
        TlsExtensionId::SUPPORTED_GROUPS,
        TlsExtensionId::EC_POINT_FORMATS,
        TlsExtensionId::SESSION_TICKET,
        TlsExtensionId::ALPN,
        TlsExtensionId::STATUS_REQUEST,
        TlsExtensionId::SIGNATURE_ALGORITHMS,
        TlsExtensionId::SIGNED_CERTIFICATE_TIMESTAMP,
        TlsExtensionId::KEY_SHARE,
        TlsExtensionId::PSK_KEY_EXCHANGE_MODES,
        TlsExtensionId::SUPPORTED_VERSIONS,
        TlsExtensionId::COMPRESS_CERTIFICATE,
        TlsExtensionId::APPLICATION_SETTINGS,
        TlsExtensionId::PADDING,
    ],
    supported_groups: vec![
        SupportedGroup::X25519Kyber768,
        SupportedGroup::X25519,
        SupportedGroup::SecP256r1,
        SupportedGroup::SecP384r1,
    ],
    signature_algorithms: vec![
        SignatureAlgorithm::ECDSA_SECP256R1_SHA256,
        SignatureAlgorithm::RSA_PSS_RSAE_SHA256,
        SignatureAlgorithm::RSA_PKCS1_SHA256,
        SignatureAlgorithm::ECDSA_SECP384R1_SHA384,
        SignatureAlgorithm::RSA_PSS_RSAE_SHA384,
        SignatureAlgorithm::RSA_PKCS1_SHA384,
        SignatureAlgorithm::RSA_PSS_RSAE_SHA512,
        SignatureAlgorithm::RSA_PKCS1_SHA512,
    ],
    alpn_protocols: vec![AlpnProtocol::H2, AlpnProtocol::Http11],
});

/// Mozilla Firefox 133 TLS fingerprint profile.
///
/// Firefox uses a different cipher-suite and extension order than Chromium
/// browsers, notably preferring `ChaCha20` and including `delegated_credentials`
/// and `record_size_limit`.
///
/// # Example
///
/// ```
/// use stygian_browser::tls::FIREFOX_133;
///
/// assert_eq!(FIREFOX_133.name, "Firefox 133");
/// ```
pub static FIREFOX_133: LazyLock<TlsProfile> = LazyLock::new(|| TlsProfile {
    name: "Firefox 133".to_string(),
    cipher_suites: vec![
        CipherSuiteId::TLS_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_CHACHA20_POLY1305_SHA256,
        CipherSuiteId::TLS_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
        CipherSuiteId::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA,
        CipherSuiteId::TLS_RSA_WITH_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_RSA_WITH_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_RSA_WITH_AES_128_CBC_SHA,
        CipherSuiteId::TLS_RSA_WITH_AES_256_CBC_SHA,
    ],
    tls_versions: vec![TlsVersion::Tls12, TlsVersion::Tls13],
    extensions: vec![
        TlsExtensionId::SERVER_NAME,
        TlsExtensionId::EXTENDED_MASTER_SECRET,
        TlsExtensionId::RENEGOTIATION_INFO,
        TlsExtensionId::SUPPORTED_GROUPS,
        TlsExtensionId::EC_POINT_FORMATS,
        TlsExtensionId::SESSION_TICKET,
        TlsExtensionId::ALPN,
        TlsExtensionId::STATUS_REQUEST,
        TlsExtensionId::DELEGATED_CREDENTIALS,
        TlsExtensionId::KEY_SHARE,
        TlsExtensionId::SUPPORTED_VERSIONS,
        TlsExtensionId::SIGNATURE_ALGORITHMS,
        TlsExtensionId::PSK_KEY_EXCHANGE_MODES,
        TlsExtensionId::RECORD_SIZE_LIMIT,
        TlsExtensionId::POST_HANDSHAKE_AUTH,
        TlsExtensionId::PADDING,
    ],
    supported_groups: vec![
        SupportedGroup::X25519,
        SupportedGroup::SecP256r1,
        SupportedGroup::SecP384r1,
        SupportedGroup::SecP521r1,
        SupportedGroup::Ffdhe2048,
        SupportedGroup::Ffdhe3072,
    ],
    signature_algorithms: vec![
        SignatureAlgorithm::ECDSA_SECP256R1_SHA256,
        SignatureAlgorithm::ECDSA_SECP384R1_SHA384,
        SignatureAlgorithm::ECDSA_SECP521R1_SHA512,
        SignatureAlgorithm::RSA_PSS_RSAE_SHA256,
        SignatureAlgorithm::RSA_PSS_RSAE_SHA384,
        SignatureAlgorithm::RSA_PSS_RSAE_SHA512,
        SignatureAlgorithm::RSA_PKCS1_SHA256,
        SignatureAlgorithm::RSA_PKCS1_SHA384,
        SignatureAlgorithm::RSA_PKCS1_SHA512,
        SignatureAlgorithm::ECDSA_SHA1,
        SignatureAlgorithm::RSA_PKCS1_SHA1,
    ],
    alpn_protocols: vec![AlpnProtocol::H2, AlpnProtocol::Http11],
});

/// Apple Safari 18 TLS fingerprint profile.
///
/// Safari's TLS stack differs from Chromium in extension order and supported
/// groups. Notably Safari does not advertise post-quantum key exchange.
///
/// # Example
///
/// ```
/// use stygian_browser::tls::SAFARI_18;
///
/// assert_eq!(SAFARI_18.name, "Safari 18");
/// ```
pub static SAFARI_18: LazyLock<TlsProfile> = LazyLock::new(|| TlsProfile {
    name: "Safari 18".to_string(),
    cipher_suites: vec![
        CipherSuiteId::TLS_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_CHACHA20_POLY1305_SHA256,
        CipherSuiteId::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
        CipherSuiteId::TLS_RSA_WITH_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_RSA_WITH_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_RSA_WITH_AES_256_CBC_SHA,
        CipherSuiteId::TLS_RSA_WITH_AES_128_CBC_SHA,
    ],
    tls_versions: vec![TlsVersion::Tls12, TlsVersion::Tls13],
    extensions: vec![
        TlsExtensionId::SERVER_NAME,
        TlsExtensionId::EXTENDED_MASTER_SECRET,
        TlsExtensionId::RENEGOTIATION_INFO,
        TlsExtensionId::SUPPORTED_GROUPS,
        TlsExtensionId::EC_POINT_FORMATS,
        TlsExtensionId::ALPN,
        TlsExtensionId::STATUS_REQUEST,
        TlsExtensionId::SIGNATURE_ALGORITHMS,
        TlsExtensionId::SIGNED_CERTIFICATE_TIMESTAMP,
        TlsExtensionId::KEY_SHARE,
        TlsExtensionId::PSK_KEY_EXCHANGE_MODES,
        TlsExtensionId::SUPPORTED_VERSIONS,
        TlsExtensionId::PADDING,
    ],
    supported_groups: vec![
        SupportedGroup::X25519,
        SupportedGroup::SecP256r1,
        SupportedGroup::SecP384r1,
        SupportedGroup::SecP521r1,
    ],
    signature_algorithms: vec![
        SignatureAlgorithm::ECDSA_SECP256R1_SHA256,
        SignatureAlgorithm::RSA_PSS_RSAE_SHA256,
        SignatureAlgorithm::RSA_PKCS1_SHA256,
        SignatureAlgorithm::ECDSA_SECP384R1_SHA384,
        SignatureAlgorithm::RSA_PSS_RSAE_SHA384,
        SignatureAlgorithm::RSA_PKCS1_SHA384,
        SignatureAlgorithm::RSA_PSS_RSAE_SHA512,
        SignatureAlgorithm::RSA_PKCS1_SHA512,
    ],
    alpn_protocols: vec![AlpnProtocol::H2, AlpnProtocol::Http11],
});

/// Microsoft Edge 131 TLS fingerprint profile.
///
/// Edge is Chromium-based so its TLS stack is nearly identical to Chrome.
/// Differences are minor (e.g. extension ordering around `application_settings`).
///
/// # Example
///
/// ```
/// use stygian_browser::tls::EDGE_131;
///
/// assert_eq!(EDGE_131.name, "Edge 131");
/// ```
pub static EDGE_131: LazyLock<TlsProfile> = LazyLock::new(|| TlsProfile {
    name: "Edge 131".to_string(),
    cipher_suites: vec![
        CipherSuiteId::TLS_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_CHACHA20_POLY1305_SHA256,
        CipherSuiteId::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA,
        CipherSuiteId::TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA,
        CipherSuiteId::TLS_RSA_WITH_AES_128_GCM_SHA256,
        CipherSuiteId::TLS_RSA_WITH_AES_256_GCM_SHA384,
        CipherSuiteId::TLS_RSA_WITH_AES_128_CBC_SHA,
        CipherSuiteId::TLS_RSA_WITH_AES_256_CBC_SHA,
    ],
    tls_versions: vec![TlsVersion::Tls12, TlsVersion::Tls13],
    extensions: vec![
        TlsExtensionId::SERVER_NAME,
        TlsExtensionId::EXTENDED_MASTER_SECRET,
        TlsExtensionId::RENEGOTIATION_INFO,
        TlsExtensionId::SUPPORTED_GROUPS,
        TlsExtensionId::EC_POINT_FORMATS,
        TlsExtensionId::SESSION_TICKET,
        TlsExtensionId::ALPN,
        TlsExtensionId::STATUS_REQUEST,
        TlsExtensionId::SIGNATURE_ALGORITHMS,
        TlsExtensionId::SIGNED_CERTIFICATE_TIMESTAMP,
        TlsExtensionId::KEY_SHARE,
        TlsExtensionId::PSK_KEY_EXCHANGE_MODES,
        TlsExtensionId::SUPPORTED_VERSIONS,
        TlsExtensionId::COMPRESS_CERTIFICATE,
        TlsExtensionId::PADDING,
    ],
    supported_groups: vec![
        SupportedGroup::X25519Kyber768,
        SupportedGroup::X25519,
        SupportedGroup::SecP256r1,
        SupportedGroup::SecP384r1,
    ],
    signature_algorithms: vec![
        SignatureAlgorithm::ECDSA_SECP256R1_SHA256,
        SignatureAlgorithm::RSA_PSS_RSAE_SHA256,
        SignatureAlgorithm::RSA_PKCS1_SHA256,
        SignatureAlgorithm::ECDSA_SECP384R1_SHA384,
        SignatureAlgorithm::RSA_PSS_RSAE_SHA384,
        SignatureAlgorithm::RSA_PKCS1_SHA384,
        SignatureAlgorithm::RSA_PSS_RSAE_SHA512,
        SignatureAlgorithm::RSA_PKCS1_SHA512,
    ],
    alpn_protocols: vec![AlpnProtocol::H2, AlpnProtocol::Http11],
});

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_known_vectors() {
        // RFC 1321 test vectors.
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
    }

    #[test]
    fn chrome_131_ja3_structure() {
        let ja3 = CHROME_131.ja3();
        // Must start with 771 (TLS 1.2 = 0x0303 = 771 is the *highest* in
        // the supported list, but TLS 1.3 = 0x0304 = 772 is also present;
        // ja3 picks max → 772).
        assert!(
            ja3.raw.starts_with("772,"),
            "JA3 raw should start with '772,' but was: {}",
            ja3.raw
        );
        // Has five comma-separated sections.
        assert_eq!(ja3.raw.matches(',').count(), 4);
        // Hash is 32 hex chars.
        assert_eq!(ja3.hash.len(), 32);
        assert!(ja3.hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn firefox_133_ja3_differs_from_chrome() {
        let chrome_ja3 = CHROME_131.ja3();
        let firefox_ja3 = FIREFOX_133.ja3();
        assert_ne!(chrome_ja3.hash, firefox_ja3.hash);
        assert_ne!(chrome_ja3.raw, firefox_ja3.raw);
    }

    #[test]
    fn safari_18_ja3_is_valid() {
        let ja3 = SAFARI_18.ja3();
        assert!(ja3.raw.starts_with("772,"));
        assert_eq!(ja3.hash.len(), 32);
    }

    #[test]
    fn edge_131_ja3_differs_from_chrome() {
        // Edge omits `APPLICATION_SETTINGS` extension compared to Chrome.
        let chrome_ja3 = CHROME_131.ja3();
        let edge_ja3 = EDGE_131.ja3();
        assert_ne!(chrome_ja3.hash, edge_ja3.hash);
    }

    #[test]
    fn chrome_131_ja4_format() {
        let ja4 = CHROME_131.ja4();
        // Starts with 't13d' (TCP, TLS 1.3, domain SNI).
        assert!(
            ja4.fingerprint.starts_with("t13d"),
            "JA4 should start with 't13d' but was: {}",
            ja4.fingerprint
        );
        // Three underscore-separated sections.
        assert_eq!(
            ja4.fingerprint.matches('_').count(),
            3,
            "JA4 should have three underscores: {}",
            ja4.fingerprint
        );
    }

    #[test]
    fn ja4_firefox_differs_from_chrome() {
        let chrome_ja4 = CHROME_131.ja4();
        let firefox_ja4 = FIREFOX_133.ja4();
        assert_ne!(chrome_ja4.fingerprint, firefox_ja4.fingerprint);
    }

    #[test]
    fn random_weighted_distribution() {
        let mut chrome_count = 0u32;
        let mut firefox_count = 0u32;
        let mut edge_count = 0u32;
        let mut safari_count = 0u32;

        let total = 10_000u32;
        for i in 0..total {
            let profile = TlsProfile::random_weighted(u64::from(i));
            match profile.name.as_str() {
                "Chrome 131" => chrome_count += 1,
                "Firefox 133" => firefox_count += 1,
                "Edge 131" => edge_count += 1,
                "Safari 18" => safari_count += 1,
                other => panic!("unexpected profile: {other}"),
            }
        }

        // Chrome should be the most common (>40%).
        assert!(
            chrome_count > total * 40 / 100,
            "Chrome share too low: {chrome_count}/{total}"
        );
        // Firefox should appear (>5%).
        assert!(
            firefox_count > total * 5 / 100,
            "Firefox share too low: {firefox_count}/{total}"
        );
        // Edge should appear (>5%).
        assert!(
            edge_count > total * 5 / 100,
            "Edge share too low: {edge_count}/{total}"
        );
        // Safari should appear (>3%).
        assert!(
            safari_count > total * 3 / 100,
            "Safari share too low: {safari_count}/{total}"
        );
    }

    #[test]
    fn serde_roundtrip() {
        let profile: &TlsProfile = &CHROME_131;
        let json = serde_json::to_string(profile).expect("serialize");
        let deserialized: TlsProfile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(profile, &deserialized);
    }

    #[test]
    fn ja3hash_display() {
        let ja3 = CHROME_131.ja3();
        assert_eq!(format!("{ja3}"), ja3.hash);
    }

    #[test]
    fn ja4_display() {
        let ja4 = CHROME_131.ja4();
        assert_eq!(format!("{ja4}"), ja4.fingerprint);
    }

    #[test]
    fn cipher_suite_display() {
        let cs = CipherSuiteId::TLS_AES_128_GCM_SHA256;
        assert_eq!(format!("{cs}"), "4865"); // 0x1301 = 4865
    }

    #[test]
    fn tls_version_display() {
        assert_eq!(format!("{}", TlsVersion::Tls13), "772");
    }

    #[test]
    fn alpn_protocol_as_str() {
        assert_eq!(AlpnProtocol::H2.as_str(), "h2");
        assert_eq!(AlpnProtocol::Http11.as_str(), "http/1.1");
    }

    #[test]
    fn supported_group_values() {
        assert_eq!(SupportedGroup::X25519.iana_value(), 0x001d);
        assert_eq!(SupportedGroup::SecP256r1.iana_value(), 0x0017);
        assert_eq!(SupportedGroup::X25519Kyber768.iana_value(), 0x6399);
    }
}
