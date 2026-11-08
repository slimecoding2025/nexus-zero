//! Domain model for NEXUS ZERO.
//!
//! Design rules encoded in the types:
//! - Every edge carries [`Provenance`] (source, method, timestamp, evidence).
//! - An edge is either [`Basis::Observed`] or [`Basis::Inferred`]; inferred
//!   edges must carry a rationale and a coarse [`Confidence`]. There is
//!   deliberately no numeric risk or confidence score.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

/// Errors raised when constructing domain values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    /// An identifier was empty or whitespace only.
    EmptyId,
    /// A provenance source was empty or whitespace only.
    EmptySource,
    /// An inferred edge was created without a rationale.
    EmptyRationale,
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DomainError::EmptyId => write!(f, "identifier must not be empty"),
            DomainError::EmptySource => write!(f, "provenance source must not be empty"),
            DomainError::EmptyRationale => {
                write!(f, "an inferred edge requires a non-empty rationale")
            }
        }
    }
}

impl Error for DomainError {}

/// Stable identifier of a resource, e.g. `github:repo:org/name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(String);

impl NodeId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainError::EmptyId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Seconds since the Unix epoch (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(i64);

impl Timestamp {
    pub fn from_unix_secs(secs: i64) -> Self {
        Self(secs)
    }

    pub fn as_unix_secs(self) -> i64 {
        self.0
    }
}

/// Kinds of resources (initial set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeKind {
    Repository,
    Deployment,
    Service,
    Database,
    Api,
    Domain,
    Environment,
    Worker,
    Storage,
    IdentityProvider,
    NetworkResource,
}

/// Kinds of relationships (initial set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EdgeKind {
    DependsOn,
    DeployedTo,
    ConnectsTo,
    Exposes,
    AuthenticatesWith,
    StoresDataIn,
    BuiltFrom,
    CommunicatesWith,
}

/// Which end of an edge depends on the other. Used for impact traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyDirection {
    /// `from` depends on `to` (a failure of `to` can affect `from`).
    FromDependsOnTo,
    /// `to` depends on `from` (a failure of `from` can affect `to`).
    ToDependsOnFrom,
}

impl EdgeKind {
    /// Initial modeling assumption, to be reviewed in a decision record:
    /// every kind means "from depends on to" except `Exposes`, where the
    /// exposed resource (`to`) depends on the exposing resource (`from`).
    pub fn dependency_direction(self) -> DependencyDirection {
        match self {
            EdgeKind::Exposes => DependencyDirection::ToDependsOnFrom,
            _ => DependencyDirection::FromDependsOnTo,
        }
    }
}

/// How a relationship was discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryMethod {
    /// Read directly from a provider API response.
    ProviderApi,
    /// Parsed from a manifest or configuration file.
    ManifestParse,
    /// Derived by a heuristic (for example name matching).
    Heuristic,
    /// Entered by a person.
    Manual,
}

/// Where and when a fact came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    source: String,
    method: DiscoveryMethod,
    observed_at: Timestamp,
    evidence: Vec<String>,
}

impl Provenance {
    pub fn new(
        source: impl Into<String>,
        method: DiscoveryMethod,
        observed_at: Timestamp,
        evidence: Vec<String>,
    ) -> Result<Self, DomainError> {
        let source = source.into();
        if source.trim().is_empty() {
            return Err(DomainError::EmptySource);
        }
        Ok(Self {
            source,
            method,
            observed_at,
            evidence,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn method(&self) -> DiscoveryMethod {
        self.method
    }

    pub fn observed_at(&self) -> Timestamp {
        self.observed_at
    }

    /// References (URLs, file paths, API response ids) supporting the fact.
    pub fn evidence(&self) -> &[String] {
        &self.evidence
    }
}

/// Coarse confidence for inferred edges. Intentionally not numeric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

/// Whether an edge was observed or inferred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Basis {
    Observed,
    Inferred {
        rationale: String,
        confidence: Confidence,
    },
}

/// A typed resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub name: String,
    pub attributes: BTreeMap<String, String>,
}

impl Node {
    pub fn new(id: NodeId, kind: NodeKind, name: impl Into<String>) -> Self {
        Self {
            id,
            kind,
            name: name.into(),
            attributes: BTreeMap::new(),
        }
    }
}

/// Identity of an edge: at most one edge per (from, to, kind) in a graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EdgeKey {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
}

