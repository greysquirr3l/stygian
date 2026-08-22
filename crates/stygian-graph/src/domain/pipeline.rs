//! Pipeline types with typestate pattern
//!
//! The typestate pattern ensures pipelines can only transition through valid states:
//! Unvalidated → Validated → Executing → Complete
//!
//! # Example
//!
//! ```
//! use stygian_graph::domain::pipeline::PipelineUnvalidated;
//! use serde_json::json;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let unvalidated = PipelineUnvalidated::new(json!({"nodes": []}));
//! let validated = unvalidated.validate()?;
//! let executing = validated.execute();
//! let complete = executing.complete(json!({"status": "success"}));
//! # Ok(())
//! # }
//! ```

use serde::{Deserialize, Serialize};

use super::error::{GraphError, StygianError};
use super::policy::RobotsPolicy;
use crate::ports::robots_policy::{
    PolicyOutcome, RobotsPolicyGuard, apply_policy, validate_guard_pair,
};

/// Pipeline in unvalidated state
///
/// Initial state after loading configuration from a file or API.
/// Must be validated before execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineUnvalidated {
    /// Pipeline configuration (unvalidated)
    pub config: serde_json::Value,
}

/// Pipeline in validated state
///
/// Configuration has been validated and is ready for execution.
#[derive(Debug, Clone)]
pub struct PipelineValidated {
    /// Validated configuration
    pub config: serde_json::Value,
}

/// Pipeline in executing state
///
/// Pipeline is actively being executed. Contains runtime context.
#[derive(Debug)]
pub struct PipelineExecuting {
    /// Execution context and state
    pub context: serde_json::Value,
}

/// Pipeline in completed state
///
/// Pipeline execution has finished. Contains final results.
#[derive(Debug)]
pub struct PipelineComplete {
    /// Execution results
    pub results: serde_json::Value,
}

impl PipelineUnvalidated {
    /// Create a new unvalidated pipeline from raw configuration
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::pipeline::PipelineUnvalidated;
    /// use serde_json::json;
    ///
    /// let pipeline = PipelineUnvalidated::new(json!({
    ///     "nodes": [{"id": "fetch", "service": "http"}],
    ///     "edges": []
    /// }));
    /// ```
    #[must_use]
    pub const fn new(config: serde_json::Value) -> Self {
        Self { config }
    }

    /// Validate the pipeline configuration
    ///
    /// Transitions from `Unvalidated` to `Validated` state.
    ///
    /// # Panics
    ///
    /// Panics if the validated DAG contains an edge whose source node is missing
    /// from the adjacency map. This is guarded by the cycle check above and is
    /// unreachable in well-formed input.
    ///
    /// # Errors
    ///
    /// Returns `GraphError::InvalidPipeline` if validation fails.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::pipeline::PipelineUnvalidated;
    /// use serde_json::json;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pipeline = PipelineUnvalidated::new(json!({"nodes": []}));
    /// let validated = pipeline.validate()?;
    /// # Ok(())
    /// # }
    /// ```
    #[allow(clippy::too_many_lines, clippy::unwrap_used, clippy::indexing_slicing)]
    pub fn validate(self) -> Result<PipelineValidated, StygianError> {
        use std::collections::{HashMap, HashSet, VecDeque};

        // Extract nodes and edges from config
        let nodes = self
            .config
            .get("nodes")
            .and_then(|n| n.as_array())
            .ok_or_else(|| {
                GraphError::InvalidPipeline("Pipeline must contain a 'nodes' array".to_string())
            })?;

        let empty_edges = vec![];
        let edges = self
            .config
            .get("edges")
            .and_then(|e| e.as_array())
            .unwrap_or(&empty_edges);

        // Rule 1: At least one node
        if nodes.is_empty() {
            return Err(GraphError::InvalidPipeline(
                "Pipeline must contain at least one node".to_string(),
            )
            .into());
        }

        // Build node map and validate individual nodes
        let mut node_map: HashMap<String, usize> = HashMap::new();
        let valid_services = [
            "http",
            "http_escalating",
            "browser",
            "ai_claude",
            "ai_openai",
            "ai_gemini",
            "ai_github",
            "ai_ollama",
            "javascript",
            "graphql",
            "storage",
        ];

        for (idx, node) in nodes.iter().enumerate() {
            let node_obj = node.as_object().ok_or_else(|| {
                GraphError::InvalidPipeline(format!("Node at index {idx}: must be an object"))
            })?;

            // Rule 2 & 3: Validate node ID
            let node_id = node_obj.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
                GraphError::InvalidPipeline(format!(
                    "Node at index {idx}: 'id' field is required and must be a string"
                ))
            })?;

