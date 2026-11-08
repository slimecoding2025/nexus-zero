# GitHub integration (repository inventory)

Status: library code and tests only. **It has never talked to the real GitHub API.** There is no HTTP transport in the repository yet; the network sits behind a `Transport` trait and the only implementation is a test double.

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

- It discovers **no relationships** (edges) yet. Planned next: workflow `uses:` references and deployment environments.
- It has no real HTTP transport.
- It never sleeps or retries. A rate limit is returned as an error that says how long to wait; the caller decides.
- Cloudflare, Vercel, Render and Supabase adapters do not exist yet.

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

## JSON parsing

Package installs were unavailable when the adapter was written, so `serde_json` could not be added. The crate contains a small strict JSON parser (RFC 8259, standard library only, nesting limited to 32 levels). Replace it with `serde_json` once dependencies can be fetched.

## Known gaps and open items

- No HTTP transport (needs an HTTP client dependency and a decision on TLS).
- No relationship discovery.
- A malformed `Link` header is currently read as "no next page", which could end pagination early while the report says the sync is complete. Candidate fix: report a malformed `Link` header as an error.
- Permissions of the user-repositories endpoint unverified.
- Not yet run against a real organization, so real response shapes beyond the documented fields are untested.
