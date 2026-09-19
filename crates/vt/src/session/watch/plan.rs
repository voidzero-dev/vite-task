//! Pure nested invocations are containers, not restart units. Preserve their
//! concurrency scopes while scheduling their children independently. Sequences
//! containing several command items remain atomic restart units.
use rustc_hash::FxHashSet;
use vt_plan::{
    ExecutionGraph, ExecutionItemKind, TaskExecution, execution_graph::ExecutionNodeIndex,
};

use super::TaskState;

pub(super) struct Node<'a> {
    pub graph: &'a ExecutionGraph,
    pub index: ExecutionNodeIndex,
    pub dependencies: Vec<usize>,
    scopes: Vec<(usize, usize)>,
}

impl Node<'_> {
    pub fn task(&self) -> &TaskExecution {
        &self.graph.graph[self.index]
    }
}

#[derive(Default)]
pub(super) struct WatchPlan<'a> {
    pub nodes: Vec<Node<'a>>,
    pub impact_edges: Vec<(usize, usize)>,
    limits: Vec<usize>,
}

impl<'a> WatchPlan<'a> {
    pub fn new(graph: &'a ExecutionGraph) -> Self {
        let mut plan = Self::default();
        plan.expand(graph, &[]);
        plan
    }

    fn expand(&mut self, graph: &'a ExecutionGraph, ancestors: &[(usize, usize)]) -> Vec<usize> {
        let scope = self.limits.len();
        self.limits.push(graph.concurrency_limit);
        let mut members = Vec::new();
        for index in graph.graph.node_indices() {
            let task = &graph.graph[index];
            let mut scopes = ancestors.to_vec();
            scopes.push((scope, index.index()));
            let expanded = if let [item] = task.items.as_slice() {
                if let ExecutionItemKind::Expanded(nested) = &item.kind {
                    Some(nested)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(nested) = expanded.filter(|_| task.input_config.positive_globs.is_empty()) {
                members.push(self.expand(nested, &scopes));
            } else {
                let node = self.nodes.len();
                self.nodes.push(Node { graph, index, dependencies: Vec::new(), scopes });
                members.push(vec![node]);
            }
        }
        for edge in graph.graph.raw_edges() {
            for &dependent in &members[edge.source().index()] {
                self.nodes[dependent].dependencies.extend(&members[edge.target().index()]);
            }
        }
        for &(dependent, dependency) in &graph.impact_edges {
            for &dependent in &members[dependent.index()] {
                for &dependency in &members[dependency.index()] {
                    self.impact_edges.push((dependent, dependency));
                }
            }
        }
        members.into_iter().flatten().collect()
    }

    pub fn ready(&self, node: usize, states: &[TaskState]) -> bool {
        states[node].pending
            && states[node].cancel.is_none()
            && self.nodes[node].dependencies.iter().all(|&dependency| states[dependency].succeeded)
            && self.available(node, states)
    }

    fn available(&self, node: usize, states: &[TaskState]) -> bool {
        self.nodes[node].scopes.iter().all(|&(scope, member)| {
            let active: FxHashSet<usize> = self
                .nodes
                .iter()
                .zip(states)
                .filter(|(_, state)| state.cancel.is_some())
                .filter_map(|(node, _)| {
                    node.scopes.iter().find(|&&(id, _)| id == scope).map(|&(_, member)| member)
                })
                .collect();
            active.contains(&member) || active.len() < self.limits[scope]
        })
    }
}
