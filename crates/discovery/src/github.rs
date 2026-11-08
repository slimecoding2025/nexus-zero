//! GitHub discovery adapter: a read-only repository inventory.
//!
//! ## What it does
//!
//! [`GitHubClient::discover_repositories`] lists the repositories of one
//! organization or user and turns each into a `Repository` node. It follows
//! pagination, classifies failures (including rate limits), and returns a
//! [`Discovery`]: the graph built so far plus a [`SyncReport`] saying whether
//! the inventory is complete.
//!
//! ## What it does not do
//!
//! - No relationships (edges) are discovered yet.
//! - There is no real HTTP transport. The network is behind [`Transport`];
//!   the implementation used in this crate's tests is a test double.
//! - It never sleeps or retries. A rate limit is returned as an error that
//!   says how long to wait, and the caller decides.
//!
//! ## Permissions (least privilege)
//!
//! Listing organization repositories needs read access to repository
//! metadata ("Metadata" repository permission for fine-grained tokens and
//! GitHub App installation tokens). No write permission, no repository
//! contents and no organization administration is required or requested.
//!
//! ## What is collected
//!
//! Only inventory facts: full name, numeric id, visibility, archived,
//! disabled, fork, default branch and web URL. Descriptions, topics, counts,
//! owners' details and `pushed_at` are not stored. `pushed_at` in particular
//! changes on every push, so storing it would make every snapshot diff
//! noisy without any infrastructure change.
//!
//! ## Partial results
//!
//! If a sync stops early (rate limit, network error, malformed page) the
//! graph holds what was read so far. It must not be treated as the full
//! inventory, because diffing it against a complete snapshot would report
//! the missing repositories as removed. Use [`Discovery::complete_graph`],
//! which returns the graph only when the inventory is complete.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use nexus_domain::{Node, NodeId, NodeKind};
use nexus_graph::{Graph, GraphError};

use crate::json::{self, Json};

/// Base URL of the public GitHub REST API.
pub const PUBLIC_API_BASE: &str = "https://api.github.com";
/// REST API version this adapter is written against. Pinned on purpose so
/// responses do not change shape silently.
pub const API_VERSION: &str = "2022-11-28";
/// Media type requested from GitHub.
pub const ACCEPT: &str = "application/vnd.github+json";
/// Page size requested (100 is the documented maximum for most endpoints).
pub const PER_PAGE: u32 = 100;
/// Default cap on pages per sync (a guard against runaway pagination).
pub const DEFAULT_MAX_PAGES: usize = 500;
/// Default cap on the size of one response body.
pub const DEFAULT_MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

const MAX_ATTRIBUTE_LEN: usize = 1024;
const DEFAULT_WAIT_SECS: u64 = 60;

// ---------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------

/// A GitHub access token. It exists so that secrets have one type whose
/// `Debug` output is redacted. The adapter itself never sees a token: a
/// [`Transport`] implementation holds it and adds the `Authorization`
/// header. Tokens must stay on the server and never reach the frontend.
#[derive(Clone)]
pub struct Token(String);

/// The token was empty or contained whitespace or control characters.
/// The message never repeats the rejected value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidToken;

impl fmt::Display for InvalidToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("token must be non-empty and contain no whitespace or control characters")
    }
}

impl Error for InvalidToken {}

impl Token {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidToken> {
        let value = value.into();
        if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(InvalidToken);
        }
        Ok(Self(value))
    }

    /// The raw secret. Only a transport should call this, to build the
    /// `Authorization` header.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

// ---------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------

/// Response headers with case-insensitive lookup. A transport must join
/// repeated headers with commas before inserting them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers(BTreeMap<String, String>);

impl Headers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: &str, value: &str) {
        self.0.insert(name.to_ascii_lowercase(), value.to_string());
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(&name.to_ascii_lowercase()).map(String::as_str)
    }
}

/// An HTTP response as seen by the adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Headers,
    pub body: String,
}

/// A network-level failure (DNS, TLS, connection reset, timeout).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError(pub String);

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for TransportError {}

/// The network boundary. Implementations must:
/// - send the `headers` they are given unchanged;
/// - add authentication and a `User-Agent`, and nothing else;
/// - never follow a redirect to a different host (that would leak the token);
/// - treat a non-2xx status as a normal [`Response`], not an error.
pub trait Transport {
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Response, TransportError>;
}

// ---------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerKind {
    Organization,
    User,
}

/// Whose repositories to list. Names are validated so they can be put in a
/// URL path without any escaping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    kind: OwnerKind,
    name: String,
}

/// The owner name is not a plausible GitHub login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidOwner;

impl fmt::Display for InvalidOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("owner must be 1-100 characters from A-Z, a-z, 0-9, '-' and '_'")
    }
}

impl Error for InvalidOwner {}

