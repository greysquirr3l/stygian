//! Database source adapter — queries PostgreSQL as a pipeline data source.
//!
//! Implements [`DataSourcePort`](crate::ports::data_source::DataSourcePort) and [`ScrapingService`](crate::ports::ScrapingService) so database queries
//! can participate in a DAG pipeline as a first-class node.
//!
//! Requires the `postgres` feature flag (`sqlx` dependency).
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::database::DatabaseSource;
//! use stygian_graph::ports::data_source::{DataSourcePort, QueryParams};
//! use serde_json::json;
//!
//! # async fn example() {
//! let db = DatabaseSource::new("postgres://user:pass@localhost/mydb").await.unwrap();
//! let rows = db.query(QueryParams {
//!     query: "SELECT id, name FROM users LIMIT 10".into(),
//!     parameters: vec![],
//!     limit: Some(10),
//! }).await.unwrap();
//! # }
//! ```

use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Column, PgPool, Row};

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::data_source::{DataSourcePort, QueryParams};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

// ─────────────────────────────────────────────────────────────────────────────
// DatabaseSource
// ─────────────────────────────────────────────────────────────────────────────

/// Adapter: `PostgreSQL` database as a pipeline data source.
///
/// Wraps a `sqlx::PgPool` and implements both [`DataSourcePort`] (for direct
/// querying) and [`ScrapingService`] (for DAG pipeline integration).
///
/// # Example
///
/// ```no_run
/// use stygian_graph::adapters::database::DatabaseSource;
///
/// # async fn example() {
/// let db = DatabaseSource::new("postgres://localhost/testdb").await.unwrap();
/// println!("Connected to: {}", db.source_name());
/// # }
/// ```
pub struct DatabaseSource {
    pool: PgPool,
    name: String,
}

impl DatabaseSource {
    /// Connect to a `PostgreSQL` database.
    ///
    /// # Arguments
    ///
    /// * `database_url` - `PostgreSQL` connection string
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::database::DatabaseSource;
    ///
    /// # async fn example() {
    /// let db = DatabaseSource::new("postgres://localhost/mydb").await.unwrap();
    /// # }
    /// ```
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .map_err(|e| {
                StygianError::Service(ServiceError::Unavailable(format!(
                    "database connection failed: {e}"
                )))
            })?;

        Ok(Self {
            pool,
            name: "postgres".to_string(),
        })
    }

    /// Create from an existing pool (useful for testing or shared connections).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::database::DatabaseSource;
    /// use sqlx::PgPool;
    ///
    /// # async fn example(pool: PgPool) {
    /// let db = DatabaseSource::from_pool(pool);
    /// # }
    /// ```
    #[must_use]
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool,
            name: "postgres".to_string(),
        }
    }

    /// Return the name of this source for display/logging.
    #[must_use]
    pub fn source_name(&self) -> &str {
        &self.name
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DataSourcePort
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl DataSourcePort for DatabaseSource {
    async fn query(&self, params: QueryParams) -> Result<Vec<Value>> {
        let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&params.query)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!("query failed: {e}")))
            })?;

        let mut results = Vec::with_capacity(rows.len());
        for row in &rows {
            // Convert each row to a JSON object using column metadata
            let columns = row.columns();
            let mut obj = serde_json::Map::new();
            for col in columns {
                let name = col.name().to_string();
                let value: Value = Self::extract_column_value(row, col);
                obj.insert(name, value);
            }
            results.push(Value::Object(obj));

            if let Some(limit) = params.limit
                && results.len() as u64 >= limit
            {
                break;
            }
        }

        Ok(results)
    }

    async fn healthcheck(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                StygianError::Service(ServiceError::Unavailable(format!(
                    "healthcheck failed: {e}"
                )))
            })?;
        Ok(())
    }

    fn source_name(&self) -> &str {
        &self.name
    }
}

impl DatabaseSource {
    /// Extract a column value from a `PgRow` as a `serde_json::Value`.
    ///
    /// Handles common Postgres types; falls back to the debug representation
    /// for unsupported types.
    fn extract_column_value(row: &sqlx::postgres::PgRow, col: &sqlx::postgres::PgColumn) -> Value {
        use sqlx::Column;
        use sqlx::TypeInfo;

        let type_name = col.type_info().name();
        let idx = col.ordinal();

        match type_name {
            "INT4" | "INT2" => row
                .try_get::<i32, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            "INT8" => row
                .try_get::<i64, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            "FLOAT4" => row
                .try_get::<f32, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            "FLOAT8" | "NUMERIC" => row
                .try_get::<f64, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            "BOOL" => row
                .try_get::<bool, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            "TEXT" | "VARCHAR" | "CHAR" | "NAME" => row
                .try_get::<String, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            "JSON" | "JSONB" => row.try_get::<Value, _>(idx).unwrap_or(Value::Null),
            _ => row
                .try_get::<String, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ScrapingService (DAG integration)
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl ScrapingService for DatabaseSource {
    /// Execute a database query from pipeline parameters.
    ///
    /// Expected params:
    /// ```json
    /// { "query": "SELECT ...", "parameters": [...], "limit": 100 }
    /// ```
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::{ScrapingService, ServiceInput};
    /// # use stygian_graph::adapters::database::DatabaseSource;
    /// # use serde_json::json;
    /// # async fn example(db: DatabaseSource) {
    /// let input = ServiceInput {
    ///     url: String::new(),
    ///     params: json!({"query": "SELECT 1 AS n", "parameters": [], "limit": 10}),
    /// };
    /// let result = db.execute(input).await.unwrap();
    /// # }
    /// ```
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let query_str = input
            .params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                StygianError::Service(ServiceError::InvalidResponse(
                    "missing 'query' in params".into(),
                ))
            })?
            .to_string();

        let parameters: Vec<Value> = input
            .params
            .get("parameters")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let limit = input.params.get("limit").and_then(Value::as_u64);

        let params = QueryParams {
            query: query_str,
            parameters,
            limit,
        };

        let rows = self.query(params).await?;
        let row_count = rows.len();

        Ok(ServiceOutput {
            data: serde_json::to_string(&rows).unwrap_or_default(),
            metadata: json!({
                "source": self.name,
                "row_count": row_count,
            }),
        })
    }

    fn name(&self) -> &'static str {
        "database"
    }
}
