//! Async background health checker for proxy liveness verification.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::storage::ProxyStoragePort;
use crate::types::ProxyConfig;

/// Shared health map type.
/// `true` = proxy is currently considered healthy.
pub type HealthMap = Arc<RwLock<HashMap<Uuid, bool>>>;

/// Continuously verifies proxy liveness and updates the shared [`HealthMap`].
///
/// Run one check cycle with [`check_once`](HealthChecker::check_once) or launch
/// a background task with [`spawn`](HealthChecker::spawn).
///
/// When the `tls-profiled` feature is enabled you can supply a
/// [`ProfiledRequester`](crate::http_client::ProfiledRequester) via
/// [`HealthChecker::with_profiled_client`] so health-check GET requests carry a
/// browser TLS fingerprint.
#[derive(Clone)]
pub struct HealthChecker {
    config: ProxyConfig,
    storage: Arc<dyn ProxyStoragePort>,
    health_map: HealthMap,
    /// Optional TLS-profiled HTTP client.  When `None` a vanilla
    /// `reqwest::Client` is built per check cycle.
    #[cfg(feature = "tls-profiled")]
    profiled: Option<crate::http_client::ProfiledRequester>,
}

impl HealthChecker {
    /// Access the shared health map (read it to filter candidates).
    pub const fn health_map(&self) -> &HealthMap {
        &self.health_map
    }

    /// Create a new checker.
    ///
    /// `health_map` should be the **same** `Arc` held by the `ProxyManager` so
    /// that selection decisions always see up-to-date health information.
    pub fn new(
        config: ProxyConfig,
        storage: Arc<dyn ProxyStoragePort>,
        health_map: HealthMap,
    ) -> Self {
        Self {
            config,
            storage,
            health_map,
            #[cfg(feature = "tls-profiled")]
            profiled: None,
        }
    }

    /// Attach a TLS-profiled client so that health-check requests carry a
    /// browser fingerprint instead of a default `reqwest` TLS handshake.
    ///
    /// Only available when the `tls-profiled` feature is enabled.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use stygian_proxy::{
    ///     HealthChecker,
    ///     ProxyConfig,
    ///     http_client::{ProfiledRequestMode, ProfiledRequester},
    /// };
    /// use stygian_proxy::storage::MemoryProxyStore;
    ///
    /// # fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let storage = Arc::new(MemoryProxyStore::default());
    /// let health_map = stygian_proxy::health::HealthMap::default();
    /// let requester = ProfiledRequester::chrome_mode(ProfiledRequestMode::Preset)?;
    /// let checker = HealthChecker::new(ProxyConfig::default(), storage, health_map)
    ///     .with_profiled_client(requester);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "tls-profiled")]
    #[must_use]
    pub fn with_profiled_client(
        mut self,
        requester: crate::http_client::ProfiledRequester,
    ) -> Self {
        self.profiled = Some(requester);
        self
    }

    /// Build and attach a profile-mode-based requester.
    ///
    /// Uses Chrome 131 as the baseline browser identity and applies `mode`
    /// to TLS control mapping.
    ///
    /// Only available when the `tls-profiled` feature is enabled.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::ProxyError::ConfigError`] if the profiled
    /// requester cannot be constructed.
    #[cfg(feature = "tls-profiled")]
    pub fn with_profiled_mode(
        self,
        mode: crate::types::ProfiledRequestMode,
    ) -> crate::error::ProxyResult<Self> {
        let requester = crate::http_client::ProfiledRequester::chrome_mode(mode)
            .map_err(|e| crate::error::ProxyError::ConfigError(e.to_string()))?;
        Ok(self.with_profiled_client(requester))
    }

