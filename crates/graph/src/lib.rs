//! Infrastructure graph: storage, traversal, snapshots and diff.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use nexus_domain::{Basis, Confidence, Edge, EdgeKey, Node, NodeId, Timestamp};

/// Errors from graph operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    DuplicateNode(NodeId),
    UnknownNode(NodeId),
    SelfLoop(NodeId),
    DuplicateEdge(EdgeKey),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::DuplicateNode(id) => write!(f, "node already exists: {id}"),
            GraphError::UnknownNode(id) => write!(f, "unknown node: {id}"),
            GraphError::SelfLoop(id) => write!(f, "self-loop not allowed on node: {id}"),
            GraphError::DuplicateEdge(k) => {
                write!(
                    f,
                    "edge already exists: {} -> {} ({:?})",
                    k.from, k.to, k.kind
                )
            }
        }
    }
}

impl Error for GraphError {}

/// A mutable graph under construction. Freeze it into a [`Snapshot`] to
/// compare it with another point in time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    nodes: BTreeMap<NodeId, Node>,
    edges: BTreeMap<EdgeKey, Edge>,
}

/// A chain of edges explaining why a resource is affected by a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactPath {
    /// The affected resource.
    pub affected: NodeId,
    /// Edges from the changed resource outward, in order.
    pub edges: Vec<Edge>,
}

impl ImpactPath {
    pub fn hops(&self) -> usize {
        self.edges.len()
    }

    pub fn has_inferred_edge(&self) -> bool {
        self.edges.iter().any(Edge::is_inferred)
    }

    /// Lowest confidence among inferred edges, or `None` if every edge on
    /// the path was observed.
    pub fn weakest_confidence(&self) -> Option<Confidence> {
        self.edges
            .iter()
            .filter_map(|e| match e.basis() {
                Basis::Inferred { confidence, .. } => Some(*confidence),
                Basis::Observed => None,
            })
            .min()
    }
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, node: Node) -> Result<(), GraphError> {
        if self.nodes.contains_key(&node.id) {
            return Err(GraphError::DuplicateNode(node.id));
        }
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    pub fn add_edge(&mut self, edge: Edge) -> Result<(), GraphError> {
        if edge.from() == edge.to() {
            return Err(GraphError::SelfLoop(edge.from().clone()));
        }
        for end in [edge.from(), edge.to()] {
            if !self.nodes.contains_key(end) {
                return Err(GraphError::UnknownNode(end.clone()));
            }
        }
        if self.edges.contains_key(edge.key()) {
            return Err(GraphError::DuplicateEdge(edge.key().clone()));
        }
        self.edges.insert(edge.key().clone(), edge);
        Ok(())
    }

    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn edge(&self, key: &EdgeKey) -> Option<&Edge> {
        self.edges.get(key)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    pub fn edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.values()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Resources that would be affected if `root` changed or failed, each
    /// with the shortest chain of edges explaining why. Follows
    /// "depends on" relationships in reverse. Cycles are handled.
    pub fn impacted_by(&self, root: &NodeId) -> Result<Vec<ImpactPath>, GraphError> {
        if !self.nodes.contains_key(root) {
            return Err(GraphError::UnknownNode(root.clone()));
        }
        // dependency -> edges whose dependent side relies on it
        let mut by_dependency: BTreeMap<&NodeId, Vec<&Edge>> = BTreeMap::new();
        for e in self.edges.values() {
            by_dependency.entry(e.dependency()).or_default().push(e);
        }

        let mut visited: BTreeSet<&NodeId> = BTreeSet::new();
        visited.insert(root);
        let mut parent: BTreeMap<&NodeId, &Edge> = BTreeMap::new();
        let mut order: Vec<&NodeId> = Vec::new();
        let mut queue: VecDeque<&NodeId> = VecDeque::new();
        queue.push_back(root);

        while let Some(current) = queue.pop_front() {
            if let Some(edges) = by_dependency.get(current) {
                for e in edges {
                    let next = e.dependent();
                    if visited.insert(next) {
                        parent.insert(next, e);
                        order.push(next);
                        queue.push_back(next);
                    }
                }
            }
        }

        let mut out = Vec::with_capacity(order.len());
        for target in order {
            let mut chain: Vec<Edge> = Vec::new();
            let mut cursor = target;
            while cursor != root {
                let e = parent[cursor];
                chain.push(e.clone());
                cursor = e.dependency();
            }
            chain.reverse();
            out.push(ImpactPath {
                affected: target.clone(),
                edges: chain,
            });
        }
        Ok(out)
    }

    /// Shortest chain of edges by which `from` (transitively) depends on
    /// `to`, or `None` if there is no such chain.
    pub fn dependency_path(
        &self,
        from: &NodeId,
        to: &NodeId,
    ) -> Result<Option<Vec<Edge>>, GraphError> {
        for id in [from, to] {
            if !self.nodes.contains_key(id) {
                return Err(GraphError::UnknownNode(id.clone()));
            }
        }
        // `from` depends on `to` iff `from` is impacted by `to`.
        let impacted = self.impacted_by(to)?;
        Ok(impacted
            .into_iter()
            .find(|p| &p.affected == from)
            .map(|p| p.edges))
    }
}

/// Identifier of a snapshot, chosen by the caller (for example a scan id).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SnapshotId(pub String);

/// An immutable point-in-time copy of a graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    id: SnapshotId,
    taken_at: Timestamp,
    graph: Graph,
}