            if node_id.is_empty() {
                return Err(GraphError::InvalidPipeline(format!(
                    "Node at index {idx}: id cannot be empty"
                ))
                .into());
            }

            // Check for duplicate node IDs
            if node_map.insert(node_id.to_string(), idx).is_some() {
                return Err(
                    GraphError::InvalidPipeline(format!("Duplicate node id: '{node_id}'")).into(),
                );
            }

            // Rule 4: Validate service type
            let service = node_obj
                .get("service")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    GraphError::InvalidPipeline(format!(
                        "Node '{node_id}': 'service' field is required and must be a string"
                    ))
                })?;

            if !valid_services.contains(&service) {
                return Err(GraphError::InvalidPipeline(format!(
                    "Node '{node_id}': service type '{service}' is not recognized"
                ))
                .into());
            }
        }

        // Rule 5 & 6: Validate edges
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        let mut in_degree: HashMap<String, usize> = HashMap::new();

        // Initialize in_degree for all nodes
        for node in nodes {
            if let Some(id) = node.get("id").and_then(|v| v.as_str()) {
                in_degree.insert(id.to_string(), 0);
                adjacency.insert(id.to_string(), Vec::new());
            }
        }

        for (edge_idx, edge) in edges.iter().enumerate() {
            let edge_obj = edge.as_object().ok_or_else(|| {
                GraphError::InvalidPipeline(format!("Edge at index {edge_idx}: must be an object"))
            })?;

            let from = edge_obj
                .get("from")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    GraphError::InvalidPipeline(format!(
                        "Edge at index {edge_idx}: 'from' field is required and must be a string"
                    ))
                })?;

            let to = edge_obj.get("to").and_then(|v| v.as_str()).ok_or_else(|| {
                GraphError::InvalidPipeline(format!(
                    "Edge at index {edge_idx}: 'to' field is required and must be a string"
                ))
            })?;

            // Source node must exist
            if !node_map.contains_key(from) {
                return Err(GraphError::InvalidPipeline(format!(
                    "Edge {from} -> {to}: source node '{from}' not found"
                ))
                .into());
            }

            // Target node must exist
            if !node_map.contains_key(to) {
                return Err(GraphError::InvalidPipeline(format!(
                    "Edge {from} -> {to}: target node '{to}' not found"
                ))
                .into());
            }

            // Source and target cannot be the same
            if from == to {
                return Err(GraphError::InvalidPipeline(format!(
                    "Self-loop detected at node '{from}'"
                ))
                .into());
            }

            // Build adjacency list and track in-degrees
            adjacency.get_mut(from).unwrap().push(to.to_string());
            *in_degree.get_mut(to).unwrap() += 1;
        }

        // Rule 7: Detect cycles using Kahn's algorithm (topological sort)
        let mut in_degree_copy = in_degree.clone();
        let mut queue: VecDeque<String> = VecDeque::new();

        // Add all nodes with no incoming edges (entry points)
        let entry_points: Vec<String> = in_degree_copy
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(node_id, _)| node_id.clone())
            .collect();
        for node_id in entry_points {
            queue.push_back(node_id);
        }

        let mut sorted_count = 0;
        while let Some(node_id) = queue.pop_front() {
            sorted_count += 1;

            // For each neighbor of this node
            if let Some(neighbors) = adjacency.get(&node_id) {
                let neighbors_copy = neighbors.clone();
                for neighbor in neighbors_copy {
                    *in_degree_copy.get_mut(&neighbor).unwrap() -= 1;
                    if in_degree_copy[&neighbor] == 0 {
                        queue.push_back(neighbor);
                    }
                }
            }
        }

        // If we didn't sort all nodes, there's a cycle
        if sorted_count != node_map.len() {
            return Err(GraphError::InvalidPipeline(
                "Cycle detected in pipeline graph".to_string(),
            )
            .into());
        }

        // Rule 8: Check for unreachable nodes (isolated components)
        // All nodes must form a single connected DAG with one or more entry points
        // Only start reachability from the FIRST entry point to ensure all nodes are connected
        let mut visited: HashSet<String> = HashSet::new();
        let mut to_visit: VecDeque<String> = VecDeque::new();

        // Find first entry point (node with in_degree == 0)
        let mut entry_points = Vec::new();
        for (node_id, degree) in &in_degree {
            if *degree == 0 {
                entry_points.push(node_id.clone());
            }
        }

        if entry_points.is_empty() {
            // Should not happen if cycle check passed, but be safe
            return Err(GraphError::InvalidPipeline(
                "No entry points found (all nodes have incoming edges)".to_string(),
            )
            .into());
        }

        // Start BFS from ONLY the first entry point to ensure single connected component
        to_visit.push_back(entry_points[0].clone());

        // BFS from first entry point
        while let Some(node_id) = to_visit.pop_front() {
            if visited.insert(node_id.clone()) {
                // Explore outgoing edges
                if let Some(neighbors) = adjacency.get(&node_id) {
                    for neighbor in neighbors {
                        to_visit.push_back(neighbor.clone());
                    }
                }

                // Also explore reverse adjacency (incoming edges) to handle branching
                for (source, targets) in &adjacency {
                    if targets.contains(&node_id) && !visited.contains(source) {
                        to_visit.push_back(source.clone());
                    }
                }
            }
        }

        // Check for unreachable nodes
        let all_node_ids: HashSet<String> = node_map.keys().cloned().collect();
        let unreachable: Vec<_> = all_node_ids.difference(&visited).collect();

        if !unreachable.is_empty() {
            let unreachable_str = unreachable
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("', '");
            return Err(GraphError::InvalidPipeline(format!(
                "Unreachable nodes found: '{unreachable_str}' (ensure all nodes are connected in a single DAG)"
            ))
            .into());
        }

        Ok(PipelineValidated {
            config: self.config,
        })
    }

    /// Compute the effective [`RobotsPolicy`] for this pipeline.
    ///
    /// Looks for `config["robots_policy"]` and
    /// `config["pipeline"]["robots_policy"]`; falls back to
    /// [`RobotsPolicy::Obey`] when neither is set.
    ///
    /// # Errors
    ///
    /// Returns [`StygianError::Config`] when a value is present but
    /// cannot be parsed as a `RobotsPolicy`.
    pub fn effective_robots_policy(&self) -> Result<RobotsPolicy, StygianError> {
        if let Some(raw) = self.config.get("robots_policy").and_then(|v| v.as_str()) {
            return raw.parse::<RobotsPolicy>();
        }
        if let Some(raw) = self
            .config
            .get("pipeline")
            .and_then(|p| p.get("robots_policy"))
            .and_then(|v| v.as_str())
        {
            return raw.parse::<RobotsPolicy>();
        }
        Ok(RobotsPolicy::Obey)
    }

    /// Apply the pipeline's [`RobotsPolicy`] to every URL in the
    /// pipeline against the supplied [`RobotsPolicyGuard`].
    ///
    /// This is the **content** check that complements the structural
    /// `validate()` above — it refuses to build a pipeline whose URLs
    /// would be forbidden at run time. Both recon and production share
    /// this method, so the policy cannot drift between spec-build and
    /// execution.
    ///
    /// Behaviour:
    ///
    /// - [`RobotsPolicy::Obey`] — every URL that the guard marks
    ///   `Forbid` or `Unknown` is refused; the pipeline fails with
    ///   [`GraphError::InvalidPipeline`] before any production
    ///   traffic.
    /// - [`RobotsPolicy::IgnoreWithAudit`] — every URL is permitted;
    ///   the [`PolicyOutcome::FetchWithAudit`] entries are surfaced
    ///   through the returned `Vec<RobotsAuditEvent>` so the operator
    ///   can see which URLs were ignored.
    /// - [`RobotsPolicy::IgnoreSilently`] — every URL is permitted
    ///   unconditionally; the returned audit vec is always empty.
    ///
    /// # Errors
    ///
    /// - [`GraphError::InvalidPipeline`] if `RobotsPolicy::Obey` is in
    ///   effect and any URL is refused.
    /// - The wrapped guard error if the guard itself fails to decide.
    pub async fn check_robots_policy(
        &self,
        guard: &dyn RobotsPolicyGuard,
    ) -> Result<Vec<RobotsAuditEvent>, StygianError> {
        let policy = self.effective_robots_policy()?;
        validate_guard_pair(policy, guard)?;

        let urls = collect_node_urls(&self.config);
        let mut audit = Vec::with_capacity(urls.len());

        for (node_id, url) in &urls {
            let decision = guard.decide(url).await?;
            let outcome = apply_policy(policy, decision);
            match outcome {
                PolicyOutcome::Fetch => {}
                PolicyOutcome::FetchWithAudit { reason } => {
                    audit.push(RobotsAuditEvent {
                        node_id: node_id.clone(),
                        url: url.clone(),
                        reason,
                    });
                }
                PolicyOutcome::Refuse { reason } => {
                    return Err(GraphError::InvalidPipeline(format!(
                        "robots policy '{policy}' refuses node '{node_id}' \
                         (url: {url}): {reason}"
                    ))
                    .into());
                }
            }
        }

        Ok(audit)
    }
}

