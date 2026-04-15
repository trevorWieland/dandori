# Dandori (段取り) — Design Spec v0.3 (Rust-First, Enterprise Quality)

Status: Draft for implementation
Date: 2026-04-15
Primary Audience: Aegis platform engineers

## 1. Product Thesis
Dandori is a self-hosted, multi-tenant, agent-first project management system.

It is not a GitHub wrapper. It is the authoritative operational system for planning and execution, with GitHub as a synchronized peer projection.

## 2. Quality Constitution (Non-Negotiable)
All architecture and implementation decisions are judged against these 10 pillars:

1. Completeness
2. Performance
3. Scalability
4. Strictness of compile-time verification
5. Security
6. Stability
7. Maintainability
8. Extensibility
9. Elegance
10. Style

Any design that violates a pillar must provide a written exception with rollback path.

## 3. Locked Decisions for v0.3
1. Language/runtime: Rust (stable channel).
2. Authoritative state: Dandori DB.
3. API strategy: REST and MCP are first-class adapters over one shared domain service layer.
4. Event architecture: Event-first with transactional outbox in PostgreSQL.
5. Realtime transport: one singular live channel strategy (WebSocket).
6. Multi-tenancy: from day one with PostgreSQL RLS enforcement.
7. Queue: no ARQ; durable worker pipeline via DB outbox + worker consumers.
8. Sync model: bidirectional GitHub sync with deterministic conflict workflow.

## 4. Technology Stack (Rust)
### Backend
- Web API: `axum`
- Async runtime: `tokio`
- Serialization: `serde`, `serde_json`
- Validation: `validator` + custom domain validators
- DB access: SeaORM (primary), `sqlx` for graph/CTE/perf-critical queries
- Migrations: `sea-orm-migration`
- MCP server: official Rust MCP SDK (`rmcp`)
- GitHub SDK: `octocrab`

### Data and Infra
- Database: PostgreSQL 18+
- Cache and pub/sub fanout: Valkey
- Durable events and background execution: PostgreSQL outbox + worker
- Deployment: Docker Compose (internal), Kubernetes-ready boundaries

### Frontend
- React 19 + TypeScript strict mode
- Vite
- shadcn/ui + Tailwind v4
- Strict linting and type gates in CI

## 5. System Topology
1. API service receives command/query requests.
2. Command handlers execute domain logic and append domain events atomically.
3. Outbox dispatcher publishes events to worker pipelines.
4. Workers update projections, compute readiness, run sync jobs, and publish live updates.
5. WebSocket gateway streams typed events to clients.
6. MCP adapter calls same domain services used by REST.

Key rule: no business logic in transport adapters.

## 6. Domain Model Principles
### 6.1 Metadata-Driven Core
Workflows, relation types, labels, and custom fields remain data-driven and tenant-scoped.

### 6.2 Canonical Relation Semantics
- Directed dependency edge canonical form:
  - `blocked_by`: source is blocked issue, target is blocker
  - `parent_of`: source is parent, target is child
- Symmetric relations use one canonical ordered pair.

### 6.3 Readiness Semantics
`issue.is_ready = true` only when:
1. issue state category is `open`
2. all scheduling-affecting blockers are in `done`
3. issue is not archived

Readiness is materialized and event-recomputed, never recalculated ad hoc for hot paths.

### 6.4 Workflow Semantics
- Workflow state categories are fixed semantic primitives: `open`, `active`, `done`, `cancelled`.
- Project workflows are versioned and immutable once in use.
- Transitions are explicit-only and validated against workflow version.

## 7. Multi-Tenancy and RLS Contract
### 7.1 Tenant Model
- Every tenant-owned row includes `workspace_id`.
- All API and worker DB sessions set tenant context before queries.

### 7.2 RLS Rules
- RLS enabled on all tenant-owned tables from day one.
- Default deny when tenant context not present.
- Service roles do not use `BYPASSRLS`.
- Cross-tenant operations are forbidden except explicit platform-admin workflows.

### 7.3 Query Safety
- Domain repository APIs always require tenant context.
- Raw SQL (`sqlx`) wrappers must include static tenant predicates plus RLS.

## 8. Security Model (Zero Trust)
1. OIDC JWT auth with strict issuer/audience/signature/time validation.
2. RBAC layered as workspace role + project role + explicit permission narrowing.
3. Service accounts are least-privilege and project-scoped.
4. All mutations are auditable and actor-attributed.
5. Secrets are externalized only (env/secret mounts), never embedded in configs.
6. Supply-chain checks run in CI (`cargo audit`, dependency policy checks).

## 9. Event-First and EDA Design
### 9.1 Write Path
Each mutation transaction must include:
1. aggregate state update
2. domain event append
3. outbox message enqueue

This guarantees no state mutation without an event trail.

### 9.2 Event Contracts
- Domain events are versioned schemas.
- Consumers are idempotent using event IDs and idempotency keys.
- Retriable failures use exponential backoff; poison messages go to dead-letter storage.

### 9.3 Valkey Role
Valkey pub/sub is for low-latency fanout and realtime delivery only.
Durable sequencing and correctness rely on PostgreSQL.

## 10. API and MCP Best-Practice Composition
### 10.1 Layering
- Transport layer: REST handlers and MCP tools
- Application layer: commands/queries and authorization checks
- Domain layer: aggregates, invariants, state transitions
- Infrastructure layer: DB, queue dispatch, integrations

### 10.2 Single Source of Business Logic
REST and MCP adapters call identical command/query handlers.
No duplicated validation or policy logic between adapters.

