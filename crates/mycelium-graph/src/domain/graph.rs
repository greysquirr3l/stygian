//! DAG execution engine
//!
//! Implements graph-based pipeline execution using petgraph.
//! Defines core domain entities: Node, Edge, and Pipeline.
//!
//! # Example
//!
//! ```
//! use mycelium_graph::domain::graph::{Node, Edge, DagExecutor};
//!
//! let node = Node::new("fetch", "http", serde_json::json!({"url": "https://example.com"}));
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::error::{GraphError, MyceliumError};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

/// A node in the scraping pipeline graph
///
/// Represents a single operation (HTTP fetch, AI extraction, transformation, etc.)
///
/// # Example
///
/// ```
/// use mycelium_graph::domain::graph::Node;
/// use serde_json::json;
///
/// let node = Node::new(
///     "fetch_homepage",
///     "http",
///     json!({"url": "https://example.com", "method": "GET"})
/// );
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Unique identifier for this node
    pub id: String,

    /// Service type (e.g., `"http"`, `"ai_extract"`, `"browser"`)
    pub service: String,

    /// Node-specific configuration
    pub config: serde_json::Value,

    /// Optional node metadata
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl Node {
    /// Create a new node
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::domain::graph::Node;
    /// use serde_json::json;
    ///
    /// let node = Node::new("fetch", "http", json!({"url": "https://example.com"}));
    /// assert_eq!(node.id, "fetch");
    /// assert_eq!(node.service, "http");
    /// ```
    pub fn new(
        id: impl Into<String>,
        service: impl Into<String>,
        config: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            service: service.into(),
            config,
            metadata: serde_json::Value::Null,
        }
    }

    /// Create a new node with metadata
    pub fn with_metadata(
        id: impl Into<String>,
        service: impl Into<String>,
        config: serde_json::Value,
        metadata: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            service: service.into(),
            config,
            metadata,
        }
    }

    /// Validate the node configuration
    ///
    /// # Errors
    ///
    /// Returns `GraphError::InvalidEdge` if the node has an empty ID or service type.
    pub fn validate(&self) -> Result<(), MyceliumError> {
        if self.id.is_empty() {
            return Err(GraphError::InvalidEdge("Node ID cannot be empty".into()).into());
        }
        if self.service.is_empty() {
            return Err(GraphError::InvalidEdge("Node service type cannot be empty".into()).into());
        }
        Ok(())
    }
}

/// An edge connecting two nodes in the pipeline graph
///
/// Represents data flow or dependencies between operations.
///
/// # Example
///
/// ```
/// use mycelium_graph::domain::graph::Edge;
/// use serde_json::json;
///
/// let edge = Edge::new("fetch_homepage", "extract_data");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    /// Source node ID
    pub from: String,

    /// Target node ID
    pub to: String,

    /// Optional edge configuration (data transformations, filters, etc.)
    #[serde(default)]
    pub config: serde_json::Value,
}

impl Edge {
    /// Create a new edge between two nodes
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::domain::graph::Edge;
    ///
    /// let edge = Edge::new("fetch", "extract");
    /// assert_eq!(edge.from, "fetch");
    /// assert_eq!(edge.to, "extract");
    /// ```
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            config: serde_json::Value::Null,
        }
    }

    /// Create an edge with additional configuration
    pub fn with_config(
        from: impl Into<String>,
        to: impl Into<String>,
        config: serde_json::Value,
    ) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            config,
        }
    }

    /// Validate the edge
    ///
    /// # Errors
    ///
    /// Returns `GraphError::InvalidEdge` if the edge has empty endpoints.
    pub fn validate(&self) -> Result<(), MyceliumError> {
        if self.from.is_empty() || self.to.is_empty() {
            return Err(GraphError::InvalidEdge("Edge endpoints cannot be empty".into()).into());
        }
        Ok(())
    }
}

/// A complete pipeline definition
///
/// Contains the full graph structure (nodes and edges) plus metadata.
///
/// # Example
///
/// ```
/// use mycelium_graph::domain::graph::{Pipeline, Node, Edge};
/// use serde_json::json;
///
/// let mut pipeline = Pipeline::new("example_pipeline");
/// pipeline.add_node(Node::new("fetch", "http", json!({"url": "https://example.com"})));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipeline {
    /// Pipeline name/identifier
    pub name: String,

    /// Nodes in the pipeline
    pub nodes: Vec<Node>,

    /// Edges connecting nodes
    pub edges: Vec<Edge>,

    /// Pipeline-level configuration and metadata
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl Pipeline {
    /// Create a new empty pipeline
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::domain::graph::Pipeline;
    ///
    /// let pipeline = Pipeline::new("my_scraper");
    /// assert_eq!(pipeline.name, "my_scraper");
    /// assert!(pipeline.nodes.is_empty());
    /// ```
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            nodes: Vec::new(),
            edges: Vec::new(),
            metadata: serde_json::Value::Null,
        }
    }

    /// Add a node to the pipeline
    pub fn add_node(&mut self, node: Node) {
        self.nodes.push(node);
    }

    /// Add an edge to the pipeline
    pub fn add_edge(&mut self, edge: Edge) {
        self.edges.push(edge);
    }

    /// Validate the entire pipeline
    ///
    /// # Errors
    ///
    /// Returns an error if any node or edge is invalid.
    pub fn validate(&self) -> Result<(), MyceliumError> {
        for node in &self.nodes {
            node.validate()?;
        }
        for edge in &self.edges {
            edge.validate()?;
        }
        Ok(())
    }
}

