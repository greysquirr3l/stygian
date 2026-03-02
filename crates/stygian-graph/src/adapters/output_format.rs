//! Output format helpers — CSV, JSONL, JSON.
//!
//! Implements [`OutputFormatter`](crate::ports::storage::OutputFormatter) for the three formats defined in
//! [`crate::ports::storage::OutputFormat`].

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::storage::{OutputFormat, OutputFormatter, StorageRecord};

// ─────────────────────────────────────────────────────────────────────────────
// JsonlFormatter
// ─────────────────────────────────────────────────────────────────────────────

/// Serialises records as newline-delimited JSON (one JSON object per line).
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::output_format::JsonlFormatter;
/// use stygian_graph::ports::storage::{OutputFormatter, StorageRecord};
/// use serde_json::json;
///
/// let formatter = JsonlFormatter;
/// let records = vec![StorageRecord::new("p", "n", json!({"x": 1}))];
/// let bytes = formatter.format(&records).unwrap();
/// let text = String::from_utf8(bytes).unwrap();
/// assert!(text.contains("\"x\":1") || text.contains("\"x\": 1"));
/// assert!(text.ends_with('\n'));
/// ```
pub struct JsonlFormatter;

impl OutputFormatter for JsonlFormatter {
    fn format(&self, records: &[StorageRecord]) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        for record in records {
            let line = serde_json::to_string(record).map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "JSONL serialisation error: {e}"
                )))
            })?;
            out.extend_from_slice(line.as_bytes());
            out.push(b'\n');
        }
        Ok(out)
    }

    fn format_type(&self) -> OutputFormat {
        OutputFormat::Jsonl
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// JsonFormatter
// ─────────────────────────────────────────────────────────────────────────────

/// Serialises records as a pretty-printed JSON array.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::output_format::JsonFormatter;
/// use stygian_graph::ports::storage::{OutputFormatter, StorageRecord};
/// use serde_json::json;
///
/// let formatter = JsonFormatter;
/// let records = vec![StorageRecord::new("p", "n", json!({}))];
/// let bytes = formatter.format(&records).unwrap();
/// let text = String::from_utf8(bytes).unwrap();
/// assert!(text.starts_with('['));
/// assert!(text.ends_with("]\n"));
/// ```
pub struct JsonFormatter;

impl OutputFormatter for JsonFormatter {
    fn format(&self, records: &[StorageRecord]) -> Result<Vec<u8>> {
        let mut out = serde_json::to_vec_pretty(records).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "JSON serialisation error: {e}"
            )))
        })?;
        out.push(b'\n');
        Ok(out)
    }

    fn format_type(&self) -> OutputFormat {
        OutputFormat::Json
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CsvFormatter
// ─────────────────────────────────────────────────────────────────────────────

/// Serialises records as CSV.
///
/// Columns: `id`, `pipeline_id`, `node_name`, `timestamp_ms`, `data`.
/// The `data` field is embedded as a compact JSON string (escaped per RFC 4180).
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::output_format::CsvFormatter;
/// use stygian_graph::ports::storage::{OutputFormatter, StorageRecord};
/// use serde_json::json;
///
/// let formatter = CsvFormatter;
/// let records = vec![StorageRecord::new("p", "n", json!({"k": "v"}))];
/// let bytes = formatter.format(&records).unwrap();
/// let text = String::from_utf8(bytes).unwrap();
/// assert!(text.starts_with("id,pipeline_id,node_name,timestamp_ms,data\n"));
/// ```
pub struct CsvFormatter;

impl OutputFormatter for CsvFormatter {
    fn format(&self, records: &[StorageRecord]) -> Result<Vec<u8>> {
        let mut wtr = csv::WriterBuilder::new()
            .has_headers(true)
            .from_writer(Vec::new());

        // Write header row
        wtr.write_record(["id", "pipeline_id", "node_name", "timestamp_ms", "data"])
            .map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "CSV header error: {e}"
                )))
            })?;

        for record in records {
            let data_str = serde_json::to_string(&record.data).map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "CSV data serialisation error: {e}"
                )))
            })?;
            wtr.write_record([
                &record.id,
                &record.pipeline_id,
                &record.node_name,
                &record.timestamp_ms.to_string(),
                &data_str,
            ])
            .map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "CSV write error: {e}"
                )))
            })?;
        }

        let bytes = wtr.into_inner().map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "CSV finalisation error: {e}"
            )))
        })?;

        Ok(bytes)
    }

    fn format_type(&self) -> OutputFormat {
        OutputFormat::Csv
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Convenience constructor
// ─────────────────────────────────────────────────────────────────────────────

/// Return the appropriate [`OutputFormatter`] boxed for the given format.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::output_format::{formatter_for, CsvFormatter};
/// use stygian_graph::ports::storage::OutputFormat;
///
/// let f = formatter_for(OutputFormat::Csv);
/// assert_eq!(f.format_type(), OutputFormat::Csv);
/// ```
pub fn formatter_for(format: OutputFormat) -> Box<dyn OutputFormatter> {
    match format {
        OutputFormat::Jsonl => Box::new(JsonlFormatter),
        OutputFormat::Json => Box::new(JsonFormatter),
        OutputFormat::Csv => Box::new(CsvFormatter),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jsonl_produces_one_line_per_record() {
        let records = vec![
            StorageRecord::new("p", "n", json!({"a": 1})),
            StorageRecord::new("p", "n", json!({"b": 2})),
        ];
        let bytes = JsonlFormatter.format(&records).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            let _: StorageRecord = serde_json::from_str(line).expect("valid JSONL");
        }
    }

    #[test]
    fn json_produces_array() {
        let records = vec![StorageRecord::new("p", "n", json!({"x": 42}))];
        let bytes = JsonFormatter.format(&records).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with('['), "should start with [");
        let _: Vec<StorageRecord> = serde_json::from_str(text.trim()).expect("valid JSON array");
    }

    #[test]
    fn csv_has_header_and_row() {
        let records = vec![StorageRecord::new("pipe-1", "node-a", json!({"k": "v"}))];
        let bytes = CsvFormatter.format(&records).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let mut lines = text.lines();
        let header = lines.next().unwrap();
        assert_eq!(header, "id,pipeline_id,node_name,timestamp_ms,data");
        let data_line = lines.next().unwrap();
        assert!(data_line.contains("pipe-1"));
        assert!(data_line.contains("node-a"));
    }

    #[test]
    fn csv_empty_records_only_header() {
        let bytes = CsvFormatter.format(&[]).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "id,pipeline_id,node_name,timestamp_ms,data");
    }

    #[test]
    fn formatter_for_selects_correct_type() {
        assert_eq!(
            formatter_for(OutputFormat::Jsonl).format_type(),
            OutputFormat::Jsonl
        );
        assert_eq!(
            formatter_for(OutputFormat::Json).format_type(),
            OutputFormat::Json
        );
        assert_eq!(
            formatter_for(OutputFormat::Csv).format_type(),
            OutputFormat::Csv
        );
    }
}