impl Snapshot {
    pub fn new(id: SnapshotId, taken_at: Timestamp, graph: Graph) -> Self {
        Self {
            id,
            taken_at,
            graph,
        }
    }

    pub fn id(&self) -> &SnapshotId {
        &self.id
    }

    pub fn taken_at(&self) -> Timestamp {
        self.taken_at
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }
}

/// Differences between two snapshots.
///
/// Node changes compare kind, name and attributes. Edge changes compare
/// basis (observed/inferred, rationale, confidence) and metadata; the
/// observation timestamp and evidence references are ignored on purpose,
/// because they change on every scan without the infrastructure changing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphDiff {
    pub added_nodes: Vec<NodeId>,
    pub removed_nodes: Vec<NodeId>,
    pub changed_nodes: Vec<NodeId>,
    pub added_edges: Vec<EdgeKey>,
    pub removed_edges: Vec<EdgeKey>,
    pub changed_edges: Vec<EdgeKey>,
}

impl GraphDiff {
    pub fn is_empty(&self) -> bool {
        self.added_nodes.is_empty()
            && self.removed_nodes.is_empty()
            && self.changed_nodes.is_empty()
            && self.added_edges.is_empty()
            && self.removed_edges.is_empty()
            && self.changed_edges.is_empty()
    }
}

