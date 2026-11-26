//! A directed graph whose nodes load one partition at a time, on first access.
//!
//! [`LazyGraph`] owns all loading state. Code built on it reads nodes and edges
//! through [`LazyGraph::get`], [`LazyGraph::nodes_in`] and [`LazyGraph::neighbors`],
//! and never decides what to load or when.
//!
//! Loading happens at two levels:
//!
//! - A partition's nodes load together, the first time any of them is read.
//! - A node's outgoing edges are resolved the first time they are read.
//!
//! Resolving edges may load other partitions, but never resolves another node's
//! edges ([`Lookup`] has no way to). So edges that form a cycle across partitions
//! can't make two loads wait on each other.

use std::{fmt, hash::Hash};

use elsa::FrozenMap;
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use tokio::sync::OnceCell;

/// A node that carries the key it's stored under.
pub trait Keyed {
    type Key;

    fn key(&self) -> &Self::Key;
}

/// Loads the nodes of a partition, and resolves the edges of a node.
#[async_trait::async_trait(?Send)]
pub trait Loader: Sized {
    /// A group of nodes that load together.
    type Partition: Copy + Eq + Hash;

    /// Identifies a node across the whole graph.
    type Key: Clone + Eq + Hash;

    type Node: Keyed<Key = Self::Key>;

    /// What a node's edges are resolved from.
    type EdgeSpec;

    type Error;

    /// The partition holding the node with `key`.
    fn partition_of(key: &Self::Key) -> Self::Partition;

    /// Loads every node of `partition`, each with the spec its edges are resolved from.
    ///
    /// Each node's key must be unique, and must map back to `partition`.
    async fn load(
        &self,
        partition: Self::Partition,
    ) -> Result<Vec<(Self::Node, Self::EdgeSpec)>, Self::Error>;

    /// Resolves the keys of the nodes that `from` has edges to.
    ///
    /// Every returned key must be the key of a node that `graph` can find.
    async fn resolve_edges(
        &self,
        from: &Self::Node,
        spec: &Self::EdgeSpec,
        graph: Lookup<'_, Self>,
    ) -> Result<Vec<Self::Key>, Self::Error>;
}

/// A graph whose partitions load on first access. See the [module docs](self).
pub struct LazyGraph<L: Loader> {
    loader: L,

    /// Every loaded node. Entries are never removed and their boxes never move,
    /// so references to them last as long as the graph.
    nodes: FrozenMap<L::Key, Box<Slot<L>>, FxBuildHasher>,

    /// Each partition's node keys in load order, set once the partition has loaded.
    partitions: FxHashMap<L::Partition, KeysCell<L>>,
}

/// Keys that are set on first access.
type KeysCell<L> = OnceCell<Box<[<L as Loader>::Key]>>;

struct Slot<L: Loader> {
    node: L::Node,
    edge_spec: L::EdgeSpec,
    /// The keys of the nodes this one has edges to, set once they're resolved.
    edges: KeysCell<L>,
}

impl<L: Loader> fmt::Debug for LazyGraph<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LazyGraph").finish_non_exhaustive()
    }
}

impl<L: Loader> LazyGraph<L> {
    /// Creates a graph over `partitions`, with nothing loaded yet.
    pub fn new(loader: L, partitions: impl IntoIterator<Item = L::Partition>) -> Self {
        Self {
            loader,
            nodes: FrozenMap::default(),
            partitions: partitions
                .into_iter()
                .map(|partition| (partition, OnceCell::new()))
                .collect(),
        }
    }

    pub const fn loader(&self) -> &L {
        &self.loader
    }

    /// The node with `key`, or `None` if its partition has no such node.
    ///
    /// # Errors
    ///
    /// Returns the loader's error if the node's partition fails to load.
    pub async fn get(&self, key: &L::Key) -> Result<Option<&L::Node>, L::Error> {
        self.load(L::partition_of(key)).await?;
        Ok(self.nodes.get(key).map(|slot| &slot.node))
    }

    /// The nodes of `partition`, in the order the loader returned them.
    ///
    /// # Errors
    ///
    /// Returns the loader's error if the partition fails to load.
    pub async fn nodes_in(
        &self,
        partition: L::Partition,
    ) -> Result<impl Iterator<Item = &L::Node>, L::Error> {
        let keys = self.load(partition).await?;
        Ok(keys.iter().map(|key| self.loaded(key)))
    }

