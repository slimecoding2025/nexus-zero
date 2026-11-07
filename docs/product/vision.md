# Product vision

NEXUS ZERO is a living, evidence-based representation of an organization's infrastructure, with intelligence for dependency analysis, change impact, security investigation, and controlled simulation.

## Questions it should help answer

1. What does our infrastructure actually look like?
2. Which services depend on one another?
3. What happens if a component fails?
4. What could break after a proposed deployment?
5. Which resources are exposed or misconfigured?
6. How can changes be validated before production?
7. How do we investigate incidents?
8. Which remediation actions are supported by evidence?

## Honest scope

This is a long-term vision. The first releases deliver a small, working foundation (graph engine, GitHub discovery, snapshots, diffs, a CLI). Capabilities are only described as available once they are implemented and tested.

## Differentiation (hypothesis, not yet validated)

Many existing tools catalog services, monitor runtime behavior, or assess cloud security posture. A possible gap is evidence-based, pre-change impact analysis across the developer-platform stack (GitHub, Vercel, Render, Cloudflare, Supabase) where every edge records provenance and confidence. This needs a documented competitive review before it is claimed anywhere public; see the roadmap.
