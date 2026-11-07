# Architecture overview (proposal)

Start as a modular monolith; extract services only when justified.

## Workspace layout (proposed)

```
crates/
  domain/        typed resources, relationships, evidence, errors
  graph/         graph store, traversal, snapshots, diff
  discovery/     provider adapters (GitHub first)
  analysis/      change-impact analysis
  security/      audit rules and findings
  simulation/    isolated model-based scenarios
apps/
  cli/           command-line interface
  api/           REST API (OpenAPI documented)
  web/           React + TypeScript + Vite
enterprise/      NOT in this repository (separate private product layer)
```

Packages are added only when code needs them.

## Graph model

- **Node types** (initial): Repository, Deployment, Service, Database, Api, Domain, Environment, Worker, Storage, IdentityProvider, NetworkResource.
- **Edge types** (initial): DependsOn, DeployedTo, ConnectsTo, Exposes, AuthenticatesWith, StoresDataIn, BuiltFrom, CommunicatesWith.
- **Every edge records**: source, timestamp, provenance, confidence, discovery method, metadata.
- **Observed vs inferred** is a first-class distinction in the type system, so inferred edges are never presented as confirmed facts.
- **Snapshots** are immutable and versioned; diffs compare two snapshots.

## Storage

SQLite for local and standalone use; PostgreSQL (optionally via Supabase) for hosted deployments. Both behind one storage interface.

## Integrations

Priority: GitHub, Cloudflare, Vercel, Render, Supabase. Each adapter handles authentication, least-privilege scopes, pagination, rate limits, failed syncs, and has its own tests. Tokens stay server-side.

## Open trade-offs to resolve (documented as decisions)

- Graph storage: relational tables vs an embedded graph library.
- Sync model: full vs incremental per provider.
- Plugin boundary: in-process traits vs out-of-process.
