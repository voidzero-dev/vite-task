use petgraph::graph::{DefaultIx, EdgeIndex, IndexType, NodeIndex};

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

/// The inner directed graph type before acyclicity wrapping.
/// Used during graph construction in `plan_query_request` before validation.
type InnerExecutionGraph = petgraph::graph::DiGraph<TaskExecution, (), ExecutionIx>;

/// A directed acyclic execution graph.
///
/// Wraps `petgraph::graph::DiGraph` in `petgraph::acyclic::Acyclic` to enforce at the
/// type level that the graph has no cycles. This guarantee is established at plan time
/// when the graph is constructed in `plan_query_request`, eliminating the need for
/// runtime cycle detection during execution.
///
/// `Acyclic` implements `Deref<Target = DiGraph<...>>`, so all read operations on the
/// inner graph (indexing, iteration, node/edge counts) work transparently.
pub type ExecutionGraph = petgraph::acyclic::Acyclic<InnerExecutionGraph>;
