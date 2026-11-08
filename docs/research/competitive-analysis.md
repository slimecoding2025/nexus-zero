# Competitive analysis (v0, 2026-11-08)

Method: web research on public documentation and vendor pages, read on 2026-11-08. Items marked **not verified** were not confirmed by a source we read. Not yet researched: Datadog, OpenTelemetry, Cloudflare/Vercel/Render/Supabase-specific tooling, Terraform-ecosystem tools beyond those below. Secondary (third-party comparison) sources are flagged as such. No pricing is recorded because the figures we found were inconsistent across sources.

## Summary table

| Project | Kind | License | Core model | Closest overlap with NEXUS ZERO |
|---|---|---|---|---|
| Cartography | Open-source asset graph | Apache-2.0 (CNCF sandbox) | Python collectors writing to Neo4j | Infrastructure graph, GitHub discovery, security rules |
| Backstage | Open-source developer portal | Open source (CNCF; license not re-checked) | Human-declared YAML catalog | Service/dependency catalog |
| Port, Cortex | Commercial developer portals | Commercial SaaS | Blueprints (Port); scorecards (Cortex) | Catalog, ownership, standards |
| Wiz | Commercial cloud security | Closed source | Agentless scanning into a Security Graph | Graph-based risk and attack paths |
| Grafana (entity graph) | Observability | Grafana Cloud docs | Runtime-derived service graph | Blast-radius reasoning from live data |
| Harness IaCM Blast Radius Agent | Commercial CI/CD feature | Commercial | LLM + dependency graph over a Terraform/OpenTofu plan | Pre-apply change impact |
| "Blast Radius" (AWS Builder Center series) | Open-source CLI | Not verified | Reads AWS Config / Resource Explorer for IaC changes | Pre-deploy impact, AWS only |

## Findings by project

### Cartography
- Python tool that pulls infrastructure assets and relationships into a Neo4j graph. Apache-2.0, a CNCF sandbox project.
- Lists 30+ platforms. Its GitHub module covers repositories, branches, users, teams, dependency-graph manifests and dependencies. It also ships a rules runner (`cartography-rules`) for security checks.
- Requires a Neo4j database. Used by organizations including Lyft (listed in its README).
- **Not verified:** whether it has modules for Cloudflare, Vercel, Render or Supabase. This must be checked before any claim about coverage gaps.
- Implication: this is the closest open-source overlap. NEXUS ZERO must either differentiate clearly or consider contributing upstream.
- Sources: github.com/cartography-cncf/cartography, pypi.org/project/cartography.

### Backstage
- Catalog entities are described in YAML descriptors (commonly `catalog-info.yaml`) stored next to code; kinds include Component, API, Resource, System, Domain, Group, User.
- The catalog is therefore largely declared by people rather than discovered, and relations depend on what teams write.
- Third-party comparisons describe a large plugin ecosystem and a multi-week to multi-month setup (secondary sources, not verified first-hand).
- Sources: backstage.io/docs/features/software-catalog/descriptor-format.

### Port and Cortex
- Both are commercial. Port's distinguishing idea is user-defined blueprints for any entity type and its relations; Cortex emphasizes scorecards that track services against standards (secondary comparison sources).
- Their main job is portal, ownership and standards, not evidence-backed change simulation.

### Wiz
- Closed-source, commercial SaaS. Agentless: it reads cloud provider control-plane APIs. Its Security Graph correlates misconfigurations, identities, exposure, secrets and vulnerabilities into attack paths; it has added runtime signals to the graph.
- Public material we read names AWS, Azure, GCP, OCI and Kubernetes. Developer-platform providers were not mentioned in what we read (not verified for the full product).
- Sources: wiz.io blog and academy pages; platformengineering.org/tools/wiz.

### Grafana entity graph
- Documentation describes using the entity graph to assess change impact and blast radius by walking upstream and downstream dependencies. It is derived from observed runtime data, so it reflects what is already instrumented.
- Source: grafana.com/docs/grafana-cloud/knowledge-graph/use-cases/explore-dependencies.

### Harness IaCM Blast Radius Agent
- Runs after a Terraform or OpenTofu plan, combines dependency analysis with LLM reasoning, assigns a 1 to 10 risk score and shows an interactive dependency graph. Documented as pending release.
- Takeaway: pre-apply impact analysis exists commercially for IaC plans, with a numeric score. NEXUS ZERO's stated principle is explanations and evidence without unsupported numeric scores.
- Source: developer.harness.io (blast-radius-agent).

### "Blast Radius" open-source CLI (AWS)
- Discovers downstream AWS dependencies from CDK, CloudFormation or Terraform changes, scores risk, and can gate CI/CD; requires AWS Config and Resource Explorer. AWS-specific.
- Source: builder.aws.com article series. License not verified.

## Where NEXUS ZERO could differ (hypotheses, not claims)

1. **Provenance on every edge:** source, timestamp, discovery method and confidence, with observed facts separated from inferred relationships in the type system. We found no competitor documenting this as a first-class model (not exhaustive).
2. **Evidence-based impact reports without invented risk scores:** conclusions explained by traversed paths and evidence.
3. **Developer-platform stack focus:** GitHub plus Cloudflare, Vercel, Render and Supabase. Gap not confirmed until Cartography and others are checked.
4. **Lightweight deployment:** a Rust core with SQLite for standalone use, avoiding a separate graph database for small teams. A design choice that needs to be proven in practice.

## Risks and open questions

- Cartography already overlaps on graph, GitHub and rules. Validate its provider coverage and decide on positioning (or upstream collaboration).
- Pre-change impact analysis is crowded at the IaC layer (Harness, AWS-specific tools); our angle must be the cross-provider, evidence-based graph, not "impact analysis" in general.
- Do not state "first" or "only" anywhere public.

## Next research steps

1. Read Cartography's module list for Cloudflare, Vercel, Render, Supabase.
2. Research Datadog and OpenTelemetry service maps and their data models.
3. Read each provider's official API docs for read-only scopes and rate limits.
