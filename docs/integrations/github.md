# GitHub integration (repository inventory)

Status: library code and tests only. **It has never talked to the real GitHub API.** The network sits behind a `Transport` trait; the only built and tested implementation is a test double. An HTTPS transport (`http::UreqTransport`, feature `http`) was written but **never compiled or run** (see "HTTP transport").

Crate: `nexus-discovery`, module `github`.

## What it does

`GitHubClient::discover_repositories` lists the repositories of one organization or user and turns each into a `Repository` node. It returns a `Discovery`: the graph built so far plus a `SyncReport`.

Request sent for an organization:

```
GET {base}/orgs/{org}/repos?per_page=100&sort=full_name&direction=asc
Accept: application/vnd.github+json
X-GitHub-Api-Version: 2022-11-28
```

The API version is pinned on purpose so response shapes do not change silently.

## What it does not do

- Relationship discovery covers only `uses:` references in workflow files (see below). Deployment environments, package manifests and other relationships are not discovered.
- Its HTTPS transport is optional (feature `http`) and unverified.
- It never sleeps or retries. A rate limit is returned as an error that says how long to wait; the caller decides.
- Cloudflare, Vercel, Render and Supabase adapters do not exist yet.

## Relationship discovery (workflow `uses:` references)

`GitHubClient::discover_workflow_dependencies` takes the inventory from `discover_repositories` and, for each listed repository, lists `.github/workflows` and reads each `.yml`/`.yaml` file with the raw media type (`application/vnd.github.raw+json`). Every `uses: owner/repo[/path]@ref` line becomes a `DependsOn` edge from the repository to the referenced repository. Edges are **observed** (the text is in the file), method `ManifestParse`, with evidence such as `acme/web/.github/workflows/ci.yml:8 (uses: actions/checkout@v4)`.

- Reading file contents needs the **Contents** read permission, which the inventory alone does not need. Per GitHub's docs, a missing path answers 404 and directory listings are limited to 1,000 files. Both checked on 2026-11-09 against the docs; the per-endpoint permission wording was not re-checked after that.
- Repositories referenced but not in the inventory are added as nodes with `referenced_only=true`.
- Skipped: local (`./`) and `docker://` references, block scalars, and unusual values. This is a line scanner, **not a YAML parser**: it can miss references written in unusual YAML (flow style, anchors) and cannot see reusable workflows' own dependencies. Absence of an edge does not prove absence of a dependency.
- Limits: files per repository (default 100) and bytes per file (default 1 MiB). Skips and fetch errors are recorded in the report; `WorkflowDiscovery::complete_graph()` returns the graph only when nothing was skipped or failed.
- Tested with a test double only. Never run against the real API.

## What is collected

Inventory facts only: full name, numeric id, visibility, archived, disabled, fork, default branch and web URL, plus a `provider` attribute. Descriptions, topics, counts, owner details and `pushed_at` are not stored. `pushed_at` changes on every push, so storing it would make snapshot diffs noisy without any infrastructure change.

Node ids are `github:repo:{owner}/{name}` in lowercase, because repository names are case-insensitive on GitHub.

## Permissions (least privilege)

For organizations, GitHub's documentation for `GET /orgs/{org}/repos` lists read access to the **Metadata** repository permission for fine-grained personal access tokens and GitHub App tokens. Checked against the GitHub docs page for this endpoint (the Enterprise Server 3.22 edition of the REST reference) on 2026-11-09. No write permission and no repository contents are needed for the inventory.

Not verified: the permissions of `GET /users/{username}/repos`, used by `Owner::user`. Treat user listing as unverified until it is checked against the docs.

## Pagination

The client follows `Link: rel="next"` and refuses to follow a link that is not under the configured API base URL, so the token is never sent to another host. It also stops on a link loop and after a page limit (default 500 pages), and rejects response bodies over a size limit (default 16 MiB).

## Rate limits and errors

HTTP and rate-limit failures are classified into distinct errors: 401, 403 (permission), 404 (owner missing or not visible; GitHub answers 404 for private resources), 410, 5xx, and rate limits split into primary, secondary and unknown with retry advice. The classification follows the client rules cited in the source comments; those rules were not re-checked against GitHub's current documentation when this page was written.

## Partial results

If a sync stops early (rate limit, network error, malformed page) the graph holds what was read so far. A partial inventory must not be diffed against a complete one, because the missing repositories would look removed. `Discovery::complete_graph()` returns the graph only when every page was read and no entry was skipped.

## Authentication and secrets

The adapter never sees a token. A `Transport` implementation holds the credential and adds the `Authorization` header. The `Token` type exists so secrets have one type whose `Debug` output is redacted. Tokens must stay on the server, never reach the frontend, and never be committed to the repository.

A `Transport` implementation must:

- send the headers it is given unchanged;
- add authentication and a `User-Agent`, and nothing else;
- never follow a redirect to a different host;
- return a non-2xx status as a normal response, not as an error.

## HTTP transport (feature `http`, unverified)

`http::UreqTransport` implements `Transport` with `ureq` 2.x (rustls). Off by default, so the default build does not use it. Written when crates.io was unreachable, so it has **not been compiled, linted or run**, and `Cargo.lock` does not yet contain `ureq`; the first build with network access will update it.

Design, by construction (only the origin checks are covered by tests):

- Only `https://` origins; the token is sent only to URLs under the configured origin (default `https://api.github.com`), anything else is refused before a request.
- Redirects are never followed.
- Non-2xx responses are returned as `Response`; only network failures are `TransportError`.
- Bodies are read up to a limit plus one byte (default equals `DEFAULT_MAX_BODY_BYTES`; keep it equal to `SyncOptions::max_body_bytes`).
- Repeated response headers are joined with commas.
- 30 second timeout. No retries, no sleeping.

To verify: `cargo build -p nexus-discovery --features http`, then clippy and tests with the same flag.

## JSON parsing

Package installs were unavailable when the adapter was written, so `serde_json` could not be added. The crate contains a small strict JSON parser (RFC 8259, standard library only, nesting limited to 32 levels). Replace it with `serde_json` once dependencies can be fetched.

## Known gaps and open items

- The HTTP transport is unverified: it was written offline against the `ureq` 2.x API from memory. Needs `cargo build/clippy/test --features http` and `Cargo.lock` regenerated with network access, then a run against a real organization.
- Relationship discovery is limited to workflow `uses:` references and uses a line scanner instead of a YAML parser.
- Resolved: a malformed `Link` header now stops the sync with `FetchError::MalformedLinkHeader` instead of being read as "no next page".
- Permissions of the user-repositories endpoint unverified.
- Not yet run against a real organization, so real response shapes beyond the documented fields are untested.
