# Threat model (draft v0)

Scope: the open-source core while it holds provider credentials and infrastructure metadata. To be revised before any sensitive feature is implemented.

## Assets

Provider credentials; infrastructure graph and snapshots (reveal topology and exposure); findings and evidence; audit logs.

## Main threats and planned mitigations

| Threat | Mitigation direction |
|---|---|
| Credential theft or leakage | Server-side storage only, encryption at rest, no tokens in logs or the frontend, secret scanning in CI |
| Over-privileged integrations | Minimum scopes, read-only by default, scope documented per provider |
| Cross-tenant data access (hosted mode) | Tenant ID enforced in the data layer, authorization tests for every endpoint |
| Malicious or malformed provider data | Strict validation and typed parsing, size limits, safe error handling |
| Simulation abuse | No arbitrary code execution; model-based scenarios only; sandboxing before anything executable is considered |
| Unauthorized production changes | The core performs no production writes; any future write needs explicit human approval and an audit entry |
| Tampered evidence or audit trail | Append-only records, timestamps, provenance fields |
| Supply-chain risk | Pinned dependencies, dependency scanning, license review |
| Denial of service via API | Rate limiting, pagination, request size limits |

## Out of scope for now

Hosted multi-tenant operation, SSO, and enterprise controls (Enterprise layer).
