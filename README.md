# NEXUS ZERO

**UNDERSTAND EVERYTHING. CHANGE NOTHING BLINDLY.**

NEXUS ZERO is a Digital Infrastructure Intelligence Platform: an evidence-based model of how an organization's infrastructure fits together, with analysis for dependencies, change impact, and security posture.

> **Status: pre-alpha, foundation stage. Nothing described under "Planned" is implemented yet.**
> This repository contains the license, security policy, design documents, and the first two Rust library crates (`nexus-domain`, `nexus-graph`: typed graph model, impact traversal, snapshots and diff). There is no CLI, no provider integration, no UI, no screenshots, and no benchmarks yet. This README will be updated as capabilities are built and verified.

## What it aims to do

Modern infrastructure is spread across GitHub, Cloudflare, Vercel, Render, Supabase, CI/CD, and more, and each is usually inspected separately. NEXUS ZERO aims to connect them into one typed graph where every relationship records where it came from and how confident we are in it.

Planned capabilities (see [docs/roadmap.md](docs/roadmap.md)):

- **Infrastructure graph**: typed nodes and edges, snapshots, and diffs between snapshots.
- **Discovery**: read-only, authorized integrations, starting with GitHub.
- **Change impact analysis**: upstream/downstream traversal with an explanation for every conclusion.
- **Simulation**: isolated, model-based what-if scenarios, clearly labeled as models and never as guarantees.
- **Security intelligence**: findings that always carry severity, evidence, method, confidence, and remediation.
- **Evidence and audit**: reproducible investigations.

## Principles

- Observed facts and inferred relationships are stored and shown separately.
- No unsupported risk scores, no invented metrics, no fake data outside clearly labeled demo environments.
- Provider integrations are read-only by default; production changes always need explicit human authorization.

## Open source and Enterprise

- **Core (this repository)**: Apache-2.0. The core is intended to be useful on its own; no artificial limits will be added to push users toward Enterprise.
- **Enterprise (planned)**: a separate, private product layer (multi-organization management, SSO, advanced RBAC and audit, compliance reporting). Its source will not live in this repository.

Details: [docs/decisions/0001-license-apache-2.md](docs/decisions/0001-license-apache-2.md).

## Documentation

- [Product vision](docs/product/vision.md)
- [Architecture overview](docs/architecture/overview.md)
- [Threat model (draft)](docs/security/threat-model.md)
- [Roadmap](docs/roadmap.md)
- [Development log](docs/DEVLOG.md)
- [Security policy](SECURITY.md)

## License

Apache License 2.0. See [LICENSE](LICENSE).