### 10.3 Contract Discipline
- API base path `/v1`
- Breaking changes allowed during internal phase behind revision notes
- Before external launch, adopt strict compatibility policy and schema versioning

## 11. GitHub Sync (Bidirectional, Authoritative Local)
1. Inbound webhooks are verified, idempotent, and persisted.
2. Mapping applies only supported mirrored fields.
3. Conflict policy is field-aware and deterministic.
4. Non-overlapping changes auto-merge.
5. Overlapping changes enter explicit conflict state requiring resolution action.
6. Sync operations are fully evented and auditable.

## 12. Performance and Scale Strategy
### 12.1 Baseline Targets (Internal v1)
- P95 board query: < 250ms at 500 issues/project
- P95 transition mutation: < 120ms
- P95 readiness propagation: < 2s
- P95 webhook apply latency: < 10s

### 12.2 Scale Architecture Guardrails
- Partition large append-only tables by time and workspace where appropriate
- Use covering/partial indexes for ready queue and board queries
- Cache only read models; never cache authoritative write invariants
- Keep command path deterministic and side-effect bounded

### 12.3 Million-User Readiness Posture
v1 remains internal, but all contracts avoid single-tenant shortcuts that block horizontal scale.

## 13. Compile-Time Strictness Standards (Rust)
1. `#![forbid(unsafe_code)]` unless explicitly approved by ADR.
2. `#![deny(warnings)]` in CI.
3. Clippy strict profile with denied lints for correctness and style.
4. Newtype wrappers for domain identifiers and constrained values.
5. State transition APIs modeled with strongly typed command objects.
6. Error types are explicit enums; no stringly-typed domain failures.
7. Exhaustive `match` for domain state categories.

## 14. Stability and Reliability Standards
1. Idempotency required on all mutation endpoints.
2. Optimistic concurrency controls on mutable aggregates.
3. Retry-safe consumers with dedupe store.
4. Blue/green friendly migration sequencing.
5. Feature flags for high-risk integrations.

## 15. Maintainability, Extensibility, Elegance, Style
### 15.1 Maintainability
- Bounded contexts by module (`issues`, `workflow`, `relations`, `sync`, `auth`)
- Clear ownership boundaries and interface traits
- ADR-required for non-trivial cross-module coupling

### 15.2 Extensibility
- Metadata tables drive relation/workflow/custom field evolution
- Event schemas versioned with migration adapters
- Integration connectors isolated behind trait-based ports

### 15.3 Elegance
- Prefer simple, composable domain primitives
- Remove speculative abstraction with no immediate leverage
- Optimize hot paths only with measured evidence

### 15.4 Style
- Rustfmt, clippy, and style guide enforced in CI
- Modern idioms only; no legacy pattern carryover without justification

## 16. Observability and Auditability
1. Structured logs with correlation IDs and actor context.
2. OpenTelemetry traces across API, worker, DB, and integrations.
3. Metrics for queue depth, lag, conflicts, transition failures, and RLS denials.
4. Immutable activity trail for all mutating operations.

## 17. Delivery Model and Gates
### Gate A: Architectural Integrity
- Multi-tenant schema + RLS active
- Event write path + outbox operational
- Shared command/query layer used by REST and MCP

### Gate B: Core Domain Integrity
- Workflow transitions validated strictly
- Relation cycle detection property-tested
- Readiness propagation verified under concurrency

### Gate C: Security Integrity
- OIDC and RBAC matrix fully tested
- Service-account least privilege validated
- Supply-chain and static checks green

### Gate D: Integration Integrity
- GitHub sync happy path and conflict path pass
- WebSocket event stream stable under load
- MCP and REST behavior parity tests pass

### Gate E: Operational Integrity
- SLO dashboards live
- Alerting and runbooks complete
- Backup/restore and migration rollback drill passed

## 18. Pillar-to-Architecture Traceability
1. Completeness: explicit domain contracts + gate-based delivery
2. Performance: hot-path indexing + async workers + measured optimization
3. Scalability: tenant-safe schema, partition-ready event stores, stateless APIs
4. Compile-time strictness: Rust type system + denied warnings + strict lint profile
5. Security: zero trust authz/authn + RLS + immutable audit
6. Stability: idempotency + optimistic locking + retries and dead-lettering
7. Maintainability: modular bounded contexts + ADR discipline
8. Extensibility: metadata-driven model + versioned events and workflows
9. Elegance: constrained abstractions and explicit invariants
10. Style: enforced modern Rust and TypeScript standards

## 19. Open Questions to Finalize Before Build Starts
1. SeaORM-only or SeaORM + sqlx hybrid from day one for graph-heavy queries?
2. Exact worker framework choice around outbox consumption.
3. GitHub sync conflict UX scope for v1 (minimal resolver vs advanced merge view).
4. Default tenant/session context mechanism in DB pool layer.
5. Exact SLO numbers for internal alpha vs beta.

## 20. Initial Build Order (Rust v0.3)
1. Project skeleton (`api`, `worker`, `mcp`, `frontend`) and CI quality gates.
2. Tenant/RLS-first schema and migrations.
3. Core domain aggregates and command/query handlers.
4. REST + MCP adapters on shared handlers.
5. Outbox/event pipeline and readiness projection workers.
6. Workflow/relation engines and graph queries.
7. Auth/RBAC/service-account policies.
8. GitHub sync inbound/outbound/conflict flow.
9. Frontend board/detail/settings + live updates.
10. Hardening, observability, runbooks, and release criteria.