    /// The nodes that the node with `key` has edges to, each once, in the order
    /// the loader resolved them. A key with no node has no edges.
    ///
    /// # Errors
    ///
    /// Returns the loader's error if a partition fails to load or the edges fail
    /// to resolve.
    pub async fn neighbors(
        &self,
        key: &L::Key,
    ) -> Result<impl Iterator<Item = &L::Node>, L::Error> {
        self.load(L::partition_of(key)).await?;
        let edges: &[L::Key] = match self.nodes.get(key) {
            Some(slot) => slot.edges.get_or_try_init(|| self.resolve_edges(slot)).await?,
            None => &[],
        };
        Ok(edges.iter().map(|key| self.loaded(key)))
    }

    async fn load(&self, partition: L::Partition) -> Result<&[L::Key], L::Error> {
        let cell =
            self.partitions.get(&partition).expect("partitions are all passed to `LazyGraph::new`");
        let keys = cell
            .get_or_try_init(|| async {
                let loaded = self.loader.load(partition).await?;
                let mut keys = Vec::with_capacity(loaded.len());
                for (node, edge_spec) in loaded {
                    let key = node.key().clone();
                    keys.push(key.clone());
                    self.nodes
                        .insert(key, Box::new(Slot { node, edge_spec, edges: OnceCell::new() }));
                }
                Ok(keys.into_boxed_slice())
            })
            .await?;
        Ok(keys)
    }

    async fn resolve_edges(&self, slot: &Slot<L>) -> Result<Box<[L::Key]>, L::Error> {
        let keys = self.loader.resolve_edges(&slot.node, &slot.edge_spec, Lookup(self)).await?;
        let mut seen = FxHashSet::<L::Key>::default();
        let mut edges = Vec::with_capacity(keys.len());
        for key in keys {
            if seen.contains(&key) {
                continue;
            }
            // Loads the target's partition, so `loaded` can read it later.
            let found = self.get(&key).await?.is_some();
            assert!(found, "`Loader::resolve_edges` returned the key of a node that doesn't exist");
            seen.insert(key.clone());
            edges.push(key);
        }
        Ok(edges.into_boxed_slice())
    }

    /// The node with `key`, whose partition has loaded.
    fn loaded(&self, key: &L::Key) -> &L::Node {
        &self.nodes.get(key).expect("keys handed out by the graph belong to loaded nodes").node
    }
}

/// What [`Loader::resolve_edges`] can read: nodes, but not edges.
pub struct Lookup<'g, L: Loader>(&'g LazyGraph<L>);

impl<L: Loader> Clone for Lookup<'_, L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L: Loader> Copy for Lookup<'_, L> {}

