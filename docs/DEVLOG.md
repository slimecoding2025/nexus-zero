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
- Resumed work from a fresh clone. Toolchain in this environment is Rust 1.97.0; fmt, clippy (-D warnings) and all 16 existing tests passed before any change.
- The default git identity in this environment was not the owner's. A repo-local identity matching the owner's existing commits was set before committing, and no global git config was changed.
- Added crate `nexus-analysis` on branch `feat/analysis-impact-report`: `analyze_change` returns the resources that may be affected by a proposed change, each with its chain of relationships, provenance and evidence references. Each result is labeled as reached through observed relationships only, or through at least one inferred relationship (with its weakest confidence). There is no numeric score. The Markdown output always ends with a limitations section (static analysis, undiscovered dependencies, undocumented redundancy).
- `cargo fmt --check` initially flagged one signature in the new crate; fixed with `cargo fmt`. After that, fmt, clippy (-D warnings) and 22 tests (6 domain, 10 graph, 6 analysis) passed locally.
- Not verified: CI on GitHub, Rust 1.75 (declared minimum), behavior on a graph built from real provider data.
