//! Opinionated acquisition runner with deterministic escalation.
//!
//! The runner executes a mode-specific strategy ladder and returns a terminal
//! [`AcquisitionResult`] for every request, including setup-failure and timeout
//! paths.

use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(feature = "browserbase")]
use chromiumoxide::Browser;
#[cfg(feature = "browserbase")]
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(feature = "browserbase")]
use tokio::time::timeout;

use crate::BrowserPool;
use crate::error::BrowserError;
use crate::page::WaitUntil;

/// Opinionated acquisition mode for the escalation ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionMode {
    /// Prioritize lowest-latency paths.
    Fast,
    /// Favor reliability with broader escalation.
    Resilient,
    /// Start from stronger anti-bot paths.
    Hostile,
    /// Enter from a policy-guided start point.
    Investigate,
}

/// Strategy stage attempted by the acquisition runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyUsed {
    /// Plain HTTP fetch.
    DirectHttp,
    /// HTTP fetch using a TLS-profiled client.
    TlsProfiledHttp,
    /// Browser session with opinionated light-stealth defaults.
    BrowserLightStealth,
    /// Browser session scoped to a sticky context id.
    StickyProxyBrowserSession,
    /// Managed remote browser session routed through Browserbase.
    #[cfg(feature = "browserbase")]
    BrowserbaseManagedSession,
    /// Policy-guided entry marker for investigation mode.
    InvestigateEntry,
}

/// One acquisition request.
#[derive(Debug, Clone)]
pub struct AcquisitionRequest {
    /// Target URL.
    pub url: String,
    /// Acquisition mode.
    pub mode: AcquisitionMode,
    /// Optional selector that must be present for browser-stage success.
    pub wait_for_selector: Option<String>,
    /// Optional JavaScript extraction expression evaluated in browser stages.
    pub extraction_js: Option<String>,
    /// Hard wall-clock timeout for the whole acquisition attempt.
    pub total_timeout: Duration,
    /// Per-navigation timeout for browser stages.
    pub navigation_timeout: Duration,
    /// Per-request timeout for HTTP stages.
    pub request_timeout: Duration,
    /// Maximum HTML bytes captured into `html_excerpt`.
    pub html_excerpt_bytes: usize,
    /// Optional policy-guided stage that `Investigate` mode starts from.
    pub investigate_start: Option<StrategyUsed>,
    /// Opt into the optional Browserbase-managed stage when available.
    pub browserbase_enabled: bool,
}

impl Default for AcquisitionRequest {
    fn default() -> Self {
        Self {
            url: String::new(),
            mode: AcquisitionMode::Resilient,
            wait_for_selector: None,
            extraction_js: None,
            total_timeout: Duration::from_secs(45),
            navigation_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(15),
            html_excerpt_bytes: 4_096,
            investigate_start: None,
            browserbase_enabled: false,
        }
    }
}

/// Failure class recorded per strategy stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageFailureKind {
    /// Stage initialization/setup failed.
    Setup,
    /// Stage hit a timeout.
    Timeout,
    /// Stage reached a known anti-bot block class.
    Blocked,
    /// Transport/runtime failure.
    Transport,
    /// Extraction/validation failure.
    Extraction,
}

/// Captured failure record for one stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageFailure {
    /// Stage where the failure happened.
    pub strategy: StrategyUsed,
    /// Coarse failure kind.
    pub kind: StageFailureKind,
    /// Compact diagnostic message.
    pub message: String,
}

/// Terminal acquisition result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcquisitionResult {
    /// `true` when any stage satisfied success criteria.
    pub success: bool,
    /// Stage that produced the terminal success, if any.
    pub strategy_used: Option<StrategyUsed>,
    /// Ordered stage attempts.
    pub attempted: Vec<StrategyUsed>,
    /// Final URL observed from the successful stage.
    pub final_url: Option<String>,
    /// HTTP status code observed from the successful stage.
    pub status_code: Option<u16>,
    /// Best-effort HTML excerpt from the successful stage.
    pub html_excerpt: Option<String>,
    /// Optional extraction payload.
    pub extracted: Option<Value>,
    /// Failure bundle collected across stages.
    pub failures: Vec<StageFailure>,
    /// `true` when the wall-clock timeout fired before completion.
    pub timed_out: bool,
}

