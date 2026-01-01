use std::{collections::BTreeMap, fmt::Display};

use petgraph::{
    graph::DiGraph,
    visit::{EdgeRef as _, IntoNodeReferences},
};
use serde::{Serialize, Serializer, ser::SerializeMap};

/// Trait for getting a unique key for a node in the graph.
/// This key is used for serializing the graph with `serialize_by_key`.
pub trait GetKey {
    type Key<'a>: Serialize + Ord
    where
        Self: 'a;
    fn key(&self) -> Result<Self::Key<'_>, String>;
}

#[derive(Serialize)]
#[serde(bound = "E: Serialize, N: Serialize")]
struct DiGraphValue<'a, N: GetKey, E> {
    node: &'a N,
    neighbors: BTreeMap<N::Key<'a>, &'a E>,
}

/// A wrapper around `DiGraph` that serializes nodes by their keys.
#[derive(Serialize)]
#[serde(transparent)]
pub struct SerializeByKey<'a, N: GetKey + Serialize, E: Serialize, Ix: petgraph::graph::IndexType>(
    #[serde(serialize_with = "serialize_by_key")] pub &'a DiGraph<N, E, Ix>,
);

/// Serialize a directed graph into a map from node keys to their values and neighbors by keys.
///
/// Keys in nodes and edges are sorted lexicographically.
///
/// If there are multiple nodes with the same key, or multiple edges between nodes with the same keys,
/// an error will be returned.
///
/// This is useful for serializing graphs in a stable and human-readable way.
pub fn serialize_by_key<
    N: GetKey + Serialize,
    E: Serialize,
    Ix: petgraph::graph::IndexType,
    S: Serializer,
>(
    graph: &DiGraph<N, E, Ix>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut map = BTreeMap::<N::Key<'_>, DiGraphValue<'_, N, E>>::new();

    for (node_idx, node) in graph.node_references() {
        let mut neighbors = BTreeMap::<N::Key<'_>, &E>::new();

        for edge in graph.edges(node_idx) {
            let target_idx = edge.target();
            let target_node = graph.node_weight(target_idx).unwrap();
            let existing = neighbors
                .insert(target_node.key().map_err(serde::ser::Error::custom)?, edge.weight());
            if existing.is_some() {
                return Err(serde::ser::Error::custom(
                    "multiple edges between nodes with same id are not supported",
                ));
            }
        }
        let existing = map.insert(
            node.key().map_err(serde::ser::Error::custom)?,
            DiGraphValue { node, neighbors },
        );
        if existing.is_some() {
            return Err(serde::ser::Error::custom(
                "multiple nodes with the same id are not supported",
            ));
        }
    }
    map.serialize(serializer)
}

#[cfg(test)]
mod tests {
    use petgraph::graph::DiGraph;

    use super::*;

    #[derive(Debug, Clone, Serialize)]
    struct TestNode {
        id: &'static str,
        value: i32,
    }

    impl GetKey for TestNode {
        type Key<'a>
            = &'a str
        where
            Self: 'a;

        fn key(&self) -> Result<Self::Key<'_>, String> {
            Ok(self.id)
        }
    }

    #[derive(Serialize)]
    struct GraphWrapper {
        #[serde(serialize_with = "serialize_by_key")]
        graph: DiGraph<TestNode, &'static str>,
    }

    #[test]
    fn test_serialize_graph_happy_path() {
        let mut graph = DiGraph::<TestNode, &'static str>::new();
        let a = graph.add_node(TestNode { id: "a", value: 1 });
        let b = graph.add_node(TestNode { id: "b", value: 2 });
        let c = graph.add_node(TestNode { id: "c", value: 3 });

        graph.add_edge(a, b, "a->b");
        graph.add_edge(a, c, "a->c");
        graph.add_edge(b, c, "b->c");

        let json = serde_json::to_value(GraphWrapper { graph }).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "graph": {
                    "a": {
                        "node": {"id": "a", "value": 1},
                        "neighbors": {"b": "a->b", "c": "a->c"}
                    },
                    "b": {
                        "node": {"id": "b", "value": 2},
                        "neighbors": {"c": "b->c"}
                    },
                    "c": {
                        "node": {"id": "c", "value": 3},
                        "neighbors": {}
                    }
                }
            })
        );
    }

    #[test]
    fn test_serialize_graph_error_duplicate_nodes() {
        let mut graph = DiGraph::<TestNode, &'static str>::new();
        graph.add_node(TestNode { id: "a", value: 1 });
        graph.add_node(TestNode { id: "a", value: 2 }); // duplicate id

        let err = serde_json::to_string(&GraphWrapper { graph }).unwrap_err();
        assert!(err.to_string().contains("multiple nodes with the same id"));
    }

    #[test]
    fn test_serialize_graph_error_duplicate_edges() {
        let mut graph = DiGraph::<TestNode, &'static str>::new();
        let a = graph.add_node(TestNode { id: "a", value: 1 });
        let b = graph.add_node(TestNode { id: "b", value: 2 });

        graph.add_edge(a, b, "first");
        graph.add_edge(a, b, "second"); // duplicate edge

        let err = serde_json::to_string(&GraphWrapper { graph }).unwrap_err();
        assert!(err.to_string().contains("multiple edges between nodes with same id"));
    }
}