/// Compare an older snapshot with a newer one.
pub fn diff(old: &Snapshot, new: &Snapshot) -> GraphDiff {
    let (a, b) = (old.graph(), new.graph());
    let mut d = GraphDiff::default();

    for (id, n) in &b.nodes {
        match a.nodes.get(id) {
            None => d.added_nodes.push(id.clone()),
            Some(old_n) if old_n != n => d.changed_nodes.push(id.clone()),
            Some(_) => {}
        }
    }
    for id in a.nodes.keys() {
        if !b.nodes.contains_key(id) {
            d.removed_nodes.push(id.clone());
        }
    }

    for (key, e) in &b.edges {
        match a.edges.get(key) {
            None => d.added_edges.push(key.clone()),
            Some(old_e) if old_e.basis() != e.basis() || old_e.metadata != e.metadata => {
                d.changed_edges.push(key.clone())
            }
            Some(_) => {}
        }
    }
    for key in a.edges.keys() {
        if !b.edges.contains_key(key) {
            d.removed_edges.push(key.clone());
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_domain::{DiscoveryMethod, EdgeKind, NodeKind, Provenance};

    fn id(s: &str) -> NodeId {
        NodeId::new(s).unwrap()
    }

    fn prov(at: i64) -> Provenance {
        Provenance::new(
            "test",
            DiscoveryMethod::ProviderApi,
            Timestamp::from_unix_secs(at),
            vec![],
        )
        .unwrap()
    }

    fn node(s: &str, kind: NodeKind) -> Node {
        Node::new(id(s), kind, s)
    }

    fn dep(from: &str, to: &str) -> Edge {
        Edge::observed(id(from), id(to), EdgeKind::DependsOn, prov(1))
    }

    /// web -> api -> db, where web -> api is inferred (Medium).
    fn chain() -> Graph {
        let mut g = Graph::new();
        g.add_node(node("web", NodeKind::Service)).unwrap();
        g.add_node(node("api", NodeKind::Service)).unwrap();
        g.add_node(node("db", NodeKind::Database)).unwrap();
        g.add_edge(dep("api", "db")).unwrap();
        g.add_edge(
            Edge::inferred(
                id("web"),
                id("api"),
                EdgeKind::DependsOn,
                "web config references api hostname",
                Confidence::Medium,
                prov(1),
            )
            .unwrap(),
        )
        .unwrap();
        g
    }

    #[test]
    fn rejects_duplicate_nodes() {
        let mut g = Graph::new();
        g.add_node(node("a", NodeKind::Service)).unwrap();
        assert_eq!(
            g.add_node(node("a", NodeKind::Service)),
            Err(GraphError::DuplicateNode(id("a")))
        );
    }

    #[test]
    fn rejects_edges_with_unknown_nodes_self_loops_and_duplicates() {
        let mut g = Graph::new();
        g.add_node(node("a", NodeKind::Service)).unwrap();
        g.add_node(node("b", NodeKind::Service)).unwrap();
        assert_eq!(
            g.add_edge(dep("a", "zzz")),
            Err(GraphError::UnknownNode(id("zzz")))
        );
        assert_eq!(
            g.add_edge(dep("a", "a")),
            Err(GraphError::SelfLoop(id("a")))
        );
        g.add_edge(dep("a", "b")).unwrap();
        assert!(matches!(
            g.add_edge(dep("a", "b")),
            Err(GraphError::DuplicateEdge(_))
        ));
        assert_eq!(g.edge_count(), 1);
    }

    #[test]
    fn impact_follows_dependencies_in_reverse_with_evidence_chain() {
        let g = chain();
        let impact = g.impacted_by(&id("db")).unwrap();
        assert_eq!(impact.len(), 2);

        let api = &impact[0];
        assert_eq!(api.affected, id("api"));
        assert_eq!(api.hops(), 1);
        assert!(!api.has_inferred_edge());
        assert_eq!(api.weakest_confidence(), None);

        let web = &impact[1];
        assert_eq!(web.affected, id("web"));
        assert_eq!(web.hops(), 2);
        assert!(web.has_inferred_edge());
        assert_eq!(web.weakest_confidence(), Some(Confidence::Medium));
        assert_eq!(web.edges[0].from(), &id("api"));
        assert_eq!(web.edges[1].from(), &id("web"));
    }

    #[test]
    fn impact_of_a_leaf_dependent_is_empty() {
        let g = chain();
        assert!(g.impacted_by(&id("web")).unwrap().is_empty());
    }

    #[test]
    fn impact_on_unknown_node_is_an_error() {
        let g = chain();
        assert_eq!(
            g.impacted_by(&id("nope")),
            Err(GraphError::UnknownNode(id("nope")))
        );
    }

    #[test]
    fn impact_terminates_on_cycles() {
        let mut g = Graph::new();
        g.add_node(node("a", NodeKind::Service)).unwrap();
        g.add_node(node("b", NodeKind::Service)).unwrap();
        g.add_edge(dep("a", "b")).unwrap();
        g.add_edge(dep("b", "a")).unwrap();
        let impact = g.impacted_by(&id("a")).unwrap();
        assert_eq!(impact.len(), 1);
        assert_eq!(impact[0].affected, id("b"));
    }

    #[test]
    fn exposes_edge_propagates_from_exposer_to_exposed() {
        let mut g = Graph::new();
        g.add_node(node("svc", NodeKind::Service)).unwrap();
        g.add_node(node("example.com", NodeKind::Domain)).unwrap();
        g.add_edge(Edge::observed(
            id("svc"),
            id("example.com"),
            EdgeKind::Exposes,
            prov(1),
        ))
        .unwrap();
        let impact = g.impacted_by(&id("svc")).unwrap();
        assert_eq!(impact.len(), 1);
        assert_eq!(impact[0].affected, id("example.com"));
    }

    #[test]
    fn dependency_path_found_and_absent() {
        let g = chain();
        let p = g.dependency_path(&id("web"), &id("db")).unwrap().unwrap();
        assert_eq!(p.len(), 2);
        assert!(g.dependency_path(&id("db"), &id("web")).unwrap().is_none());
    }

    #[test]
    fn identical_snapshots_have_empty_diff_even_if_timestamps_differ() {
        let mut g1 = Graph::new();
        g1.add_node(node("a", NodeKind::Service)).unwrap();
        g1.add_node(node("b", NodeKind::Service)).unwrap();
        g1.add_edge(Edge::observed(
            id("a"),
            id("b"),
            EdgeKind::DependsOn,
            prov(1),
        ))
        .unwrap();
        let mut g2 = g1.clone();
        // Rebuild g2's edge with a later observation time.
        g2.edges.clear();
        g2.add_edge(Edge::observed(
            id("a"),
            id("b"),
            EdgeKind::DependsOn,
            prov(999),
        ))
        .unwrap();
        let s1 = Snapshot::new(SnapshotId("s1".into()), Timestamp::from_unix_secs(1), g1);
        let s2 = Snapshot::new(SnapshotId("s2".into()), Timestamp::from_unix_secs(2), g2);
        assert!(diff(&s1, &s2).is_empty());
    }

    #[test]
    fn diff_reports_added_removed_and_changed() {
        let old_g = chain();

        let mut new_g = Graph::new();
        new_g.add_node(node("web", NodeKind::Service)).unwrap();
        new_g.add_node(node("api", NodeKind::Service)).unwrap();
        // db removed, cache added
        new_g.add_node(node("cache", NodeKind::Storage)).unwrap();
        // api -> cache added; web -> api now observed (changed basis)
        new_g.add_edge(dep("api", "cache")).unwrap();
        new_g.add_edge(dep("web", "api")).unwrap();

        let old = Snapshot::new(SnapshotId("a".into()), Timestamp::from_unix_secs(1), old_g);
        let new = Snapshot::new(SnapshotId("b".into()), Timestamp::from_unix_secs(2), new_g);
        let d = diff(&old, &new);

        assert_eq!(d.added_nodes, vec![id("cache")]);
        assert_eq!(d.removed_nodes, vec![id("db")]);
        assert!(d.changed_nodes.is_empty());
        assert_eq!(d.added_edges.len(), 1);
        assert_eq!(d.added_edges[0].to, id("cache"));
        assert_eq!(d.removed_edges.len(), 1);
        assert_eq!(d.removed_edges[0].to, id("db"));
        assert_eq!(d.changed_edges.len(), 1);
        assert_eq!(d.changed_edges[0].from, id("web"));
        assert!(!d.is_empty());
    }
}