impl AcquisitionResult {
    const fn empty() -> Self {
        Self {
            success: false,
            strategy_used: None,
            attempted: Vec::new(),
            final_url: None,
            status_code: None,
            html_excerpt: None,
            extracted: None,
            failures: Vec::new(),
            timed_out: false,
        }
    }
}

#[derive(Debug, Clone)]
struct StageSuccess {
    final_url: Option<String>,
    status_code: Option<u16>,
    html_excerpt: Option<String>,
    extracted: Option<Value>,
}

#[derive(Debug, Clone)]
enum StageOutcome {
    Marker,
    Success(StageSuccess),
    Failure(StageFailure),
}

/// Runner facade for opinionated acquisition.
#[derive(Clone)]
pub struct AcquisitionRunner {
    pool: Arc<BrowserPool>,
}

impl AcquisitionRunner {
    /// Create a new acquisition runner.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{AcquisitionRunner, BrowserConfig, BrowserPool};
    ///
    /// # async fn run() -> stygian_browser::Result<()> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let _runner = AcquisitionRunner::new(pool);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn new(pool: Arc<BrowserPool>) -> Self {
        Self { pool }
    }

    /// Return the deterministic stage ladder for a mode.
    ///
    /// Investigation mode starts at `investigate_start` when provided.
    #[must_use]
    pub fn strategy_ladder(
        mode: AcquisitionMode,
        investigate_start: Option<StrategyUsed>,
    ) -> Vec<StrategyUsed> {
        let mut stages = match mode {
            AcquisitionMode::Fast => vec![
                StrategyUsed::DirectHttp,
                StrategyUsed::TlsProfiledHttp,
                StrategyUsed::BrowserLightStealth,
            ],
            AcquisitionMode::Resilient => vec![
                StrategyUsed::DirectHttp,
                StrategyUsed::TlsProfiledHttp,
                StrategyUsed::BrowserLightStealth,
                StrategyUsed::StickyProxyBrowserSession,
            ],
            AcquisitionMode::Hostile => vec![
                StrategyUsed::BrowserLightStealth,
                StrategyUsed::StickyProxyBrowserSession,
                StrategyUsed::TlsProfiledHttp,
                StrategyUsed::DirectHttp,
            ],
            AcquisitionMode::Investigate => {
                let start = investigate_start.unwrap_or(StrategyUsed::BrowserLightStealth);
                vec![
                    StrategyUsed::InvestigateEntry,
                    start,
                    StrategyUsed::StickyProxyBrowserSession,
                    StrategyUsed::TlsProfiledHttp,
                ]
            }
        };

        dedupe_preserve_order(&mut stages);
        stages
    }

    /// Execute the acquisition ladder and return a terminal result.
    ///
    /// This method never panics and always returns an [`AcquisitionResult`],
    /// including timeout and setup-failure paths.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{AcquisitionMode, AcquisitionRequest, AcquisitionRunner, BrowserConfig, BrowserPool};
    ///
    /// # async fn run() -> stygian_browser::Result<()> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let runner = AcquisitionRunner::new(pool);
    /// let request = AcquisitionRequest {
    ///     url: "https://example.com".to_string(),
    ///     mode: AcquisitionMode::Resilient,
    ///     ..AcquisitionRequest::default()
    /// };
    /// let _result = runner.run(request).await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run(&self, request: AcquisitionRequest) -> AcquisitionResult {
        let timeout = request.total_timeout;
        let timeout_strategy = Self::strategy_ladder(request.mode, request.investigate_start)
            .into_iter()
            .find(|strategy| *strategy != StrategyUsed::InvestigateEntry)
            .unwrap_or(StrategyUsed::DirectHttp);
        let mut result = tokio::time::timeout(timeout, self.run_inner(&request))
            .await
            .unwrap_or_else(|_| {
                let mut timed_out = AcquisitionResult::empty();
                timed_out.timed_out = true;
                timed_out.failures.push(StageFailure {
                    strategy: timeout_strategy,
                    kind: StageFailureKind::Timeout,
                    message: format!("acquisition timed out after {}ms", timeout.as_millis()),
                });
                timed_out
            });

        if !result.success {
            // Guarantee deterministic terminal output for all unsuccessful runs.
            if result.failures.is_empty() {
                result.failures.push(StageFailure {
                    strategy: timeout_strategy,
                    kind: StageFailureKind::Transport,
                    message: "acquisition ended without stage output".to_string(),
                });
            }
        }

        result
    }

    async fn run_inner(&self, request: &AcquisitionRequest) -> AcquisitionResult {
        let mut result = AcquisitionResult::empty();

        #[cfg(feature = "browserbase")]
        let mut ladder = Self::strategy_ladder(request.mode, request.investigate_start);

        #[cfg(not(feature = "browserbase"))]
        let ladder = Self::strategy_ladder(request.mode, request.investigate_start);

        #[cfg(feature = "browserbase")]
        {
            maybe_insert_browserbase_stage(&mut ladder, request.browserbase_enabled);
        }
        let started = Instant::now();

        for strategy in ladder {
            if started.elapsed() >= request.total_timeout {
                result.timed_out = true;
                result.failures.push(StageFailure {
                    strategy,
                    kind: StageFailureKind::Timeout,
                    message: "wall-clock timeout reached before stage execution".to_string(),
                });
                break;
            }

            result.attempted.push(strategy);
            match self.execute_stage(strategy, request).await {
                StageOutcome::Marker => {}
                StageOutcome::Success(success) => {
                    result.success = true;
                    result.strategy_used = Some(strategy);
                    result.final_url = success.final_url;
                    result.status_code = success.status_code;
                    result.html_excerpt = success.html_excerpt;
                    result.extracted = success.extracted;
                    break;
                }
                StageOutcome::Failure(failure) => result.failures.push(failure),
            }
        }

        result
    }

    async fn execute_stage(
        &self,
        strategy: StrategyUsed,
        request: &AcquisitionRequest,
    ) -> StageOutcome {
        match strategy {
            StrategyUsed::DirectHttp => {
                #[cfg(feature = "tls-config")]
                {
                    self.run_http_stage(request, false).await
                }

                #[cfg(not(feature = "tls-config"))]
                {
                    self.run_http_stage(request, false)
                }
            }
            StrategyUsed::TlsProfiledHttp => {
                #[cfg(feature = "tls-config")]
                {
                    self.run_http_stage(request, true).await
                }

                #[cfg(not(feature = "tls-config"))]
                {
                    self.run_http_stage(request, true)
                }
            }
            StrategyUsed::BrowserLightStealth => self.run_browser_stage(request, false).await,
            StrategyUsed::StickyProxyBrowserSession => self.run_browser_stage(request, true).await,
            #[cfg(feature = "browserbase")]
            StrategyUsed::BrowserbaseManagedSession => Self::run_browserbase_stage(request).await,
            StrategyUsed::InvestigateEntry => StageOutcome::Marker,
        }
    }

    #[cfg(feature = "browserbase")]
    #[allow(clippy::too_many_lines)]
    async fn run_browserbase_stage(request: &AcquisitionRequest) -> StageOutcome {
        if !request.browserbase_enabled {
            return StageOutcome::Failure(StageFailure {
                strategy: StrategyUsed::BrowserbaseManagedSession,
                kind: StageFailureKind::Setup,
                message: "browserbase stage disabled for this request".to_string(),
            });
        }

        let api_key = match std::env::var("BROWSERBASE_API_KEY") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => {
                return StageOutcome::Failure(StageFailure {
                    strategy: StrategyUsed::BrowserbaseManagedSession,
                    kind: StageFailureKind::Setup,
                    message: "browserbase requires BROWSERBASE_API_KEY".to_string(),
                });
            }
        };

        let project_id = match std::env::var("BROWSERBASE_PROJECT_ID") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => {
                return StageOutcome::Failure(StageFailure {
                    strategy: StrategyUsed::BrowserbaseManagedSession,
                    kind: StageFailureKind::Setup,
                    message: "browserbase requires BROWSERBASE_PROJECT_ID".to_string(),
                });
            }
        };

        let session = match create_browserbase_session(request, &api_key, &project_id).await {
            Ok(session) => session,
            Err(err) => {
                return StageOutcome::Failure(StageFailure {
                    strategy: StrategyUsed::BrowserbaseManagedSession,
                    kind: classify_browser_error(&err),
                    message: err.to_string(),
                });
            }
        };

        let connect_timeout = request.request_timeout.min(request.total_timeout);
        let (mut browser, mut handler) = match timeout(
            connect_timeout,
            Browser::connect(session.connect_url.clone()),
        )
        .await
        {
            Ok(Ok(pair)) => pair,
            Ok(Err(err)) => {
                let _ = delete_browserbase_session(request, &api_key, &session.id).await;
                return StageOutcome::Failure(StageFailure {
                    strategy: StrategyUsed::BrowserbaseManagedSession,
                    kind: StageFailureKind::Transport,
                    message: format!("browserbase connect failed: {err}"),
                });
            }
            Err(_) => {
                let _ = delete_browserbase_session(request, &api_key, &session.id).await;
                return StageOutcome::Failure(StageFailure {
                    strategy: StrategyUsed::BrowserbaseManagedSession,
                    kind: StageFailureKind::Timeout,
                    message: format!(
                        "browserbase connect timed out after {}ms",
                        connect_timeout.as_millis()
                    ),
                });
            }
        };

        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(error) = event {
                    tracing::warn!(%error, "browserbase handler error");
                    break;
                }
            }
        });

        let run_result =
            async {
                let raw_page = browser.new_page("about:blank").await.map_err(|err| {
                    BrowserError::CdpError {
                        operation: "Browser.newPage".to_string(),
                        message: err.to_string(),
                    }
                })?;

                let mut page = crate::page::PageHandle::new(raw_page, request.navigation_timeout);

                page.navigate(
                    &request.url,
                    WaitUntil::DomContentLoaded,
                    request.navigation_timeout,
                )
                .await?;

                if let Some(selector) = &request.wait_for_selector {
                    page.wait_for_selector(selector, request.navigation_timeout)
                        .await?;
                }

                let extracted = match request.extraction_js.as_deref() {
                    Some(script) => Some(page.eval::<Value>(script).await.map_err(|err| {
                        BrowserError::ScriptExecutionFailed {
                            script: script.to_string(),
                            reason: err.to_string(),
                        }
                    })?),
                    None => None,
                };

                let html = page.content().await?;
                let final_url = page.url().await.ok();
                let status_code = page.status_code().ok().flatten();

                Ok::<StageSuccess, BrowserError>(StageSuccess {
                    final_url,
                    status_code,
                    html_excerpt: Some(truncate_html(&html, request.html_excerpt_bytes)),
                    extracted,
                })
            }
            .await;

        let _ = timeout(Duration::from_secs(5), browser.close()).await;
        handler_task.abort();
        let _ = delete_browserbase_session(request, &api_key, &session.id).await;

        match run_result {
            Ok(success) => {
                if is_block_status(success.status_code) {
                    StageOutcome::Failure(StageFailure {
                        strategy: StrategyUsed::BrowserbaseManagedSession,
                        kind: StageFailureKind::Blocked,
                        message: format!(
                            "blocked status during browserbase stage: {:?}",
                            success.status_code
                        ),
                    })
                } else {
                    StageOutcome::Success(success)
                }
            }
            Err(err) => StageOutcome::Failure(StageFailure {
                strategy: StrategyUsed::BrowserbaseManagedSession,
                kind: classify_browser_error(&err),
                message: err.to_string(),
            }),
        }
    }

    async fn run_browser_stage(&self, request: &AcquisitionRequest, sticky: bool) -> StageOutcome {
        let strategy = if sticky {
            StrategyUsed::StickyProxyBrowserSession
        } else {
            StrategyUsed::BrowserLightStealth
        };

        let handle_result = if sticky {
            let context = host_hint(&request.url).unwrap_or_else(|| "default".to_string());
            self.pool.acquire_for(&context).await
        } else {
            self.pool.acquire().await
        };

        let handle = match handle_result {
            Ok(handle) => handle,
            Err(err) => {
                return StageOutcome::Failure(StageFailure {
                    strategy,
                    kind: StageFailureKind::Setup,
                    message: format!("browser acquire failed: {err}"),
                });
            }
        };

        let page_result = async {
            let browser = handle.browser().ok_or_else(|| {
                BrowserError::ConfigError("browser handle already released".to_string())
            })?;
            let mut page = browser.new_page().await?;
            page.navigate(
                &request.url,
                WaitUntil::DomContentLoaded,
                request.navigation_timeout,
            )
            .await?;

            if let Some(selector) = &request.wait_for_selector {
                page.wait_for_selector(selector, request.navigation_timeout)
                    .await?;
            }

            let extracted = match request.extraction_js.as_deref() {
                Some(script) => Some(page.eval::<Value>(script).await.map_err(|err| {
                    BrowserError::ScriptExecutionFailed {
                        script: script.to_string(),
                        reason: err.to_string(),
                    }
                })?),
                None => None,
            };

            let html = page.content().await?;
            let final_url = page.url().await.ok();
            let status_code = page.status_code().ok().flatten();
            let html_excerpt = truncate_html(&html, request.html_excerpt_bytes);

            drop(page);

            Ok::<StageSuccess, BrowserError>(StageSuccess {
                final_url,
                status_code,
                html_excerpt: Some(html_excerpt),
                extracted,
            })
        }
        .await;

        handle.release().await;

        match page_result {
            Ok(success) => {
                if is_block_status(success.status_code) {
                    StageOutcome::Failure(StageFailure {
                        strategy,
                        kind: StageFailureKind::Blocked,
                        message: format!(
                            "blocked status during browser stage: {:?}",
                            success.status_code
                        ),
                    })
                } else {
                    StageOutcome::Success(success)
                }
            }
            Err(err) => StageOutcome::Failure(StageFailure {
                strategy,
                kind: classify_browser_error(&err),
                message: err.to_string(),
            }),
        }
    }

    #[cfg(feature = "tls-config")]
    async fn run_http_stage(
        &self,
        request: &AcquisitionRequest,
        tls_profiled: bool,
    ) -> StageOutcome {
        if request.wait_for_selector.is_some() || request.extraction_js.is_some() {
            return StageOutcome::Failure(StageFailure {
                strategy: if tls_profiled {
                    StrategyUsed::TlsProfiledHttp
                } else {
                    StrategyUsed::DirectHttp
                },
                kind: StageFailureKind::Extraction,
                message: "HTTP stages cannot satisfy selector/extraction requirements".to_string(),
            });
        }

        self.run_http_stage_impl(request, tls_profiled).await
    }

    #[cfg(not(feature = "tls-config"))]
    fn run_http_stage(&self, request: &AcquisitionRequest, tls_profiled: bool) -> StageOutcome {
        if request.wait_for_selector.is_some() || request.extraction_js.is_some() {
            return StageOutcome::Failure(StageFailure {
                strategy: if tls_profiled {
                    StrategyUsed::TlsProfiledHttp
                } else {
                    StrategyUsed::DirectHttp
                },
                kind: StageFailureKind::Extraction,
                message: "HTTP stages cannot satisfy selector/extraction requirements".to_string(),
            });
        }

        self.run_http_stage_impl(request, tls_profiled)
    }

    #[cfg(feature = "tls-config")]
    async fn run_http_stage_impl(
        &self,
        request: &AcquisitionRequest,
        tls_profiled: bool,
    ) -> StageOutcome {
        use crate::tls::{CHROME_131, build_profiled_client_preset};

        let strategy = if tls_profiled {
            StrategyUsed::TlsProfiledHttp
        } else {
            StrategyUsed::DirectHttp
        };

        let client = if tls_profiled {
            match build_profiled_client_preset(&CHROME_131, None) {
                Ok(client) => client,
                Err(err) => {
                    return StageOutcome::Failure(StageFailure {
                        strategy,
                        kind: StageFailureKind::Setup,
                        message: format!("tls-profiled client setup failed: {err}"),
                    });
                }
            }
        } else {
            match reqwest::Client::builder()
                .timeout(request.request_timeout)
                .cookie_store(true)
                .build()
            {
                Ok(client) => client,
                Err(err) => {
                    return StageOutcome::Failure(StageFailure {
                        strategy,
                        kind: StageFailureKind::Setup,
                        message: format!("http client setup failed: {err}"),
                    });
                }
            }
        };

        let response = match client
            .get(&request.url)
            .timeout(request.request_timeout)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                return StageOutcome::Failure(StageFailure {
                    strategy,
                    kind: if err.is_timeout() {
                        StageFailureKind::Timeout
                    } else {
                        StageFailureKind::Transport
                    },
                    message: err.to_string(),
                });
            }
        };

        let status_code = Some(response.status().as_u16());
        let final_url = Some(response.url().to_string());
        let html = match response.text().await {
            Ok(text) => text,
            Err(err) => {
                return StageOutcome::Failure(StageFailure {
                    strategy,
                    kind: StageFailureKind::Transport,
                    message: format!("response body read failed: {err}"),
                });
            }
        };

        if is_block_status(status_code) {
            return StageOutcome::Failure(StageFailure {
                strategy,
                kind: StageFailureKind::Blocked,
                message: format!("blocked status from HTTP stage: {status_code:?}"),
            });
        }

        StageOutcome::Success(StageSuccess {
            final_url,
            status_code,
            html_excerpt: Some(truncate_html(&html, request.html_excerpt_bytes)),
            extracted: None,
        })
    }

    #[cfg(not(feature = "tls-config"))]
    fn run_http_stage_impl(
        &self,
        _request: &AcquisitionRequest,
        tls_profiled: bool,
    ) -> StageOutcome {
        let strategy = if tls_profiled {
            StrategyUsed::TlsProfiledHttp
        } else {
            StrategyUsed::DirectHttp
        };
        StageOutcome::Failure(StageFailure {
            strategy,
            kind: StageFailureKind::Setup,
            message: "HTTP acquisition requires the `tls-config` feature".to_string(),
        })
    }
}