impl PipelineValidated {
    /// Begin executing the validated pipeline
    ///
    /// Transitions from `Validated` to `Executing` state.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::pipeline::PipelineUnvalidated;
    /// use serde_json::json;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pipeline = PipelineUnvalidated::new(json!({"nodes": []}))
    ///     .validate()?;
    /// let executing = pipeline.execute();
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn execute(self) -> PipelineExecuting {
        PipelineExecuting {
            context: self.config,
        }
    }
}

impl PipelineExecuting {
    /// Mark the pipeline as complete with results
    ///
    /// Transitions from `Executing` to `Complete` state.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::pipeline::PipelineUnvalidated;
    /// use serde_json::json;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pipeline = PipelineUnvalidated::new(json!({"nodes": []}))
    ///     .validate()?
    ///     .execute();
    ///
    /// let complete = pipeline.complete(json!({"status": "success"}));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn complete(self, results: serde_json::Value) -> PipelineComplete {
        PipelineComplete { results }
    }

    /// Abort execution with an error
    ///
    /// Transitions from `Executing` to `Complete` state with error details.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::pipeline::PipelineUnvalidated;
    /// use serde_json::json;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pipeline = PipelineUnvalidated::new(json!({"nodes": []}))
    ///     .validate()?
    ///     .execute();
    ///
    /// let complete = pipeline.abort("Network timeout");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn abort(self, error: &str) -> PipelineComplete {
        PipelineComplete {
            results: serde_json::json!({
                "status": "error",
                "error": error
            }),
        }
    }
}

