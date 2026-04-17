//! Protocol-aware routing path resolution.
//!
//! [`resolve_routing_path`] translates a proxy's advertised capabilities and
//! the caller's transport preference into a concrete [`RoutingPath`] that the
//! HTTP client layer uses when opening a connection.

use crate::types::{ProxyCapabilities, RoutingPath};

/// Preference expressed by the caller when acquiring a proxy.
///
/// # Example
/// ```
/// use stygian_proxy::routing::TransportPreference;
/// assert_eq!(TransportPreference::default(), TransportPreference::PreferH3);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransportPreference {
    /// Use HTTP/3 over QUIC if the proxy supports it; fall back to TCP.
    #[default]
    PreferH3,
    /// Always use HTTP/1.1 or HTTP/2 over a TCP CONNECT tunnel.
    ForceTcp,
}

/// Resolve the [`RoutingPath`] for a request given proxy capabilities and the
/// caller's transport preference.
///
/// # Decision logic
///
/// | Preference   | `supports_http3_tunnel` | Result          |
/// |--------------|------------------------|-----------------|
/// | `PreferH3`   | `true`                 | `H3OverUdp`    |
/// | `PreferH3`   | `false`                | `H1H2OverTcp`  |
/// | `ForceTcp`   | any                    | `H1H2OverTcp`  |
///
/// # Example
/// ```
/// use stygian_proxy::routing::{resolve_routing_path, TransportPreference};
/// use stygian_proxy::types::{ProxyCapabilities, RoutingPath};
///
/// let caps = ProxyCapabilities { supports_http3_tunnel: true, ..Default::default() };
/// assert_eq!(
///     resolve_routing_path(&caps, TransportPreference::PreferH3),
///     RoutingPath::H3OverUdp,
/// );
/// assert_eq!(
///     resolve_routing_path(&caps, TransportPreference::ForceTcp),
///     RoutingPath::H1H2OverTcp,
/// );
/// let fallback = ProxyCapabilities::default();
/// assert_eq!(
///     resolve_routing_path(&fallback, TransportPreference::PreferH3),
///     RoutingPath::H1H2OverTcp,
/// );
/// ```
pub const fn resolve_routing_path(
    capabilities: &ProxyCapabilities,
    preference: TransportPreference,
) -> RoutingPath {
    match preference {
        TransportPreference::ForceTcp => RoutingPath::H1H2OverTcp,
        TransportPreference::PreferH3 => {
            if capabilities.supports_http3_tunnel {
                RoutingPath::H3OverUdp
            } else {
                RoutingPath::H1H2OverTcp
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ProxyCapabilities;

    #[test]
    fn prefer_h3_with_udp_support_returns_h3() {
        let caps = ProxyCapabilities {
            supports_http3_tunnel: true,
            ..Default::default()
        };
        assert_eq!(
            resolve_routing_path(&caps, TransportPreference::PreferH3),
            RoutingPath::H3OverUdp,
        );
    }

    #[test]
    fn prefer_h3_without_udp_support_falls_back_to_tcp() {
        let caps = ProxyCapabilities::default();
        assert_eq!(
            resolve_routing_path(&caps, TransportPreference::PreferH3),
            RoutingPath::H1H2OverTcp,
        );
    }

    #[test]
    fn force_tcp_always_returns_tcp() {
        let caps = ProxyCapabilities {
            supports_http3_tunnel: true,
            ..Default::default()
        };
        assert_eq!(
            resolve_routing_path(&caps, TransportPreference::ForceTcp),
            RoutingPath::H1H2OverTcp,
        );
    }
}
