//! Advanced TOML pipeline definition parser & validator
//!
//! Supports `[[nodes]]` and `[[services]]` TOML blocks that describe scraping
//! Directed Acyclic Graphs (DAGs).  Includes:
//!
//! - Layered config loading: TOML file → environment overrides
//! - Template variable expansion: `${env:VAR}`, `${node:NAME.field}`
//! - DAG cycle detection via DFS
//! - Service reference validation against the registry
//! - Graph visualization export (Graphviz DOT and Mermaid)
//!
//! # TOML format
//!
//! ```toml
//! [[services]]
//! name = "fetcher"
//! kind = "http"
//!
//! [[services]]
//! name = "extractor"
//! kind = "claude"
//! api_key = "${env:ANTHROPIC_API_KEY}"
//!
//! [[nodes]]
//! name = "fetch"
//! service = "fetcher"
//! url = "https://example.com"
//!
//! [[nodes]]
//! name = "extract"
//! service = "extractor"
//! depends_on = ["fetch"]
//! ```
//!
//! # Example
//!
//! ```
//! use mycelium_graph::application::pipeline_parser::{PipelineParser, PipelineDefinition};
//!
//! let toml = r#"
//! [[nodes]]
//! name = "a"
//! service = "http"
//! url = "https://example.com"
//!
//! [[nodes]]
//! name = "b"
//! depends_on = ["a"]
//! service = "http"
//! url = "https://example.com"
//! "#;
//!
//! let def = PipelineParser::from_str(toml).unwrap();
//! assert_eq!(def.nodes.len(), 2);
//! assert!(def.validate().is_ok());
//! ```

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::time::Duration;

use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Error ────────────────────────────────────────────────────────────────────

/// Errors produced during pipeline parsing and validation
#[derive(Debug, Error)]
pub enum PipelineError {
    /// TOML deserialization failed
    #[error("TOML parse error: {0}")]
    ParseError(#[from] toml::de::Error),

    /// A node references an unknown service
    #[error("Node '{node}' references unknown service '{service}'")]
    UnknownService {
        /// The node that contains the bad reference
        node: String,
        /// The service name that could not be resolved
        service: String,
    },

    /// A node's `depends_on` references a node that doesn't exist
    #[error("Node '{node}' depends on unknown node '{dep}'")]
    UnknownDependency {
        /// The node that declares the bad dependency
        node: String,
        /// The missing upstream node name
        dep: String,
    },

    /// The DAG contains a cycle
    #[error("Cycle detected involving node '{0}'")]
    CycleDetected(String),

    /// A required field is missing
    #[error("Node '{node}' is missing required field: {field}")]
    MissingField {
        /// The node missing the required field
        node: String,
        /// The field name that is absent
        field: String,
    },

    /// I/O error reading a config file
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Figment layered config error
    #[error("Config error: {0}")]
    FigmentError(String),
}

// ─── Data model ───────────────────────────────────────────────────────────────

/// A service adapter declaration in the TOML config
///
/// Each `[[services]]` block declares an adapter that nodes can reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceDecl {
    /// Unique service name referenced by nodes
    pub name: String,
    /// Adapter kind: "http", "claude", "openai", "gemini", "copilot", "ollama", "browser", etc.
    pub kind: String,
    /// Optional model identifier
    pub model: Option<String>,
    /// Other KV configuration (`api_key`, `base_url`, …)
    #[serde(flatten)]
    pub extra: HashMap<String, toml::Value>,
}

/// A single node in the pipeline DAG
///
/// Each `[[nodes]]` block describes one pipeline step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeDecl {
    /// Unique node name within the pipeline
    pub name: String,
    /// Service adapter this node runs
    #[serde(default)]
    pub service: String,
    /// Upstream dependencies (nodes that must complete before this one runs)
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Entry point URL or content for this node
    pub url: Option<String>,
    /// Additional node parameters
    #[serde(flatten)]
    pub params: HashMap<String, toml::Value>,
}

/// Top-level pipeline definition
///
/// Parsed from a TOML document. Use [`PipelineParser::from_str`] or
/// [`PipelineParser::from_file`] to obtain an instance, then call
/// [`PipelineDefinition::validate`] to check structural correctness.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PipelineDefinition {
    /// Service adapter declarations
    #[serde(default)]
    pub services: Vec<ServiceDecl>,
    /// Node declarations that form the DAG
    #[serde(default)]
    pub nodes: Vec<NodeDecl>,
}

