//! Relationship discovery from GitHub Actions workflow files.
//!
//! [`GitHubClient::discover_workflow_dependencies`] reads the top level of
//! `.github/workflows` in every repository of an inventory. For each `uses:`
//! line that names another GitHub repository (an action or a reusable
//! workflow) it records a `DependsOn` edge from the repository that uses it
//! to the repository it uses.
//!
//! ## What the edges mean
//!
//! The edge is *observed*: the workflow file says it, and the evidence
//! reference is `owner/name/.github/workflows/file.yml:LINE`. It records a
//! declared dependency, not proof that the workflow runs or succeeds.
//!
//! ## Limits of the parser
//!
//! [`parse_workflow_uses`] is a line scanner, not a YAML parser. It skips
//! block scalars (`run: |`) so shell text is not mistaken for a `uses:` key,
//! and it ignores local (`./`) and `docker://` references and any value that
//! is not a plain `owner/name[/path]@ref`. It can miss references written in
//! YAML flow style or built with expressions. A miss is a missing edge, never
//! an invented one, so an impact report built from these edges can be short
//! but not wrong about what it lists.
//!
//! ## Permissions
//!
//! Reading files needs read access to the repository **Contents**
//! permission, in addition to the Metadata permission of the inventory.
//! That is more access than the inventory alone, which is why this step is a
//! separate call that the caller opts into.

use std::collections::BTreeMap;

use nexus_domain::{
    DiscoveryMethod, Edge, EdgeKind, Node, NodeId, NodeKind, Provenance, Timestamp,
};
use nexus_graph::Graph;

use super::{
    classify, display_safe, parse_rate_limit, valid_full_name, Discovery, FetchError, GitHubClient,
    RateLimit, Response, SyncStatus, Transport, ACCEPT, API_VERSION,
};
use crate::json::{self, Json};

const WORKFLOW_DIR: &str = ".github/workflows";
/// Media type that returns a file's raw contents.
const RAW_ACCEPT: &str = "application/vnd.github.raw+json";
const EVIDENCE_SOURCE: &str = "github:workflow-files";

/// Default cap on workflow files read per repository.
pub const DEFAULT_MAX_FILES_PER_REPO: usize = 100;
/// Default cap on the size of one response body in a workflow scan.
pub const DEFAULT_MAX_WORKFLOW_BYTES: usize = 1024 * 1024;

// ---------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------

/// A reference from a workflow file to another repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRef {
    /// `owner/name` of the referenced repository, as written.
    pub repo: String,
    /// 1-based line number in the workflow file.
    pub line: usize,
    /// The whole `uses:` value, for example `actions/checkout@v4`.
    pub raw: String,
}

/// Find the `uses: owner/name[/path]@ref` references in a workflow file.
/// See the module documentation for what this does and does not understand.
pub fn parse_workflow_uses(content: &str) -> Vec<ActionRef> {
    let mut refs = Vec::new();
    // Indent of the key that opened a block scalar; deeper lines belong to it.
    let mut block_key_indent: Option<usize> = None;

    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();

        if let Some(key_indent) = block_key_indent {
            if trimmed.is_empty() || indent > key_indent {
                continue;
            }
            block_key_indent = None;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let body = trimmed.strip_prefix("- ").map_or(trimmed, str::trim_start);
        let key_indent = indent + (trimmed.len() - body.len());

        if let Some((_, value)) = body.split_once(':') {
            if is_block_scalar_indicator(value.trim()) {
                block_key_indent = Some(key_indent);
                continue;
            }
        }

        let Some(value) = body.strip_prefix("uses:") else {
            continue;
        };
        let Some(raw) = clean_uses_value(value) else {
            continue;
        };
        if let Some(repo) = repository_of(&raw) {
            refs.push(ActionRef {
                repo,
                line: index + 1,
                raw,
            });
        }
    }
    refs
}

fn is_block_scalar_indicator(value: &str) -> bool {
    let mut chars = value.chars();
    if !matches!(chars.next(), Some('|' | '>')) {
        return false;
    }
    let rest: String = chars.collect();
    let rest = rest
        .trim_start_matches(|c: char| matches!(c, '+' | '-') || c.is_ascii_digit())
        .trim();
    rest.is_empty() || rest.starts_with('#')
}

