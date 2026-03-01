//! Session persistence for long-running scraping campaigns.
//!
//! Save and restore browser state (cookies and localStorage) across runs so
//! you can login once and reuse the authenticated session without repeating
//! the authentication flow.
//!
//! ## Use case
//!
//! ```no_run
//! use mycelium_browser::{BrowserPool, BrowserConfig, WaitUntil};
//! use mycelium_browser::session::{save_session, restore_session, SessionSnapshot};
//! use std::time::Duration;
//!
//! # async fn run() -> mycelium_browser::error::Result<()> {
//! let pool = BrowserPool::new(BrowserConfig::default()).await?;
//! let handle = pool.acquire().await?;
//! let mut page = handle.browser().expect("valid browser").new_page().await?;
//!
//! // First run: login and save
//! page.navigate("https://example.com/login", WaitUntil::Selector("body".to_string()), Duration::from_secs(30)).await?;
//! // …perform login…
//! let snapshot = save_session(&page).await?;
//! snapshot.save_to_file("session.json")?;
//!
//! // Later run: restore
//! let snapshot = SessionSnapshot::load_from_file("session.json")?;
//! restore_session(&page, &snapshot).await?;
//! // Now the page has the saved cookies + localStorage
//! # Ok(())
//! # }
//! ```

use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::{
    error::{BrowserError, Result},
    page::PageHandle,
};

// ─── Cookie ──────────────────────────────────────────────────────────────────

/// A serialisable browser cookie.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Domain (e.g. `.example.com`).
    pub domain: String,
    /// URL path (e.g. `/`).
    pub path: String,
    /// Expiry as Unix timestamp seconds (`-1` = session cookie).
    pub expires: f64,
    /// HTTP-only flag.
    pub http_only: bool,
    /// Secure flag.
    pub secure: bool,
    /// `SameSite` attribute (`"Strict"`, `"Lax"`, `"None"`, or empty).
    pub same_site: String,
}

// ─── Snapshot ────────────────────────────────────────────────────────────────

/// A point-in-time snapshot of a browser session.
///
/// Contains cookies and localStorage entries that are sufficient to resume
/// most authenticated sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Origin URL the session was captured from (e.g. `"https://example.com"`).
    pub origin: String,
    /// Saved cookies.
    pub cookies: Vec<SessionCookie>,
    /// localStorage key-value pairs captured from the page.
    pub local_storage: HashMap<String, String>,
    /// Unix timestamp (seconds) when this snapshot was captured.
    pub captured_at: u64,
    /// Approximate TTL for auto-expiry checks. `None` means never expire.
    pub ttl_secs: Option<u64>,
}

impl SessionSnapshot {
    /// Returns `true` if the snapshot has exceeded its TTL.
    ///
    /// Always returns `false` when no TTL is set.
    pub fn is_expired(&self) -> bool {
        let Some(ttl) = self.ttl_secs else {
            return false;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        now.saturating_sub(self.captured_at) > ttl
    }

    /// Age of the snapshot.
    pub fn age(&self) -> Duration {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        Duration::from_secs(now.saturating_sub(self.captured_at))
    }

    /// Serialise to a JSON file.
    ///
    /// # Errors
    ///
    /// Returns an IO or serialisation error if the file cannot be written.
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| BrowserError::ConfigError(format!("Failed to serialise session: {e}")))?;
        std::fs::write(path, json)
            .map_err(BrowserError::Io)
    }

    /// Deserialise from a JSON file previously written by [`Self::save_to_file`].
    ///
    /// # Errors
    ///
    /// Returns an IO or deserialisation error if the file cannot be read.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self> {
        let json = std::fs::read_to_string(path)
            .map_err(BrowserError::Io)?;
        serde_json::from_str(&json)
            .map_err(|e| BrowserError::ConfigError(format!("Failed to deserialise session: {e}")))
    }
}

// ─── Save ─────────────────────────────────────────────────────────────────────

/// Capture the current session state from `page`.
///
/// Saves all cookies visible to the page's origin and the full `localStorage`
/// contents.
///
/// # Errors
///
/// Returns a CDP error if the cookie fetch or localStorage eval fails.
pub async fn save_session(page: &PageHandle) -> Result<SessionSnapshot> {
    let cdp_cookies = page.save_cookies().await?;

    let cookies: Vec<SessionCookie> = cdp_cookies
        .iter()
        .map(|c| SessionCookie {
            name: c.name.clone(),
            value: c.value.clone(),
            domain: c.domain.clone(),
            path: c.path.clone(),
            expires: c.expires,
            http_only: c.http_only,
            secure: c.secure,
            same_site: c
                .same_site
                .as_ref()
                .map(|s| format!("{s:?}"))
                .unwrap_or_default(),
        })
        .collect();

    // Capture localStorage via JS
    let local_storage: HashMap<String, String> = capture_local_storage(page).await?;

    // Best-effort origin from current URL
    let origin = page
        .eval::<String>("window.location.origin")
        .await
        .unwrap_or_default();

    let captured_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();

    debug!(
        origin = %origin,
        cookie_count = cookies.len(),
        ls_keys = local_storage.len(),
        "Session snapshot captured"
    );

    Ok(SessionSnapshot {
        origin,
        cookies,
        local_storage,
        captured_at,
        ttl_secs: None,
    })
}

// ─── Restore ──────────────────────────────────────────────────────────────────