impl PipelineDefinition {
    /// Validate the pipeline definition.
    ///
    /// Checks:
    /// 1. All node `service` references exist in `services` (or known external names).
    /// 2. All `depends_on` entries refer to existing nodes.
    /// 3. The dependency graph is acyclic.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::pipeline_parser::PipelineParser;
    ///
    /// let toml = r#"
    /// [[nodes]]
    /// name = "a"
    /// service = "http"
    ///
    /// [[nodes]]
    /// name = "b"
    /// service = "http"
    /// depends_on = ["a"]
    /// "#;
    ///
    /// let def = PipelineParser::from_str(toml).unwrap();
    /// assert!(def.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), PipelineError> {
        let service_names: HashSet<&str> = self.services.iter().map(|s| s.name.as_str()).collect();
        let node_names: HashSet<&str> = self.nodes.iter().map(|n| n.name.as_str()).collect();

        for node in &self.nodes {
            // Service field must not be empty
            if node.service.is_empty() {
                return Err(PipelineError::MissingField {
                    node: node.name.clone(),
                    field: "service".to_string(),
                });
            }

            // Service reference check — skip if no services declared (external registry)
            if !self.services.is_empty() && !service_names.contains(node.service.as_str()) {
                return Err(PipelineError::UnknownService {
                    node: node.name.clone(),
                    service: node.service.clone(),
                });
            }

            // Dependency existence check
            for dep in &node.depends_on {
                if !node_names.contains(dep.as_str()) {
                    return Err(PipelineError::UnknownDependency {
                        node: node.name.clone(),
                        dep: dep.clone(),
                    });
                }
            }
        }

        // Cycle detection via Kahn's algorithm (topological sort)
        self.detect_cycles()?;

