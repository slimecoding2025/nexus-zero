# Roadmap

Status legend: planned / in progress / done. Nothing below is done unless marked.

## Phase 1: Research (in progress)
- Repository assessment: done (repository was empty at start)
- GSAP Skills review: partial (README read; individual SKILL.md files still to be read)
- Competitive analysis with sources: partial (v0 in docs/research/competitive-analysis.md; Datadog, OpenTelemetry and provider coverage still to do)
- Differentiation validated: not yet (hypotheses listed; Cartography overlap must be checked)

## Phase 2: Architecture
- Domain model, graph design, API contracts: planned
- Threat model v1: planned (draft v0 exists)
- Open-source / Enterprise boundary: planned

## Phase 3: Core engine
- Rust workspace, domain and graph crates, snapshots, diff: in progress (22 unit tests across the three crates pass locally on Rust 1.97; the declared minimum of Rust 1.75 was not re-verified in the latest environment; CI workflow added but not yet confirmed on GitHub; no serialization or storage yet)
- Change impact report (`nexus-analysis`): first version implemented (affected resources with evidence chain, observed vs inferred labeling, Markdown output, fixed limitations section; 6 unit tests). Not yet exposed through a CLI or API, and not yet run against a graph built from real provider data.

## Phase 4: Integrations
- GitHub, then Cloudflare, Vercel, Render, Supabase: planned

## Later
Change impact engine beyond the first report (upstream view, security and availability concerns), security analysis, simulation, dashboard, landing page, Enterprise layer, release.

## v0.1 acceptance criteria (draft)
- A read-only scan of a real GitHub organization produces a graph.
- Two snapshots can be diffed.
- An impact report for a dependency change lists its evidence.
- All claimed tests actually run and pass in CI.
