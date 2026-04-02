//! DAG execution engine
//!
//! Implements graph-based pipeline execution using petgraph.
//! Defines core domain entities: Node, Edge, and Pipeline.
//!
//! # Example
//!
//! ```
//! use stygian_graph::domain::graph::{Node, Edge, DagExecutor};
//!
//! let node = Node::new("fetch", "http", serde_json::json!({"url": "https://example.com"}));
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::error::{GraphError, StygianError};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

/// A node in the scraping pipeline graph
///
/// Represents a single operation (HTTP fetch, AI extraction, transformation, etc.)
///
/// # Example
///
/// ```
/// use stygian_graph::domain::graph::Node;
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
    /// use stygian_graph::domain::graph::Node;
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
    pub fn validate(&self) -> Result<(), StygianError> {
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
/// use stygian_graph::domain::graph::Edge;
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
    /// use stygian_graph::domain::graph::Edge;
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
    pub fn validate(&self) -> Result<(), StygianError> {
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
/// use stygian_graph::domain::graph::{Pipeline, Node, Edge};
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
    /// use stygian_graph::domain::graph::Pipeline;
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
    pub fn validate(&self) -> Result<(), StygianError> {
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
    /// use stygian_graph::domain::graph::DagExecutor;
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
    pub fn from_pipeline(pipeline: &Pipeline) -> Result<Self, StygianError> {
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
    ) -> Result<Vec<NodeResult>, StygianError> {
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
                    Ok::<NodeResult, StygianError>(NodeResult { node_id, output })
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

    // ═══════════════════════════════════════════════════════════════════════════
    // Graph Introspection Methods
    // ═══════════════════════════════════════════════════════════════════════════

    /// Get the total number of nodes in the graph
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("a", "http", json!({})));
    /// pipeline.add_node(Node::new("b", "http", json!({})));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// assert_eq!(executor.node_count(), 2);
    /// ```
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Get the total number of edges in the graph
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, Edge, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("a", "http", json!({})));
    /// pipeline.add_node(Node::new("b", "http", json!({})));
    /// pipeline.add_edge(Edge::new("a", "b"));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// assert_eq!(executor.edge_count(), 1);
    /// ```
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Get all node IDs in the graph
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("fetch", "http", json!({})));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// assert!(executor.node_ids().contains(&"fetch".to_string()));
    /// ```
    #[must_use]
    pub fn node_ids(&self) -> Vec<String> {
        self.graph
            .node_indices()
            .map(|idx| self.graph[idx].id.clone())
            .collect()
    }

    /// Get a node by ID
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("fetch", "http", json!({})));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// let node = executor.get_node("fetch");
    /// assert!(node.is_some());
    /// assert_eq!(node.unwrap().service, "http");
    /// ```
    #[must_use]
    pub fn get_node(&self, id: &str) -> Option<&Node> {
        self.graph
            .node_indices()
            .find(|&idx| self.graph[idx].id == id)
            .map(|idx| &self.graph[idx])
    }

    /// Get the predecessors (upstream nodes) of a node
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, Edge, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("a", "http", json!({})));
    /// pipeline.add_node(Node::new("b", "http", json!({})));
    /// pipeline.add_edge(Edge::new("a", "b"));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// let preds = executor.predecessors("b");
    /// assert_eq!(preds, vec!["a".to_string()]);
    /// ```
    #[must_use]
    pub fn predecessors(&self, id: &str) -> Vec<String> {
        self.graph
            .node_indices()
            .find(|&idx| self.graph[idx].id == id)
            .map(|idx| {
                self.graph
                    .neighbors_directed(idx, petgraph::Direction::Incoming)
                    .map(|pred| self.graph[pred].id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get the successors (downstream nodes) of a node
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, Edge, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("a", "http", json!({})));
    /// pipeline.add_node(Node::new("b", "http", json!({})));
    /// pipeline.add_edge(Edge::new("a", "b"));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// let succs = executor.successors("a");
    /// assert_eq!(succs, vec!["b".to_string()]);
    /// ```
    #[must_use]
    pub fn successors(&self, id: &str) -> Vec<String> {
        self.graph
            .node_indices()
            .find(|&idx| self.graph[idx].id == id)
            .map(|idx| {
                self.graph
                    .neighbors_directed(idx, petgraph::Direction::Outgoing)
                    .map(|succ| self.graph[succ].id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get the topological order of nodes
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, Edge, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("a", "http", json!({})));
    /// pipeline.add_node(Node::new("b", "http", json!({})));
    /// pipeline.add_edge(Edge::new("a", "b"));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// let order = executor.topological_order();
    /// // "a" must appear before "b"
    /// let a_pos = order.iter().position(|x| x == "a").unwrap();
    /// let b_pos = order.iter().position(|x| x == "b").unwrap();
    /// assert!(a_pos < b_pos);
    /// ```
    #[must_use]
    pub fn topological_order(&self) -> Vec<String> {
        toposort(&self.graph, None)
            .map(|indices| {
                indices
                    .iter()
                    .map(|&idx| self.graph[idx].id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get execution waves (groups of nodes that can run concurrently)
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, Edge, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("a", "http", json!({})));
    /// pipeline.add_node(Node::new("b", "http", json!({})));
    /// pipeline.add_node(Node::new("c", "http", json!({})));
    /// pipeline.add_edge(Edge::new("a", "c"));
    /// pipeline.add_edge(Edge::new("b", "c"));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// let waves = executor.execution_waves();
    /// // Wave 0 contains "a" and "b" (can run concurrently)
    /// // Wave 1 contains "c" (depends on both)
    /// assert_eq!(waves.len(), 2);
    /// ```
    #[must_use]
    pub fn execution_waves(&self) -> Vec<super::introspection::ExecutionWave> {
        let topo = match toposort(&self.graph, None) {
            Ok(t) => t,
            Err(_) => return vec![],
        };

        let waves = self.build_execution_waves(&topo);

        waves
            .into_iter()
            .enumerate()
            .filter(|(_, nodes)| !nodes.is_empty())
            .map(|(level, nodes)| super::introspection::ExecutionWave {
                level,
                node_ids: nodes
                    .iter()
                    .map(|&idx| self.graph[idx].id.clone())
                    .collect(),
            })
            .collect()
    }

    /// Get information about a specific node
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, Edge, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("fetch", "http", json!({"url": "https://example.com"})));
    /// pipeline.add_node(Node::new("extract", "ai", json!({})));
    /// pipeline.add_edge(Edge::new("fetch", "extract"));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// let info = executor.node_info("fetch").unwrap();
    /// assert_eq!(info.service, "http");
    /// assert_eq!(info.in_degree, 0);
    /// assert_eq!(info.out_degree, 1);
    /// ```
    #[must_use]
    pub fn node_info(&self, id: &str) -> Option<super::introspection::NodeInfo> {
        let depths = self.compute_depths();

        self.graph
            .node_indices()
            .find(|&idx| self.graph[idx].id == id)
            .map(|idx| {
                let node = &self.graph[idx];
                let predecessors: Vec<String> = self
                    .graph
                    .neighbors_directed(idx, petgraph::Direction::Incoming)
                    .map(|pred| self.graph[pred].id.clone())
                    .collect();
                let successors: Vec<String> = self
                    .graph
                    .neighbors_directed(idx, petgraph::Direction::Outgoing)
                    .map(|succ| self.graph[succ].id.clone())
                    .collect();

                super::introspection::NodeInfo {
                    id: node.id.clone(),
                    service: node.service.clone(),
                    depth: depths.get(&idx).copied().unwrap_or(0),
                    in_degree: predecessors.len(),
                    out_degree: successors.len(),
                    predecessors,
                    successors,
                    config: node.config.clone(),
                    metadata: node.metadata.clone(),
                }
            })
    }

    /// Get all nodes matching a query
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, Edge, DagExecutor};
    /// use stygian_graph::domain::introspection::NodeQuery;
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("fetch1", "http", json!({})));
    /// pipeline.add_node(Node::new("fetch2", "http", json!({})));
    /// pipeline.add_node(Node::new("extract", "ai", json!({})));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// let http_nodes = executor.query_nodes(&NodeQuery::by_service("http"));
    /// assert_eq!(http_nodes.len(), 2);
    /// ```
    #[must_use]
    pub fn query_nodes(
        &self,
        query: &super::introspection::NodeQuery,
    ) -> Vec<super::introspection::NodeInfo> {
        self.graph
            .node_indices()
            .filter_map(|idx| self.node_info(&self.graph[idx].id))
            .filter(|info| query.matches(info))
            .collect()
    }

    /// Compute the depth of each node from root nodes
    fn compute_depths(&self) -> HashMap<NodeIndex, usize> {
        let mut depths = HashMap::new();

        let topo = match toposort(&self.graph, None) {
            Ok(t) => t,
            Err(_) => return depths,
        };

        for &idx in &topo {
            let max_pred_depth = self
                .graph
                .neighbors_directed(idx, petgraph::Direction::Incoming)
                .filter_map(|pred| depths.get(&pred))
                .max()
                .copied()
                .map(|d| d + 1)
                .unwrap_or(0);
            depths.insert(idx, max_pred_depth);
        }

        depths
    }

    /// Get connectivity metrics for the graph
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, Edge, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("a", "http", json!({})));
    /// pipeline.add_node(Node::new("b", "http", json!({})));
    /// pipeline.add_edge(Edge::new("a", "b"));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// let metrics = executor.connectivity();
    /// assert_eq!(metrics.root_nodes, vec!["a".to_string()]);
    /// assert_eq!(metrics.leaf_nodes, vec!["b".to_string()]);
    /// ```
    #[must_use]
    pub fn connectivity(&self) -> super::introspection::ConnectivityMetrics {
        let depths = self.compute_depths();

        let root_nodes: Vec<String> = self
            .graph
            .node_indices()
            .filter(|&idx| {
                self.graph
                    .neighbors_directed(idx, petgraph::Direction::Incoming)
                    .next()
                    .is_none()
            })
            .map(|idx| self.graph[idx].id.clone())
            .collect();

        let leaf_nodes: Vec<String> = self
            .graph
            .node_indices()
            .filter(|&idx| {
                self.graph
                    .neighbors_directed(idx, petgraph::Direction::Outgoing)
                    .next()
                    .is_none()
            })
            .map(|idx| self.graph[idx].id.clone())
            .collect();

        let max_depth = depths.values().copied().max().unwrap_or(0);

        let total_degree: usize = self
            .graph
            .node_indices()
            .map(|idx| {
                let in_deg = self
                    .graph
                    .neighbors_directed(idx, petgraph::Direction::Incoming)
                    .count();
                let out_deg = self
                    .graph
                    .neighbors_directed(idx, petgraph::Direction::Outgoing)
                    .count();
                in_deg + out_deg
            })
            .sum();

        let node_count = self.graph.node_count();
        let avg_degree = if node_count > 0 {
            total_degree as f64 / node_count as f64
        } else {
            0.0
        };

        // Count weakly connected components
        let component_count = petgraph::algo::connected_components(&self.graph);

        super::introspection::ConnectivityMetrics {
            is_connected: component_count <= 1,
            component_count,
            root_nodes,
            leaf_nodes,
            max_depth,
            avg_degree,
        }
    }

    /// Get the critical path (longest path through the graph)
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, Edge, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("a", "http", json!({})));
    /// pipeline.add_node(Node::new("b", "http", json!({})));
    /// pipeline.add_node(Node::new("c", "http", json!({})));
    /// pipeline.add_edge(Edge::new("a", "b"));
    /// pipeline.add_edge(Edge::new("b", "c"));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// let critical = executor.critical_path();
    /// assert_eq!(critical.length, 3);
    /// assert_eq!(critical.nodes, vec!["a", "b", "c"]);
    /// ```
    #[must_use]
    pub fn critical_path(&self) -> super::introspection::CriticalPath {
        let depths = self.compute_depths();

        // Find the deepest node
        let deepest = depths.iter().max_by_key(|&(_, d)| d);

        if let Some((&end_idx, _)) = deepest {
            // Trace back from deepest to a root
            let mut path = vec![self.graph[end_idx].id.clone()];
            let mut current = end_idx;

            while let Some(pred) = self
                .graph
                .neighbors_directed(current, petgraph::Direction::Incoming)
                .max_by_key(|&p| depths.get(&p).copied().unwrap_or(0))
            {
                path.push(self.graph[pred].id.clone());
                current = pred;
            }

            path.reverse();

            super::introspection::CriticalPath {
                length: path.len(),
                nodes: path,
            }
        } else {
            super::introspection::CriticalPath {
                length: 0,
                nodes: vec![],
            }
        }
    }

    /// Analyze the impact of changing a node
    ///
    /// Returns all nodes that would be affected (upstream dependencies
    /// and downstream dependents, transitively).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, Edge, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("a", "http", json!({})));
    /// pipeline.add_node(Node::new("b", "http", json!({})));
    /// pipeline.add_node(Node::new("c", "http", json!({})));
    /// pipeline.add_edge(Edge::new("a", "b"));
    /// pipeline.add_edge(Edge::new("b", "c"));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// let impact = executor.impact_analysis("b");
    /// assert_eq!(impact.upstream, vec!["a".to_string()]);
    /// assert_eq!(impact.downstream, vec!["c".to_string()]);
    /// ```
    #[must_use]
    pub fn impact_analysis(&self, id: &str) -> super::introspection::ImpactAnalysis {
        let node_idx = self
            .graph
            .node_indices()
            .find(|&idx| self.graph[idx].id == id);

        let Some(start_idx) = node_idx else {
            return super::introspection::ImpactAnalysis {
                node_id: id.to_string(),
                upstream: vec![],
                downstream: vec![],
                total_affected: 0,
            };
        };

        // BFS upstream
        let mut upstream = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();

        for pred in self
            .graph
            .neighbors_directed(start_idx, petgraph::Direction::Incoming)
        {
            queue.push_back(pred);
        }

        while let Some(idx) = queue.pop_front() {
            if visited.insert(idx) {
                upstream.push(self.graph[idx].id.clone());
                for pred in self
                    .graph
                    .neighbors_directed(idx, petgraph::Direction::Incoming)
                {
                    queue.push_back(pred);
                }
            }
        }

        // BFS downstream
        let mut downstream = Vec::new();
        visited.clear();
        queue.clear();

        for succ in self
            .graph
            .neighbors_directed(start_idx, petgraph::Direction::Outgoing)
        {
            queue.push_back(succ);
        }

        while let Some(idx) = queue.pop_front() {
            if visited.insert(idx) {
                downstream.push(self.graph[idx].id.clone());
                for succ in self
                    .graph
                    .neighbors_directed(idx, petgraph::Direction::Outgoing)
                {
                    queue.push_back(succ);
                }
            }
        }

        let total_affected = upstream.len() + downstream.len();

        super::introspection::ImpactAnalysis {
            node_id: id.to_string(),
            upstream,
            downstream,
            total_affected,
        }
    }

    /// Get a complete snapshot of the graph for introspection
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::graph::{Pipeline, Node, Edge, DagExecutor};
    /// use serde_json::json;
    ///
    /// let mut pipeline = Pipeline::new("test");
    /// pipeline.add_node(Node::new("fetch", "http", json!({})));
    /// pipeline.add_node(Node::new("extract", "ai", json!({})));
    /// pipeline.add_edge(Edge::new("fetch", "extract"));
    ///
    /// let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
    /// let snapshot = executor.snapshot();
    ///
    /// assert_eq!(snapshot.node_count, 2);
    /// assert_eq!(snapshot.edge_count, 1);
    /// ```
    #[must_use]
    pub fn snapshot(&self) -> super::introspection::GraphSnapshot {
        let nodes: Vec<super::introspection::NodeInfo> = self
            .graph
            .node_indices()
            .filter_map(|idx| self.node_info(&self.graph[idx].id))
            .collect();

        let edges: Vec<super::introspection::EdgeInfo> = self
            .graph
            .edge_references()
            .map(|edge| {
                let from = &self.graph[edge.source()].id;
                let to = &self.graph[edge.target()].id;
                super::introspection::EdgeInfo {
                    from: from.clone(),
                    to: to.clone(),
                    config: serde_json::Value::Null,
                }
            })
            .collect();

        let mut service_distribution = HashMap::new();
        for node in &nodes {
            *service_distribution
                .entry(node.service.clone())
                .or_insert(0) += 1;
        }

        super::introspection::GraphSnapshot {
            node_count: self.node_count(),
            edge_count: self.edge_count(),
            nodes,
            edges,
            waves: self.execution_waves(),
            topological_order: self.topological_order(),
            critical_path: self.critical_path(),
            connectivity: self.connectivity(),
            service_distribution,
        }
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
            Err(StygianError::Graph(GraphError::CycleDetected))
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

    // ═══════════════════════════════════════════════════════════════════════════
    // Introspection Tests
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_introspection_node_count() {
        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new("a", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("b", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("c", "ai", serde_json::json!({})));

        let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
        assert_eq!(executor.node_count(), 3);
        assert_eq!(executor.edge_count(), 0);
    }

    #[test]
    fn test_introspection_node_ids() {
        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new("fetch", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("extract", "ai", serde_json::json!({})));

        let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
        let ids = executor.node_ids();
        assert!(ids.contains(&"fetch".to_string()));
        assert!(ids.contains(&"extract".to_string()));
    }

    #[test]
    fn test_introspection_get_node() {
        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new(
            "fetch",
            "http",
            serde_json::json!({"url": "https://example.com"}),
        ));

        let executor = DagExecutor::from_pipeline(&pipeline).unwrap();

        let node = executor.get_node("fetch");
        assert!(node.is_some());
        let node = node.unwrap();
        assert_eq!(node.service, "http");

        assert!(executor.get_node("nonexistent").is_none());
    }

    #[test]
    fn test_introspection_predecessors_successors() {
        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new("a", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("b", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("c", "ai", serde_json::json!({})));
        pipeline.add_edge(Edge::new("a", "b"));
        pipeline.add_edge(Edge::new("b", "c"));

        let executor = DagExecutor::from_pipeline(&pipeline).unwrap();

        assert_eq!(executor.predecessors("a"), Vec::<String>::new());
        assert_eq!(executor.predecessors("b"), vec!["a".to_string()]);
        assert_eq!(executor.predecessors("c"), vec!["b".to_string()]);

        assert_eq!(executor.successors("a"), vec!["b".to_string()]);
        assert_eq!(executor.successors("b"), vec!["c".to_string()]);
        assert_eq!(executor.successors("c"), Vec::<String>::new());
    }

    #[test]
    fn test_introspection_topological_order() {
        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new("a", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("b", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("c", "ai", serde_json::json!({})));
        pipeline.add_edge(Edge::new("a", "b"));
        pipeline.add_edge(Edge::new("b", "c"));

        let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
        let order = executor.topological_order();

        let a_pos = order.iter().position(|x| x == "a").unwrap();
        let b_pos = order.iter().position(|x| x == "b").unwrap();
        let c_pos = order.iter().position(|x| x == "c").unwrap();

        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn test_introspection_execution_waves_diamond() {
        // Diamond: A → (B, C) → D
        let mut pipeline = Pipeline::new("diamond");
        pipeline.add_node(Node::new("A", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("B", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("C", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("D", "http", serde_json::json!({})));
        pipeline.add_edge(Edge::new("A", "B"));
        pipeline.add_edge(Edge::new("A", "C"));
        pipeline.add_edge(Edge::new("B", "D"));
        pipeline.add_edge(Edge::new("C", "D"));

        let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
        let waves = executor.execution_waves();

        // Should have 3 waves: [A], [B, C], [D]
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0].level, 0);
        assert!(waves[0].node_ids.contains(&"A".to_string()));
        assert_eq!(waves[1].level, 1);
        assert!(waves[1].node_ids.contains(&"B".to_string()));
        assert!(waves[1].node_ids.contains(&"C".to_string()));
        assert_eq!(waves[2].level, 2);
        assert!(waves[2].node_ids.contains(&"D".to_string()));
    }

    #[test]
    fn test_introspection_node_info() {
        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new("fetch", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("extract", "ai", serde_json::json!({})));
        pipeline.add_edge(Edge::new("fetch", "extract"));

        let executor = DagExecutor::from_pipeline(&pipeline).unwrap();

        let info = executor.node_info("fetch").unwrap();
        assert_eq!(info.id, "fetch");
        assert_eq!(info.service, "http");
        assert_eq!(info.depth, 0);
        assert_eq!(info.in_degree, 0);
        assert_eq!(info.out_degree, 1);
        assert!(info.successors.contains(&"extract".to_string()));

        let info = executor.node_info("extract").unwrap();
        assert_eq!(info.depth, 1);
        assert_eq!(info.in_degree, 1);
        assert_eq!(info.out_degree, 0);
    }

    #[test]
    fn test_introspection_connectivity() {
        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new("a", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("b", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("c", "ai", serde_json::json!({})));
        pipeline.add_edge(Edge::new("a", "b"));
        pipeline.add_edge(Edge::new("b", "c"));

        let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
        let metrics = executor.connectivity();

        assert!(metrics.is_connected);
        assert_eq!(metrics.component_count, 1);
        assert_eq!(metrics.root_nodes, vec!["a".to_string()]);
        assert_eq!(metrics.leaf_nodes, vec!["c".to_string()]);
        assert_eq!(metrics.max_depth, 2);
    }

    #[test]
    fn test_introspection_critical_path() {
        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new("a", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("b", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("c", "ai", serde_json::json!({})));
        pipeline.add_edge(Edge::new("a", "b"));
        pipeline.add_edge(Edge::new("b", "c"));

        let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
        let critical = executor.critical_path();

        assert_eq!(critical.length, 3);
        assert_eq!(critical.nodes, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_introspection_impact_analysis() {
        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new("a", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("b", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("c", "ai", serde_json::json!({})));
        pipeline.add_edge(Edge::new("a", "b"));
        pipeline.add_edge(Edge::new("b", "c"));

        let executor = DagExecutor::from_pipeline(&pipeline).unwrap();

        let impact = executor.impact_analysis("b");
        assert_eq!(impact.node_id, "b");
        assert_eq!(impact.upstream, vec!["a".to_string()]);
        assert_eq!(impact.downstream, vec!["c".to_string()]);
        assert_eq!(impact.total_affected, 2);

        // Root node has no upstream
        let impact = executor.impact_analysis("a");
        assert!(impact.upstream.is_empty());
        assert_eq!(impact.downstream.len(), 2);

        // Leaf node has no downstream
        let impact = executor.impact_analysis("c");
        assert_eq!(impact.upstream.len(), 2);
        assert!(impact.downstream.is_empty());
    }

    #[test]
    fn test_introspection_snapshot() {
        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new("fetch", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("extract", "ai", serde_json::json!({})));
        pipeline.add_edge(Edge::new("fetch", "extract"));

        let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
        let snapshot = executor.snapshot();

        assert_eq!(snapshot.node_count, 2);
        assert_eq!(snapshot.edge_count, 1);
        assert_eq!(snapshot.nodes.len(), 2);
        assert_eq!(snapshot.edges.len(), 1);
        assert_eq!(snapshot.waves.len(), 2);
        assert_eq!(snapshot.topological_order.len(), 2);
        assert_eq!(snapshot.critical_path.length, 2);
        assert_eq!(snapshot.service_distribution.get("http"), Some(&1));
        assert_eq!(snapshot.service_distribution.get("ai"), Some(&1));
    }

    #[test]
    fn test_introspection_query_nodes() {
        use super::super::introspection::NodeQuery;

        let mut pipeline = Pipeline::new("test");
        pipeline.add_node(Node::new("fetch1", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("fetch2", "http", serde_json::json!({})));
        pipeline.add_node(Node::new("extract", "ai", serde_json::json!({})));
        pipeline.add_edge(Edge::new("fetch1", "extract"));
        pipeline.add_edge(Edge::new("fetch2", "extract"));

        let executor = DagExecutor::from_pipeline(&pipeline).unwrap();

        // Query by service
        let http_nodes = executor.query_nodes(&NodeQuery::by_service("http"));
        assert_eq!(http_nodes.len(), 2);

        let ai_nodes = executor.query_nodes(&NodeQuery::by_service("ai"));
        assert_eq!(ai_nodes.len(), 1);

        // Query roots
        let roots = executor.query_nodes(&NodeQuery::roots());
        assert_eq!(roots.len(), 2);

        // Query leaves
        let leaves = executor.query_nodes(&NodeQuery::leaves());
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].id, "extract");
    }
}