        Ok(())
    }

    /// Detect cycles using Kahn's topological sort algorithm.
    ///
    /// Returns `Ok(())` when the graph is a valid DAG, or
    /// `Err(PipelineError::CycleDetected(node))` on the first cycle found.
    fn detect_cycles(&self) -> Result<(), PipelineError> {
        // Build adjacency + in-degree map
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut children: HashMap<&str, Vec<&str>> = HashMap::new();

        for node in &self.nodes {
            in_degree.entry(node.name.as_str()).or_insert(0);
            children.entry(node.name.as_str()).or_default();
            for dep in &node.depends_on {
                *in_degree.entry(node.name.as_str()).or_insert(0) += 1;
                children
                    .entry(dep.as_str())
                    .or_default()
                    .push(node.name.as_str());
            }
        }

        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter_map(|(&k, &v)| if v == 0 { Some(k) } else { None })
            .collect();

        let mut processed = 0usize;

        while let Some(node) = queue.pop_front() {
            processed += 1;
            if let Some(dependents) = children.get(node) {
                for &dep in dependents {
                    if let Some(deg) = in_degree.get_mut(dep) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(dep);
                        }
                    }
                }
            }
        }

        if processed < self.nodes.len() {
            // Find a node still with nonzero in-degree as the cycle root
            let cycle_node = in_degree
                .iter()
                .find(|&(_, &v)| v > 0)
                .map_or("<unknown>", |(&k, _)| k);
            return Err(PipelineError::CycleDetected(cycle_node.to_string()));
        }

        Ok(())
    }

    /// Validate that all referenced services exist in the given registry names.
    ///
    /// Useful for runtime validation after the registry has been populated.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::pipeline_parser::PipelineParser;
    ///
    /// let toml = r#"
    /// [[nodes]]
    /// name = "fetch"
    /// service = "http"
    /// "#;
    ///
    /// let def = PipelineParser::from_str(toml).unwrap();
    /// let registered = vec!["http".to_string(), "claude".to_string()];
    /// assert!(def.validate_against_registry(&registered).is_ok());
    /// ```
    pub fn validate_against_registry<S: AsRef<str>>(
        &self,
        registered_services: &[S],
    ) -> Result<(), PipelineError> {
        let names: HashSet<&str> = registered_services.iter().map(AsRef::as_ref).collect();
        for node in &self.nodes {
            if !names.contains(node.service.as_str()) {
                return Err(PipelineError::UnknownService {
                    node: node.name.clone(),
                    service: node.service.clone(),
                });
            }
        }
        Ok(())
    }

    /// Expand template variables in string values across all nodes and services.
    ///
    /// Supports:
    /// - `${env:VAR_NAME}` — replaced with `std::env::var("VAR_NAME")` or left as-is
    ///
    /// Modifies the definition in-place and returns `self` for chaining.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::pipeline_parser::PipelineParser;
    ///
    /// // Use HOME which is always available on Unix
    /// let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    /// let toml = format!(r#"
    /// [[nodes]]
    /// name = "fetch"
    /// service = "http"
    /// url = "${{env:HOME}}"
    /// "#);
    ///
    /// let mut def = PipelineParser::from_str(&toml).unwrap();
    /// def.expand_templates();
    ///
    /// assert_eq!(def.nodes[0].url.as_deref(), Some(home.as_str()));
    /// ```
    pub fn expand_templates(&mut self) {
        for node in &mut self.nodes {
            if let Some(url) = &node.url {
                node.url = Some(expand_template(url));
            }
        }

        for service in &mut self.services {
            service.extra = service
                .extra
                .iter()
                .map(|(k, v)| (k.clone(), expand_toml_value(v)))
                .collect();
        }
    }

    /// Compute a topological ordering of the nodes (dependencies first).
    ///
    /// Returns `Err` if the graph contains a cycle.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::pipeline_parser::PipelineParser;
    ///
    /// let toml = r#"
    /// [[nodes]]
    /// name = "c"
    /// service = "http"
    /// depends_on = ["b"]
    ///
    /// [[nodes]]
    /// name = "a"
    /// service = "http"
    ///
    /// [[nodes]]
    /// name = "b"
    /// service = "http"
    /// depends_on = ["a"]
    /// "#;
    ///
    /// let def = PipelineParser::from_str(toml).unwrap();
    /// let order = def.topological_order().unwrap();
    /// assert_eq!(order[0], "a");
    /// assert_eq!(order[2], "c");
    /// ```
    pub fn topological_order(&self) -> Result<Vec<String>, PipelineError> {
        self.detect_cycles()?;

        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut children: HashMap<&str, Vec<&str>> = HashMap::new();

        for node in &self.nodes {
            in_degree.entry(node.name.as_str()).or_insert(0);
            children.entry(node.name.as_str()).or_default();
            for dep in &node.depends_on {
                *in_degree.entry(node.name.as_str()).or_insert(0) += 1;
                children
                    .entry(dep.as_str())
                    .or_default()
                    .push(node.name.as_str());
            }
        }

        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter_map(|(&k, &v)| if v == 0 { Some(k) } else { None })
            .collect();

        let mut order = Vec::new();

        while let Some(node) = queue.pop_front() {
            order.push(node.to_string());
            if let Some(dependents) = children.get(node) {
                for &dep in dependents {
                    if let Some(deg) = in_degree.get_mut(dep) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(dep);
                        }
                    }
                }
            }
        }

        Ok(order)
    }

    /// Export the pipeline DAG as a Graphviz DOT string.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::pipeline_parser::PipelineParser;
    ///
    /// let toml = r#"
    /// [[nodes]]
    /// name = "a"
    /// service = "http"
    ///
    /// [[nodes]]
    /// name = "b"
    /// service = "http"
    /// depends_on = ["a"]
    /// "#;
    ///
    /// let def = PipelineParser::from_str(toml).unwrap();
    /// let dot = def.to_dot();
    /// assert!(dot.contains("digraph"));
    /// assert!(dot.contains(r#""a" -> "b""#));
    /// ```
    pub fn to_dot(&self) -> String {
        let mut out = String::from("digraph pipeline {\n  rankdir=LR;\n");
        for node in &self.nodes {
            let _ = writeln!(
                out,
                "  \"{}\" [label=\"{}\\n({})\"]; ",
                node.name, node.name, node.service
            );
        }
        for node in &self.nodes {
            for dep in &node.depends_on {
                let _ = writeln!(out, "  \"{}\" -> \"{}\";", dep, node.name);
            }
        }
        out.push('}');
        out
    }

    /// Export the pipeline DAG as a Mermaid flowchart string.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::pipeline_parser::PipelineParser;
    ///
    /// let toml = r#"
    /// [[nodes]]
    /// name = "fetch"
    /// service = "http"
    ///
    /// [[nodes]]
    /// name = "parse"
    /// service = "claude"
    /// depends_on = ["fetch"]
    /// "#;
    ///
    /// let def = PipelineParser::from_str(toml).unwrap();
    /// let mermaid = def.to_mermaid();
    /// assert!(mermaid.contains("flowchart LR"));
    /// assert!(mermaid.contains("fetch --> parse"));
    /// ```
    pub fn to_mermaid(&self) -> String {
        let mut out = String::from("flowchart LR\n");
        for node in &self.nodes {
            let _ = writeln!(out, "  {}[\"{}\\n{}\"]", node.name, node.name, node.service);
        }
        for node in &self.nodes {
            for dep in &node.depends_on {
                let _ = writeln!(out, "  {} --> {}", dep, node.name);
            }
        }
        out
    }
}

