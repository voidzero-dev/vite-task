use std::ops::Deref;

use petgraph::graph::{DefaultIx, DiGraph, EdgeIndex, IndexType, NodeIndex};

use crate::TaskExecution;

/// newtype of `DefaultIx` for indices in task graphs
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExecutionIx(DefaultIx);
// SAFETY: ExecutionIx is a newtype over DefaultIx which already implements IndexType correctly
unsafe impl IndexType for ExecutionIx {
    fn new(x: usize) -> Self {
        Self(DefaultIx::new(x))
    }

    fn index(&self) -> usize {
        self.0.index()
    }

    fn max() -> Self {
        Self(<DefaultIx as IndexType>::max())
    }
}

pub type ExecutionNodeIndex = NodeIndex<ExecutionIx>;
pub type ExecutionEdgeIndex = EdgeIndex<ExecutionIx>;

/// The inner directed graph type used for construction and storage.
pub(crate) type InnerExecutionGraph = DiGraph<TaskExecution, (), ExecutionIx>;

/// Error returned by [`ExecutionGraph::try_from_graph`] when a cycle is detected.
///
/// Contains the [`ExecutionNodeIndex`] of one node that participates in the cycle,
/// allowing the caller to look up task details for error reporting.
#[derive(Debug)]
pub struct CycleError {
    node_id: ExecutionNodeIndex,
}

impl CycleError {
    /// Return a node index that participates in the detected cycle.
    #[must_use]
    pub const fn node_id(&self) -> ExecutionNodeIndex {
        self.node_id
    }
}

/// A directed acyclic execution graph.
///
/// Wraps a `petgraph::graph::DiGraph` with a compile-time guarantee that it contains
/// no cycles. The acyclicity invariant is validated at construction time by
/// [`try_from_graph`](Self::try_from_graph), which performs a topological sort and
/// caches the resulting order for later use.
///
/// Unlike `petgraph::acyclic::Acyclic`, this type is `Sync` (no internal `RefCell`),
/// so it can be safely shared via `Arc` across threads.
///
/// The type implements `Deref<Target = DiGraph<...>>`, so all read operations on the
/// inner graph (indexing, iteration, node/edge counts) work transparently.
#[derive(Debug)]
pub struct ExecutionGraph {
    /// The underlying directed graph.
    graph: InnerExecutionGraph,
    /// Pre-computed topological order of node indices.
    /// For every edge A→B in the graph, A appears before B in this vector.
    toposort: Vec<ExecutionNodeIndex>,
}

impl ExecutionGraph {
    /// Validate that `graph` is acyclic and wrap it in an `ExecutionGraph`.
    ///
    /// Performs a topological sort using `petgraph::algo::toposort`. If the graph
    /// contains a cycle, returns a [`CycleError`] identifying one node in the cycle.
    /// On success, the topological order is cached so that subsequent calls to
    /// [`toposort`](Self::toposort) are free.
    ///
    /// # Errors
    ///
    /// Returns [`CycleError`] if the graph contains a cycle.
    pub fn try_from_graph(graph: InnerExecutionGraph) -> Result<Self, CycleError> {
        let toposort = petgraph::algo::toposort(&graph, None)
            .map_err(|cycle| CycleError { node_id: cycle.node_id() })?;
        Ok(Self { graph, toposort })
    }

    /// Return a reference to the underlying `DiGraph`.
    ///
    /// Useful when an API requires `&DiGraph` explicitly (e.g. `vite_graph_ser::serialize_by_key`).
    #[must_use]
    pub const fn inner(&self) -> &InnerExecutionGraph {
        &self.graph
    }

    /// Build an `ExecutionGraph` from an iterator of task executions with no edges.
    ///
    /// Each task execution becomes an independent node in the graph. Since there are
    /// no edges, the graph is trivially acyclic.
    pub fn from_node_list(nodes: impl IntoIterator<Item = TaskExecution>) -> Self {
        let mut graph = InnerExecutionGraph::default();
        let mut toposort = Vec::new();
        for node in nodes {
            toposort.push(graph.add_node(node));
        }
        Self { graph, toposort }
    }

    /// Return the pre-computed topological order of node indices.
    ///
    /// For every edge A→B in the graph, A appears before B in the returned slice.
    /// Since our edges mean "A depends on B", dependents come before their
    /// dependencies. Callers that need execution order (dependencies first) should
    /// iterate in reverse.
    #[must_use]
    pub fn toposort(&self) -> &[ExecutionNodeIndex] {
        &self.toposort
    }
}

impl Default for ExecutionGraph {
    /// Create an empty `ExecutionGraph` (no nodes, no edges).
    ///
    /// An empty graph is trivially acyclic.
    fn default() -> Self {
        Self { graph: InnerExecutionGraph::default(), toposort: Vec::new() }
    }
}

/// Deref to the inner `DiGraph` so that read-only graph operations
/// (`node_count()`, `node_weights()`, `node_indices()`, indexing by `NodeIndex`, etc.)
/// work transparently on `ExecutionGraph`.
impl Deref for ExecutionGraph {
    type Target = InnerExecutionGraph;

    fn deref(&self) -> &Self::Target {
        &self.graph
    }
}
