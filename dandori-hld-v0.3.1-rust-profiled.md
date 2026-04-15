# Dandori (段取り) — Design Spec v0.3.1 (Rust-First, Profiled)

Status: Draft for implementation
Date: 2026-04-15

## 1. System Identity
Dandori is a multi-tenant, agent-first PM control plane.

- Dandori is authoritative for planning semantics.
- GitHub is a bidirectional synchronized peer projection.
- REST and MCP are equal interfaces over one shared application contract.

## 2. Ten-Pillar Constitution (Hard Constraints)
Every implementation and review decision must satisfy:
1. Completeness
2. Performance
3. Scalability
4. Compile-time strictness
5. Security
6. Stability
7. Maintainability
8. Extensibility
9. Elegance
10. Style

Policy: any exception requires ADR + rollback path.

## 3. Ecosystem-Informed Design Invariants
(Aligned to patterns used in forgeclaw, tanren rewrite, kaidoku)

### 3.1 Contract-first, interface-second
- Canonical domain contract defines commands/events/errors/entities.
- REST/MCP payload schemas are derived from the shared contract layer.
- Interface parity is test-gated; no duplicated business logic per interface.

### 3.2 Event-first with CQRS split
- Request/response operations use typed command handlers.
- Side-effects and observability fanout use typed domain events.
- Rule: command handlers mutate state and emit events atomically.

### 3.3 Typed state machines for lifecycle-critical flows
Model as explicit states + transition guards:
- Issue workflow state transitions
- GitHub sync conflict state
- Worker job state
- Webhook ingestion state

### 3.4 Isolation, not trust
- RLS and scoped auth are the enforcement mechanism.
- Identity context comes from validated auth/session context, never user payload claims.

### 3.5 Thin binaries, thick libraries
- `bin/*` crates are composition roots only.
- Domain/policy/orchestration logic lives in library crates.

## 4. Rust Technology Baseline
- Runtime: `tokio`
- API: `axum` + `tower`
- MCP: Rust MCP SDK (`rmcp`)
- Persistence: PostgreSQL 18+
- ORM/query: SeaORM primary + sanctioned `sqlx` escape-hatch modules for graph/CTE hotspots
- Queue/event execution: PostgreSQL outbox + worker consumers
- Realtime fanout: Valkey pub/sub (non-authoritative)
- GitHub integration: `octocrab`
- Errors: `thiserror` (libs), `anyhow` (bins only)
- Secrets: `secrecy`

## 5. Workspace and Crate Topology
Cargo workspace with strict bounded contexts:

- `crates/dandori-domain` — entities, commands, events, invariants
- `crates/dandori-contract` — schema + interface contract mapping
- `crates/dandori-policy` — RBAC + governance decisions
- `crates/dandori-store` — event log, projections, migrations
- `crates/dandori-graph` — dependency graph + readiness engine
- `crates/dandori-workflow` — state machine + transitions
- `crates/dandori-sync-github` — inbound/outbound/conflict handling
- `crates/dandori-orchestrator` — command coordination
- `crates/dandori-observability` — tracing/metrics/audit helpers
- `crates/dandori-app-services` — stable use-case API for all interfaces

Interface bins:
- `bin/dandori-api`
- `bin/dandori-mcp`
- `bin/dandori-worker`
- `bin/dandori-cli` (optional early)

### Layering rules
1. `domain` imports no workspace crates.
2. transport bins depend on `app-services` + `contract` only.
3. only `store` owns SQL + migration details.
4. `policy` returns typed decisions, never transport-shaped errors.
5. no circular dependencies.

## 6. Compile-Time Strictness Profile
Mandatory workspace policy:
- Rust edition `2024`, stable toolchain pinned in `rust-toolchain.toml`
- `unsafe_code = "forbid"`
- warnings denied in CI (`RUSTFLAGS="-D warnings"`)
- clippy deny list includes: `unwrap_used`, `panic`, `todo`, `dbg_macro`, `print_stdout`, `print_stderr`, `unimplemented`
- inline suppression prohibited (`#[allow]`, `#[expect]`) in source files

Domain typing rules:
- UUIDv7 newtype IDs for all entity identifiers
- enums for finite state/value sets (no stringly-typed states)
- explicit error enums at crate boundaries

## 7. Multi-Tenancy and RLS-First Security
### 7.1 Tenant model
- All tenant-owned tables include `workspace_id`.
- Tenant context is mandatory for API and worker DB sessions.

### 7.2 RLS policy
- RLS enabled on all tenant tables from first migration.
- Default deny when tenant context absent.
- No `BYPASSRLS` for service roles.
- Platform-admin operations isolated behind explicit privileged workflows.

### 7.3 RBAC model
- Workspace role + project role + explicit permission narrowing.
- Service accounts are least-privilege, explicit allow-list only.
- All authz denials are typed outcomes with reason codes.

