# Phase 1 Project Brief — Core Domain Foundation

Date: April 15, 2026
Owner: Dandori core team
Phase window: 1 sprint (target 1-2 weeks)

## 1) Purpose

Phase 1 establishes the durable domain and data foundation for all later Dandori work. The goal is to ship a narrow but complete vertical slice of core domain behavior with strict multi-tenant safety, evented mutation semantics, and interface parity scaffolding.

At the end of this phase, Dandori should have a reliable baseline for implementing graph/workflow/auth/sync without revisiting foundational contracts.

## 2) Phase Outcome (Definition of Done)

Phase 1 is complete when all of the following are true:

1. Core aggregates (`workspace`, `project`, `issue`) and baseline metadata tables exist with migrations and compile-time-checked repository APIs.
2. Mutations are command-driven and produce transactional activity + outbox records.
3. Multi-tenancy is enforced in the data model and PostgreSQL RLS is active on tenant tables.
4. One canonical use case is exposed through both REST and MCP using a shared app-service contract.
5. Phase gate commands validate structure, policy, and quality with reproducible local/CI parity.

## 3) Scope and Non-Goals

In scope:

- Domain contracts and typed command/event boundary for core entities
- PostgreSQL schema + migration baseline for core entities, activity log, and outbox
- RLS policy baseline and tenant-context enforcement path
- Shared app-service use case with REST and MCP parity for one write + one read path
- Phase-level test harness and quality gate updates

Out of scope:

- Full dependency graph engine and critical-path logic
- Full workflow engine and transition matrix
- Full OIDC/RBAC service account model
- Full GitHub sync engine and conflict UI
- Frontend feature implementation beyond basic integration stubs

## 4) Key Deliverables (Groundwork-First)

## D1. Domain Contract Baseline

Description:

- Define typed domain IDs, core entities, and command/event contracts for `workspace`, `project`, and `issue`.
- Add explicit error taxonomy for precondition and validation failures.
- Lock crate boundaries so transport and infrastructure cannot own domain rules.

Acceptance criteria:

1. Domain types compile with no stringly-typed state for fixed sets.
2. Command/event contracts are versioned and documented in crate-level docs.
3. Layering guard (`check-deps`) prevents boundary regressions.

Demo checkpoint:

- `cargo check --workspace --all-targets` passes with new domain contracts and no layering violations.

## D2. Store + Migration + RLS Foundation

Description:

- Implement migration set for phase-1 tables: `workspace`, `project`, `issue`, `activity`, `outbox`.
- Enforce tenant column requirements and activate RLS policies for tenant-owned tables.
- Provide repository interfaces for phase-1 read/write operations.

Acceptance criteria:

1. Migrations apply on fresh database and can be validated in CI.
2. RLS blocks cross-tenant and missing-tenant-context access.
3. Repository APIs for phase-1 entities are covered by integration tests.

Demo checkpoint:

- Integration tests show allowed same-tenant access and denied cross-tenant access.

## D3. Transactional Command Write Path

Description:

- Implement one command pipeline that atomically updates aggregate state, appends activity event, and enqueues outbox record.
- Add idempotency handling and deterministic error mapping for duplicate mutation attempts.

Acceptance criteria:

1. No successful state mutation occurs without activity + outbox rows.
2. Replayed idempotent command does not duplicate business state.
3. Outbox records include enough metadata for downstream consumers.

Demo checkpoint:

- End-to-end test demonstrates atomic write behavior under retry and failure conditions.

## D4. REST/MCP Parity Vertical Slice

Description:

- Expose one shared use case through both REST and MCP (recommended: create issue + get issue).
- Keep transport adapters thin and defer all business logic to app-services.

Acceptance criteria:

1. REST and MCP paths produce equivalent business outcomes and error semantics.
2. No domain/policy logic exists in API/MCP adapters.
3. Contract tests validate parity for success and precondition failure cases.

Demo checkpoint:

- Scripted test calls REST and MCP for the same use case and verifies equivalent result envelopes.

## D5. Phase Gate and Operational Baseline

Description:

- Add phase-1 gate target that verifies migrations, parity tests, and policy checks in one reproducible flow.
- Update runbook snippets for local bring-up and validation commands.

Acceptance criteria:

1. `just ci` includes all required phase-1 validations.
2. CI quality-gate job is green with phase-1 artifacts.
3. Developer docs include a clean-clone path to run phase checks.

Demo checkpoint:

- Clean environment run of `just ci` completes without manual intervention.

## 5) Delivery Sequence and Handoff Units

1. D1 domain contracts and crate-boundary hardening
2. D2 migration/RLS baseline
3. D3 transactional command write path
4. D4 REST/MCP parity slice
5. D5 phase gate and docs hardening

Each deliverable should land as PR-sized units with:

- implementation
- tests
- docs updates
- explicit demo command in PR description

## 6) Demo Package for Phase Review

At phase close, present:

1. Command/event contract summary for phase-1 entities.
2. Migration apply log and RLS access-control test evidence.
3. Transactional mutation proof (state + activity + outbox).
4. REST/MCP parity test transcript.
5. Link to green CI run with phase-1 gate.

## 7) Risks and Mitigations (Phase 1)

1. Schema churn risk
- Mitigation: keep phase-1 schema minimal and reserve extensions for phase-2 migrations.

2. Tenant-context misuse risk
- Mitigation: enforce tenant context at repository boundary and test denied paths first.

3. Transport-logic leakage risk
- Mitigation: adapter thinness checks plus parity tests anchored in app-service behavior.

4. Outbox correctness risk
- Mitigation: atomic transaction tests with fault injection around write boundaries.

## 8) Immediate Next Tickets (First 8)

1. Define phase-1 domain contracts and typed IDs in `dandori-domain`.
2. Add phase-1 migration crate modules for core tables + outbox.
3. Implement tenant-context plumbing and RLS policy bootstrap.
4. Implement store repositories for workspace/project/issue read-write paths.
5. Add create-issue command handler with activity + outbox atomic write.
6. Add idempotency-key handling for phase-1 mutation path.
7. Add REST + MCP adapters for create/get issue parity slice.
8. Add integration/contract tests and wire phase checks into `just ci`.