#[cfg(feature = "browserbase")]
#[derive(Debug, Clone)]
struct BrowserbaseSession {
    id: String,
    connect_url: String,
}

#[cfg(feature = "browserbase")]
async fn create_browserbase_session(
    request: &AcquisitionRequest,
    api_key: &str,
    project_id: &str,
) -> Result<BrowserbaseSession, BrowserError> {
    let client = reqwest::Client::builder()
        .timeout(request.request_timeout)
        .build()
        .map_err(|err| {
            BrowserError::ConfigError(format!("browserbase client setup failed: {err}"))
        })?;

    let create_url = format!("{}/sessions", browserbase_api_base());
    let response = client
        .post(create_url.clone())
        .bearer_auth(api_key)
        .header("x-bb-api-key", api_key)
        .json(&serde_json::json!({ "projectId": project_id }))
        .send()
        .await
        .map_err(|err| BrowserError::ConnectionError {
            url: create_url.clone(),
            reason: err.to_string(),
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(BrowserError::ConnectionError {
            url: create_url,
            reason: format!("session create failed ({status}): {body}"),
        });
    }

    let payload: Value = response
        .json()
        .await
        .map_err(|err| BrowserError::ConnectionError {
            url: browserbase_api_base(),
            reason: format!("session create response parse failed: {err}"),
        })?;

    let connect_url = browserbase_connect_url(&payload).ok_or_else(|| {
        BrowserError::ConfigError("browserbase response missing connect URL".to_string())
    })?;
    let session_id = browserbase_session_id(&payload).ok_or_else(|| {
        BrowserError::ConfigError("browserbase response missing session id".to_string())
    })?;

    Ok(BrowserbaseSession {
        id: session_id,
        connect_url,
    })
}

#[cfg(feature = "browserbase")]
async fn delete_browserbase_session(
    request: &AcquisitionRequest,
    api_key: &str,
    session_id: &str,
) -> Result<(), BrowserError> {
    let client = reqwest::Client::builder()
        .timeout(request.request_timeout)
        .build()
        .map_err(|err| {
            BrowserError::ConfigError(format!("browserbase client setup failed: {err}"))
        })?;

    let delete_url = format!("{}/sessions/{session_id}", browserbase_api_base());
    let response = client
        .delete(delete_url.clone())
        .bearer_auth(api_key)
        .header("x-bb-api-key", api_key)
        .send()
        .await
        .map_err(|err| BrowserError::ConnectionError {
            url: delete_url.clone(),
            reason: err.to_string(),
        })?;

    if response.status().is_success() {
        Ok(())
    } else {
        Err(BrowserError::ConnectionError {
            url: delete_url,
            reason: format!("session delete failed with status {}", response.status()),
        })
    }
}

#[cfg(feature = "browserbase")]
fn browserbase_api_base() -> String {
    std::env::var("BROWSERBASE_API_BASE")
        .unwrap_or_else(|_| "https://api.browserbase.com/v1".to_string())
        .trim_end_matches('/')
        .to_string()
}

#[cfg(feature = "browserbase")]
fn browserbase_session_id(payload: &Value) -> Option<String> {
    payload
        .get("id")
        .or_else(|| payload.get("sessionId"))
        .or_else(|| payload.get("session_id"))
        .or_else(|| payload.get("data").and_then(|v| v.get("id")))
        .or_else(|| payload.get("data").and_then(|v| v.get("sessionId")))
        .or_else(|| payload.get("data").and_then(|v| v.get("session_id")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

#[cfg(feature = "browserbase")]
fn browserbase_connect_url(payload: &Value) -> Option<String> {
    [
        "connectUrl",
        "connect_url",
        "wsUrl",
        "ws_url",
        "websocketUrl",
        "websocket_url",
        "browserWSEndpoint",
        "wsEndpoint",
        "ws_endpoint",
    ]
    .iter()
    .find_map(|key| payload.get(*key).and_then(Value::as_str))
    .or_else(|| {
        payload.get("data").and_then(|data| {
            [
                "connectUrl",
                "connect_url",
                "wsUrl",
                "ws_url",
                "websocketUrl",
                "websocket_url",
                "browserWSEndpoint",
                "wsEndpoint",
                "ws_endpoint",
            ]
            .iter()
            .find_map(|key| data.get(*key).and_then(Value::as_str))
        })
    })
    .map(ToString::to_string)
}

fn dedupe_preserve_order(stages: &mut Vec<StrategyUsed>) {
    let mut seen = Vec::new();
    stages.retain(|stage| {
        if seen.contains(stage) {
            false
        } else {
            seen.push(*stage);
            true
        }
    });
}

#[cfg(feature = "browserbase")]
fn maybe_insert_browserbase_stage(stages: &mut Vec<StrategyUsed>, enabled: bool) {
    if !enabled || stages.contains(&StrategyUsed::BrowserbaseManagedSession) {
        return;
    }

    if let Some(pos) = stages
        .iter()
        .position(|stage| *stage == StrategyUsed::StickyProxyBrowserSession)
    {
        stages.insert(pos, StrategyUsed::BrowserbaseManagedSession);
    } else {
        stages.push(StrategyUsed::BrowserbaseManagedSession);
    }
}

fn classify_browser_error(error: &BrowserError) -> StageFailureKind {
    match error {
        BrowserError::Timeout { .. } => StageFailureKind::Timeout,
        BrowserError::NavigationFailed { reason, .. } if reason.contains("selector") => {
            StageFailureKind::Blocked
        }
        BrowserError::ScriptExecutionFailed { .. } => StageFailureKind::Extraction,
        BrowserError::ConfigError(_) | BrowserError::PoolExhausted { .. } => {
            StageFailureKind::Setup
        }
        BrowserError::ProxyUnavailable { .. }
        | BrowserError::ConnectionError { .. }
        | BrowserError::CdpError { .. }
        | BrowserError::LaunchFailed { .. }
        | BrowserError::NavigationFailed { .. }
        | BrowserError::Io(_)
        | BrowserError::StaleNode { .. } => StageFailureKind::Transport,
        #[cfg(feature = "extract")]
        BrowserError::ExtractionFailed(_) => StageFailureKind::Extraction,
    }
}

const fn is_block_status(status: Option<u16>) -> bool {
    matches!(status, Some(401 | 403 | 407 | 429 | 503))
}

fn truncate_html(html: &str, max_bytes: usize) -> String {
    if html.len() <= max_bytes {
        return html.to_string();
    }

    let mut out = String::new();
    for ch in html.chars() {
        if out.len() + ch.len_utf8() > max_bytes {
            break;
        }
        out.push(ch);
    }
    out
}

fn host_hint(url: &str) -> Option<String> {
    let without_scheme = url.split_once("://")?.1;
    let authority = without_scheme.split('/').next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_is_deterministic_for_modes() {
        assert_eq!(
            AcquisitionRunner::strategy_ladder(AcquisitionMode::Fast, None),
            vec![
                StrategyUsed::DirectHttp,
                StrategyUsed::TlsProfiledHttp,
                StrategyUsed::BrowserLightStealth,
            ]
        );

        assert_eq!(
            AcquisitionRunner::strategy_ladder(
                AcquisitionMode::Investigate,
                Some(StrategyUsed::StickyProxyBrowserSession)
            ),
            vec![
                StrategyUsed::InvestigateEntry,
                StrategyUsed::StickyProxyBrowserSession,
                StrategyUsed::TlsProfiledHttp,
            ]
        );
    }

    #[test]
    fn block_statuses_are_classified() {
        assert!(is_block_status(Some(403)));
        assert!(is_block_status(Some(429)));
        assert!(!is_block_status(Some(200)));
        assert!(!is_block_status(None));
    }

    #[test]
    fn host_hint_extracts_authority() {
        assert_eq!(
            host_hint("https://user:pass@example.com:8443/path"),
            Some("example.com".to_string())
        );
    }

    #[test]
    fn truncate_html_respects_utf8_boundaries() {
        let src = "abc😀def";
        let out = truncate_html(src, 5);
        assert_eq!(out, "abc");
    }

    #[cfg(feature = "browserbase")]
    #[test]
    fn browserbase_connect_url_is_extracted_from_nested_data() {
        let payload = serde_json::json!({
            "data": {
                "connectUrl": "wss://connect.browserbase.example/devtools/browser/abc"
            }
        });

        assert_eq!(
            browserbase_connect_url(&payload),
            Some("wss://connect.browserbase.example/devtools/browser/abc".to_string())
        );
    }

    #[cfg(feature = "browserbase")]
    #[test]
    fn browserbase_stage_is_inserted_before_sticky_stage() {
        let mut ladder = vec![
            StrategyUsed::DirectHttp,
            StrategyUsed::StickyProxyBrowserSession,
            StrategyUsed::TlsProfiledHttp,
        ];

        maybe_insert_browserbase_stage(&mut ladder, true);

        assert_eq!(
            ladder,
            vec![
                StrategyUsed::DirectHttp,
                StrategyUsed::BrowserbaseManagedSession,
                StrategyUsed::StickyProxyBrowserSession,
                StrategyUsed::TlsProfiledHttp,
            ]
        );
    }
}
