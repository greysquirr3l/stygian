//! Graph introspection types and queries
//!
//! Provides runtime inspection of DAG structure, execution state, and analysis.
//!
//! # Example
//!
//! ```
//! use stygian_graph::domain::graph::{Pipeline, Node, Edge, DagExecutor};
//! use stygian_graph::domain::introspection::GraphSnapshot;
//! use serde_json::json;
//!
//! let mut pipeline = Pipeline::new("example");
//! pipeline.add_node(Node::new("fetch", "http", json!({"url": "https://example.com"})));
//! pipeline.add_node(Node::new("extract", "ai", json!({})));
//! pipeline.add_edge(Edge::new("fetch", "extract"));
//!
//! let executor = DagExecutor::from_pipeline(&pipeline).unwrap();
//! let snapshot = executor.snapshot();
//!
//! assert_eq!(snapshot.node_count, 2);
//! assert_eq!(snapshot.edge_count, 1);
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Information about a single node in the graph
///
/// # Example
///
/// ```
/// use stygian_graph::domain::introspection::NodeInfo;
///
/// let info = NodeInfo {
///     id: "fetch".to_string(),
///     service: "http".to_string(),
///     depth: 0,
///     predecessors: vec![],
///     successors: vec!["extract".to_string()],
///     in_degree: 0,
///     out_degree: 1,
///     config: serde_json::json!({"url": "https://example.com"}),
///     metadata: serde_json::json!(null),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Unique node identifier
    pub id: String,

    /// Service type (http, ai, browser, etc.)
    pub service: String,

    /// Depth in the graph (distance from root nodes)
    pub depth: usize,

    /// IDs of nodes that feed into this node
    pub predecessors: Vec<String>,

    /// IDs of nodes this node feeds into
    pub successors: Vec<String>,

    /// Number of incoming edges
    pub in_degree: usize,

    /// Number of outgoing edges
    pub out_degree: usize,

    /// Node configuration
    pub config: serde_json::Value,

    /// Node metadata
    pub metadata: serde_json::Value,
}

/// Information about a single edge in the graph
///
/// # Example
///
/// ```
/// use stygian_graph::domain::introspection::EdgeInfo;
///
/// let edge = EdgeInfo {
///     from: "fetch".to_string(),
///     to: "extract".to_string(),
///     config: serde_json::json!(null),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeInfo {
    /// Source node ID
    pub from: String,

    /// Target node ID
    pub to: String,

    /// Edge configuration (transforms, filters, etc.)
    pub config: serde_json::Value,
}

/// Execution wave: a group of nodes that can run concurrently
///
/// Nodes in the same wave have no dependencies on each other.
///
/// # Example
///
/// ```
/// use stygian_graph::domain::introspection::ExecutionWave;
///
/// let wave = ExecutionWave {
///     level: 0,
///     node_ids: vec!["fetch_a".to_string(), "fetch_b".to_string()],
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionWave {
    /// Wave level (0 = root nodes, 1 = their dependents, etc.)
    pub level: usize,

    /// Node IDs in this wave (can run concurrently)
    pub node_ids: Vec<String>,
}

/// Critical path analysis result
///
/// Identifies the longest execution path through the graph.
///
/// # Example
///
/// ```
/// use stygian_graph::domain::introspection::CriticalPath;
///
/// let path = CriticalPath {
///     nodes: vec!["fetch".to_string(), "process".to_string(), "store".to_string()],
///     length: 3,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriticalPath {
    /// Node IDs on the critical path, in execution order
    pub nodes: Vec<String>,

    /// Path length (number of nodes)
    pub length: usize,
}

/// Graph connectivity metrics
///
/// # Example
///
/// ```
/// use stygian_graph::domain::introspection::ConnectivityMetrics;
///
/// let metrics = ConnectivityMetrics {
///     is_connected: true,
///     component_count: 1,
///     root_nodes: vec!["fetch".to_string()],
///     leaf_nodes: vec!["store".to_string()],
///     max_depth: 3,
///     avg_degree: 1.5,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectivityMetrics {
    /// Whether all nodes are reachable from at least one root
    pub is_connected: bool,

    /// Number of weakly connected components
    pub component_count: usize,

    /// Nodes with no incoming edges (starting points)
    pub root_nodes: Vec<String>,

    /// Nodes with no outgoing edges (endpoints)
    pub leaf_nodes: Vec<String>,

    /// Maximum depth of the graph
    pub max_depth: usize,

    /// Average node degree (in + out)
    pub avg_degree: f64,
}

