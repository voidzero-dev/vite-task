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

pub type ExecutionGraph = petgraph::graph::DiGraph<TaskExecution, (), ExecutionIx>;