fn valid_login(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Owner {
    /// An organization, listed with `GET /orgs/{org}/repos`.
    pub fn organization(name: &str) -> Result<Self, InvalidOwner> {
        Self::new(OwnerKind::Organization, name)
    }

    /// A user, listed with `GET /users/{username}/repos`. This endpoint's
    /// permissions and parameters were not checked against the docs when
    /// the adapter was written; see `docs/integrations/github.md`.
    pub fn user(name: &str) -> Result<Self, InvalidOwner> {
        Self::new(OwnerKind::User, name)
    }

    fn new(kind: OwnerKind, name: &str) -> Result<Self, InvalidOwner> {
        if !valid_login(name) {
            return Err(InvalidOwner);
        }
        Ok(Self {
            kind,
            name: name.to_string(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> String {
        match self.kind {
            OwnerKind::Organization => format!("/orgs/{}/repos", self.name),
            OwnerKind::User => format!("/users/{}/repos", self.name),
        }
    }
}

/// Limits for one sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOptions {
    pub max_pages: usize,
    pub max_body_bytes: usize,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            max_pages: DEFAULT_MAX_PAGES,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }
}

/// The client was configured with an unusable base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for ConfigError {}

// ---------------------------------------------------------------------
// Errors and rate limits
// ---------------------------------------------------------------------

/// Rate limit information read from response headers. Fields are `None`
/// when the header was absent or not a number.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimit {
    pub remaining: Option<u64>,
    /// Unix time (seconds) at which the current window resets.
    pub reset_unix: Option<i64>,
    pub retry_after_secs: Option<u64>,
}

/// Which limit was hit, as far as the response shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitKind {
    /// The hourly budget is used up (`x-ratelimit-remaining: 0`).
    Primary,
    /// GitHub's abuse protection fired (the response says so).
    Secondary,
    /// A 429, or a `retry-after`, without enough detail to tell.
    Unknown,
}

/// When it is acceptable to try again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryAdvice {
    /// Wait at least this many seconds from now.
    AfterSeconds(u64),
    /// Do not retry before this Unix time (seconds).
    AtUnixTime(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimited {
    pub kind: RateLimitKind,
    pub retry: RetryAdvice,
}

/// Why a sync stopped before the inventory was complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    Transport(String),
    /// 401: missing, invalid or expired credentials.
    Unauthorized,
    /// 403 that is not a rate limit: the token lacks a permission.
    Forbidden {
        accepted_permissions: Option<String>,
    },
    /// 404: the owner does not exist, or the token cannot see it. GitHub
    /// answers 404 for private resources, so the two cannot be told apart.
    NotFound,
    /// 410: for this endpoint, most likely an unsupported API version.
    Gone,
    RateLimited(RateLimited),
    ServerError(u16),
    UnexpectedStatus(u16),
    MalformedResponse(String),
    BodyTooLarge {
        limit: usize,
    },
    /// A `Link: rel="next"` pointed outside the configured API base.
    UnexpectedNextLink(String),
    /// A `Link: rel="next"` pointed at a page already fetched.
    LinkLoop,
    PageLimitReached {
        max_pages: usize,
    },
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Transport(m) => write!(f, "network error: {m}"),
            FetchError::Unauthorized => f.write_str(
                "GitHub rejected the credentials (401); the token may be missing, invalid or expired",
            ),
            FetchError::Forbidden {
                accepted_permissions,
            } => {
                f.write_str("GitHub refused the request (403); the token lacks a required permission")?;
                if let Some(p) = accepted_permissions {
                    write!(f, " (GitHub reports accepted permissions: {p})")?;
                }
                Ok(())
            }
            FetchError::NotFound => f.write_str(
                "GitHub returned 404; the owner does not exist or the token cannot see it",
            ),
            FetchError::Gone => f.write_str(
                "GitHub returned 410 Gone; the pinned API version may no longer be supported",
            ),
            FetchError::RateLimited(r) => {
                write!(f, "rate limited ({:?}); ", r.kind)?;
                match r.retry {
                    RetryAdvice::AfterSeconds(s) => write!(f, "retry after {s} seconds"),
                    RetryAdvice::AtUnixTime(t) => write!(f, "do not retry before unix time {t}"),
                }
            }
            FetchError::ServerError(s) => write!(f, "GitHub server error ({s})"),
            FetchError::UnexpectedStatus(s) => write!(f, "unexpected HTTP status {s}"),
            FetchError::MalformedResponse(m) => write!(f, "malformed response: {m}"),
            FetchError::BodyTooLarge { limit } => {
                write!(f, "response body larger than the {limit} byte limit")
            }
            FetchError::UnexpectedNextLink(u) => {
                write!(f, "pagination link points outside the API base: {u}")
            }
            FetchError::LinkLoop => f.write_str("pagination links loop back to a page already read"),
            FetchError::PageLimitReached { max_pages } => {
                write!(f, "stopped after the {max_pages} page limit")
            }
        }
    }
}

impl Error for FetchError {}

// ---------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    /// Every page was read.
    Complete,
    /// The sync stopped early; the graph is partial.
    Incomplete(FetchError),
}

/// A list entry that could not be turned into a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedItem {
    /// Zero-based page number.
    pub page: usize,
    /// Zero-based position within the page.
    pub index: usize,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub status: SyncStatus,
    pub pages_fetched: usize,
    pub repositories_seen: usize,
    pub nodes_added: usize,
    /// Entries already present (GitHub can repeat an item across pages when
    /// the list changes during pagination).
    pub duplicates_skipped: usize,
    pub skipped_items: Vec<SkippedItem>,
    /// Rate limit headers from the last response received, if any.
    pub last_rate_limit: Option<RateLimit>,
}

/// The outcome of one sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub graph: Graph,
    pub report: SyncReport,
}

impl Discovery {
    /// The graph, but only if the inventory is complete: every page was
    /// read and no entry had to be skipped. Use this, not `graph`, before
    /// taking a snapshot to diff.
    pub fn complete_graph(&self) -> Option<&Graph> {
        let complete = matches!(self.report.status, SyncStatus::Complete)
            && self.report.skipped_items.is_empty();
        complete.then_some(&self.graph)
    }
}

