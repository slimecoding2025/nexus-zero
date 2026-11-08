# Development log

## 2026-11-07
- Repository found empty; project directive received.
- Read the GSAP Skills README. Individual skill files could not be fetched automatically and remain to be reviewed.
- Chose Apache-2.0 for the core (see decision 0001).
- Added license, README, security policy, and initial design documents (no code yet).
- Known limitation: the Rust toolchain was not available in the working environment at this point, so no Rust code has been compiled or tested.

## 2026-11-08
- First batch pushed to GitHub (license, README, security policy, design docs).
- Competitive research v0 written (Cartography, Backstage, Port/Cortex, Wiz, Grafana, Harness blast radius, AWS Blast Radius). Closest overlap found: Cartography (Apache-2.0, CNCF sandbox, Neo4j, GitHub module). Differentiation is still a hypothesis.
- Not yet done: Datadog/OpenTelemetry research, GSAP skill files, Rust toolchain setup.
- Installed Rust 1.75 (Ubuntu packages) in the working environment; cargo test, rustfmt and clippy (-D warnings) ran locally.
- Added crates `nexus-domain` (typed nodes/edges, provenance, observed vs inferred, coarse confidence) and `nexus-graph` (graph store, impact traversal, dependency paths, snapshots, diff).
- Modeling assumption to review (ADR pending): every edge kind means "from depends on to" except Exposes, which is reversed for impact traversal. CommunicatesWith is treated as directional for now.
- Diff ignores observation timestamps and evidence references on edges, since they change on every scan.
- CI workflow added (fmt, clippy, test); it has not run on GitHub yet.