    /// Spawn an infinite background task that checks proxies on every
    /// `config.health_check_interval` tick.
    ///
    /// Cancel `token` to stop the task gracefully.  Missed ticks are skipped.
    pub fn spawn(self, token: CancellationToken) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(self.config.health_check_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = token.cancelled() => {
                        tracing::info!("health checker: shutdown requested");
                        break;
                    }
                    _ = interval.tick() => {
                        self.check_all().await;
                    }
                }
            }
            tracing::info!("health checker: stopped");
        })
    }

    /// Run one full check cycle synchronously (useful for tests).
    pub async fn check_once(&self) {
        self.check_all().await;
    }

    async fn check_all(&self) {
        let records = match self.storage.list().await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("health checker: storage list failed: {e}");
                return;
            }
        };

        let health_url = self.config.health_check_url.clone();
        let timeout = self.config.health_check_timeout;

        let mut set: JoinSet<(Uuid, Result<u64, String>)> = JoinSet::new();
        for record in records {
            let proxy_url = record.proxy.url.clone();
            let username = record.proxy.username.clone();
            let password = record.proxy.password.clone();
            let id = record.id;
            let check_url = health_url.clone();

            // When the `tls-profiled` feature is enabled, build a fresh profiled
            // client per proxy that routes through that proxy's URL so health
            // checks present a browser TLS fingerprint for proxy-routed requests.
            #[cfg(feature = "tls-profiled")]
            let routed_proxy_url =
                proxy_url_with_auth(&proxy_url, username.as_deref(), password.as_deref());

            #[cfg(feature = "tls-profiled")]
            let preset_client: Option<reqwest::Client> = self.profiled.as_ref().and_then(|p| {
                crate::http_client::ProfiledRequester::from_profile(
                    p.profile(),
                    Some(&routed_proxy_url),
                )
                .map(crate::http_client::ProfiledRequester::into_client)
                .map_err(|e| {
                    tracing::warn!(
                        error = %e,
                        proxy = %routed_proxy_url,
                        "tls-profiled health-check client build failed; falling back to vanilla"
                    );
                })
                .ok()
            });

            #[cfg(not(feature = "tls-profiled"))]
            let preset_client: Option<reqwest::Client> = None;

            set.spawn(async move {
                let result = do_check(
                    &proxy_url,
                    username.as_deref(),
                    password.as_deref(),
                    &check_url,
                    timeout,
                    preset_client,
                )
                .await;
                (id, result)
            });
        }

        let mut updates: Vec<(Uuid, bool, u64)> = Vec::new();
        while let Some(task_result) = set.join_next().await {
            match task_result {
                Ok((id, Ok(latency_ms))) => updates.push((id, true, latency_ms)),
                Ok((id, Err(e))) => {
                    tracing::warn!(proxy = %id, error = %e, "health check failed");
                    updates.push((id, false, 0));
                }
                Err(join_err) => {
                    tracing::error!("health check task panicked: {join_err}");
                }
            }
        }

        let total = u32::try_from(updates.len()).unwrap_or(u32::MAX);
        let healthy_count =
            u32::try_from(updates.iter().filter(|(_, h, _)| *h).count()).unwrap_or(u32::MAX);

        {
            let mut map = self.health_map.write().await;
            for (id, healthy, _) in &updates {
                map.insert(*id, *healthy);
            }
        }

        for (id, success, latency) in updates {
            if let Err(e) = self.storage.update_metrics(id, success, latency).await {
                tracing::warn!("health checker: metrics update failed for {id}: {e}");
            }
        }

        tracing::info!(
            total,
            healthy = healthy_count,
            unhealthy = total - healthy_count,
            "health check cycle complete"
        );
    }
}

#[cfg(any(test, feature = "tls-profiled"))]
fn proxy_url_with_auth(proxy_url: &str, username: Option<&str>, password: Option<&str>) -> String {
    let (Some(user), Some(pass)) = (username, password) else {
        return proxy_url.to_string();
    };

    let Ok(mut url) = reqwest::Url::parse(proxy_url) else {
        return proxy_url.to_string();
    };

    if url.username().is_empty() && url.set_username(user).is_err() {
        return proxy_url.to_string();
    }
    if url.password().is_none() && url.set_password(Some(pass)).is_err() {
        return proxy_url.to_string();
    }

    url.to_string()
}