/// Complete snapshot of graph structure and analysis
///
/// Provides a comprehensive view of the graph state for introspection.
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
/// let snapshot = executor.snapshot();
///
/// assert_eq!(snapshot.node_count, 1);
/// assert_eq!(snapshot.edge_count, 0);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphSnapshot {
    /// Total number of nodes
    pub node_count: usize,

    /// Total number of edges
    pub edge_count: usize,

    /// All nodes with their info
    pub nodes: Vec<NodeInfo>,

    /// All edges with their info
    pub edges: Vec<EdgeInfo>,

    /// Execution waves (concurrent groups)
    pub waves: Vec<ExecutionWave>,

    /// Topological execution order
    pub topological_order: Vec<String>,

    /// Critical path through the graph
    pub critical_path: CriticalPath,

    /// Connectivity metrics
    pub connectivity: ConnectivityMetrics,

    /// Service type distribution (service -> count)
    pub service_distribution: HashMap<String, usize>,
}

/// Query for filtering nodes in introspection
///
/// # Example
///
/// ```
/// use stygian_graph::domain::introspection::NodeQuery;
///
/// // Find all HTTP service nodes
/// let query = NodeQuery {
///     service: Some("http".to_string()),
///     ..Default::default()
/// };
///
/// // Find root nodes only
/// let root_query = NodeQuery {
///     is_root: Some(true),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeQuery {
    /// Filter by service type
    pub service: Option<String>,

    /// Filter by exact node ID
    pub id: Option<String>,

    /// Filter by ID pattern (substring match)
    pub id_pattern: Option<String>,

    /// Only root nodes (in_degree = 0)
    pub is_root: Option<bool>,

    /// Only leaf nodes (out_degree = 0)
    pub is_leaf: Option<bool>,

    /// Minimum depth
    pub min_depth: Option<usize>,

    /// Maximum depth
    pub max_depth: Option<usize>,
}

impl NodeQuery {
    /// Create a query that matches all nodes
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::introspection::NodeQuery;
    ///
    /// let query = NodeQuery::all();
    /// ```
    #[must_use]
    pub fn all() -> Self {
        Self::default()
    }

    /// Create a query filtering by service type
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::introspection::NodeQuery;
    ///
    /// let query = NodeQuery::by_service("http");
    /// ```
    #[must_use]
    pub fn by_service(service: impl Into<String>) -> Self {
        Self {
            service: Some(service.into()),
            ..Default::default()
        }
    }

    /// Create a query for root nodes
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::introspection::NodeQuery;
    ///
    /// let query = NodeQuery::roots();
    /// ```
    #[must_use]
    pub fn roots() -> Self {
        Self {
            is_root: Some(true),
            ..Default::default()
        }
    }

    /// Create a query for leaf nodes
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::introspection::NodeQuery;
    ///
    /// let query = NodeQuery::leaves();
    /// ```
    #[must_use]
    pub fn leaves() -> Self {
        Self {
            is_leaf: Some(true),
            ..Default::default()
        }
    }

    /// Check if a node matches this query
    #[must_use]
    pub fn matches(&self, node: &NodeInfo) -> bool {
        if let Some(ref service) = self.service
            && &node.service != service
        {
            return false;
        }

        if let Some(ref id) = self.id
            && &node.id != id
        {
            return false;
        }

        if let Some(ref pattern) = self.id_pattern
            && !node.id.contains(pattern)
        {
            return false;
        }

        if let Some(is_root) = self.is_root
            && is_root != (node.in_degree == 0)
        {
            return false;
        }

        if let Some(is_leaf) = self.is_leaf
            && is_leaf != (node.out_degree == 0)
        {
            return false;
        }

        if let Some(min_depth) = self.min_depth
            && node.depth < min_depth
        {
            return false;
        }

        if let Some(max_depth) = self.max_depth
            && node.depth > max_depth
        {
            return false;
        }

        true
    }
}

