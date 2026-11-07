# Roadmap

Status legend: planned / in progress / done. Nothing below is done unless marked.

## Phase 1: Research (in progress)
- Repository assessment: done (repository was empty at start)
- GSAP Skills review: partial (README read; individual SKILL.md files still to be read)
- Competitive analysis with sources: planned
- Differentiation validated: planned

## Phase 2: Architecture
- Domain model, graph design, API contracts: planned
- Threat model v1: planned (draft v0 exists)
- Open-source / Enterprise boundary: planned

## Phase 3: Core engine
- Rust workspace, domain and graph crates, snapshots, diff: planned

## Phase 4: Integrations
- GitHub, then Cloudflare, Vercel, Render, Supabase: planned

## Later
Change impact engine, security analysis, simulation, dashboard, landing page, Enterprise layer, release.

## v0.1 acceptance criteria (draft)
- A read-only scan of a real GitHub organization produces a graph.
- Two snapshots can be diffed.
- An impact report for a dependency change lists its evidence.
- All claimed tests actually run and pass in CI.