// ─── Template expansion helpers ───────────────────────────────────────────────

/// Expand `${env:VAR}` tokens in a string using `std::env::var`.
pub(crate) fn expand_template(s: &str) -> String {
    let mut result = s.to_string();
    let mut start = 0;
    while let Some(pos) = result[start..].find("${env:") {
        let abs = start + pos;
        if let Some(end) = result[abs..].find('}') {
            let token = &result[abs..=abs + end]; // e.g. "${env:FOO}"
            let var_name = &token[6..token.len() - 1]; // "FOO"
            if let Ok(value) = std::env::var(var_name) {
                result = result.replace(token, &value);
                start = abs + value.len();
            } else {
                start = abs + token.len();
            }
        } else {
            break;
        }
    }
    result
}

/// Recursively expand templates inside a TOML value (strings only)
fn expand_toml_value(v: &toml::Value) -> toml::Value {
    match v {
        toml::Value::String(s) => toml::Value::String(expand_template(s)),
        toml::Value::Table(map) => toml::Value::Table(
            map.iter()
                .map(|(k, v)| (k.clone(), expand_toml_value(v)))
                .collect(),
        ),
        toml::Value::Array(arr) => toml::Value::Array(arr.iter().map(expand_toml_value).collect()),
        other => other.clone(),
    }
}

// ─── Parser ───────────────────────────────────────────────────────────────────

/// TOML pipeline parser
pub struct PipelineParser;

impl PipelineParser {
    /// Parse a pipeline definition from a TOML string.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::pipeline_parser::PipelineParser;
    ///
    /// let def = PipelineParser::from_str(r#"
    /// [[nodes]]
    /// name = "n1"
    /// service = "http"
    /// "#).unwrap();
    ///
    /// assert_eq!(def.nodes[0].name, "n1");
    /// ```
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(toml: &str) -> Result<PipelineDefinition, PipelineError> {
        Ok(toml::from_str(toml)?)
    }

    /// Load a pipeline definition from a TOML file on disk.
    ///
    /// Applies template variables after loading.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_graph::application::pipeline_parser::PipelineParser;
    ///
    /// let def = PipelineParser::from_file("pipeline.toml").unwrap();
    /// assert!(!def.nodes.is_empty());
    /// ```
    pub fn from_file(path: &str) -> Result<PipelineDefinition, PipelineError> {
        let content = std::fs::read_to_string(path)?;
        let mut def: PipelineDefinition = toml::from_str(&content)?;
        def.expand_templates();
        Ok(def)
    }

    /// Load a pipeline definition using Figment layered configuration.
    ///
    /// Layers (later overrides earlier):
    /// 1. TOML file at `path`
    /// 2. Environment variables with prefix `MYCELIUM_` (e.g. `MYCELIUM_NODES_0_URL`)
    ///
    /// Applies template variable expansion after loading.
    ///
    /// # Errors
    ///
    /// Returns `Err(PipelineError::FigmentError)` if figment cannot extract
    /// the config, or `Err(PipelineError::Io)` if the file cannot be read.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_graph::application::pipeline_parser::PipelineParser;
    ///
    /// let def = PipelineParser::from_figment_file("pipeline.toml").unwrap();
    /// assert!(!def.nodes.is_empty());
    /// ```
    pub fn from_figment_file(path: &str) -> Result<PipelineDefinition, PipelineError> {
        let mut def: PipelineDefinition = Figment::new()
            .merge(Toml::file(path))
            .merge(Env::prefixed("MYCELIUM_").lowercase(true))
            .extract()
            .map_err(|e| PipelineError::FigmentError(e.to_string()))?;
        def.expand_templates();
        Ok(def)
    }
}