/// The value of a `uses:` key without a trailing comment or one pair of quotes.
fn clean_uses_value(value: &str) -> Option<String> {
    let mut v = value.trim();
    if let Some(pos) = v.find(" #").or_else(|| v.find("\t#")) {
        v = v[..pos].trim_end();
    }
    let quoted = v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')));
    let v = if quoted { &v[1..v.len() - 1] } else { v };
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

/// `owner/name` of a `owner/name[/path]@ref` reference, or `None` for local
/// paths, Docker images and anything with unexpected characters.
fn repository_of(raw: &str) -> Option<String> {
    if raw.starts_with("./") || raw.starts_with("docker://") {
        return None;
    }
    let safe = raw
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'@' | b'+' | b'-'));
    if !safe {
        return None;
    }
    let (target, reference) = raw.split_once('@')?;
    if reference.is_empty() || reference.contains('@') {
        return None;
    }
    let mut parts = target.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    let full = format!("{owner}/{name}");
    valid_full_name(&full).then_some(full)
}

fn is_workflow_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".yml") || lower.ends_with(".yaml")
}

/// A top-level file of `.github/workflows` with a plain name.
fn is_workflow_path(path: &str) -> bool {
    path.strip_prefix(".github/workflows/").is_some_and(|rest| {
        !rest.is_empty()
            && rest.len() <= 200
            && !rest.contains("..")
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            && is_workflow_name(rest)
    })
}

// ---------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------

/// Limits for one workflow scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowOptions {
    pub max_files_per_repo: usize,
    pub max_body_bytes: usize,
}

impl Default for WorkflowOptions {
    fn default() -> Self {
        Self {
            max_files_per_repo: DEFAULT_MAX_FILES_PER_REPO,
            max_body_bytes: DEFAULT_MAX_WORKFLOW_BYTES,
        }
    }
}

/// Something that could not be read or used, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSkip {
    /// A repository (`owner/name`) or a file (`owner/name/path`).
    pub scope: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowReport {
    /// `Incomplete` if the scan had to stop (rate limit, bad credentials).
    pub status: SyncStatus,
    /// Whether the inventory this scan started from was complete.
    pub inventory_complete: bool,
    /// Repositories whose workflow directory was read or found missing.
    pub repositories_scanned: usize,
    /// Repositories answered with 404 for `.github/workflows`. GitHub also
    /// answers 404 for repositories the token cannot see, so "has no
    /// workflows" and "not visible" cannot be told apart here.
    pub repositories_without_workflows: usize,
    pub workflow_files_read: usize,
    /// Repositories that are only referenced by a workflow, not listed in
    /// the inventory. They carry the attribute `referenced_only=true`.
    pub nodes_added: usize,
    pub edges_added: usize,
    pub skipped: Vec<WorkflowSkip>,
    /// Rate limit headers from the last response received, if any.
    pub last_rate_limit: Option<RateLimit>,
}

/// The inventory graph plus the relationships found in workflow files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowDiscovery {
    pub graph: Graph,
    pub report: WorkflowReport,
}

impl WorkflowDiscovery {
    /// The graph, but only if the inventory was complete, the scan was not
    /// stopped early and nothing had to be skipped. Use this, not `graph`,
    /// before taking a snapshot to diff: missing edges would look removed.
    pub fn complete_graph(&self) -> Option<&Graph> {
        let complete = self.report.inventory_complete
            && matches!(self.report.status, SyncStatus::Complete)
            && self.report.skipped.is_empty();
        complete.then_some(&self.graph)
    }
}

#[derive(Default)]
struct Found {
    /// (from, to) -> evidence references, one per `uses:` line.
    edges: BTreeMap<(NodeId, NodeId), Vec<String>>,
    /// Referenced repositories, with the name as first written.
    referenced: BTreeMap<NodeId, String>,
}

// ---------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------

