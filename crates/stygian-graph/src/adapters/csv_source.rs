//! CSV / TSV [`DataSourcePort`] and [`ScrapingService`] adapter.
//!
//! Reads structured data from CSV or TSV files, returning rows as JSON objects
//! with column names from the header row as keys.
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::csv_source::CsvSource;
//! use stygian_graph::ports::data_source::{DataSourcePort, QueryParams};
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let source = CsvSource::default();
//! let params = QueryParams {
//!     query: "/data/users.csv".into(),
//!     parameters: vec![],
//!     limit: Some(100),
//! };
//! let rows = source.query(params).await.unwrap();
//! # });
//! ```

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use std::io::Read;
use std::path::Path;

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::data_source::{DataSourcePort, QueryParams};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

// ─── Configuration ────────────────────────────────────────────────────────────

/// Delimiter byte for CSV parsing.
#[derive(Debug, Clone, Copy)]
pub enum Delimiter {
    /// Comma (default CSV)
    Comma,
    /// Tab (TSV)
    Tab,
    /// Pipe
    Pipe,
    /// Semicolon
    Semicolon,
    /// Custom byte
    Custom(u8),
}

impl Delimiter {
    fn as_byte(self) -> u8 {
        match self {
            Self::Comma => b',',
            Self::Tab => b'\t',
            Self::Pipe => b'|',
            Self::Semicolon => b';',
            Self::Custom(b) => b,
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "tab" | "tsv" | "\t" => Self::Tab,
            "pipe" | "|" => Self::Pipe,
            "semicolon" | ";" => Self::Semicolon,
            "comma" | "," => Self::Comma,
            _ => {
                // Try single-char custom delimiter
                let bytes = s.as_bytes();
                if bytes.len() == 1 {
                    Self::Custom(bytes[0])
                } else {
                    Self::Comma
                }
            }
        }
    }
}

// ─── Adapter ──────────────────────────────────────────────────────────────────

/// CSV / TSV data source adapter.
///
/// Reads CSV or TSV files using streaming iteration (no full-file buffering).
/// Supports configurable delimiters and optional headers.
#[derive(Default)]
pub struct CsvSource;

impl CsvSource {
    /// Parse CSV data from a reader, returning JSON rows.
    ///
    /// # Arguments
    ///
    /// * `reader` — any `Read` source
    /// * `delimiter` — field separator byte
    /// * `has_headers` — whether the first row contains column names
    /// * `skip` — number of data rows to skip
    /// * `limit` — maximum number of data rows to return
    fn parse_reader<R: Read>(
        reader: R,
        delimiter: Delimiter,
        has_headers: bool,
        skip: usize,
        limit: Option<u64>,
    ) -> Result<Vec<Value>> {
        let mut csv_reader = csv::ReaderBuilder::new()
            .delimiter(delimiter.as_byte())
            .has_headers(has_headers)
            .flexible(true)
            .from_reader(reader);

        let headers: Vec<String> = if has_headers {
            let hdrs = csv_reader.headers().map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "CSV header parse error: {e}"
                )))
            })?;
            hdrs.iter().map(|h| strip_bom(h).to_string()).collect()
        } else {
            Vec::new()
        };

        let mut rows = Vec::new();
        let mut skipped = 0;

        for result in csv_reader.records() {
            let record = result.map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "CSV record parse error: {e}"
                )))
            })?;

            if skipped < skip {
                skipped += 1;
                continue;
            }

            let row = if headers.is_empty() {
                // Generate column_0, column_1, ... keys
                let mut map = Map::new();
                for (i, field) in record.iter().enumerate() {
                    map.insert(format!("column_{i}"), Value::String(field.to_string()));
                }
                Value::Object(map)
            } else {
                let mut map = Map::new();
                for (i, field) in record.iter().enumerate() {
                    let key = headers
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("column_{i}"));
                    map.insert(key, Value::String(field.to_string()));
                }
                Value::Object(map)
            };

            rows.push(row);

            if let Some(max) = limit
                && rows.len() as u64 >= max
            {
                break;
            }
        }

        Ok(rows)
    }
}