/// A typed relationship with provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    key: EdgeKey,
    basis: Basis,
    provenance: Provenance,
    pub metadata: BTreeMap<String, String>,
}

impl Edge {
    /// An edge directly observed from a source.
    pub fn observed(from: NodeId, to: NodeId, kind: EdgeKind, provenance: Provenance) -> Self {
        Self {
            key: EdgeKey { from, to, kind },
            basis: Basis::Observed,
            provenance,
            metadata: BTreeMap::new(),
        }
    }

    /// An edge inferred from other evidence. A rationale is required.
    pub fn inferred(
        from: NodeId,
        to: NodeId,
        kind: EdgeKind,
        rationale: impl Into<String>,
        confidence: Confidence,
        provenance: Provenance,
    ) -> Result<Self, DomainError> {
        let rationale = rationale.into();
        if rationale.trim().is_empty() {
            return Err(DomainError::EmptyRationale);
        }
        Ok(Self {
            key: EdgeKey { from, to, kind },
            basis: Basis::Inferred {
                rationale,
                confidence,
            },
            provenance,
            metadata: BTreeMap::new(),
        })
    }

    pub fn key(&self) -> &EdgeKey {
        &self.key
    }

    pub fn from(&self) -> &NodeId {
        &self.key.from
    }

    pub fn to(&self) -> &NodeId {
        &self.key.to
    }

    pub fn kind(&self) -> EdgeKind {
        self.key.kind
    }

    pub fn basis(&self) -> &Basis {
        &self.basis
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    pub fn is_inferred(&self) -> bool {
        matches!(self.basis, Basis::Inferred { .. })
    }

    /// The end of the edge that depends on the other end.
    pub fn dependent(&self) -> &NodeId {
        match self.key.kind.dependency_direction() {
            DependencyDirection::FromDependsOnTo => &self.key.from,
            DependencyDirection::ToDependsOnFrom => &self.key.to,
        }
    }

    /// The end of the edge that the other end depends on.
    pub fn dependency(&self) -> &NodeId {
        match self.key.kind.dependency_direction() {
            DependencyDirection::FromDependsOnTo => &self.key.to,
            DependencyDirection::ToDependsOnFrom => &self.key.from,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> NodeId {
        NodeId::new(s).unwrap()
    }

    fn prov() -> Provenance {
        Provenance::new(
            "test",
            DiscoveryMethod::ProviderApi,
            Timestamp::from_unix_secs(1),
            vec!["ref".into()],
        )
        .unwrap()
    }

    #[test]
    fn empty_ids_are_rejected() {
        assert_eq!(NodeId::new(""), Err(DomainError::EmptyId));
        assert_eq!(NodeId::new("   "), Err(DomainError::EmptyId));
        assert!(NodeId::new("github:repo:org/a").is_ok());
    }

    #[test]
    fn empty_provenance_source_is_rejected() {
        let r = Provenance::new(
            " ",
            DiscoveryMethod::Manual,
            Timestamp::from_unix_secs(0),
            vec![],
        );
        assert_eq!(r, Err(DomainError::EmptySource));
    }

    #[test]
    fn inferred_edge_requires_rationale() {
        let r = Edge::inferred(
            id("a"),
            id("b"),
            EdgeKind::DependsOn,
            "  ",
            Confidence::Low,
            prov(),
        );
        assert_eq!(r, Err(DomainError::EmptyRationale));
    }

    #[test]
    fn observed_and_inferred_are_distinguishable() {
        let o = Edge::observed(id("a"), id("b"), EdgeKind::DependsOn, prov());
        let i = Edge::inferred(
            id("a"),
            id("b"),
            EdgeKind::ConnectsTo,
            "env var name matches",
            Confidence::Medium,
            prov(),
        )
        .unwrap();
        assert!(!o.is_inferred());
        assert!(i.is_inferred());
    }

    #[test]
    fn dependency_direction_is_reversed_only_for_exposes() {
        let dep = Edge::observed(id("web"), id("db"), EdgeKind::DependsOn, prov());
        assert_eq!(dep.dependent(), &id("web"));
        assert_eq!(dep.dependency(), &id("db"));

        let exp = Edge::observed(id("svc"), id("domain"), EdgeKind::Exposes, prov());
        assert_eq!(exp.dependent(), &id("domain"));
        assert_eq!(exp.dependency(), &id("svc"));
    }

    #[test]
    fn confidence_is_ordered() {
        assert!(Confidence::Low < Confidence::Medium);
        assert!(Confidence::Medium < Confidence::High);
    }
}