/// Dependency chain from one node to another
///
/// # Example
///
/// ```
/// use stygian_graph::domain::introspection::DependencyChain;
///
/// let chain = DependencyChain {
///     from: "fetch".to_string(),
///     to: "store".to_string(),
///     path: vec!["fetch".to_string(), "process".to_string(), "store".to_string()],
///     distance: 2,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyChain {
    /// Starting node ID
    pub from: String,

    /// Ending node ID
    pub to: String,

    /// All nodes in the path, from start to end
    pub path: Vec<String>,

    /// Number of edges between from and to
    pub distance: usize,
}

/// Impact analysis when modifying a node
///
/// Shows what would be affected if a node changes.
///
/// # Example
///
/// ```
/// use stygian_graph::domain::introspection::ImpactAnalysis;
///
/// let impact = ImpactAnalysis {
///     node_id: "fetch".to_string(),
///     upstream: vec![],
///     downstream: vec!["extract".to_string(), "store".to_string()],
///     total_affected: 2,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactAnalysis {
    /// The node being analyzed
    pub node_id: String,

    /// All upstream dependencies (transitively)
    pub upstream: Vec<String>,

    /// All downstream dependents (transitively)
    pub downstream: Vec<String>,

    /// Total nodes affected (upstream + downstream)
    pub total_affected: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_query_all() {
        let query = NodeQuery::all();
        let node = NodeInfo {
            id: "test".to_string(),
            service: "http".to_string(),
            depth: 0,
            predecessors: vec![],
            successors: vec![],
            in_degree: 0,
            out_degree: 0,
            config: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
        };
        assert!(query.matches(&node));
    }

    #[test]
    fn test_node_query_by_service() {
        let query = NodeQuery::by_service("http");
        let http_node = NodeInfo {
            id: "fetch".to_string(),
            service: "http".to_string(),
            depth: 0,
            predecessors: vec![],
            successors: vec![],
            in_degree: 0,
            out_degree: 0,
            config: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
        };
        let ai_node = NodeInfo {
            id: "extract".to_string(),
            service: "ai".to_string(),
            depth: 1,
            predecessors: vec!["fetch".to_string()],
            successors: vec![],
            in_degree: 1,
            out_degree: 0,
            config: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
        };

        assert!(query.matches(&http_node));
        assert!(!query.matches(&ai_node));
    }

    #[test]
    fn test_node_query_roots() {
        let query = NodeQuery::roots();
        let root = NodeInfo {
            id: "fetch".to_string(),
            service: "http".to_string(),
            depth: 0,
            predecessors: vec![],
            successors: vec!["extract".to_string()],
            in_degree: 0,
            out_degree: 1,
            config: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
        };
        let non_root = NodeInfo {
            id: "extract".to_string(),
            service: "ai".to_string(),
            depth: 1,
            predecessors: vec!["fetch".to_string()],
            successors: vec![],
            in_degree: 1,
            out_degree: 0,
            config: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
        };

        assert!(query.matches(&root));
        assert!(!query.matches(&non_root));
    }

    #[test]
    fn test_node_query_depth_range() {
        let query = NodeQuery {
            min_depth: Some(1),
            max_depth: Some(2),
            ..Default::default()
        };

        let depth_0 = NodeInfo {
            id: "a".to_string(),
            service: "http".to_string(),
            depth: 0,
            predecessors: vec![],
            successors: vec![],
            in_degree: 0,
            out_degree: 0,
            config: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
        };
        let depth_1 = NodeInfo {
            id: "b".to_string(),
            service: "http".to_string(),
            depth: 1,
            predecessors: vec![],
            successors: vec![],
            in_degree: 1,
            out_degree: 0,
            config: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
        };
        let depth_3 = NodeInfo {
            id: "c".to_string(),
            service: "http".to_string(),
            depth: 3,
            predecessors: vec![],
            successors: vec![],
            in_degree: 1,
            out_degree: 0,
            config: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
        };

        assert!(!query.matches(&depth_0));
        assert!(query.matches(&depth_1));
        assert!(!query.matches(&depth_3));
    }
}