/// Hot-reload watcher for pipeline definition files.
///
/// Polls the file's modification time at the configured interval and
/// invokes the callback whenever the file changes on disk.
///
/// # Example
///
/// ```no_run
/// use mycelium_graph::application::pipeline_parser::PipelineWatcher;
/// use std::time::Duration;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let handle = PipelineWatcher::new("pipeline.toml")
///     .with_interval(Duration::from_secs(2))
///     .watch(|def| {
///         println!("Pipeline reloaded: {} nodes", def.nodes.len());
///     });
/// // Abort the watcher when no longer needed
/// handle.abort();
/// # });
/// ```
pub struct PipelineWatcher {
    path: String,
    interval: Duration,
}

impl PipelineWatcher {
    /// Create a new watcher for the given pipeline TOML file.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            interval: Duration::from_secs(5),
        }
    }

    /// Override the polling interval (default: 5 seconds).
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Spawn a background Tokio task that calls `callback` whenever the file changes.
    ///
    /// Returns a `JoinHandle` that callers can `abort()` to stop watching.
    pub fn watch<F>(self, callback: F) -> tokio::task::JoinHandle<()>
    where
        F: Fn(PipelineDefinition) + Send + 'static,
    {
        tokio::spawn(async move {
            let mut last_mtime: Option<std::time::SystemTime> = None;
            let mut ticker = tokio::time::interval(self.interval);

            loop {
                ticker.tick().await;

                let mtime = tokio::fs::metadata(&self.path)
                    .await
                    .ok()
                    .and_then(|m| m.modified().ok());

                if mtime != last_mtime {
                    last_mtime = mtime;
                    if let Ok(mut def) = PipelineParser::from_file(&self.path) {
                        def.expand_templates();
                        callback(def);
                    }
                }
            }
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::literal_string_with_formatting_args
)]
mod tests {
    use super::*;

    const SIMPLE: &str = r#"
[[services]]
name = "http"
kind = "http"

[[services]]
name = "claude"
kind = "claude"

[[nodes]]
name = "fetch"
service = "http"
url = "https://example.com"

[[nodes]]
name = "extract"
service = "claude"
depends_on = ["fetch"]
"#;

    #[test]
    fn parse_valid_pipeline() {
        let def = PipelineParser::from_str(SIMPLE).unwrap();
        assert_eq!(def.services.len(), 2);
        assert_eq!(def.nodes.len(), 2);
        assert_eq!(def.nodes[0].name, "fetch");
        assert_eq!(def.nodes[1].depends_on, vec!["fetch"]);
    }

    #[test]
    fn validate_valid_pipeline() {
        let def = PipelineParser::from_str(SIMPLE).unwrap();
        assert!(def.validate().is_ok());
    }

    #[test]
    fn validate_unknown_service() {
        let toml = r#"
[[services]]
name = "http"
kind = "http"

[[nodes]]
name = "n"
service = "nonexistent"
"#;
        let def = PipelineParser::from_str(toml).unwrap();
        let err = def.validate().unwrap_err();
        assert!(matches!(err, PipelineError::UnknownService { .. }));
    }

    #[test]
    fn validate_unknown_dependency() {
        let toml = r#"
[[nodes]]
name = "n"
service = "http"
depends_on = ["ghost"]
"#;
        let def = PipelineParser::from_str(toml).unwrap();
        let err = def.validate().unwrap_err();
        assert!(matches!(err, PipelineError::UnknownDependency { .. }));
    }

    #[test]
    fn validate_cycle_detected() {
        let toml = r#"
[[nodes]]
name = "a"
service = "http"
depends_on = ["b"]

[[nodes]]
name = "b"
service = "http"
depends_on = ["a"]
"#;
        let def = PipelineParser::from_str(toml).unwrap();
        let err = def.validate().unwrap_err();
        assert!(matches!(err, PipelineError::CycleDetected(_)));
    }

    #[test]
    fn validate_against_registry() {
        let toml = r#"
[[nodes]]
name = "n"
service = "http"
"#;
        let def = PipelineParser::from_str(toml).unwrap();
        assert!(def.validate_against_registry(&["http".to_string()]).is_ok());
        assert!(
            def.validate_against_registry(&["other".to_string()])
                .is_err()
        );
    }

    #[test]
    fn topological_order() {
        let toml = r#"
[[nodes]]
name = "c"
service = "http"
depends_on = ["b"]

[[nodes]]
name = "a"
service = "http"

[[nodes]]
name = "b"
service = "http"
depends_on = ["a"]
"#;
        let def = PipelineParser::from_str(toml).unwrap();
        let order = def.topological_order().unwrap();
        let pos_a = order.iter().position(|x| x == "a").unwrap();
        let pos_b = order.iter().position(|x| x == "b").unwrap();
        let pos_c = order.iter().position(|x| x == "c").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn to_dot_output() {
        let def = PipelineParser::from_str(SIMPLE).unwrap();
        let dot = def.to_dot();
        assert!(dot.contains("digraph pipeline"));
        assert!(dot.contains("fetch"));
        assert!(dot.contains("extract"));
        assert!(dot.contains(r#""fetch" -> "extract""#));
    }

    #[test]
    fn to_mermaid_output() {
        let def = PipelineParser::from_str(SIMPLE).unwrap();
        let mermaid = def.to_mermaid();
        assert!(mermaid.contains("flowchart LR"));
        assert!(mermaid.contains("fetch --> extract"));
    }

    #[test]
    fn template_env_expansion() {
        // Use HOME which is always set on Unix — avoids unsafe set_var
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let toml = r#"
[[nodes]]
name = "n"
service = "http"
url = "${env:HOME}"
"#;
        let mut def = PipelineParser::from_str(toml).unwrap();
        def.expand_templates();
        assert_eq!(def.nodes[0].url.as_deref(), Some(home.as_str()));
    }

    #[test]
    fn template_missing_env_left_as_is() {
        let toml = r#"
[[nodes]]
name = "n"
service = "http"
url = "${env:MYCELIUM_DEFINITELY_UNSET_VAR}"
"#;
        let mut def = PipelineParser::from_str(toml).unwrap();
        def.expand_templates();
        // Missing env var: keep original token
        assert_eq!(
            def.nodes[0].url.as_deref(),
            Some("${env:MYCELIUM_DEFINITELY_UNSET_VAR}")
        );
    }

    #[test]
    fn empty_pipeline_valid() {
        let def = PipelineParser::from_str("").unwrap();
        assert!(def.validate().is_ok());
        assert!(def.topological_order().unwrap().is_empty());
    }

    #[test]
    fn dot_empty_pipeline() {
        let def = PipelineParser::from_str("").unwrap();
        let dot = def.to_dot();
        assert!(dot.starts_with("digraph pipeline"));
    }

    // ── T20 new tests ────────────────────────────────────────────────────────

    #[test]
    fn missing_service_field_fails_validation() {
        // A node with an empty (default) service string should fail at validate()
        let toml = r#"
[[nodes]]
name = "orphan"
"#;
        let def = PipelineParser::from_str(toml).unwrap();
        let err = def.validate().unwrap_err();
        assert!(
            matches!(err, PipelineError::MissingField { ref field, .. } if field == "service"),
            "expected MissingField(service), got {err}"
        );
    }

    #[test]
    fn nonexistent_ai_provider_returns_clear_error() {
        // validate_against_registry reports an UnknownService error (the "AI
        // provider" is just another service kind from the registry's perspective)
        let toml = r#"
[[nodes]]
name = "extract"
service = "claude"
"#;
        let def = PipelineParser::from_str(toml).unwrap();
        let registered = vec!["http".to_string()]; // "claude" not registered
        let err = def.validate_against_registry(&registered).unwrap_err();
        assert!(
            matches!(err, PipelineError::UnknownService { ref service, .. } if service == "claude"),
            "expected UnknownService(claude), got {err}"
        );
    }

    #[test]
    fn from_figment_file_loads_toml() {
        use std::io::Write as _;

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            tmp,
            r#"
[[services]]
name = "http"
kind = "http"

[[nodes]]
name = "fetch"
service = "http"
url = "https://example.com"
"#
        )
        .unwrap();

        let def = PipelineParser::from_figment_file(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(def.nodes.len(), 1);
        assert_eq!(def.nodes[0].name, "fetch");
        assert!(def.validate().is_ok());
    }
}