/// Strip UTF-8 BOM (U+FEFF) from the beginning of a string.
fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{FEFF}').unwrap_or(s)
}

/// Extract delimiter, skip, limit, and has_headers from params.
fn extract_csv_params(params: &Value) -> (Delimiter, usize, Option<u64>, bool) {
    let delimiter = params
        .get("delimiter")
        .and_then(|v| v.as_str())
        .map(Delimiter::from_str)
        .unwrap_or(Delimiter::Comma);

    let skip = params.get("skip").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    let limit = params.get("limit").and_then(|v| v.as_u64());

    let has_headers = params
        .get("has_headers")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    (delimiter, skip, limit, has_headers)
}

// ─── DataSourcePort ───────────────────────────────────────────────────────────

#[async_trait]
impl DataSourcePort for CsvSource {
    /// Query a CSV file.
    ///
    /// `params.query` is the file path. `params.parameters[0]` can be a JSON
    /// object with `delimiter`, `skip`, `has_headers` keys.
    async fn query(&self, params: QueryParams) -> Result<Vec<Value>> {
        let path = Path::new(&params.query);
        if !path.exists() {
            return Err(StygianError::Service(ServiceError::Unavailable(format!(
                "CSV file not found: {}",
                params.query
            ))));
        }

        let extra = params.parameters.first().cloned().unwrap_or(json!({}));
        let (delimiter, skip, _, has_headers) = extract_csv_params(&extra);
        let limit = params.limit;

        let file = std::fs::File::open(path).map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "failed to open CSV file: {e}"
            )))
        })?;

        // Offload blocking I/O to a spawn_blocking task
        tokio::task::spawn_blocking(move || {
            Self::parse_reader(file, delimiter, has_headers, skip, limit)
        })
        .await
        .map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "CSV parse task failed: {e}"
            )))
        })?
    }

    async fn healthcheck(&self) -> Result<()> {
        Ok(()) // File-based — always "healthy"
    }

    fn source_name(&self) -> &str {
        "csv"
    }
}

// ─── ScrapingService ──────────────────────────────────────────────────────────

#[async_trait]
impl ScrapingService for CsvSource {
    /// Parse a CSV file, returning rows as JSON array.
    ///
    /// `input.url` is the file path. `input.params` can contain:
    /// * `delimiter` — "comma", "tab", "pipe", "semicolon", or single char
    /// * `skip` — number of data rows to skip
    /// * `limit` — max rows to return
    /// * `has_headers` — boolean (default true)
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let path = Path::new(&input.url);
        if !path.exists() {
            return Err(StygianError::Service(ServiceError::Unavailable(format!(
                "CSV file not found: {}",
                input.url
            ))));
        }

        let (delimiter, skip, limit, has_headers) = extract_csv_params(&input.params);