async fn do_check(
    proxy_url: &str,
    username: Option<&str>,
    password: Option<&str>,
    health_url: &str,
    timeout: std::time::Duration,
    preset_client: Option<reqwest::Client>,
) -> Result<u64, String> {
    // Use the pre-built profiled client (already includes proxy routing) when
    // available; otherwise build a vanilla client with per-proxy routing and
    // optional basic-auth credentials.
    let client = if let Some(c) = preset_client {
        c
    } else {
        let mut proxy = reqwest::Proxy::all(proxy_url).map_err(|e| e.to_string())?;
        if let (Some(user), Some(pass)) = (username, password) {
            proxy = proxy.basic_auth(user, pass);
        }
        reqwest::Client::builder()
            .proxy(proxy)
            .timeout(timeout)
            .build()
            .map_err(|e| e.to_string())?
    };

    let start = Instant::now();
    client
        .get(health_url)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    Ok(start.elapsed().as_millis().try_into().unwrap_or(u64::MAX))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::storage::MemoryProxyStore;
    use crate::types::{Proxy, ProxyType};

    fn make_proxy(url: &str) -> Proxy {
        Proxy {
            url: url.into(),
            proxy_type: ProxyType::Http,
            username: None,
            password: None,
            weight: 1,
            tags: vec![],
            capabilities: crate::types::ProxyCapabilities::default(),
        }
    }
    #[test]
    fn proxy_url_with_auth_injects_credentials() {
        let proxy_url = proxy_url_with_auth(
            "http://proxy.example.com:8080",
            Some("alice"),
            Some("s3cr3t"),
        );
        assert!(proxy_url.starts_with("http://alice:s3cr3t@proxy.example.com:8080"));
    }

    #[cfg(feature = "tls-profiled")]
    #[test]
    fn proxy_url_with_auth_leaves_existing_credentials_untouched() {
        let proxy_url = proxy_url_with_auth(
            "http://already:present@proxy.example.com:8080",
            Some("alice"),
            Some("s3cr3t"),
        );
        assert!(proxy_url.starts_with("http://already:present@proxy.example.com:8080"));
    }

    #[tokio::test]
    async fn healthy_and_unhealthy_proxies() -> crate::error::ProxyResult<()> {
        // Mock server acts as both the HTTP proxy and the health-check target.
        // reqwest sends the GET in absolute-form; wiremock responds 200.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let storage = Arc::new(MemoryProxyStore::default());
        // Proxy 1: URL points to the mock server → health check will succeed.
        storage.add(make_proxy(&server.uri())).await?;
        // Proxy 2: invalid address → health check will fail.
        storage.add(make_proxy("http://192.0.2.1:9999")).await?;

        let health_map: HealthMap = Arc::new(RwLock::new(HashMap::new()));
        let config = ProxyConfig {
            health_check_url: format!("{}/", server.uri()),
            health_check_interval: Duration::from_hours(1),
            health_check_timeout: Duration::from_secs(2),
            ..ProxyConfig::default()
        };
        let checker = HealthChecker::new(config, storage.clone(), health_map.clone());
        checker.check_once().await;

        let map = health_map.read().await;
        let healthy = map.values().filter(|&&v| v).count();
        let unhealthy = map.values().filter(|&&v| !v).count();
        drop(map);
        assert_eq!(healthy, 1, "expected 1 healthy proxy");
        assert_eq!(unhealthy, 1, "expected 1 unhealthy proxy");
        Ok(())
    }

    #[tokio::test]
    async fn graceful_shutdown() {
        let storage = Arc::new(MemoryProxyStore::default());
        let health_map: HealthMap = Arc::new(RwLock::new(HashMap::new()));
        let config = ProxyConfig {
            health_check_interval: Duration::from_hours(1),
            ..ProxyConfig::default()
        };
        let token = CancellationToken::new();
        let checker = HealthChecker::new(config, storage, health_map);
        let handle = checker.spawn(token.clone());

        token.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(
            result.is_ok(),
            "task should exit within 1s after cancellation"
        );
    }
}