## 8. Data and Event Architecture
### 8.1 Authoritative write path
Single transaction per mutation includes:
1. aggregate state mutation
2. activity/event append
3. outbox enqueue

### 8.2 Outbox and consumers
- Outbox consumers are idempotent via event id + idempotency key.
- Retries use bounded exponential backoff.
- Poison events move to dead-letter stream/table.

### 8.3 Realtime
- WebSocket is the single live-update transport.
- Events are delivered to clients from projection-safe streams.

## 9. Domain Semantics (Locked)
### 9.1 Relations
- Canonical edge direction only (no inverse duplication rows).
- `blocked_by`: source blocked issue -> target blocker.
- Symmetric types stored once with deterministic ordering.

### 9.2 Readiness
`is_ready = true` iff:
1. issue workflow category is `open`
2. all scheduling-affecting blockers are `done`
3. issue is not archived

### 9.3 Workflow
- State categories fixed: `open|active|done|cancelled`
- Transitions explicit-only and version-pinned per project workflow version
- Invalid transitions return typed precondition failures with allowed transitions

## 10. API and MCP Composition
- `/v1` prefix for all HTTP endpoints.
- MCP tools map 1:1 to app-service use cases.
- parity tests enforce equivalent behavior (status codes/errors/outcomes).
- interface binaries remain thin; no policy/domain duplication.

## 11. GitHub Sync Contract
- Inbound webhooks: signature verification + delivery idempotency + persisted envelope.
- Outbound sync: event-driven from outbox.
- Conflict detection: field-aware overlap detection from synced snapshot baseline.
- Conflict lifecycle: `clean -> pending -> conflicted -> resolved`.
- Resolution actions are explicit commands with audit events.

## 12. Quality Gates (`just ci` as single gate)
Dandori adopts a strict unified gate profile inspired by forgeclaw/tanren/kaidoku.

Required sub-gates:
1. `fmt` (Rust + TOML)
2. `lint` (clippy strict, warnings denied)
3. `check` (`cargo check --workspace --all-targets`)
4. `test` (`cargo nextest run --workspace --profile ci`)
5. `coverage` (`cargo llvm-cov nextest`)
6. `deny` (license/advisory/source policy)
7. `machete` (unused deps)
8. `doc` (`RUSTDOCFLAGS="-D warnings"`)
9. `check-lines` (max 500 lines per `.rs` file)
10. `check-suppression` (no inline lint suppression)
11. `check-deps` (crate layering constraints)
12. `check-ci-parity` (local recipes match CI commands)
13. Architecture guards: thin-interface + store-boundary checks

## 13. Test Strategy (Three-Tier)
### Unit tests
- colocated in source (`#[cfg(test)]`), no I/O/network, target <250ms/test.

### Integration tests
- under `tests/`, real crate public API, target <5s/test.
- use wiremock/testcontainers as needed.

### Property + snapshot tests
- `proptest` for invariants and state transitions.
- `insta` for stable structured outputs and contract fixtures.

Rules:
- no skipped tests (`#[ignore]` prohibited)
- no conditional silent skips
- flaky tests must be fixed or removed

## 14. Observability and Audit Defaults
- Structured logs via `tracing` with correlation IDs.
- OTel traces across API -> store -> worker -> sync adapters.
- Metrics: queue lag, outbox lag, transition failures, RLS denials, sync conflict counts.
- Immutable activity trail for all mutation commands and policy decisions.

## 15. Delivery Model (Phase-Gated, Vertical Slices)
Each phase must ship a working, demonstrable increment with explicit exit criteria.

### Phase 0 Foundation
- workspace, lints, migrations skeleton, contract primitives, outbox scaffold.

### Phase 1 Core domain
- projects/issues/workflow/relation schema + command handlers + event append.

### Phase 2 Graph/workflow engine
- cycle detection, readiness recompute, strict transition execution.

### Phase 3 Auth/policy/rls
- OIDC authn, RBAC, service-account policy, RLS end-to-end tests.

### Phase 4 Sync and workers
- webhook ingestion, outbound sync, conflict handling, dead-letter behavior.

### Phase 5 Interfaces and live UX
- board/detail APIs, MCP parity, WebSocket live updates.

### Phase 6 Hardening
- perf tuning, load tests, runbooks, security audit, recovery drills.

## 16. Build-Start Decisions Still Needed
1. SeaORM-only vs SeaORM+sqlx hybrid from phase 1.
2. Worker runtime framework selection around outbox polling/execution.
3. Initial GitHub conflict resolver UX depth (minimal vs advanced).
4. Tenant context propagation mechanism (DB session variable pattern).
5. Internal alpha vs beta SLO thresholds.

## 17. Practical Implementation Posture
- Internal tool first, enterprise-grade scaffold from day one.
- Breaking changes allowed during internal build phase with revision notes.
- Before external release: enforce strict compatibility policy.

