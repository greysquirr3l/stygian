//! Database source port — query databases as pipeline data sources.
//!
//! Defines the [`DataSourcePort`](crate::ports::data_source::DataSourcePort) trait for executing queries against
//! relational or document databases and returning results as
//! [`serde_json::Value`] rows.
//!
//! # Architecture
//!
//! ```text
//! stygian-graph
//!   ├─ DataSourcePort (this file)          ← always compiled
//!   └─ Adapters (adapters/)
//!        └─ DatabaseSource (feature="postgres")  → sqlx PgPool
//! ```
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::ports::data_source::{DataSourcePort, QueryParams};
//! use serde_json::json;
//!
//! async fn query<D: DataSourcePort>(db: &D) {
//!     let params = QueryParams {
//!         query: "SELECT id, name FROM users WHERE active = $1".into(),
//!         parameters: vec![json!(true)],
//!         limit: Some(100),
//!     };
//!     let rows = db.query(params).await.unwrap();
//!     for row in &rows {
//!         println!("{row}");
//!     }
//! }
//! ```

use crate::domain::error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─────────────────────────────────────────────────────────────────────────────
// QueryParams
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for a database query.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::data_source::QueryParams;
/// use serde_json::json;
///
/// let params = QueryParams {
///     query: "SELECT * FROM items WHERE price > $1".into(),
///     parameters: vec![json!(9.99)],
///     limit: Some(50),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// SQL or query-language statement to execute
    pub query: String,
    /// Positional bind parameters
    pub parameters: Vec<Value>,
    /// Optional row limit (applied as `LIMIT` or equivalent)
    pub limit: Option<u64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// DataSourcePort
// ─────────────────────────────────────────────────────────────────────────────

/// Port: query a database and return rows as JSON values.
///
/// Implementations connect to PostgreSQL, MySQL, SQLite, MongoDB, or any
/// other datastore and return results as `Vec<Value>`.
///
/// # Example
///
/// ```no_run
/// use stygian_graph::ports::data_source::{DataSourcePort, QueryParams};
/// use stygian_graph::domain::error::Result;
/// use async_trait::async_trait;
/// use serde_json::{json, Value};
///
/// struct MockDb;
///
/// #[async_trait]
/// impl DataSourcePort for MockDb {
///     async fn query(&self, params: QueryParams) -> Result<Vec<Value>> {
///         Ok(vec![json!({"id": 1, "name": "test"})])
///     }
///
///     async fn healthcheck(&self) -> Result<()> {
///         Ok(())
///     }
///
///     fn source_name(&self) -> &str {
///         "mock-db"
///     }
/// }
/// ```
#[async_trait]
pub trait DataSourcePort: Send + Sync {
    /// Execute a query and return results as JSON rows.
    ///
    /// # Arguments
    ///
    /// * `params` - Query statement, bind parameters, and optional limit
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<Value>)` - Result rows serialised as JSON objects
    /// * `Err(StygianError)` - Query or connection error
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::data_source::{DataSourcePort, QueryParams};
    /// # use serde_json::json;
    /// # async fn example(db: impl DataSourcePort) {
    /// let params = QueryParams {
    ///     query: "SELECT 1 AS n".into(),
    ///     parameters: vec![],
    ///     limit: None,
    /// };
    /// let rows = db.query(params).await.unwrap();
    /// # }
    /// ```
    async fn query(&self, params: QueryParams) -> Result<Vec<Value>>;

    /// Check that the underlying connection is alive.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::data_source::DataSourcePort;
    /// # async fn example(db: impl DataSourcePort) {
    /// db.healthcheck().await.unwrap();
    /// # }
    /// ```
    async fn healthcheck(&self) -> Result<()>;

    /// Human-readable name of this data source (e.g. `"postgres"`, `"sqlite"`).
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::data_source::DataSourcePort;
    /// # fn example(db: impl DataSourcePort) {
    /// println!("Connected to: {}", db.source_name());
    /// # }
    /// ```
    fn source_name(&self) -> &str;
}