/// Result of executing a single node
#[derive(Debug, Clone)]
pub struct NodeResult {
    /// The node ID that produced this result
    pub node_id: String,
    /// The output from the service execution
    pub output: ServiceOutput,
}

/// DAG executor that processes pipeline graphs
///
/// Executes scraping pipelines as directed acyclic graphs using petgraph.
/// Independent branches are executed concurrently using `tokio::spawn`.
/// Data from upstream nodes is passed as input to downstream nodes.
pub struct DagExecutor {
    graph: DiGraph<Node, ()>,
    _node_indices: HashMap<String, NodeIndex>,
}

impl DagExecutor {
    /// Create a new DAG executor
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::domain::graph::DagExecutor;
    ///
    /// let executor = DagExecutor::new();
    /// ```
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            _node_indices: HashMap::new(),
        }
    }

    /// Build a graph from a pipeline definition
    ///
    /// # Errors
    ///
    /// Returns `GraphError::CycleDetected` if the pipeline contains a cycle.
    /// Returns `GraphError::NodeNotFound` if an edge references an unknown node.
    pub fn from_pipeline(pipeline: &Pipeline) -> Result<Self, MyceliumError> {
        pipeline.validate()?;

        let mut graph = DiGraph::new();
        let mut node_indices = HashMap::new();

        // Add nodes
        for node in &pipeline.nodes {
            let idx = graph.add_node(node.clone());
            node_indices.insert(node.id.clone(), idx);
        }

        // Add edges
        for edge in &pipeline.edges {
            let from_idx = node_indices
                .get(&edge.from)
                .ok_or_else(|| GraphError::NodeNotFound(edge.from.clone()))?;
            let to_idx = node_indices
                .get(&edge.to)
                .ok_or_else(|| GraphError::NodeNotFound(edge.to.clone()))?;
            graph.add_edge(*from_idx, *to_idx, ());
        }

        // Check for cycles
        if petgraph::algo::is_cyclic_directed(&graph) {
            return Err(GraphError::CycleDetected.into());
        }

        Ok(Self {
            graph,
            _node_indices: node_indices,
        })
    }

    /// Execute the pipeline using the provided service registry.
    ///
    /// Nodes are executed in topological order. Independent nodes at the same
    /// depth are spawned concurrently via `tokio::spawn`. The output of each
    /// node is available to all downstream nodes as their `ServiceInput.params`.
    ///
    /// # Errors
    ///
    /// Returns `GraphError::ExecutionFailed` if any node execution fails.
    pub async fn execute(
        &self,
        services: &HashMap<String, Arc<dyn ScrapingService>>,
    ) -> Result<Vec<NodeResult>, MyceliumError> {
        // Topological sort — safe because we checked for cycles in from_pipeline
        let topo_order = toposort(&self.graph, None).map_err(|_| GraphError::CycleDetected)?;

        // Group nodes into execution waves by their level (longest path from root)
        let waves = self.build_execution_waves(&topo_order);

        // Shared result store
        let results: Arc<Mutex<HashMap<String, ServiceOutput>>> =
            Arc::new(Mutex::new(HashMap::new()));

        for wave in waves {
            // Spawn all nodes in this wave concurrently
            let mut handles = Vec::new();

            for node_idx in wave {
                let node = self.graph[node_idx].clone();
                let service = services.get(&node.service).cloned().ok_or_else(|| {
                    GraphError::InvalidPipeline(format!(
                        "No service registered for type '{}'",
                        node.service
                    ))
                })?;

                // Collect upstream outputs as input params
                let upstream_data = {
                    let store = results.lock().await;
                    let mut data = serde_json::Map::new();
                    for pred_idx in self
                        .graph
                        .neighbors_directed(node_idx, petgraph::Direction::Incoming)
                    {
                        let pred_id = &self.graph[pred_idx].id;
                        if let Some(out) = store.get(pred_id) {
                            data.insert(
                                pred_id.clone(),
                                serde_json::Value::String(out.data.clone()),
                            );
                        }
                    }
                    serde_json::Value::Object(data)
                };

                let input = ServiceInput {
                    url: node
                        .config
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    params: upstream_data,
                };

                let results_clone = Arc::clone(&results);
                let node_id = node.id.clone();

                handles.push(tokio::spawn(async move {
                    let output = service.execute(input).await?;
                    results_clone
                        .lock()
                        .await
                        .insert(node_id.clone(), output.clone());
                    Ok::<NodeResult, MyceliumError>(NodeResult { node_id, output })
                }));
            }

            // Await all handles in this wave, propagating errors
            for handle in handles {
                handle
                    .await
                    .map_err(|e| GraphError::ExecutionFailed(format!("Task join error: {e}")))??;
            }
        }

        // Collect final results in topological order
        let store = results.lock().await;
        let final_results = topo_order
            .iter()
            .filter_map(|idx| {
                let node_id = &self.graph[*idx].id;
                store.get(node_id).map(|output| NodeResult {
                    node_id: node_id.clone(),
                    output: output.clone(),
                })
            })
            .collect();

        Ok(final_results)
    }

    /// Build execution waves: groups of nodes that can run concurrently.
    ///
    /// Each wave contains nodes whose predecessors are all in earlier waves.
    fn build_execution_waves(&self, topo_order: &[NodeIndex]) -> Vec<Vec<NodeIndex>> {
        let mut level: HashMap<NodeIndex, usize> = HashMap::new();

        for &idx in topo_order {
            let max_pred_level = self
                .graph
                .neighbors_directed(idx, petgraph::Direction::Incoming)
                .map(|pred| level.get(&pred).copied().unwrap_or(0) + 1)
                .max()
                .unwrap_or(0);
            level.insert(idx, max_pred_level);
        }

        let max_level = level.values().copied().max().unwrap_or(0);
        let mut waves: Vec<Vec<NodeIndex>> = vec![Vec::new(); max_level + 1];
        for (idx, lvl) in level {
            if let Some(wave) = waves.get_mut(lvl) {
                wave.push(idx);
            }
        }
        waves
    }
}

