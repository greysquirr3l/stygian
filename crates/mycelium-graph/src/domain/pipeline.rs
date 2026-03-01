//! Pipeline types with typestate pattern
//!
//! The typestate pattern ensures pipelines can only transition through valid states:
//! Unvalidated → Validated → Executing → Complete
//!
//! # Example
//!
//! ```
//! use mycelium_graph::domain::pipeline::PipelineUnvalidated;
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

use super::error::{GraphError, MyceliumError};

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
    /// use mycelium_graph::domain::pipeline::PipelineUnvalidated;
    /// use serde_json::json;
    ///
    /// let pipeline = PipelineUnvalidated::new(json!({
    ///     "nodes": [{"id": "fetch", "service": "http"}],
    ///     "edges": []
    /// }));
    /// ```
    pub const fn new(config: serde_json::Value) -> Self {
        Self { config }
    }

    /// Validate the pipeline configuration
    ///
    /// Transitions from `Unvalidated` to `Validated` state.
    ///
    /// # Errors
    ///
    /// Returns `GraphError::InvalidPipeline` if validation fails.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::domain::pipeline::PipelineUnvalidated;
    /// use serde_json::json;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pipeline = PipelineUnvalidated::new(json!({"nodes": []}));
    /// let validated = pipeline.validate()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn validate(self) -> Result<PipelineValidated, MyceliumError> {
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
                GraphError::InvalidPipeline(format!("Node at index {}: must be an object", idx))
            })?;

            // Rule 2 & 3: Validate node ID
            let node_id = node_obj.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
                GraphError::InvalidPipeline(format!(
                    "Node at index {}: 'id' field is required and must be a string",
                    idx
                ))
            })?;

            if node_id.is_empty() {
                return Err(GraphError::InvalidPipeline(format!(
                    "Node at index {}: id cannot be empty",
                    idx
                ))
                .into());
            }

            // Check for duplicate node IDs
            if node_map.insert(node_id.to_string(), idx).is_some() {
                return Err(GraphError::InvalidPipeline(format!(
                    "Duplicate node id: '{}'",
                    node_id
                ))
                .into());
            }

            // Rule 4: Validate service type
            let service = node_obj
                .get("service")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    GraphError::InvalidPipeline(format!(
                        "Node '{}': 'service' field is required and must be a string",
                        node_id
                    ))
                })?;

            if !valid_services.contains(&service) {
                return Err(GraphError::InvalidPipeline(format!(
                    "Node '{}': service type '{}' is not recognized",
                    node_id, service
                ))
                .into());
            }
        }

        // Rule 5 & 6: Validate edges
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        let mut in_degree: HashMap<String, usize> = HashMap::new();

        // Initialize in_degree for all nodes
        for node in nodes.iter() {
            if let Some(id) = node.get("id").and_then(|v| v.as_str()) {
                in_degree.insert(id.to_string(), 0);
                adjacency.insert(id.to_string(), Vec::new());
            }
        }

        for (edge_idx, edge) in edges.iter().enumerate() {
            let edge_obj = edge.as_object().ok_or_else(|| {
                GraphError::InvalidPipeline(format!(
                    "Edge at index {}: must be an object",
                    edge_idx
                ))
            })?;

            let from = edge_obj
                .get("from")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    GraphError::InvalidPipeline(format!(
                        "Edge at index {}: 'from' field is required and must be a string",
                        edge_idx
                    ))
                })?;

            let to = edge_obj.get("to").and_then(|v| v.as_str()).ok_or_else(|| {
                GraphError::InvalidPipeline(format!(
                    "Edge at index {}: 'to' field is required and must be a string",
                    edge_idx
                ))
            })?;

            // Source node must exist
            if !node_map.contains_key(from) {
                return Err(GraphError::InvalidPipeline(format!(
                    "Edge {} -> {}: source node '{}' not found",
                    from, to, from
                ))
                .into());
            }

            // Target node must exist
            if !node_map.contains_key(to) {
                return Err(GraphError::InvalidPipeline(format!(
                    "Edge {} -> {}: target node '{}' not found",
                    from, to, to
                ))
                .into());
            }

            // Source and target cannot be the same
            if from == to {
                return Err(GraphError::InvalidPipeline(format!(
                    "Self-loop detected at node '{}'",
                    from
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
                "Unreachable nodes found: '{}' (ensure all nodes are connected in a single DAG)",
                unreachable_str
            ))
            .into());
        }

        Ok(PipelineValidated {
            config: self.config,
        })
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
    /// use mycelium_graph::domain::pipeline::PipelineUnvalidated;
    /// use serde_json::json;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pipeline = PipelineUnvalidated::new(json!({"nodes": []}))
    ///     .validate()?;
    /// let executing = pipeline.execute();
    /// # Ok(())
    /// # }
    /// ```
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
    /// use mycelium_graph::domain::pipeline::PipelineUnvalidated;
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
    /// use mycelium_graph::domain::pipeline::PipelineUnvalidated;
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
    /// use mycelium_graph::domain::pipeline::PipelineUnvalidated;
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
    pub fn is_success(&self) -> bool {
        self.results
            .get("status")
            .and_then(|s| s.as_str())
            .is_some_and(|s| s == "success")
    }

    /// Get the execution results
    pub const fn results(&self) -> &serde_json::Value {
        &self.results
    }
}

#[cfg(test)]
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
}