// ---------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------

pub struct GitHubClient<T: Transport> {
    transport: T,
    base_url: String,
}

impl<T: Transport> GitHubClient<T> {
    /// `base_url` must be an `https://` URL (for example
    /// [`PUBLIC_API_BASE`], or `https://HOST/api/v3` for GitHub Enterprise
    /// Server). Pagination links are only followed if they stay under it.
    pub fn new(transport: T, base_url: &str) -> Result<Self, ConfigError> {
        let base = base_url.trim_end_matches('/');
        let has_host = base.len() > "https://".len();
        let clean = !base
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '?' || c == '#');
        if !base.starts_with("https://") || !has_host || !clean {
            return Err(ConfigError(
                "base URL must be an https:// URL without whitespace, '?' or '#'".to_string(),
            ));
        }
        Ok(Self {
            transport,
            base_url: base.to_string(),
        })
    }

    /// List the owner's repositories. Always returns a [`Discovery`]; check
    /// `report.status` (or use [`Discovery::complete_graph`]).
    pub fn discover_repositories(&self, owner: &Owner, options: &SyncOptions) -> Discovery {
        let mut graph = Graph::new();
        // Placeholder; replaced below once `run` has finished.
        let mut report = SyncReport {
            status: SyncStatus::Complete,
            pages_fetched: 0,
            repositories_seen: 0,
            nodes_added: 0,
            duplicates_skipped: 0,
            skipped_items: Vec::new(),
            last_rate_limit: None,
        };
        report.status = match self.run(owner, options, &mut graph, &mut report) {
            Ok(()) => SyncStatus::Complete,
            Err(e) => SyncStatus::Incomplete(e),
        };
        Discovery { graph, report }
    }

    fn run(
        &self,
        owner: &Owner,
        options: &SyncOptions,
        graph: &mut Graph,
        report: &mut SyncReport,
    ) -> Result<(), FetchError> {
        let headers = [("Accept", ACCEPT), ("X-GitHub-Api-Version", API_VERSION)];
        let allowed_prefix = format!("{}/", self.base_url);
        let mut next = Some(format!(
            "{}{}?per_page={}&sort=full_name&direction=asc",
            self.base_url,
            owner.path(),
            PER_PAGE
        ));
        let mut seen = BTreeSet::new();

        while let Some(url) = next.take() {
            if report.pages_fetched >= options.max_pages {
                return Err(FetchError::PageLimitReached {
                    max_pages: options.max_pages,
                });
            }
            if !seen.insert(url.clone()) {
                return Err(FetchError::LinkLoop);
            }

            let response = self
                .transport
                .get(&url, &headers)
                .map_err(|e| FetchError::Transport(e.0))?;
            report.last_rate_limit = Some(parse_rate_limit(&response.headers));
            classify(&response)?;
            if response.body.len() > options.max_body_bytes {
                return Err(FetchError::BodyTooLarge {
                    limit: options.max_body_bytes,
                });
            }

            let parsed = json::parse(&response.body).map_err(|e| {
                FetchError::MalformedResponse(format!(
                    "invalid JSON at byte {}: {}",
                    e.offset, e.message
                ))
            })?;
            let items = parsed.as_array().ok_or_else(|| {
                FetchError::MalformedResponse("expected a JSON array of repositories".to_string())
            })?;

            let page = report.pages_fetched;
            report.pages_fetched += 1;
            for (index, item) in items.iter().enumerate() {
                report.repositories_seen += 1;
                match repository_node(item) {
                    Ok(node) => match graph.add_node(node) {
                        Ok(()) => report.nodes_added += 1,
                        Err(GraphError::DuplicateNode(_)) => report.duplicates_skipped += 1,
                        Err(other) => report.skipped_items.push(SkippedItem {
                            page,
                            index,
                            reason: other.to_string(),
                        }),
                    },
                    Err(reason) => report.skipped_items.push(SkippedItem {
                        page,
                        index,
                        reason,
                    }),
                }
            }

            next = match next_link(&response.headers) {
                None => None,
                Some(link) => {
                    if !link.starts_with(&allowed_prefix) {
                        return Err(FetchError::UnexpectedNextLink(display_safe(&link)));
                    }
                    Some(link)
                }
            };
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Response handling
// ---------------------------------------------------------------------

fn parse_rate_limit(headers: &Headers) -> RateLimit {
    RateLimit {
        remaining: headers
            .get("x-ratelimit-remaining")
            .and_then(|v| v.trim().parse().ok()),
        reset_unix: headers
            .get("x-ratelimit-reset")
            .and_then(|v| v.trim().parse().ok()),
        retry_after_secs: headers
            .get("retry-after")
            .and_then(|v| v.trim().parse().ok()),
    }
}

fn classify(response: &Response) -> Result<(), FetchError> {
    match response.status {
        200 => Ok(()),
        401 => Err(FetchError::Unauthorized),
        403 | 429 => Err(limited_or_forbidden(response)),
        404 => Err(FetchError::NotFound),
        410 => Err(FetchError::Gone),
        500..=599 => Err(FetchError::ServerError(response.status)),
        other => Err(FetchError::UnexpectedStatus(other)),
    }
}

/// Follows GitHub's documented client rules for 403/429:
/// - primary limit (`x-ratelimit-remaining: 0`): do not retry before the
///   reset time;
/// - secondary limit: honor `retry-after`; otherwise, if remaining is 0,
///   wait for the reset; otherwise wait at least a minute.
fn limited_or_forbidden(response: &Response) -> FetchError {
    let limit = parse_rate_limit(&response.headers);
    let after = limit.retry_after_secs.map(RetryAdvice::AfterSeconds);
    let at_reset = limit.reset_unix.map(RetryAdvice::AtUnixTime);
    let default_wait = RetryAdvice::AfterSeconds(DEFAULT_WAIT_SECS);
    let exhausted = limit.remaining == Some(0);

    if mentions_secondary_limit(&response.body) {
        let retry = after
            .or(if exhausted { at_reset } else { None })
            .unwrap_or(default_wait);
        return FetchError::RateLimited(RateLimited {
            kind: RateLimitKind::Secondary,
            retry,
        });
    }
    if exhausted {
        let retry = at_reset.or(after).unwrap_or(default_wait);
        return FetchError::RateLimited(RateLimited {
            kind: RateLimitKind::Primary,
            retry,
        });
    }
    if response.status == 429 || after.is_some() {
        return FetchError::RateLimited(RateLimited {
            kind: RateLimitKind::Unknown,
            retry: after.unwrap_or(default_wait),
        });
    }
    FetchError::Forbidden {
        accepted_permissions: response
            .headers
            .get("x-accepted-github-permissions")
            .and_then(printable),
    }
}

fn mentions_secondary_limit(body: &str) -> bool {
    let head: String = body.chars().take(4096).collect();
    head.to_ascii_lowercase().contains("secondary rate limit")
}

/// The URL of the `rel="next"` entry of a `Link` header, if there is one.
fn next_link(headers: &Headers) -> Option<String> {
    let mut rest = headers.get("link")?;
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        let end = after.find('>')?;
        let url = &after[..end];
        let tail = &after[end + 1..];
        let params_len = tail.find('<').unwrap_or(tail.len());
        if has_rel_next(tail[..params_len].trim().trim_end_matches(',')) {
            return Some(url.to_string());
        }
        rest = &tail[params_len..];
    }
    None
}

fn has_rel_next(params: &str) -> bool {
    params.split(';').any(|param| {
        let Some((name, value)) = param.split_once('=') else {
            return false;
        };
        name.trim().eq_ignore_ascii_case("rel")
            && value
                .trim()
                .trim_matches('"')
                .split_whitespace()
                .any(|rel| rel.eq_ignore_ascii_case("next"))
    })
}

// ---------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------

/// Text that came from the network is only kept if it is short, non-empty
/// and has no control characters.
fn clean(value: &str) -> Option<String> {
    if value.is_empty() || value.len() > MAX_ATTRIBUTE_LEN || value.chars().any(char::is_control) {
        None
    } else {
        Some(value.to_string())
    }
}

fn printable(value: &str) -> Option<String> {
    let value = value.trim();
    let ok = !value.is_empty()
        && value.len() <= 256
        && value.chars().all(|c| c.is_ascii_graphic() || c == ' ');
    ok.then(|| value.to_string())
}

fn display_safe(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect()
}

fn valid_full_name(full_name: &str) -> bool {
    let Some((owner, repo)) = full_name.split_once('/') else {
        return false;
    };
    let part_ok = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    part_ok(owner) && part_ok(repo)
}

/// Turn one entry of the repository list into a node, or say why not.
fn repository_node(item: &Json) -> Result<Node, String> {
    if !item.is_object() {
        return Err("entry is not a JSON object".to_string());
    }
    let full_name = item
        .get("full_name")
        .and_then(Json::as_str)
        .ok_or("missing full_name")?;
    if !valid_full_name(full_name) {
        return Err("full_name is not in the expected owner/name form".to_string());
    }
    // Repository names are case-insensitive on GitHub, so the id is
    // lowercased to stay stable if only the capitalization changes.
    let id = NodeId::new(format!("github:repo:{}", full_name.to_ascii_lowercase()))
        .map_err(|e| e.to_string())?;
    let mut node = Node::new(id, NodeKind::Repository, full_name);
    let attributes = &mut node.attributes;
    attributes.insert("provider".to_string(), "github".to_string());

    if let Some(number) = item
        .get("id")
        .and_then(Json::as_number_str)
        .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
    {
        attributes.insert("github_id".to_string(), number.to_string());
    }

    let visibility = item
        .get("visibility")
        .and_then(Json::as_str)
        .and_then(clean)
        .or_else(|| match item.get("private").and_then(Json::as_bool) {
            Some(true) => Some("private".to_string()),
            Some(false) => Some("public".to_string()),
            None => None,
        });
    if let Some(visibility) = visibility {
        attributes.insert("visibility".to_string(), visibility);
    }

    for key in ["archived", "disabled", "fork"] {
        if let Some(flag) = item.get(key).and_then(Json::as_bool) {
            attributes.insert(key.to_string(), flag.to_string());
        }
    }
    for key in ["default_branch", "html_url"] {
        if let Some(value) = item.get(key).and_then(Json::as_str).and_then(clean) {
            attributes.insert(key.to_string(), value);
        }
    }
    Ok(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    const BASE: &str = "https://api.github.test";
    const FIRST_ORG_URL: &str =
        "https://api.github.test/orgs/acme/repos?per_page=100&sort=full_name&direction=asc";
    const NEXT_URL: &str =
        "https://api.github.test/orgs/acme/repos?per_page=100&sort=full_name&direction=asc&page=2";

    type Reply = Result<Response, TransportError>;
    type RequestLog = Vec<(String, Vec<(String, String)>)>;

    /// A scripted stand-in for the network. It is a test double, not a
    /// GitHub integration: nothing in these tests contacts GitHub.
    struct Fake {
        replies: RefCell<VecDeque<Reply>>,
        requests: RefCell<RequestLog>,
    }

    impl Transport for Rc<Fake> {
        fn get(&self, url: &str, headers: &[(&str, &str)]) -> Reply {
            let sent = headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            self.requests.borrow_mut().push((url.to_string(), sent));
            self.replies
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(TransportError("no canned reply left".to_string())))
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

    /// Synthetic entry shaped after the documented list response (only the
    /// fields the adapter reads, plus some it must ignore). Not captured
    /// from the live API.
    fn repo(full_name: &str) -> String {
        format!(
            r#"{{"id":123,"full_name":"{full_name}","private":false,"visibility":"public","archived":false,"disabled":false,"fork":false,"default_branch":"main","html_url":"https://github.com/{full_name}","description":"ignored","pushed_at":"2026-01-01T00:00:00Z"}}"#
        )
    }

    fn page(names: &[&str]) -> String {
        let items: Vec<String> = names.iter().map(|n| repo(n)).collect();
        format!("[{}]", items.join(","))
    }

    fn setup_at(base: &str, replies: Vec<Reply>) -> (Rc<Fake>, GitHubClient<Rc<Fake>>) {
        let fake = Rc::new(Fake {
            replies: RefCell::new(replies.into()),
            requests: RefCell::new(Vec::new()),
        });
        let client = GitHubClient::new(Rc::clone(&fake), base).unwrap();
        (fake, client)
    }

    fn setup(replies: Vec<Reply>) -> (Rc<Fake>, GitHubClient<Rc<Fake>>) {
        setup_at(BASE, replies)
    }

    fn org() -> Owner {
        Owner::organization("acme").unwrap()
    }

    fn discover(client: &GitHubClient<Rc<Fake>>) -> Discovery {
        client.discover_repositories(&org(), &SyncOptions::default())
    }

    fn status_for(reply: Reply) -> SyncStatus {
        let (_fake, client) = setup(vec![reply]);
        discover(&client).report.status
    }

    fn attr<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
        node.attributes.get(key).map(String::as_str)
    }

    fn node_id(s: &str) -> NodeId {
        NodeId::new(s).unwrap()
    }

    fn link_header(value: &str) -> Headers {
        let mut h = Headers::new();
        h.insert("Link", value);
        h
    }

    #[test]
    fn single_page_becomes_repository_nodes() {
        let (fake, client) = setup(vec![ok(&page(&["Acme/Alpha", "acme/beta"]))]);
        let d = discover(&client);

        assert_eq!(d.report.status, SyncStatus::Complete);
        assert_eq!(d.report.pages_fetched, 1);
        assert_eq!(d.report.repositories_seen, 2);
        assert_eq!(d.report.nodes_added, 2);
        assert!(d.report.skipped_items.is_empty());
        assert!(d.complete_graph().is_some());

        let node = d.graph.node(&node_id("github:repo:acme/alpha")).unwrap();
        assert_eq!(node.kind, NodeKind::Repository);
        assert_eq!(node.name, "Acme/Alpha");
        assert_eq!(attr(node, "provider"), Some("github"));
        assert_eq!(attr(node, "github_id"), Some("123"));
        assert_eq!(attr(node, "visibility"), Some("public"));
        assert_eq!(attr(node, "archived"), Some("false"));
        assert_eq!(attr(node, "disabled"), Some("false"));
        assert_eq!(attr(node, "fork"), Some("false"));
        assert_eq!(attr(node, "default_branch"), Some("main"));
        assert_eq!(
            attr(node, "html_url"),
            Some("https://github.com/Acme/Alpha")
        );
        // Collected only what is needed: no description, no pushed_at.
        assert!(!node.attributes.contains_key("description"));
        assert!(!node.attributes.contains_key("pushed_at"));
        assert_eq!(node.attributes.len(), 8);

        let requests = fake.requests.borrow();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, FIRST_ORG_URL);
        assert!(requests[0]
            .1
            .contains(&("Accept".to_string(), ACCEPT.to_string())));
        assert!(requests[0]
            .1
            .contains(&("X-GitHub-Api-Version".to_string(), API_VERSION.to_string())));
        assert_eq!(requests[0].1.len(), 2);
    }

    #[test]
    fn follows_next_links_across_pages() {
        let link = format!(
            "<{NEXT_URL}>; rel=\"next\", <https://api.github.test/orgs/acme/repos?page=9>; rel=\"last\""
        );
        let (fake, client) = setup(vec![
            resp(200, &[("Link", link.as_str())], &page(&["acme/a"])),
            ok(&page(&["acme/b"])),
        ]);
        let d = discover(&client);

        assert_eq!(d.report.status, SyncStatus::Complete);
        assert_eq!(d.report.pages_fetched, 2);
        assert_eq!(d.report.nodes_added, 2);
        let requests = fake.requests.borrow();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].0, NEXT_URL);
    }

    #[test]
    fn user_owner_uses_the_users_endpoint() {
        let (fake, client) = setup(vec![ok("[]")]);
        let owner = Owner::user("octocat").unwrap();
        let d = client.discover_repositories(&owner, &SyncOptions::default());
        assert_eq!(d.report.status, SyncStatus::Complete);
        assert_eq!(d.report.nodes_added, 0);
        assert_eq!(
            fake.requests.borrow()[0].0,
            "https://api.github.test/users/octocat/repos?per_page=100&sort=full_name&direction=asc"
        );
    }

    #[test]
    fn next_links_outside_the_api_base_are_never_followed() {
        for evil in [
            "https://evil.test/orgs/acme/repos?page=2",
            "https://api.github.test.evil.test/orgs/acme/repos?page=2",
            "http://api.github.test/orgs/acme/repos?page=2",
        ] {
            let link = format!("<{evil}>; rel=\"next\"");
            let (fake, client) = setup(vec![resp(
                200,
                &[("Link", link.as_str())],
                &page(&["acme/a"]),
            )]);
            let d = discover(&client);

            assert_eq!(
                d.report.status,
                SyncStatus::Incomplete(FetchError::UnexpectedNextLink(evil.to_string()))
            );
            assert_eq!(
                fake.requests.borrow().len(),
                1,
                "{evil} must not be requested"
            );
            assert_eq!(d.report.nodes_added, 1);
            assert!(d.complete_graph().is_none());
        }
    }

    #[test]
    fn enterprise_style_base_urls_keep_their_path_prefix() {
        let base = "https://ghe.test/api/v3";
        let next = "https://ghe.test/api/v3/orgs/acme/repos?page=2";
        let link = format!("<{next}>; rel=\"next\"");
        let (fake, client) = setup_at(
            base,
            vec![
                resp(200, &[("Link", link.as_str())], &page(&["acme/a"])),
                ok(&page(&["acme/b"])),
            ],
        );
        let d = discover(&client);
        assert_eq!(d.report.status, SyncStatus::Complete);
        let requests = fake.requests.borrow();
        assert!(requests[0]
            .0
            .starts_with("https://ghe.test/api/v3/orgs/acme/repos?"));
        assert_eq!(requests[1].0, next);
    }

    #[test]
    fn a_link_loop_stops_the_sync() {
        let link = format!("<{FIRST_ORG_URL}>; rel=\"next\"");
        let (fake, client) = setup(vec![resp(
            200,
            &[("Link", link.as_str())],
            &page(&["acme/a"]),
        )]);
        let d = discover(&client);
        assert_eq!(
            d.report.status,
            SyncStatus::Incomplete(FetchError::LinkLoop)
        );
        assert_eq!(fake.requests.borrow().len(), 1);
    }

    #[test]
    fn the_page_limit_stops_the_sync() {
        let link = format!("<{NEXT_URL}>; rel=\"next\"");
        let (fake, client) = setup(vec![resp(
            200,
            &[("Link", link.as_str())],
            &page(&["acme/a"]),
        )]);
        let options = SyncOptions {
            max_pages: 1,
            ..SyncOptions::default()
        };
        let d = client.discover_repositories(&org(), &options);
        assert_eq!(
            d.report.status,
            SyncStatus::Incomplete(FetchError::PageLimitReached { max_pages: 1 })
        );
        assert_eq!(fake.requests.borrow().len(), 1);
        assert_eq!(d.report.nodes_added, 1);
        assert!(d.complete_graph().is_none());
    }

    #[test]
    fn a_failure_on_a_later_page_keeps_earlier_pages_but_is_not_complete() {
        let link = format!("<{NEXT_URL}>; rel=\"next\"");
        let (_fake, client) = setup(vec![
            resp(200, &[("Link", link.as_str())], &page(&["acme/a"])),
            resp(
                403,
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", "1700000000"),
                ],
                r#"{"message":"API rate limit exceeded"}"#,
            ),
        ]);
        let d = discover(&client);

        assert_eq!(d.report.pages_fetched, 1);
        assert_eq!(d.graph.node_count(), 1);
        assert!(d.complete_graph().is_none());
        assert_eq!(
            d.report.status,
            SyncStatus::Incomplete(FetchError::RateLimited(RateLimited {
                kind: RateLimitKind::Primary,
                retry: RetryAdvice::AtUnixTime(1_700_000_000),
            }))
        );
        assert_eq!(
            d.report.last_rate_limit,
            Some(RateLimit {
                remaining: Some(0),
                reset_unix: Some(1_700_000_000),
                retry_after_secs: None,
            })
        );
    }

    #[test]
    fn rate_limits_are_classified_by_the_documented_rules() {
        let secondary_body = r#"{"message":"You have exceeded a secondary rate limit."}"#;
        let limited = |kind, retry| {
            SyncStatus::Incomplete(FetchError::RateLimited(RateLimited { kind, retry }))
        };

        // Primary: remaining 0, wait for the reset (works for 403 and 429).
        for status in [403, 429] {
            assert_eq!(
                status_for(resp(
                    status,
                    &[
                        ("x-ratelimit-remaining", "0"),
                        ("x-ratelimit-reset", "1700000000")
                    ],
                    "{}"
                )),
                limited(
                    RateLimitKind::Primary,
                    RetryAdvice::AtUnixTime(1_700_000_000)
                )
            );
        }
        // Secondary with retry-after: wait that long.
        assert_eq!(
            status_for(resp(403, &[("retry-after", "30")], secondary_body)),
            limited(RateLimitKind::Secondary, RetryAdvice::AfterSeconds(30))
        );
        // Secondary, no retry-after, budget not exhausted: at least a minute.
        assert_eq!(
            status_for(resp(403, &[("x-ratelimit-remaining", "5")], secondary_body)),
            limited(RateLimitKind::Secondary, RetryAdvice::AfterSeconds(60))
        );
        // Secondary, no retry-after, budget exhausted: wait for the reset.
        assert_eq!(
            status_for(resp(
                403,
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", "1700000100")
                ],
                secondary_body
            )),
            limited(
                RateLimitKind::Secondary,
                RetryAdvice::AtUnixTime(1_700_000_100)
            )
        );
        // A bare 429 or a bare retry-after: unknown kind.
        assert_eq!(
            status_for(resp(429, &[], "")),
            limited(RateLimitKind::Unknown, RetryAdvice::AfterSeconds(60))
        );
        assert_eq!(
            status_for(resp(403, &[("retry-after", "7")], "")),
            limited(RateLimitKind::Unknown, RetryAdvice::AfterSeconds(7))
        );
    }

    #[test]
    fn a_plain_403_is_a_permission_problem() {
        assert_eq!(
            status_for(resp(
                403,
                &[("x-accepted-github-permissions", "metadata=read")],
                r#"{"message":"Resource not accessible by integration"}"#
            )),
            SyncStatus::Incomplete(FetchError::Forbidden {
                accepted_permissions: Some("metadata=read".to_string())
            })
        );
        assert_eq!(
            status_for(resp(403, &[], "{}")),
            SyncStatus::Incomplete(FetchError::Forbidden {
                accepted_permissions: None
            })
        );
        // A header with unexpected characters is dropped, not echoed.
        assert_eq!(
            status_for(resp(
                403,
                &[("x-accepted-github-permissions", "a\u{1}b")],
                "{}"
            )),
            SyncStatus::Incomplete(FetchError::Forbidden {
                accepted_permissions: None
            })
        );
    }

    #[test]
    fn other_statuses_map_to_distinct_errors() {
        let cases = [
            (401, FetchError::Unauthorized),
            (404, FetchError::NotFound),
            (410, FetchError::Gone),
            (500, FetchError::ServerError(500)),
            (503, FetchError::ServerError(503)),
            (418, FetchError::UnexpectedStatus(418)),
            (304, FetchError::UnexpectedStatus(304)),
        ];
        for (status, expected) in cases {
            assert_eq!(
                status_for(resp(status, &[], "")),
                SyncStatus::Incomplete(expected),
                "status {status}"
            );
        }
    }

    #[test]
    fn transport_failures_are_reported() {
        assert_eq!(
            status_for(Err(TransportError("connection reset".to_string()))),
            SyncStatus::Incomplete(FetchError::Transport("connection reset".to_string()))
        );
    }

    #[test]
    fn malformed_bodies_are_reported_not_ignored() {
        match status_for(ok("not json")) {
            SyncStatus::Incomplete(FetchError::MalformedResponse(m)) => {
                assert!(m.starts_with("invalid JSON at byte 0"), "{m}");
            }
            other => panic!("unexpected status: {other:?}"),
        }
        assert_eq!(
            status_for(ok(r#"{"message":"hi"}"#)),
            SyncStatus::Incomplete(FetchError::MalformedResponse(
                "expected a JSON array of repositories".to_string()
            ))
        );
    }

    #[test]
    fn oversized_bodies_are_rejected_before_parsing() {
        let (_fake, client) = setup(vec![ok(&page(&["acme/a"]))]);
        let options = SyncOptions {
            max_body_bytes: 10,
            ..SyncOptions::default()
        };
        let d = client.discover_repositories(&org(), &options);
        assert_eq!(
            d.report.status,
            SyncStatus::Incomplete(FetchError::BodyTooLarge { limit: 10 })
        );
        assert_eq!(d.graph.node_count(), 0);
    }

    #[test]
    fn unusable_entries_are_skipped_visibly() {
        let body = r#"[
            {"full_name":"acme/ok"},
            {"nope":1},
            5,
            {"full_name":"bad name/x"},
            {"full_name":"acme/ok2"}
        ]"#;
        let (_fake, client) = setup(vec![ok(body)]);
        let d = discover(&client);

        assert_eq!(d.report.status, SyncStatus::Complete);
        assert_eq!(d.report.repositories_seen, 5);
        assert_eq!(d.report.nodes_added, 2);
        let skipped: Vec<(usize, usize)> = d
            .report
            .skipped_items
            .iter()
            .map(|s| (s.page, s.index))
            .collect();
        assert_eq!(skipped, vec![(0, 1), (0, 2), (0, 3)]);
        assert_eq!(d.report.skipped_items[0].reason, "missing full_name");
        assert_eq!(
            d.report.skipped_items[1].reason,
            "entry is not a JSON object"
        );
        // A skipped entry means the inventory may be missing a repository.
        assert!(d.complete_graph().is_none());
    }

    #[test]
    fn repeated_entries_across_pages_are_counted_not_duplicated() {
        let link = format!("<{NEXT_URL}>; rel=\"next\"");
        let (_fake, client) = setup(vec![
            resp(200, &[("Link", link.as_str())], &page(&["acme/a"])),
            ok(&page(&["acme/a", "acme/b"])),
        ]);
        let d = discover(&client);
        assert_eq!(d.report.status, SyncStatus::Complete);
        assert_eq!(d.report.nodes_added, 2);
        assert_eq!(d.report.duplicates_skipped, 1);
        assert_eq!(d.graph.node_count(), 2);
        assert!(d.complete_graph().is_some());
    }

    #[test]
    fn attributes_are_derived_and_sanitized() {
        let body = r#"[
            {"full_name":"acme/priv","private":true},
            {"full_name":"acme/internal","visibility":"internal","archived":true,"fork":true},
            {"full_name":"acme/odd","default_branch":"ma\u0001in","html_url":"","id":12.5}
        ]"#;
        let (_fake, client) = setup(vec![ok(body)]);
        let d = discover(&client);
        assert_eq!(d.report.nodes_added, 3);

        let private = d.graph.node(&node_id("github:repo:acme/priv")).unwrap();
        assert_eq!(attr(private, "visibility"), Some("private"));
        assert_eq!(attr(private, "archived"), None);

        let internal = d.graph.node(&node_id("github:repo:acme/internal")).unwrap();
        assert_eq!(attr(internal, "visibility"), Some("internal"));
        assert_eq!(attr(internal, "archived"), Some("true"));
        assert_eq!(attr(internal, "fork"), Some("true"));

        let odd = d.graph.node(&node_id("github:repo:acme/odd")).unwrap();
        assert_eq!(attr(odd, "default_branch"), None);
        assert_eq!(attr(odd, "html_url"), None);
        assert_eq!(attr(odd, "github_id"), None);
        assert_eq!(attr(odd, "provider"), Some("github"));
    }

    #[test]
    fn owner_names_are_validated() {
        for bad in [
            "",
            "a/b",
            "a b",
            "a?x",
            "../x",
            "é",
            "a%2Fb",
            &"x".repeat(101),
        ] {
            assert_eq!(Owner::organization(bad), Err(InvalidOwner), "{bad:?}");
            assert_eq!(Owner::user(bad), Err(InvalidOwner), "{bad:?}");
        }
        for good in ["acme", "octo-cat", "emu_user", "A1"] {
            assert_eq!(Owner::organization(good).unwrap().name(), good);
        }
    }

    #[test]
    fn base_urls_must_be_https_and_clean() {
        let fake = || {
            Rc::new(Fake {
                replies: RefCell::new(VecDeque::new()),
                requests: RefCell::new(Vec::new()),
            })
        };
        for bad in [
            "http://api.github.test",
            "https://",
            "https:///",
            "api.github.test",
            "https://api.github.test/ x",
            "https://api.github.test?x=1",
            "https://api.github.test#frag",
        ] {
            assert!(GitHubClient::new(fake(), bad).is_err(), "{bad:?}");
        }
        assert!(GitHubClient::new(fake(), PUBLIC_API_BASE).is_ok());
    }

    #[test]
    fn trailing_slashes_on_the_base_url_do_not_double_up() {
        let (fake, client) = setup_at("https://api.github.test//", vec![ok("[]")]);
        discover(&client);
        assert_eq!(fake.requests.borrow()[0].0, FIRST_ORG_URL);
    }

    #[test]
    fn tokens_are_redacted_in_debug_output_and_validated() {
        let token = Token::new("ghp_exampleSecret123").unwrap();
        let shown = format!("{token:?}");
        assert!(!shown.contains("exampleSecret"), "{shown}");
        assert_eq!(shown, "Token(<redacted>)");
        assert_eq!(token.expose(), "ghp_exampleSecret123");
        assert_eq!(token.clone().expose(), token.expose());

        for bad in ["", " ", "a b", "a\nb", "tab\there"] {
            assert_eq!(Token::new(bad).unwrap_err(), InvalidToken, "{bad:?}");
        }
        assert!(!InvalidToken.to_string().contains("ghp_"));
    }

    #[test]
    fn link_headers_are_parsed() {
        let next = |value: &str| next_link(&link_header(value));
        assert_eq!(next_link(&Headers::new()), None);
        assert_eq!(
            next("<https://a.test/1>; rel=\"prev\", <https://a.test/3>; rel=\"next\", <https://a.test/9>; rel=\"last\""),
            Some("https://a.test/3".to_string())
        );
        assert_eq!(
            next("<https://a.test/3>; rel=next, <https://a.test/9>; rel=last"),
            Some("https://a.test/3".to_string())
        );
        assert_eq!(
            next("<https://a.test/3>;rel=\"NEXT\""),
            Some("https://a.test/3".to_string())
        );
        assert_eq!(
            next("<https://a.test/3>; rel=\"next last\""),
            Some("https://a.test/3".to_string())
        );
        assert_eq!(next("<https://a.test/1>; rel=\"prev\""), None);
        assert_eq!(next("rel=\"next\""), None);
        assert_eq!(next("<https://a.test/1"), None);
        assert_eq!(next(""), None);
    }

    #[test]
    fn rate_limit_headers_ignore_garbage() {
        let mut h = Headers::new();
        h.insert("X-RateLimit-Remaining", "abc");
        h.insert("X-RateLimit-Reset", "-5");
        h.insert("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT");
        assert_eq!(
            parse_rate_limit(&h),
            RateLimit {
                remaining: None,
                reset_unix: Some(-5),
                retry_after_secs: None,
            }
        );
        h.insert("x-ratelimit-remaining", " 12 ");
        assert_eq!(parse_rate_limit(&h).remaining, Some(12));
    }

    #[test]
    fn errors_have_readable_messages() {
        let rate = FetchError::RateLimited(RateLimited {
            kind: RateLimitKind::Primary,
            retry: RetryAdvice::AtUnixTime(5),
        });
        assert_eq!(
            rate.to_string(),
            "rate limited (Primary); do not retry before unix time 5"
        );
        assert!(FetchError::Forbidden {
            accepted_permissions: Some("metadata=read".to_string())
        }
        .to_string()
        .contains("metadata=read"));
        assert!(FetchError::NotFound.to_string().contains("404"));
    }
}
