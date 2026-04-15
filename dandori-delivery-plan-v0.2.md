# Dandori — Delivery Plan v0.2

Date: 2026-04-15
Target: Production-ready V1 with controlled risk

## 1) Program Structure
Use 6 implementation phases with entry/exit gates. Do not start a phase without passing prior exit gate.

## 2) Phase Plan

### Phase 0: Foundations (1 week)
Scope:
- Repo scaffold (API, worker, frontend)
- Config loading, health endpoints, logging baseline
- CI pipeline with lint, type-check, test, migration check

Deliverables:
- Running local stack via Docker Compose
- Baseline OpenAPI + MCP service booting

Exit Criteria:
- CI green on main
- One smoke endpoint and one MCP tool validated in integration test

### Phase 1: Domain Core (2 weeks)
Scope:
- Workspace/project/milestone/issue schema
- Workflow version model
- Activity log transactional append
- CRUD for projects/milestones/issues with row-version checks

Deliverables:
- Alembic migrations + seed data
- Domain services and repository layer

Exit Criteria:
- Migration upgrade/downgrade pass on fresh + fixture DB
- 90%+ coverage on domain services for core aggregates

### Phase 2: Graph + Workflow Runtime (2 weeks)
Scope:
- Relation types and canonical edge persistence
- Cycle detection and readiness recompute queue
- Transition engine with condition checks and side effects

Deliverables:
- Blockers/dependents/graph APIs
- Ready-issues query optimized with indexes

Exit Criteria:
- Property tests for cycle detection and readiness correctness
- Transition matrix tests for default workflow (all valid/invalid paths)

### Phase 3: Auth + RBAC (1.5 weeks)
Scope:
- OIDC validation middleware
- Member/workspace/project roles and permission resolver
- Service-account explicit permission policy

Deliverables:
- Permission-guarded endpoints and MCP tools
- RBAC test matrix for owner/developer/viewer/service account/admin

Exit Criteria:
- Zero unauthorized access in negative tests
- Token validation failure cases covered (issuer, audience, expiry, signature)

### Phase 4: GitHub Sync (2 weeks)
Scope:
- GitHub App webhook receiver + signature/idempotency checks
- Inbound/outbound sync workers with snapshot-based diff
- Conflict state machine and resolution endpoint
- Initial import CLI

Deliverables:
- Sync state persistence and admin visibility
- Dead-letter handling and retry policy

Exit Criteria:
- Replay-safe webhook tests
- Conflict fixtures: overlapping field edits conflict, disjoint edits auto-merge

### Phase 5: Frontend + Integrations (2.5 weeks)
Scope:
- Board, issue panel, milestones, settings, search surface
- WebSocket live updates
- MCP integration touchpoints for tanren/nanoclaw

Deliverables:
- Usable end-to-end planning workflow
- Integration playbooks for service accounts

Exit Criteria:
- E2E tests for create -> plan -> transition -> done path
- Latency targets met in staging dataset (>=500 issues/project)

## 3) Cross-Cutting Quality Tracks
- Testing: unit + integration + E2E per phase (no deferred test debt)
- Observability: logs/metrics/traces added as each feature lands
- Security: threat-model review at end of Phase 3 and Phase 4
- Data safety: migration rollback drill before Phase 5 deploy

## 4) Risk Register

| Risk | Probability | Impact | Mitigation | Owner |
|---|---:|---:|---|---|
| Dependency semantics bugs cause incorrect readiness | M | H | Canonical edge rules + property tests | Backend |
| Workflow edits break active projects | M | H | Immutable workflow versions + migration endpoint | Backend |
| GitHub webhook duplication/out-of-order events | H | H | Delivery idempotency + snapshot merge + retries | Integrations |
| RBAC gaps expose write actions | M | H | Deny-by-default + exhaustive permission tests | Platform |
| Queue backlog delays readiness/sync | M | M | Queue depth alerts + autoscale worker concurrency | Platform |

## 5) Definition of Done (Program)
V1 is done when all are true:
1. All phase exit criteria pass.
2. Critical-path integrations (tanren, nanoclaw) run in staging for 7 days without Sev-1/Sev-2 defects.
3. GitHub sync conflict workflow is exercised by at least 3 real scenarios and resolved correctly.
4. Runbooks exist and on-call dry-run is completed.
5. Documentation includes API, MCP tool catalog, and operator guide.

## 6) First 15 Build Tickets (Recommended)
1. Bootstrap API/worker/frontend services and CI checks.
2. Add config schema + env parsing + startup validation.
3. Implement base DB models + shared mixins (`id`, timestamps, `row_version`).
4. Add workflows + workflow versions schema and seed default flow.
5. Add issue schema with `archived_at`, `is_ready`, `custom_fields`.
6. Add activity schema with `project_seq` and idempotency key.
7. Build project/issue CRUD with transactional activity append.
8. Implement transition engine with strict transition validation.
9. Add relation type registry and canonical relation persistence.
10. Implement cycle detection CTE and relation insertion guard.
11. Add readiness recompute worker + debounce/dedupe.
12. Add blockers/dependents/graph APIs.
13. Implement OIDC auth dependency + member resolution.
14. Implement RBAC resolver and endpoint guards.
15. Add webhook receiver with signature verification + dedupe store.

## 7) Weekly Execution Cadence
- Monday: planning and risk review
- Daily: burnup and blocker review
- Thursday: integration demo and defect triage
- Friday: release candidate cut + rollback readiness check