impl Default for DagExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::Result;

    #[test]
    fn test_node_creation() {
        let node = Node::new(
            "test",
            "http",
            serde_json::json!({"url": "https://example.com"}),
        );
        assert_eq!(node.id, "test");
        assert_eq!(node.service, "http");
    }

    #[test]
    fn test_edge_creation() {
        let edge = Edge::new("a", "b");
        assert_eq!(edge.from, "a");
        assert_eq!(edge.to, "b");
    }

    #[test]
    fn test_pipeline_validation() {
        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new("fetch", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("extract", "ai", serde_json::json!({})));
        pipeline.add_edge(Edge::new("fetch", "extract"));

        assert!(pipeline.validate().is_ok());
    }

    #[test]
    fn test_cycle_detection() {
        let mut pipeline = Pipeline::new("cyclic");
        pipeline.add_node(Node::new("a", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("b", "http", serde_json::json!({})));
        pipeline.add_edge(Edge::new("a", "b"));
        pipeline.add_edge(Edge::new("b", "a")); // Creates a cycle

        let result = DagExecutor::from_pipeline(&pipeline);
        assert!(matches!(
            result,
            Err(MyceliumError::Graph(GraphError::CycleDetected))
        ));
    }

    /// Diamond graph: A → B, A → C, B+C → D
    /// B and C run concurrently in the same wave; D waits for both.
    #[tokio::test]
    async fn test_diamond_concurrent_execution() -> Result<()> {
        use crate::adapters::noop::NoopService;

        // Build diamond pipeline
        let mut pipeline = Pipeline::new("diamond");
        pipeline.add_node(Node::new("A", "noop", serde_json::json!({"url": ""})));
        pipeline.add_node(Node::new("B", "noop", serde_json::json!({"url": ""})));
        pipeline.add_node(Node::new("C", "noop", serde_json::json!({"url": ""})));
        pipeline.add_node(Node::new("D", "noop", serde_json::json!({"url": ""})));
        pipeline.add_edge(Edge::new("A", "B"));
        pipeline.add_edge(Edge::new("A", "C"));
        pipeline.add_edge(Edge::new("B", "D"));
        pipeline.add_edge(Edge::new("C", "D"));

        let executor = DagExecutor::from_pipeline(&pipeline)?;

        let mut services: HashMap<String, std::sync::Arc<dyn crate::ports::ScrapingService>> =
            HashMap::new();
        services.insert("noop".to_string(), std::sync::Arc::new(NoopService));

        let results = executor.execute(&services).await?;

        // All 4 nodes should produce a result
        assert_eq!(results.len(), 4);
        let ids: Vec<&str> = results.iter().map(|r| r.node_id.as_str()).collect();
        assert!(ids.contains(&"A"));
        assert!(ids.contains(&"B"));
        assert!(ids.contains(&"C"));
        assert!(ids.contains(&"D"));
        Ok(())
    }

    #[tokio::test]
    async fn test_missing_service_returns_error() -> Result<()> {
        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new("fetch", "http", serde_json::json!({})));

        let executor = DagExecutor::from_pipeline(&pipeline)?;
        let services: HashMap<String, std::sync::Arc<dyn crate::ports::ScrapingService>> =
            HashMap::new();

        let result = executor.execute(&services).await;
        assert!(result.is_err());
        Ok(())
    }
}