impl<'g, L: Loader> Lookup<'g, L> {
    /// See [`LazyGraph::get`].
    ///
    /// # Errors
    ///
    /// Returns the loader's error if the node's partition fails to load.
    pub async fn get(self, key: &L::Key) -> Result<Option<&'g L::Node>, L::Error> {
        self.0.get(key).await
    }

    /// See [`LazyGraph::nodes_in`].
    ///
    /// # Errors
    ///
    /// Returns the loader's error if the partition fails to load.
    pub async fn nodes_in(
        self,
        partition: L::Partition,
    ) -> Result<impl Iterator<Item = &'g L::Node>, L::Error> {
        self.0.nodes_in(partition).await
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use vt_str::Str;

    use super::*;

    type Key = (u32, &'static str);

    #[derive(Debug)]
    struct Node(Key);

    impl Keyed for Node {
        type Key = Key;

        fn key(&self) -> &Key {
            &self.0
        }
    }

    /// Serves fixed partitions and records every load.
    #[derive(Default)]
    struct FakeLoader {
        partitions: FxHashMap<u32, Vec<(&'static str, Vec<Key>)>>,
        failing: RefCell<FxHashSet<u32>>,
        loads: RefCell<Vec<u32>>,
    }

    impl FakeLoader {
        fn with(mut self, partition: u32, nodes: Vec<(&'static str, Vec<Key>)>) -> Self {
            self.partitions.insert(partition, nodes);
            self
        }

        fn graph(self) -> LazyGraph<Self> {
            let partitions: Vec<u32> = self.partitions.keys().copied().collect();
            LazyGraph::new(self, partitions)
        }
    }

    #[async_trait::async_trait(?Send)]
    impl Loader for FakeLoader {
        type EdgeSpec = Vec<Key>;
        type Error = Str;
        type Key = Key;
        type Node = Node;
        type Partition = u32;

        fn partition_of(key: &Key) -> u32 {
            key.0
        }

        async fn load(&self, partition: u32) -> Result<Vec<(Node, Vec<Key>)>, Str> {
            self.loads.borrow_mut().push(partition);
            if self.failing.borrow().contains(&partition) {
                return Err(vt_str::format!("partition {partition} failed"));
            }
            Ok(self.partitions[&partition]
                .iter()
                .map(|(name, edges)| (Node((partition, name)), edges.clone()))
                .collect())
        }

        async fn resolve_edges(
            &self,
            _from: &Node,
            spec: &Vec<Key>,
            graph: Lookup<'_, Self>,
        ) -> Result<Vec<Key>, Str> {
            let mut keys = Vec::new();
            for key in spec {
                if graph.get(key).await?.is_some() {
                    keys.push(*key);
                }
            }
            Ok(keys)
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(future)
    }

    fn keys<'a>(nodes: impl Iterator<Item = &'a Node>) -> Vec<Key> {
        nodes.map(|node| node.0).collect()
    }

    #[test]
    fn get_loads_only_the_partition_of_the_key() {
        let graph = FakeLoader::default()
            .with(0, vec![("a", vec![]), ("b", vec![])])
            .with(1, vec![("c", vec![])])
            .graph();
        block_on(async {
            assert_eq!(graph.get(&(0, "a")).await.unwrap().unwrap().0, (0, "a"));
            assert!(graph.get(&(0, "missing")).await.unwrap().is_none());
            assert_eq!(graph.get(&(0, "b")).await.unwrap().unwrap().0, (0, "b"));
        });
        assert_eq!(*graph.loader().loads.borrow(), [0]);
    }

    #[test]
    fn nodes_in_keeps_the_loader_order() {
        let graph = FakeLoader::default()
            .with(0, vec![("b", vec![]), ("a", vec![]), ("c", vec![])])
            .graph();
        let nodes = block_on(async { keys(graph.nodes_in(0).await.unwrap()) });
        assert_eq!(nodes, [(0, "b"), (0, "a"), (0, "c")]);
    }

    #[test]
    fn neighbors_load_only_the_partitions_they_point_to() {
        let graph = FakeLoader::default()
            .with(0, vec![("a", vec![(1, "b"), (1, "missing")])])
            .with(1, vec![("b", vec![])])
            .with(2, vec![("c", vec![])])
            .graph();
        block_on(async {
            assert_eq!(keys(graph.neighbors(&(0, "a")).await.unwrap()), [(1, "b")]);
            // Edges resolve once.
            assert_eq!(keys(graph.neighbors(&(0, "a")).await.unwrap()), [(1, "b")]);
        });
        assert_eq!(*graph.loader().loads.borrow(), [0, 1]);
    }

    #[test]
    fn neighbors_keep_each_target_once_in_resolved_order() {
        let graph = FakeLoader::default()
            .with(0, vec![("a", vec![(0, "c"), (0, "b"), (0, "c")]), ("b", vec![]), ("c", vec![])])
            .graph();
        let neighbors = block_on(async { keys(graph.neighbors(&(0, "a")).await.unwrap()) });
        assert_eq!(neighbors, [(0, "c"), (0, "b")]);
    }

    #[test]
    fn neighbors_of_a_missing_node_are_empty() {
        let graph = FakeLoader::default().with(0, vec![]).graph();
        let neighbors = block_on(async { keys(graph.neighbors(&(0, "missing")).await.unwrap()) });
        assert!(neighbors.is_empty());
    }

    #[test]
    fn edges_that_cycle_across_partitions_resolve() {
        let graph = FakeLoader::default()
            .with(0, vec![("a", vec![(1, "b")])])
            .with(1, vec![("b", vec![(0, "a")])])
            .graph();
        block_on(async {
            assert_eq!(keys(graph.neighbors(&(0, "a")).await.unwrap()), [(1, "b")]);
            assert_eq!(keys(graph.neighbors(&(1, "b")).await.unwrap()), [(0, "a")]);
        });
        assert_eq!(*graph.loader().loads.borrow(), [0, 1]);
    }

    #[test]
    fn a_failed_load_is_retried_on_the_next_read() {
        let graph = FakeLoader::default().with(0, vec![("a", vec![])]).graph();
        graph.loader().failing.borrow_mut().insert(0);
        block_on(async {
            assert_eq!(graph.get(&(0, "a")).await.unwrap_err(), "partition 0 failed");
            graph.loader().failing.borrow_mut().clear();
            assert!(graph.get(&(0, "a")).await.unwrap().is_some());
        });
        assert_eq!(*graph.loader().loads.borrow(), [0, 0]);
    }

    #[test]
    fn a_failed_target_load_fails_the_edges() {
        let graph = FakeLoader::default()
            .with(0, vec![("a", vec![(1, "b")])])
            .with(1, vec![("b", vec![])])
            .graph();
        graph.loader().failing.borrow_mut().insert(1);
        block_on(async {
            assert_eq!(graph.neighbors(&(0, "a")).await.err().unwrap(), "partition 1 failed");
        });
    }
}