impl PipelineComplete {
    /// Check if the pipeline completed successfully
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::pipeline::PipelineUnvalidated;
    /// use serde_json::json;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pipeline = PipelineUnvalidated::new(json!({"nodes": []}))
    ///     .validate()?
    ///     .execute()
    ///     .complete(json!({"status": "success"}));
    ///
    /// assert!(pipeline.is_success());
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.results
            .get("status")
            .and_then(|s| s.as_str())
            .is_some_and(|s| s == "success")
    }

    /// Get the execution results
    #[must_use]
    pub const fn results(&self) -> &serde_json::Value {
        &self.results
    }
}

/// One URL the pipeline was allowed to fetch despite a guard
/// `Forbid` verdict, surfaced under [`RobotsPolicy::IgnoreWithAudit`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobotsAuditEvent {
    /// Node id this URL was attached to.
    pub node_id: String,
    /// The URL that was fetched.
    pub url: String,
    /// Reason recorded by the guard.
    pub reason: String,
}

/// Collect every `(node_id, url)` pair the pipeline declares.
///
/// Walks the `nodes` array; for each node, looks at the `url` field
/// or `params.url` (the canonical shapes used by the example TOML
/// configs and the MCP server). Nodes without a URL are skipped —
/// non-fetching nodes (e.g. `ai_claude`, `storage`) shouldn't trigger
/// a robots check.
fn collect_node_urls(config: &serde_json::Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(nodes) = config.get("nodes").and_then(|n| n.as_array()) else {
        return out;
    };
    for node in nodes {
        let Some(node_obj) = node.as_object() else {
            continue;
        };
        let Some(id) = node_obj.get("id").and_then(|v| v.as_str()) else {
            continue;
        };

        // Shape 1: top-level `url` on the node.
        if let Some(url) = node_obj.get("url").and_then(|v| v.as_str()) {
            out.push((id.to_string(), url.to_string()));
            continue;
        }

        // Shape 2: `params.url` (the example TOML convention).
        if let Some(url) = node_obj
            .get("params")
            .and_then(|p| p.get("url"))
            .and_then(|v| v.as_str())
        {
            out.push((id.to_string(), url.to_string()));
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_empty_nodes_array() {
        let pipe = PipelineUnvalidated::new(json!({"nodes": [], "edges": []}));
        let result = pipe.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("at least one node")
        );
    }

    #[test]
    fn validate_missing_nodes_field() {
        let pipe = PipelineUnvalidated::new(json!({"edges": []}));
        let result = pipe.validate();
        assert!(result.is_err());
    }

    #[test]
    fn validate_missing_node_id() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [{"service": "http"}],
            "edges": []
        }));
        let result = pipe.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("'id' field is required")
        );
    }

    #[test]
    fn validate_empty_node_id() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [{"id": "", "service": "http"}],
            "edges": []
        }));
        let result = pipe.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("id cannot be empty")
        );
    }

    #[test]
    fn validate_duplicate_node_ids() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [
                {"id": "fetch", "service": "http"},
                {"id": "fetch", "service": "browser"}
            ],
            "edges": []
        }));
        let result = pipe.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Duplicate node id")
        );
    }

    #[test]
    fn validate_invalid_service_type() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [{"id": "fetch", "service": "invalid_service"}],
            "edges": []
        }));
        let result = pipe.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not recognized"));
    }

    #[test]
    fn validate_edge_nonexistent_source() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [{"id": "extract", "service": "ai_claude"}],
            "edges": [{"from": "fetch", "to": "extract"}]
        }));
        let result = pipe.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("source node 'fetch' not found")
        );
    }

    #[test]
    fn validate_edge_nonexistent_target() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [{"id": "fetch", "service": "http"}],
            "edges": [{"from": "fetch", "to": "extract"}]
        }));
        let result = pipe.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("target node 'extract' not found")
        );
    }

    #[test]
    fn validate_self_loop() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [{"id": "node1", "service": "http"}],
            "edges": [{"from": "node1", "to": "node1"}]
        }));
        let result = pipe.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Self-loop"));
    }

    #[test]
    fn validate_cycle_detection() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [
                {"id": "a", "service": "http"},
                {"id": "b", "service": "ai_claude"},
                {"id": "c", "service": "browser"}
            ],
            "edges": [
                {"from": "a", "to": "b"},
                {"from": "b", "to": "c"},
                {"from": "c", "to": "a"}
            ]
        }));
        let result = pipe.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cycle"));
    }

    #[test]
    fn validate_unreachable_nodes() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [
                {"id": "a", "service": "http"},
                {"id": "orphan", "service": "browser"}
            ],
            "edges": []
        }));
        let result = pipe.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unreachable"));
    }

    #[test]
    fn validate_valid_single_node() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [{"id": "fetch", "service": "http"}],
            "edges": []
        }));
        assert!(pipe.validate().is_ok());
    }

    #[test]
    fn validate_valid_linear_pipeline() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [
                {"id": "fetch", "service": "http"},
                {"id": "extract", "service": "ai_claude"},
                {"id": "store", "service": "storage"}
            ],
            "edges": [
                {"from": "fetch", "to": "extract"},
                {"from": "extract", "to": "store"}
            ]
        }));
        assert!(pipe.validate().is_ok());
    }

    #[test]
    fn validate_valid_dag_branching() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [
                {"id": "fetch", "service": "http"},
                {"id": "extract_ai", "service": "ai_claude"},
                {"id": "extract_browser", "service": "browser"},
                {"id": "merge", "service": "storage"}
            ],
            "edges": [
                {"from": "fetch", "to": "extract_ai"},
                {"from": "fetch", "to": "extract_browser"},
                {"from": "extract_ai", "to": "merge"},
                {"from": "extract_browser", "to": "merge"}
            ]
        }));
        assert!(pipe.validate().is_ok());
    }

    // ── T111 robots-policy integration tests ──────────────────────────

    use crate::domain::policy::RobotsDecision;

    /// Programmable guard for the tests — decides URLs from a
    /// pre-populated allow/forbid table.
    struct ScriptedGuard {
        name: &'static str,
        /// URL → decision (None means Unknown).
        table: std::collections::HashMap<String, crate::domain::policy::RobotsDecision>,
    }

    #[async_trait::async_trait]
    impl crate::ports::robots_policy::RobotsPolicyGuard for ScriptedGuard {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn decide(&self, url: &str) -> Result<RobotsDecision, StygianError> {
            Ok(self
                .table
                .get(url)
                .cloned()
                .unwrap_or(RobotsDecision::Unknown))
        }
    }

    fn guard_allowing_all() -> std::sync::Arc<ScriptedGuard> {
        std::sync::Arc::new(ScriptedGuard {
            name: "test-allow",
            table: std::collections::HashMap::new(),
        })
    }

    fn guard_forbidding(url: &str) -> std::sync::Arc<ScriptedGuard> {
        let mut table = std::collections::HashMap::new();
        table.insert(
            url.to_string(),
            RobotsDecision::Forbid {
                reason: "Disallow: /private".to_string(),
            },
        );
        std::sync::Arc::new(ScriptedGuard {
            name: "test-forbid",
            table,
        })
    }

    #[test]
    fn effective_robots_policy_defaults_to_obey() {
        let pipe = PipelineUnvalidated::new(json!({
            "nodes": [{"id": "fetch", "service": "http", "url": "https://example.com"}]
        }));
        assert_eq!(
            pipe.effective_robots_policy().unwrap(),
            crate::domain::policy::RobotsPolicy::Obey
        );
    }

    #[test]
    fn effective_robots_policy_reads_top_level_key() {
        let pipe = PipelineUnvalidated::new(json!({
            "robots_policy": "ignore_with_audit",
            "nodes": [{"id": "fetch", "service": "http", "url": "https://example.com"}]
        }));
        assert_eq!(
            pipe.effective_robots_policy().unwrap(),
            crate::domain::policy::RobotsPolicy::IgnoreWithAudit
        );
    }

    #[test]
    fn effective_robots_policy_reads_nested_pipeline_key() {
        let pipe = PipelineUnvalidated::new(json!({
            "pipeline": {"robots_policy": "ignore_silently"},
            "nodes": [{"id": "fetch", "service": "http", "url": "https://example.com"}]
        }));
        assert_eq!(
            pipe.effective_robots_policy().unwrap(),
            crate::domain::policy::RobotsPolicy::IgnoreSilently
        );
    }

    #[test]
    fn effective_robots_policy_rejects_unknown_variant() {
        let pipe = PipelineUnvalidated::new(json!({
            "robots_policy": "always_obey",
            "nodes": [{"id": "fetch", "service": "http", "url": "https://example.com"}]
        }));
        let err = pipe.effective_robots_policy().unwrap_err();
        assert!(format!("{err}").contains("always_obey"));
    }

    #[test]
    fn collect_node_urls_picks_both_shapes() {
        let cfg = json!({
            "nodes": [
                {"id": "a", "service": "http", "url": "https://a.example"},
                {"id": "b", "service": "browser", "params": {"url": "https://b.example"}},
                {"id": "c", "service": "ai_claude"},          // no URL — skip
                {"id": "d", "service": "http", "url": 42}     // wrong type — skip
            ]
        });
        let urls = collect_node_urls(&cfg);
        let pairs: std::collections::HashSet<(String, String)> = urls.into_iter().collect();
        assert!(pairs.contains(&("a".to_string(), "https://a.example".to_string())));
        assert!(pairs.contains(&("b".to_string(), "https://b.example".to_string())));
        assert_eq!(pairs.len(), 2);
    }

    #[tokio::test]
    async fn obey_with_forbidden_url_refuses_to_validate() {
        let pipe = PipelineUnvalidated::new(json!({
            "robots_policy": "obey",
            "nodes": [{"id": "fetch", "service": "http", "url": "https://private.example/x"}]
        }));
        let guard = guard_forbidding("https://private.example/x");
        let err = pipe.check_robots_policy(&*guard).await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("refuses node 'fetch'"), "{msg}");
        assert!(msg.contains("Disallow: /private"), "{msg}");
    }

    #[tokio::test]
    async fn obey_with_unknown_decision_refuses() {
        // Guard returns `Unknown` (empty table) and the policy is
        // `Obey` — refuse. This is the conservative behaviour: an
        // unknown answer must not let a forbidden URL slip through
        // just because the guard has no data.
        let pipe = PipelineUnvalidated::new(json!({
            "robots_policy": "obey",
            "nodes": [{"id": "fetch", "service": "http", "url": "https://public.example/"}]
        }));
        let guard = guard_allowing_all();
        let err = pipe.check_robots_policy(&*guard).await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Unknown under Obey"), "{msg}");
    }

    #[tokio::test]
    async fn obey_with_explicit_allow_passes() {
        // Guard explicitly returns `Allow` for the URL — Obey
        // permits. The audit vec is empty (no policy violation to
        // record).
        let url = "https://public.example/";
        let mut table = std::collections::HashMap::new();
        table.insert(
            url.to_string(),
            RobotsDecision::Allow {
                reason: "no rule matched".to_string(),
            },
        );
        let guard = std::sync::Arc::new(ScriptedGuard {
            name: "test-allow-explicit",
            table,
        });
        let pipe = PipelineUnvalidated::new(json!({
            "robots_policy": "obey",
            "nodes": [{"id": "fetch", "service": "http", "url": url}]
        }));
        let audit = pipe.check_robots_policy(&*guard).await.unwrap();
        assert!(audit.is_empty());
    }

    #[tokio::test]
    async fn ignore_with_audit_emits_audit_event_for_forbidden_url() {
        let url = "https://private.example/x";
        let pipe = PipelineUnvalidated::new(json!({
            "robots_policy": "ignore_with_audit",
            "nodes": [{"id": "fetch", "service": "http", "url": url}]
        }));
        let guard = guard_forbidding(url);
        let audit = pipe.check_robots_policy(&*guard).await.unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].node_id, "fetch");
        assert_eq!(audit[0].url, url);
        assert!(audit[0].reason.contains("Disallow"));
    }

    #[tokio::test]
    async fn ignore_silently_passes_forbidden_url_without_audit() {
        let url = "https://private.example/x";
        let pipe = PipelineUnvalidated::new(json!({
            "robots_policy": "ignore_silently",
            "nodes": [{"id": "fetch", "service": "http", "url": url}]
        }));
        let guard = guard_forbidding(url);
        let audit = pipe.check_robots_policy(&*guard).await.unwrap();
        assert!(audit.is_empty());
    }

    #[tokio::test]
    async fn obey_with_permissive_guard_is_rejected_at_check_time() {
        // Default-permissive guard + Obey policy = contradiction
        // (pipeline claims to obey but guard has no data). Must fail
        // loudly.
        let pipe = PipelineUnvalidated::new(json!({
            "robots_policy": "obey",
            "nodes": [{"id": "fetch", "service": "http", "url": "https://example.com"}]
        }));
        let guard = crate::ports::robots_policy::permissive_guard();
        let err = pipe.check_robots_policy(&*guard).await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Obey"), "{msg}");
        assert!(msg.contains("permissive"), "{msg}");
    }
}