impl<T: Transport> GitHubClient<T> {
    /// Read the workflow files of the repositories in `inventory` and add a
    /// `DependsOn` edge for each referenced repository. The inventory is not
    /// modified; the result holds a copy of its graph plus the new edges.
    ///
    /// Requests per repository: one directory listing, then one request per
    /// workflow file. A rate limit or rejected credentials stop the scan;
    /// any other failure skips that repository or file and is reported.
    pub fn discover_workflow_dependencies(
        &self,
        inventory: &Discovery,
        observed_at: Timestamp,
        options: &WorkflowOptions,
    ) -> WorkflowDiscovery {
        let mut graph = inventory.graph.clone();
        let mut report = WorkflowReport {
            status: SyncStatus::Complete,
            inventory_complete: inventory.complete_graph().is_some(),
            repositories_scanned: 0,
            repositories_without_workflows: 0,
            workflow_files_read: 0,
            nodes_added: 0,
            edges_added: 0,
            skipped: Vec::new(),
            last_rate_limit: None,
        };
        let mut found = Found::default();

        let repositories = listed_repositories(&graph);
        if let Err(stop) = self.scan(&repositories, options, &mut report, &mut found) {
            report.status = SyncStatus::Incomplete(stop);
        }

        for (id, name) in &found.referenced {
            if graph.node(id).is_some() {
                continue;
            }
            let mut node = Node::new(id.clone(), NodeKind::Repository, name.clone());
            node.attributes
                .insert("provider".to_string(), "github".to_string());
            node.attributes
                .insert("referenced_only".to_string(), "true".to_string());
            match graph.add_node(node) {
                Ok(()) => report.nodes_added += 1,
                Err(e) => report.skipped.push(WorkflowSkip {
                    scope: id.to_string(),
                    reason: e.to_string(),
                }),
            }
        }
        for ((from, to), evidence) in found.edges {
            let scope = format!("{from} -> {to}");
            let provenance = match Provenance::new(
                EVIDENCE_SOURCE,
                DiscoveryMethod::ManifestParse,
                observed_at,
                evidence,
            ) {
                Ok(p) => p,
                Err(e) => {
                    report.skipped.push(WorkflowSkip {
                        scope,
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            match graph.add_edge(Edge::observed(from, to, EdgeKind::DependsOn, provenance)) {
                Ok(()) => report.edges_added += 1,
                Err(e) => report.skipped.push(WorkflowSkip {
                    scope,
                    reason: e.to_string(),
                }),
            }
        }

        WorkflowDiscovery { graph, report }
    }

    fn scan(
        &self,
        repositories: &[(NodeId, String)],
        options: &WorkflowOptions,
        report: &mut WorkflowReport,
        found: &mut Found,
    ) -> Result<(), FetchError> {
        for (id, full_name) in repositories {
            if !valid_full_name(full_name) {
                report.skipped.push(WorkflowSkip {
                    scope: id.to_string(),
                    reason: "repository name is not in the expected owner/name form".to_string(),
                });
                continue;
            }
            match self.scan_repository(id, full_name, options, report, found) {
                Ok(()) => {}
                Err(stop @ (FetchError::RateLimited(_) | FetchError::Unauthorized)) => {
                    return Err(stop)
                }
                Err(other) => report.skipped.push(WorkflowSkip {
                    scope: full_name.clone(),
                    reason: other.to_string(),
                }),
            }
        }
        Ok(())
    }

    fn scan_repository(
        &self,
        repo_id: &NodeId,
        full_name: &str,
        options: &WorkflowOptions,
        report: &mut WorkflowReport,
        found: &mut Found,
    ) -> Result<(), FetchError> {
        let dir_url = format!(
            "{}/repos/{}/contents/{}",
            self.base_url, full_name, WORKFLOW_DIR
        );
        let listing = match self.fetch(&dir_url, ACCEPT, options.max_body_bytes, report) {
            Ok(response) => response,
            Err(FetchError::NotFound) => {
                report.repositories_scanned += 1;
                report.repositories_without_workflows += 1;
                return Ok(());
            }
            Err(other) => return Err(other),
        };
        let parsed = json::parse(&listing.body).map_err(|e| {
            FetchError::MalformedResponse(format!(
                "invalid JSON at byte {}: {}",
                e.offset, e.message
            ))
        })?;
        let entries = parsed.as_array().ok_or_else(|| {
            FetchError::MalformedResponse("expected a directory listing".to_string())
        })?;

        let mut paths = Vec::new();
        for entry in entries {
            if entry.get("type").and_then(Json::as_str) != Some("file") {
                continue;
            }
            let Some(name) = entry.get("name").and_then(Json::as_str) else {
                continue;
            };
            if !is_workflow_name(name) {
                continue;
            }
            match entry
                .get("path")
                .and_then(Json::as_str)
                .filter(|p| is_workflow_path(p))
            {
                Some(path) => paths.push(path.to_string()),
                None => report.skipped.push(WorkflowSkip {
                    scope: full_name.to_string(),
                    reason: format!("workflow file {} has an unusable path", display_safe(name)),
                }),
            }
        }
        paths.sort();
        paths.dedup();
        if paths.len() > options.max_files_per_repo {
            report.skipped.push(WorkflowSkip {
                scope: full_name.to_string(),
                reason: format!(
                    "{} workflow files found; only the first {} were read",
                    paths.len(),
                    options.max_files_per_repo
                ),
            });
            paths.truncate(options.max_files_per_repo);
        }
        report.repositories_scanned += 1;

        for path in paths {
            let url = format!("{}/repos/{}/contents/{}", self.base_url, full_name, path);
            match self.fetch(&url, RAW_ACCEPT, options.max_body_bytes, report) {
                Ok(file) => {
                    report.workflow_files_read += 1;
                    for reference in parse_workflow_uses(&file.body) {
                        let Ok(target) = NodeId::new(format!(
                            "github:repo:{}",
                            reference.repo.to_ascii_lowercase()
                        )) else {
                            continue;
                        };
                        if &target == repo_id {
                            continue;
                        }
                        found
                            .edges
                            .entry((repo_id.clone(), target.clone()))
                            .or_default()
                            .push(format!(
                                "{}/{}:{} (uses: {})",
                                full_name, path, reference.line, reference.raw
                            ));
                        found.referenced.entry(target).or_insert(reference.repo);
                    }
                }
                Err(stop @ (FetchError::RateLimited(_) | FetchError::Unauthorized)) => {
                    return Err(stop)
                }
                Err(other) => report.skipped.push(WorkflowSkip {
                    scope: format!("{full_name}/{path}"),
                    reason: other.to_string(),
                }),
            }
        }
        Ok(())
    }

    fn fetch(
        &self,
        url: &str,
        accept: &str,
        max_body_bytes: usize,
        report: &mut WorkflowReport,
    ) -> Result<Response, FetchError> {
        let headers = [("Accept", accept), ("X-GitHub-Api-Version", API_VERSION)];
        let response = self
            .transport
            .get(url, &headers)
            .map_err(|e| FetchError::Transport(e.0))?;
        report.last_rate_limit = Some(parse_rate_limit(&response.headers));
        classify(&response)?;
        if response.body.len() > max_body_bytes {
            return Err(FetchError::BodyTooLarge {
                limit: max_body_bytes,
            });
        }
        Ok(response)
    }
}

/// The repositories the inventory listed (not the ones only referenced).
fn listed_repositories(graph: &Graph) -> Vec<(NodeId, String)> {
    graph
        .nodes()
        .filter(|n| {
            n.kind == NodeKind::Repository
                && n.attributes.get("provider").map(String::as_str) == Some("github")
                && !n.attributes.contains_key("referenced_only")
        })
        .map(|n| (n.id.clone(), n.name.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{Headers, Owner, SyncOptions, TransportError};
    use nexus_domain::{Basis, EdgeKey};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    const BASE: &str = "https://api.github.test";
    const LIST_URL: &str =
        "https://api.github.test/orgs/acme/repos?per_page=100&sort=full_name&direction=asc";
    const NOW: i64 = 1_700_000_000;

    type Reply = Result<Response, TransportError>;

    /// A scripted stand-in for the network, keyed by URL. It is a test
    /// double, not a GitHub integration: nothing here contacts GitHub.
    struct Fake {
        replies: RefCell<HashMap<String, Reply>>,
        /// (url, Accept header) of every request, in order.
        requests: RefCell<Vec<(String, String)>>,
    }

    impl Transport for Rc<Fake> {
        fn get(&self, url: &str, headers: &[(&str, &str)]) -> Reply {
            let accept = headers
                .iter()
                .find(|(k, _)| *k == "Accept")
                .map(|(_, v)| v.to_string())
                .unwrap_or_default();
            self.requests.borrow_mut().push((url.to_string(), accept));
            self.replies
                .borrow()
                .get(url)
                .cloned()
                .unwrap_or_else(|| Err(TransportError(format!("no canned reply for {url}"))))
        }
    }

    fn resp(status: u16, headers: &[(&str, &str)], body: &str) -> Reply {
        let mut h = Headers::new();
        for (name, value) in headers {
            h.insert(name, value);
        }
        Ok(Response {
            status,
            headers: h,
            body: body.to_string(),
        })
    }

    fn ok(body: &str) -> Reply {
        resp(200, &[], body)
    }

    fn not_found() -> Reply {
        resp(404, &[], r#"{"message":"Not Found"}"#)
    }

    fn dir_url(repo: &str) -> String {
        format!("{BASE}/repos/{repo}/contents/.github/workflows")
    }

    fn file_url(repo: &str, name: &str) -> String {
        format!("{BASE}/repos/{repo}/contents/.github/workflows/{name}")
    }

    /// Synthetic directory listing shaped after the documented response
    /// (only the fields the scanner reads). Not captured from the live API.
    fn listing(entries: &[(&str, &str)]) -> Reply {
        let items: Vec<String> = entries
            .iter()
            .map(|(kind, name)| {
                format!(r#"{{"type":"{kind}","name":"{name}","path":".github/workflows/{name}"}}"#)
            })
            .collect();
        ok(&format!("[{}]", items.join(",")))
    }

    fn repo_list(names: &[&str]) -> (String, Reply) {
        let items: Vec<String> = names
            .iter()
            .map(|n| format!(r#"{{"id":1,"full_name":"{n}","private":false}}"#))
            .collect();
        (LIST_URL.to_string(), ok(&format!("[{}]", items.join(","))))
    }

    fn client(replies: Vec<(String, Reply)>) -> (Rc<Fake>, GitHubClient<Rc<Fake>>) {
        let fake = Rc::new(Fake {
            replies: RefCell::new(replies.into_iter().collect()),
            requests: RefCell::new(Vec::new()),
        });
        let client = GitHubClient::new(Rc::clone(&fake), BASE).unwrap();
        (fake, client)
    }

    fn inventory(client: &GitHubClient<Rc<Fake>>) -> Discovery {
        client.discover_repositories(
            &Owner::organization("acme").unwrap(),
            &SyncOptions::default(),
        )
    }

    fn scan_with(
        client: &GitHubClient<Rc<Fake>>,
        inventory: &Discovery,
        options: &WorkflowOptions,
    ) -> WorkflowDiscovery {
        client.discover_workflow_dependencies(inventory, Timestamp::from_unix_secs(NOW), options)
    }

    fn scan(client: &GitHubClient<Rc<Fake>>, inventory: &Discovery) -> WorkflowDiscovery {
        scan_with(client, inventory, &WorkflowOptions::default())
    }

    fn id(s: &str) -> NodeId {
        NodeId::new(s).unwrap()
    }

    fn key(from: &str, to: &str) -> EdgeKey {
        EdgeKey {
            from: id(from),
            to: id(to),
            kind: EdgeKind::DependsOn,
        }
    }

    const CI: &str = "name: ci\non: push\njobs:\n  call:\n    uses: acme/api/.github/workflows/build.yml@main\n  build:\n    steps:\n      - uses: actions/checkout@v4\n      - run: |\n          echo uses: evil/repo@v1\n";

    // ----- parser -----

    #[test]
    fn finds_action_and_reusable_workflow_references_with_line_numbers() {
        let refs = parse_workflow_uses(CI);
        assert_eq!(
            refs,
            vec![
                ActionRef {
                    repo: "acme/api".to_string(),
                    line: 5,
                    raw: "acme/api/.github/workflows/build.yml@main".to_string(),
                },
                ActionRef {
                    repo: "actions/checkout".to_string(),
                    line: 8,
                    raw: "actions/checkout@v4".to_string(),
                },
            ]
        );
    }

    #[test]
    fn quotes_comments_and_list_item_forms_are_understood() {
        let text = "steps:\n  - name: one\n    uses: \"actions/setup-node@v4\"\n  - uses: 'actions/cache@v4' # pinned\n  -   uses: owner/tool@abc123\n";
        let repos: Vec<_> = parse_workflow_uses(text)
            .into_iter()
            .map(|r| (r.repo, r.line, r.raw))
            .collect();
        assert_eq!(
            repos,
            vec![
                (
                    "actions/setup-node".to_string(),
                    3,
                    "actions/setup-node@v4".to_string()
                ),
                (
                    "actions/cache".to_string(),
                    4,
                    "actions/cache@v4".to_string()
                ),
                ("owner/tool".to_string(), 5, "owner/tool@abc123".to_string()),
            ]
        );
    }

    #[test]
    fn local_docker_and_unusual_values_are_ignored() {
        let text = "a:\n  uses: ./.github/actions/local\nb:\n  uses: docker://alpine:3.19\nc:\n  uses: ${{ matrix.action }}@v1\nd:\n  uses: owner/name\ne:\n  uses: owner/name@v1 extra\nf:\n  uses: owner/@v1\n";
        assert!(parse_workflow_uses(text).is_empty());
    }

    #[test]
    fn block_scalars_hide_shell_text_but_not_what_follows() {
        let text = "steps:\n  - run: >-\n      uses: fake/one@v1\n\n      uses: fake/two@v1\n  - uses: real/three@v1\n  - run: |  # comment\n      uses: fake/four@v1\n  - name: x\n    uses: real/five@v1\n";
        let repos: Vec<_> = parse_workflow_uses(text)
            .into_iter()
            .map(|r| r.repo)
            .collect();
        assert_eq!(repos, vec!["real/three", "real/five"]);
    }

    #[test]
    fn comments_and_empty_input_give_nothing() {
        assert!(parse_workflow_uses("").is_empty());
        assert!(parse_workflow_uses("# uses: a/b@v1\n\n   \n").is_empty());
    }

    // ----- client -----

    #[test]
    fn reads_workflow_files_and_records_observed_dependencies() {
        let (fake, client) = client(vec![
            repo_list(&["acme/api", "acme/web"]),
            (
                dir_url("acme/web"),
                listing(&[("file", "ci.yml"), ("file", "README.md"), ("dir", "nested")]),
            ),
            (file_url("acme/web", "ci.yml"), ok(CI)),
            (dir_url("acme/api"), not_found()),
        ]);
        let inv = inventory(&client);
        let d = scan(&client, &inv);
        let r = &d.report;

        assert_eq!(r.status, SyncStatus::Complete);
        assert!(r.inventory_complete);
        assert_eq!(r.repositories_scanned, 2);
        assert_eq!(r.repositories_without_workflows, 1);
        assert_eq!(r.workflow_files_read, 1);
        assert_eq!(r.edges_added, 2);
        assert_eq!(r.nodes_added, 1);
        assert!(r.skipped.is_empty());
        assert!(d.complete_graph().is_some());

        let checkout = d.graph.node(&id("github:repo:actions/checkout")).unwrap();
        assert_eq!(checkout.kind, NodeKind::Repository);
        assert_eq!(
            checkout
                .attributes
                .get("referenced_only")
                .map(String::as_str),
            Some("true")
        );
        let api = d.graph.node(&id("github:repo:acme/api")).unwrap();
        assert!(!api.attributes.contains_key("referenced_only"));
        assert!(d.graph.node(&id("github:repo:evil/repo")).is_none());

        let edge = d
            .graph
            .edge(&key("github:repo:acme/web", "github:repo:actions/checkout"))
            .unwrap();
        assert_eq!(edge.basis(), &Basis::Observed);
        let p = edge.provenance();
        assert_eq!(p.source(), "github:workflow-files");
        assert_eq!(p.method(), DiscoveryMethod::ManifestParse);
        assert_eq!(p.observed_at().as_unix_secs(), NOW);
        assert_eq!(
            p.evidence(),
            ["acme/web/.github/workflows/ci.yml:8 (uses: actions/checkout@v4)"]
        );
        let reusable = d
            .graph
            .edge(&key("github:repo:acme/web", "github:repo:acme/api"))
            .unwrap();
        assert_eq!(
            reusable.provenance().evidence(),
            ["acme/web/.github/workflows/ci.yml:5 (uses: acme/api/.github/workflows/build.yml@main)"]
        );

        // The inventory itself is untouched.
        assert_eq!(inv.graph.edge_count(), 0);

        let requests = fake.requests.borrow();
        assert!(requests.contains(&(dir_url("acme/web"), ACCEPT.to_string())));
        assert!(requests.contains(&(file_url("acme/web", "ci.yml"), RAW_ACCEPT.to_string())));
        assert!(requests
            .iter()
            .all(|(u, _)| !u.contains("README") && !u.contains("nested")));
    }

    #[test]
    fn workflow_edges_feed_the_change_impact_report() {
        use nexus_analysis::{analyze_change, Certainty, ChangeKind, ProposedChange};

        let (_fake, client) = client(vec![
            repo_list(&["acme/web"]),
            (dir_url("acme/web"), listing(&[("file", "ci.yml")])),
            (file_url("acme/web", "ci.yml"), ok(CI)),
        ]);
        let inv = inventory(&client);
        let d = scan(&client, &inv);

        let report = analyze_change(
            &d.graph,
            &ProposedChange {
                target: id("github:repo:actions/checkout"),
                kind: ChangeKind::Removal,
                description: String::new(),
            },
        )
        .unwrap();
        assert_eq!(report.affected.len(), 1);
        assert_eq!(report.affected[0].id, id("github:repo:acme/web"));
        assert_eq!(report.affected[0].certainty, Certainty::ObservedChain);
        assert!(report
            .to_markdown()
            .contains("acme/web/.github/workflows/ci.yml:8"));
    }

    #[test]
    fn a_rate_limit_stops_the_scan_but_keeps_what_was_found() {
        let (fake, client) = client(vec![
            repo_list(&["acme/a", "acme/b", "acme/c"]),
            (dir_url("acme/a"), listing(&[("file", "ci.yml")])),
            (
                file_url("acme/a", "ci.yml"),
                ok("steps:\n  - uses: actions/checkout@v4\n"),
            ),
            (
                dir_url("acme/b"),
                resp(429, &[("retry-after", "30")], "slow down"),
            ),
        ]);
        let inv = inventory(&client);
        let d = scan(&client, &inv);

        assert!(matches!(
            d.report.status,
            SyncStatus::Incomplete(FetchError::RateLimited(_))
        ));
        assert_eq!(d.report.edges_added, 1);
        assert!(d
            .graph
            .edge(&key("github:repo:acme/a", "github:repo:actions/checkout"))
            .is_some());
        assert!(d.complete_graph().is_none());
        assert!(fake
            .requests
            .borrow()
            .iter()
            .all(|(u, _)| u != &dir_url("acme/c")));
    }

    #[test]
    fn rejected_credentials_stop_the_scan() {
        let (fake, client) = client(vec![
            repo_list(&["acme/a", "acme/b"]),
            (dir_url("acme/a"), resp(401, &[], "bad credentials")),
        ]);
        let inv = inventory(&client);
        let d = scan(&client, &inv);
        assert_eq!(
            d.report.status,
            SyncStatus::Incomplete(FetchError::Unauthorized)
        );
        assert!(fake
            .requests
            .borrow()
            .iter()
            .all(|(u, _)| u != &dir_url("acme/b")));
    }

    #[test]
    fn a_forbidden_repository_is_skipped_and_the_scan_continues() {
        let (_fake, client) = client(vec![
            repo_list(&["acme/a", "acme/b"]),
            (dir_url("acme/a"), resp(403, &[], r#"{"message":"no"}"#)),
            (dir_url("acme/b"), listing(&[("file", "ci.yml")])),
            (
                file_url("acme/b", "ci.yml"),
                ok("steps:\n  - uses: actions/checkout@v4\n"),
            ),
        ]);
        let inv = inventory(&client);
        let d = scan(&client, &inv);

        assert_eq!(d.report.status, SyncStatus::Complete);
        assert_eq!(d.report.skipped.len(), 1);
        assert_eq!(d.report.skipped[0].scope, "acme/a");
        assert!(d.report.skipped[0].reason.contains("403"));
        assert_eq!(d.report.edges_added, 1);
        assert!(d.complete_graph().is_none());
    }

    #[test]
    fn an_incomplete_inventory_makes_the_result_incomplete() {
        let (_fake, client) = client(vec![
            repo_list(&["acme/a"]),
            (dir_url("acme/a"), not_found()),
        ]);
        let mut inv = inventory(&client);
        inv.report.status = SyncStatus::Incomplete(FetchError::LinkLoop);
        let d = scan(&client, &inv);

        assert_eq!(d.report.status, SyncStatus::Complete);
        assert!(d.report.skipped.is_empty());
        assert!(!d.report.inventory_complete);
        assert!(d.complete_graph().is_none());
    }

    #[test]
    fn unsafe_workflow_paths_are_reported_and_never_requested() {
        let body = r#"[
            {"type":"file","name":"evil.yml","path":"../../evil.yml"},
            {"type":"file","name":"sneaky.yml","path":".github/workflows/../x.yml"},
            {"type":"file","name":"with space.yml","path":".github/workflows/with space.yml"}
        ]"#;
        let (fake, client) = client(vec![repo_list(&["acme/a"]), (dir_url("acme/a"), ok(body))]);
        let inv = inventory(&client);
        let d = scan(&client, &inv);

        assert_eq!(d.report.skipped.len(), 3);
        assert!(d
            .report
            .skipped
            .iter()
            .all(|s| s.reason.contains("unusable path")));
        assert_eq!(d.report.workflow_files_read, 0);
        assert_eq!(fake.requests.borrow().len(), 2); // repository list + directory
        assert!(d.complete_graph().is_none());
    }

    #[test]
    fn the_per_repository_file_limit_is_applied_and_reported() {
        let (fake, client) = client(vec![
            repo_list(&["acme/a"]),
            (
                dir_url("acme/a"),
                listing(&[("file", "b.yml"), ("file", "a.yml")]),
            ),
            (file_url("acme/a", "a.yml"), ok("")),
        ]);
        let inv = inventory(&client);
        let options = WorkflowOptions {
            max_files_per_repo: 1,
            ..WorkflowOptions::default()
        };
        let d = scan_with(&client, &inv, &options);

        assert_eq!(d.report.workflow_files_read, 1);
        assert_eq!(d.report.skipped.len(), 1);
        assert!(d.report.skipped[0].reason.contains("only the first 1"));
        assert!(fake
            .requests
            .borrow()
            .iter()
            .all(|(u, _)| u != &file_url("acme/a", "b.yml")));
    }

    #[test]
    fn self_references_make_no_edge_and_repeats_share_one_edge() {
        let text = "a:\n  uses: acme/web/.github/workflows/s.yml@main\nb:\n  uses: actions/checkout@v4\nc:\n  uses: actions/checkout@v3\n";
        let (_fake, client) = client(vec![
            repo_list(&["acme/web"]),
            (dir_url("acme/web"), listing(&[("file", "ci.yml")])),
            (file_url("acme/web", "ci.yml"), ok(text)),
        ]);
        let inv = inventory(&client);
        let d = scan(&client, &inv);

        assert_eq!(d.report.edges_added, 1);
        assert_eq!(d.graph.edge_count(), 1);
        assert!(d
            .graph
            .edge(&key("github:repo:acme/web", "github:repo:acme/web"))
            .is_none());
        let edge = d
            .graph
            .edge(&key("github:repo:acme/web", "github:repo:actions/checkout"))
            .unwrap();
        assert_eq!(
            edge.provenance().evidence(),
            [
                "acme/web/.github/workflows/ci.yml:4 (uses: actions/checkout@v4)",
                "acme/web/.github/workflows/ci.yml:6 (uses: actions/checkout@v3)"
            ]
        );
    }

    #[test]
    fn an_oversized_workflow_file_is_skipped() {
        let (_fake, client) = client(vec![
            repo_list(&["acme/a"]),
            (dir_url("acme/a"), listing(&[("file", "ci.yml")])),
            (file_url("acme/a", "ci.yml"), ok(&"#".repeat(300))),
        ]);
        let inv = inventory(&client);
        let options = WorkflowOptions {
            max_body_bytes: 200,
            ..WorkflowOptions::default()
        };
        let d = scan_with(&client, &inv, &options);

        assert_eq!(d.report.workflow_files_read, 0);
        assert_eq!(d.report.skipped.len(), 1);
        assert!(d.report.skipped[0].reason.contains("larger than"));
    }

    #[test]
    fn a_non_directory_answer_is_skipped_with_a_reason() {
        let (_fake, client) = client(vec![
            repo_list(&["acme/a"]),
            (
                dir_url("acme/a"),
                ok(r#"{"type":"file","name":"workflows"}"#),
            ),
        ]);
        let inv = inventory(&client);
        let d = scan(&client, &inv);

        assert_eq!(d.report.skipped.len(), 1);
        assert!(d.report.skipped[0].reason.contains("directory listing"));
    }

    #[test]
    fn repositories_only_referenced_are_not_scanned() {
        let (fake, client) = client(vec![
            repo_list(&["acme/web"]),
            (dir_url("acme/web"), listing(&[("file", "ci.yml")])),
            (
                file_url("acme/web", "ci.yml"),
                ok("steps:\n  - uses: actions/checkout@v4\n"),
            ),
        ]);
        let inv = inventory(&client);
        let first = scan(&client, &inv);
        // Scanning the result again must not request the referenced-only repo.
        let again = Discovery {
            graph: first.graph.clone(),
            report: inv.report.clone(),
        };
        let before = fake.requests.borrow().len();
        let second = scan(&client, &again);
        let requests = fake.requests.borrow();
        assert!(requests[before..]
            .iter()
            .all(|(u, _)| !u.contains("actions/checkout")));
        assert_eq!(second.report.repositories_scanned, 1);
    }
}
