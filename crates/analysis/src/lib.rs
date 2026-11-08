//! Change impact analysis for NEXUS ZERO.
//!
//! Given a [`Graph`] and a [`ProposedChange`], [`analyze_change`] lists the
//! resources that may be affected, each with the chain of recorded
//! relationships that explains why.
//!
//! Design rules:
//! - This is static analysis of recorded relationships. It is not a
//!   simulation and it does not predict real-world behavior.
//! - There is deliberately no numeric risk score. Each affected resource is
//!   labeled by how it was reached: through observed relationships only, or
//!   through at least one inferred relationship (with its coarse confidence).
//! - Every hop in the explanation carries its provenance, so the report can
//!   be checked against the sources it came from.

use std::fmt;

use nexus_domain::{Basis, Confidence, Edge, NodeId, NodeKind};
use nexus_graph::{Graph, GraphError};

/// What is proposed to happen to the target resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// The resource is deleted or decommissioned.
    Removal,
    /// The resource becomes unavailable (outage, failed deploy, rollback gap).
    Outage,
    /// The resource's configuration or interface is modified.
    Modification,
}

impl ChangeKind {
    fn label(self) -> &'static str {
        match self {
            ChangeKind::Removal => "removal",
            ChangeKind::Outage => "outage",
            ChangeKind::Modification => "modification",
        }
    }
}

/// A change someone is considering, expressed against one resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedChange {
    pub target: NodeId,
    pub kind: ChangeKind,
    /// Free-text context from the person proposing the change. May be empty.
    pub description: String,
}

/// How an affected resource was reached from the changed resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Certainty {
    /// Every relationship on the chain was observed.
    ObservedChain,
    /// At least one relationship on the chain is inferred. `weakest` is the
    /// lowest confidence among the inferred relationships.
    InferredChain { weakest: Confidence },
}

/// A resource that may be affected, with the evidence chain explaining why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffectedResource {
    pub id: NodeId,
    pub kind: NodeKind,
    pub name: String,
    pub certainty: Certainty,
    /// Relationships from the changed resource outward, in order.
    pub path: Vec<Edge>,
}

/// Statements that apply to every report. They are printed with it so the
/// reader never has to guess what the analysis does not cover.
pub const LIMITATIONS: [&str; 3] = [
    "Static analysis of recorded relationships only; this is not a simulation.",
    "Dependencies that were never discovered are not represented, so an empty or short list does not prove safety.",
    "A listed resource may still keep working if it has redundancy or fallbacks that the graph does not record.",
];

/// The result of analysing one proposed change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactReport {
    pub change: ProposedChange,
    pub target_kind: NodeKind,
    pub target_name: String,
    /// Closest resources first (breadth-first order).
    pub affected: Vec<AffectedResource>,
}

impl ImpactReport {
    /// Number of affected resources reached through observed relationships only.
    pub fn observed_chain_count(&self) -> usize {
        self.affected
            .iter()
            .filter(|a| a.certainty == Certainty::ObservedChain)
            .count()
    }

    /// Number of affected resources whose chain includes an inferred relationship.
    pub fn inferred_chain_count(&self) -> usize {
        self.affected.len() - self.observed_chain_count()
    }

    /// Render the report as Markdown.
    pub fn to_markdown(&self) -> String {
        self.to_string()
    }
}

/// Analyse a proposed change against a graph.
///
/// Returns [`GraphError::UnknownNode`] if the target is not in the graph.
pub fn analyze_change(graph: &Graph, change: &ProposedChange) -> Result<ImpactReport, GraphError> {
    let target = graph
        .node(&change.target)
        .ok_or_else(|| GraphError::UnknownNode(change.target.clone()))?;
    let paths = graph.impacted_by(&change.target)?;

    let mut affected = Vec::with_capacity(paths.len());
    for path in paths {
        let certainty = match path.weakest_confidence() {
            Some(weakest) => Certainty::InferredChain { weakest },
            None => Certainty::ObservedChain,
        };
        let node = graph
            .node(&path.affected)
            .ok_or_else(|| GraphError::UnknownNode(path.affected.clone()))?;
        affected.push(AffectedResource {
            id: path.affected,
            kind: node.kind,
            name: node.name.clone(),
            certainty,
            path: path.edges,
        });
    }

    Ok(ImpactReport {
        change: change.clone(),
        target_kind: target.kind,
        target_name: target.name.clone(),
        affected,
    })
}

