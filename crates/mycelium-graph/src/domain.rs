//! Domain layer - core business logic
//!
//! Contains pure business logic with no infrastructure dependencies.
//! Domain layer only imports from ports (trait definitions), never from adapters.

/// DAG execution engine using petgraph
pub mod graph;

/// Pipeline types with typestate pattern
pub mod pipeline;

/// Worker pool executor with backpressure
pub mod executor;

/// Idempotency tracking system
pub mod idempotency;

/// Domain error types
pub mod error {
    use thiserror::Error;

    /// Primary error type for the mycelium-graph crate
    ///
    /// Encompasses all domain-level errors following hexagonal architecture principles.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::domain::error::{MyceliumError, GraphError};
    ///
    /// fn validate_pipeline() -> Result<(), MyceliumError> {
    ///     Err(MyceliumError::Graph(GraphError::CycleDetected))
    /// }
    /// ```
    #[derive(Debug, Error)]
    pub enum MyceliumError {
        /// Graph execution errors
        #[error("Graph error: {0}")]
        Graph(#[from] GraphError),

        /// Service interaction errors
        #[error("Service error: {0}")]
        Service(#[from] ServiceError),

        /// AI provider errors
        #[error("Provider error: {0}")]
        Provider(#[from] ProviderError),

        /// Configuration errors
        #[error("Config error: {0}")]
        Config(#[from] ConfigError),

        /// Rate limiting errors
        #[error("Rate limit error: {0}")]
        RateLimit(#[from] RateLimitError),

        /// Caching errors
        #[error("Cache error: {0}")]
        Cache(#[from] CacheError),
    }

    /// Graph-specific errors
    #[derive(Debug, Error)]
    pub enum GraphError {
        /// Graph contains a cycle (DAG violation)
        #[error("Graph contains cycle, cannot execute")]
        CycleDetected,

        /// Invalid node reference
        #[error("Node not found: {0}")]
        NodeNotFound(String),

        /// Invalid edge configuration
        #[error("Invalid edge: {0}")]
        InvalidEdge(String),

        /// Pipeline validation failed
        #[error("Invalid pipeline: {0}")]
        InvalidPipeline(String),

        /// Execution failed
        #[error("Execution failed: {0}")]
        ExecutionFailed(String),
    }

    /// Service-level errors
    #[derive(Debug, Error)]
    pub enum ServiceError {
        /// Service unavailable
        #[error("Service unavailable: {0}")]
        Unavailable(String),

        /// Service timeout
        #[error("Service timeout after {0}ms")]
        Timeout(u64),

        /// Service returned invalid response
        #[error("Invalid response: {0}")]
        InvalidResponse(String),

        /// Service authentication failed
        #[error("Authentication failed: {0}")]
        AuthenticationFailed(String),

        /// Service is rate-limiting; caller should retry after the given delay
        #[error("Rate limited, retry after {retry_after_ms}ms")]
        RateLimited {
            /// Milliseconds to wait before retrying
            retry_after_ms: u64,
        },
    }

    /// AI provider errors
    #[derive(Debug, Error)]
    pub enum ProviderError {
        /// Provider API error
        #[error("API error: {0}")]
        ApiError(String),

        /// Invalid API key or credentials
        #[error("Invalid credentials")]
        InvalidCredentials,

        /// Token limit exceeded
        #[error("Token limit exceeded: {0}")]
        TokenLimitExceeded(String),

        /// Model not available
        #[error("Model not available: {0}")]
        ModelUnavailable(String),

        /// Content policy violation
        #[error("Content policy violation: {0}")]
        ContentPolicyViolation(String),
    }

    /// Configuration errors
    #[derive(Debug, Error)]
    pub enum ConfigError {
        /// Missing required configuration
        #[error("Missing required config: {0}")]
        MissingConfig(String),

        /// Invalid configuration value
        #[error("Invalid config value for '{key}': {reason}")]
        InvalidValue {
            /// Configuration key
            key: String,
            /// Reason for invalidity
            reason: String,
        },

        /// Configuration file error
        #[error("Config file error: {0}")]
        FileError(String),

        /// TOML parsing error
        #[error("TOML parse error: {0}")]
        ParseError(String),
    }

    /// Rate limiting errors
    #[derive(Debug, Error)]
    pub enum RateLimitError {
        /// Rate limit exceeded
        #[error("Rate limit exceeded: {0} requests per {1} seconds")]
        Exceeded(u32, u32),

        /// Quota exhausted
        #[error("Quota exhausted: {0}")]
        QuotaExhausted(String),

        /// Retry after duration
        #[error("Retry after {0} seconds")]
        RetryAfter(u64),
    }

    /// Caching errors
    #[derive(Debug, Error)]
    pub enum CacheError {
        /// Cache miss
        #[error("Cache miss: {0}")]
        Miss(String),

        /// Cache write failed
        #[error("Cache write failed: {0}")]
        WriteFailed(String),

        /// Cache read failed
        #[error("Cache read failed: {0}")]
        ReadFailed(String),

        /// Cache eviction failed
        #[error("Cache eviction failed: {0}")]
        EvictionFailed(String),

        /// Cache corrupted
        #[error("Cache corrupted: {0}")]
        Corrupted(String),
    }

    /// Domain result type using `MyceliumError`
    pub type Result<T> = std::result::Result<T, MyceliumError>;

    /// Legacy domain error (kept for backward compatibility)
    #[derive(Debug, Error)]
    pub enum DomainError {
        /// Graph contains cycle
        #[error("Graph contains cycle, cannot execute")]
        CycleDetected,

        /// Invalid pipeline configuration
        #[error("Invalid pipeline: {0}")]
        InvalidPipeline(String),

        /// Execution failed
        #[error("Execution failed: {0}")]
        ExecutionFailed(String),

        /// Service error
        #[error("Service error: {0}")]
        ServiceError(String),
    }

    /// Legacy domain result type
    pub type DomainResult<T> = std::result::Result<T, DomainError>;
}