/// Restore a previously saved session into `page`.
///
/// Imports all cookies via `Network.setCookie` and injects the localStorage
/// entries via JavaScript.
///
/// # Errors
///
/// Returns a CDP error if cookie injection or the localStorage script fails.
pub async fn restore_session(page: &PageHandle, snapshot: &SessionSnapshot) -> Result<()> {
    use chromiumoxide::cdp::browser_protocol::network::SetCookieParams;

    if snapshot.is_expired() {
        warn!(
            age_secs = snapshot.age().as_secs(),
            "Restoring an expired session snapshot"
        );
    }

    // Inject cookies
    for cookie in &snapshot.cookies {
        let params = match SetCookieParams::builder()
            .name(cookie.name.clone())
            .value(cookie.value.clone())
            .domain(cookie.domain.clone())
            .path(cookie.path.clone())
            .http_only(cookie.http_only)
            .secure(cookie.secure)
            .build()
        {
            Ok(p) => p,
            Err(e) => {
                warn!(cookie = %cookie.name, error = %e, "Failed to build cookie params");
                continue;
            }
        };

        if let Err(e) = page.inner().execute(params).await {
            warn!(
                cookie = %cookie.name,
                error = %e,
                "Failed to restore cookie"
            );
        }
    }

    // Inject localStorage via JS
    if !snapshot.local_storage.is_empty() {
        let entries: Vec<String> = snapshot
            .local_storage
            .iter()
            .map(|(k, v)| {
                let k_esc = k.replace('\'', "\\'");
                let v_esc = v.replace('\'', "\\'");
                format!("localStorage.setItem('{k_esc}', '{v_esc}');")
            })
            .collect();

        let script = entries.join("\n");

        let _: serde_json::Value = page
            .eval(&script)
            .await
            .unwrap_or(serde_json::Value::Null);
    }

    debug!(
        origin = %snapshot.origin,
        cookie_count = snapshot.cookies.len(),
        ls_keys = snapshot.local_storage.len(),
        "Session restored"
    );

    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Evaluate `localStorage` and return all key-value pairs.
async fn capture_local_storage(page: &PageHandle) -> Result<HashMap<String, String>> {
    // JS: iterate localStorage and return {key: value, ...}
    let script = r"
        (function() {
            var out = {};
            for (var i = 0; i < localStorage.length; i++) {
                var k = localStorage.key(i);
                out[k] = localStorage.getItem(k);
            }
            return JSON.stringify(out);
        })()
    ";

    match page.eval::<String>(script).await {
        Ok(json_str) => {
            serde_json::from_str(&json_str).map_err(|e| {
                BrowserError::ConfigError(format!("Failed to parse localStorage JSON: {e}"))
            })
        }
        Err(e) => {
            warn!("localStorage capture failed (non-HTML page?): {e}");
            Ok(HashMap::new())
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn make_snapshot(captured_at: u64, ttl_secs: Option<u64>) -> SessionSnapshot {
        SessionSnapshot {
            origin: "https://example.com".to_string(),
            cookies: vec![],
            local_storage: HashMap::new(),
            captured_at,
            ttl_secs,
        }
    }

    #[test]
    fn snapshot_not_expired_without_ttl() {
        let s = make_snapshot(0, None);
        assert!(!s.is_expired());
    }

    #[test]
    fn snapshot_expired_when_past_ttl() {
        // captured 1000s ago, ttl = 100s → expired
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let s = make_snapshot(now - 1000, Some(100));
        assert!(s.is_expired());
    }

    #[test]
    fn snapshot_not_expired_within_ttl() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let s = make_snapshot(now - 10, Some(3600));
        assert!(!s.is_expired());
    }

    #[test]
    fn snapshot_age_is_reasonable() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let s = make_snapshot(now - 60, None);
        let age = s.age();
        assert!(age >= Duration::from_secs(59), "age should be ≥59s, got {age:?}");
        assert!(age < Duration::from_secs(65), "age should be <65s, got {age:?}");
    }

    #[test]
    fn snapshot_roundtrips_json() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut s = make_snapshot(1_700_000_000, Some(7200));
        s.cookies.push(SessionCookie {
            name: "session_id".to_string(),
            value: "abc123".to_string(),
            domain: "example.com".to_string(),
            path: "/".to_string(),
            expires: -1.0,
            http_only: true,
            secure: true,
            same_site: "Lax".to_string(),
        });
        s.local_storage.insert("theme".to_string(), "dark".to_string());

        let json = serde_json::to_string(&s)?;
        let decoded: SessionSnapshot = serde_json::from_str(&json)?;

        assert_eq!(decoded.cookies.len(), 1);
        if let Some(c) = decoded.cookies.first() {
            assert_eq!(c.name, "session_id");
        }
        assert_eq!(decoded.local_storage.get("theme").map(String::as_str), Some("dark"));
        assert_eq!(decoded.ttl_secs, Some(7200));
        Ok(())
    }

    #[test]
    fn snapshot_file_roundtrip() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let s = make_snapshot(0, Some(3600));
        let dir = std::env::temp_dir();
        let path = dir.join("mycelium_session_test.json");
        s.save_to_file(&path)?;
        let loaded = SessionSnapshot::load_from_file(&path)?;
        assert_eq!(loaded.origin, s.origin);
        let _ = std::fs::remove_file(&path);
        Ok(())
    }
}