fn write_hop(f: &mut fmt::Formatter<'_>, n: usize, edge: &Edge) -> fmt::Result {
    let basis = match edge.basis() {
        Basis::Observed => "observed".to_string(),
        Basis::Inferred {
            rationale,
            confidence,
        } => format!("inferred, {confidence:?} confidence: {rationale}"),
    };
    let provenance = edge.provenance();
    let evidence = if provenance.evidence().is_empty() {
        "none recorded".to_string()
    } else {
        provenance.evidence().join(", ")
    };
    writeln!(
        f,
        "  {}. `{}` relies on `{}` ({:?} edge `{}` -> `{}`; {}; source: {}, method: {:?}, observed at unix {}; evidence: {})",
        n,
        edge.dependent(),
        edge.dependency(),
        edge.kind(),
        edge.from(),
        edge.to(),
        basis,
        provenance.source(),
        provenance.method(),
        provenance.observed_at().as_unix_secs(),
        evidence,
    )
}

impl fmt::Display for ImpactReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "# Change impact report")?;
        writeln!(f)?;
        writeln!(
            f,
            "- Change: {} of `{}` ({}, {:?})",
            self.change.kind.label(),
            self.change.target,
            self.target_name,
            self.target_kind,
        )?;
        if !self.change.description.trim().is_empty() {
            writeln!(f, "- Context: {}", self.change.description.trim())?;
        }
        writeln!(
            f,
            "- Method: traversal of recorded dependency relationships (static analysis)"
        )?;
        writeln!(f)?;

        if self.affected.is_empty() {
            writeln!(
                f,
                "No resource depending on `{}` was found in this graph. This does not prove that nothing depends on it; only recorded relationships are considered.",
                self.change.target,
            )?;
        } else {
            writeln!(
                f,
                "{} resource(s) may be affected: {} through observed relationships only, {} through at least one inferred relationship.",
                self.affected.len(),
                self.observed_chain_count(),
                self.inferred_chain_count(),
            )?;
            for (i, a) in self.affected.iter().enumerate() {
                writeln!(f)?;
                writeln!(f, "## {}. `{}` ({}, {:?})", i + 1, a.id, a.name, a.kind)?;
                match a.certainty {
                    Certainty::ObservedChain => {
                        writeln!(f, "Reached through observed relationships only.")?;
                    }
                    Certainty::InferredChain { weakest } => {
                        writeln!(
                            f,
                            "Reached through at least one inferred relationship; weakest confidence: {weakest:?}."
                        )?;
                    }
                }
                for (n, edge) in a.path.iter().enumerate() {
                    write_hop(f, n + 1, edge)?;
                }
            }
        }

        writeln!(f)?;
        writeln!(f, "## Limitations")?;
        for limitation in LIMITATIONS {
            writeln!(f, "- {limitation}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_domain::{DiscoveryMethod, EdgeKind, Node, Provenance, Timestamp};

    fn id(s: &str) -> NodeId {
        NodeId::new(s).unwrap()
    }

    fn prov(evidence: &[&str]) -> Provenance {
        Provenance::new(
            "unit-test",
            DiscoveryMethod::ProviderApi,
            Timestamp::from_unix_secs(42),
            evidence.iter().map(|s| s.to_string()).collect(),
        )
        .unwrap()
    }

    fn node(s: &str, kind: NodeKind) -> Node {
        Node::new(id(s), kind, s)
    }

    /// web -> api (inferred, Medium) and api -> db (observed).
    fn chain() -> Graph {
        let mut g = Graph::new();
        g.add_node(node("web", NodeKind::Service)).unwrap();
        g.add_node(node("api", NodeKind::Service)).unwrap();
        g.add_node(node("db", NodeKind::Database)).unwrap();
        g.add_edge(Edge::observed(
            id("api"),
            id("db"),
            EdgeKind::DependsOn,
            prov(&["https://example.invalid/api/config"]),
        ))
        .unwrap();
        g.add_edge(
            Edge::inferred(
                id("web"),
                id("api"),
                EdgeKind::DependsOn,
                "web config references api hostname",
                Confidence::Medium,
                prov(&[]),
            )
            .unwrap(),
        )
        .unwrap();
        g
    }

    fn change(target: &str, kind: ChangeKind) -> ProposedChange {
        ProposedChange {
            target: id(target),
            kind,
            description: String::new(),
        }
    }

    #[test]
    fn labels_each_affected_resource_by_how_it_was_reached() {
        let report = analyze_change(&chain(), &change("db", ChangeKind::Removal)).unwrap();
        assert_eq!(report.affected.len(), 2);

        let api = &report.affected[0];
        assert_eq!(api.id, id("api"));
        assert_eq!(api.certainty, Certainty::ObservedChain);
        assert_eq!(api.path.len(), 1);

        let web = &report.affected[1];
        assert_eq!(web.id, id("web"));
        assert_eq!(
            web.certainty,
            Certainty::InferredChain {
                weakest: Confidence::Medium
            }
        );
        assert_eq!(web.path.len(), 2);

        assert_eq!(report.observed_chain_count(), 1);
        assert_eq!(report.inferred_chain_count(), 1);
    }

    #[test]
    fn unknown_target_is_an_error() {
        let r = analyze_change(&chain(), &change("missing", ChangeKind::Outage));
        assert_eq!(r, Err(GraphError::UnknownNode(id("missing"))));
    }

    #[test]
    fn nothing_depending_on_target_gives_an_empty_report_with_a_caveat() {
        let report = analyze_change(&chain(), &change("web", ChangeKind::Modification)).unwrap();
        assert!(report.affected.is_empty());
        let md = report.to_markdown();
        assert!(md.contains("No resource depending on `web` was found"));
        assert!(md.contains("does not prove"));
    }

    #[test]
    fn markdown_shows_evidence_provenance_and_inferred_rationale() {
        let mut c = change("db", ChangeKind::Outage);
        c.description = "  planned maintenance window  ".to_string();
        let md = analyze_change(&chain(), &c).unwrap().to_markdown();

        assert!(md.contains("- Change: outage of `db`"));
        assert!(md.contains("- Context: planned maintenance window"));
        assert!(md.contains("2 resource(s) may be affected: 1 through observed"));
        assert!(md.contains("https://example.invalid/api/config"));
        assert!(md.contains("source: unit-test, method: ProviderApi, observed at unix 42"));
        assert!(md.contains("inferred, Medium confidence: web config references api hostname"));
        assert!(md.contains("evidence: none recorded"));
        assert!(md.contains("weakest confidence: Medium"));
    }

    #[test]
    fn markdown_always_ends_with_the_limitations() {
        let md = analyze_change(&chain(), &change("db", ChangeKind::Removal))
            .unwrap()
            .to_markdown();
        assert!(md.contains("## Limitations"));
        for limitation in LIMITATIONS {
            assert!(md.contains(limitation));
        }
    }

    #[test]
    fn exposes_hops_are_described_from_the_dependent_side() {
        let mut g = Graph::new();
        g.add_node(node("svc", NodeKind::Service)).unwrap();
        g.add_node(node("example.com", NodeKind::Domain)).unwrap();
        g.add_edge(Edge::observed(
            id("svc"),
            id("example.com"),
            EdgeKind::Exposes,
            prov(&[]),
        ))
        .unwrap();

        let report = analyze_change(&g, &change("svc", ChangeKind::Outage)).unwrap();
        assert_eq!(report.affected.len(), 1);
        let md = report.to_markdown();
        assert!(md.contains("`example.com` relies on `svc` (Exposes edge `svc` -> `example.com`"));
    }
}