        let file = std::fs::File::open(path).map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "failed to open CSV file: {e}"
            )))
        })?;

        let rows = tokio::task::spawn_blocking(move || {
            Self::parse_reader(file, delimiter, has_headers, skip, limit)
        })
        .await
        .map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "CSV parse task failed: {e}"
            )))
        })??;

        let count = rows.len();
        let data = serde_json::to_string(&rows).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "CSV serialization failed: {e}"
            )))
        })?;

        Ok(ServiceOutput {
            data,
            metadata: json!({
                "source": "csv",
                "row_count": count,
                "source_path": input.url,
            }),
        })
    }

    fn name(&self) -> &'static str {
        "csv"
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const CSV_DATA: &str = "name,age,city\nAlice,30,NYC\nBob,25,SF\nCharlie,35,LA\n";

    #[test]
    fn parse_csv_with_headers() {
        let reader = Cursor::new(CSV_DATA);
        let rows = CsvSource::parse_reader(reader, Delimiter::Comma, true, 0, None).expect("parse");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["name"], "Alice");
        assert_eq!(rows[0]["age"], "30");
        assert_eq!(rows[0]["city"], "NYC");
        assert_eq!(rows[2]["name"], "Charlie");
    }

    #[test]
    fn parse_tsv() {
        let tsv = "name\tage\nAlice\t30\nBob\t25\n";
        let reader = Cursor::new(tsv);
        let rows = CsvSource::parse_reader(reader, Delimiter::Tab, true, 0, None).expect("parse");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "Alice");
        assert_eq!(rows[1]["age"], "25");
    }

    #[test]
    fn headerless_csv_generates_column_keys() {
        let csv = "Alice,30,NYC\nBob,25,SF\n";
        let reader = Cursor::new(csv);
        let rows =
            CsvSource::parse_reader(reader, Delimiter::Comma, false, 0, None).expect("parse");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["column_0"], "Alice");
        assert_eq!(rows[0]["column_1"], "30");
        assert_eq!(rows[0]["column_2"], "NYC");
    }

    #[test]
    fn row_limit() {
        let reader = Cursor::new(CSV_DATA);
        let rows =
            CsvSource::parse_reader(reader, Delimiter::Comma, true, 0, Some(2)).expect("parse");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "Alice");
        assert_eq!(rows[1]["name"], "Bob");
    }

    #[test]
    fn skip_rows() {
        let reader = Cursor::new(CSV_DATA);
        let rows = CsvSource::parse_reader(reader, Delimiter::Comma, true, 1, None).expect("parse");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "Bob");
        assert_eq!(rows[1]["name"], "Charlie");
    }

    #[test]
    fn skip_and_limit() {
        let reader = Cursor::new(CSV_DATA);
        let rows =
            CsvSource::parse_reader(reader, Delimiter::Comma, true, 1, Some(1)).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "Bob");
    }

    #[test]
    fn strip_utf8_bom() {
        let csv = "\u{FEFF}name,age\nAlice,30\n";
        let reader = Cursor::new(csv);
        let rows = CsvSource::parse_reader(reader, Delimiter::Comma, true, 0, None).expect("parse");
        assert_eq!(rows.len(), 1);
        // Key should NOT have the BOM
        assert!(rows[0].get("name").is_some(), "BOM should be stripped");
    }

    #[test]
    fn pipe_delimiter() {
        let csv = "a|b|c\n1|2|3\n";
        let reader = Cursor::new(csv);
        let rows = CsvSource::parse_reader(reader, Delimiter::Pipe, true, 0, None).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["a"], "1");
        assert_eq!(rows[0]["b"], "2");
    }

    #[test]
    fn semicolon_delimiter() {
        let csv = "x;y\n10;20\n";
        let reader = Cursor::new(csv);
        let rows =
            CsvSource::parse_reader(reader, Delimiter::Semicolon, true, 0, None).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["x"], "10");
    }

    #[test]
    fn empty_csv_returns_empty() {
        let csv = "name,age\n";
        let reader = Cursor::new(csv);
        let rows = CsvSource::parse_reader(reader, Delimiter::Comma, true, 0, None).expect("parse");
        assert!(rows.is_empty());
    }

    #[test]
    fn delimiter_from_str_parsing() {
        assert_eq!(Delimiter::from_str("tab").as_byte(), b'\t');
        assert_eq!(Delimiter::from_str("tsv").as_byte(), b'\t');
        assert_eq!(Delimiter::from_str("pipe").as_byte(), b'|');
        assert_eq!(Delimiter::from_str("semicolon").as_byte(), b';');
        assert_eq!(Delimiter::from_str("comma").as_byte(), b',');
        assert_eq!(Delimiter::from_str(",").as_byte(), b',');
        assert_eq!(Delimiter::from_str("unknown").as_byte(), b','); // default
    }

    #[tokio::test]
    async fn file_not_found_returns_error() {
        let source = CsvSource;
        let params = QueryParams {
            query: "/tmp/nonexistent_csv_file_stygian.csv".into(),
            parameters: vec![],
            limit: None,
        };
        let result = source.query(params).await;
        assert!(result.is_err());
    }
}
