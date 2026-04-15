# ADR-0005: Workflow and Dependency Graph Semantics

- Status: Accepted
- Date: 2026-04-15

## Context

Planner correctness depends on deterministic workflow transitions and dependency semantics.

## Decision

Adopt strict, canonical graph/workflow semantics:

- Canonical relation storage direction only (no inverse duplicate rows).
- `blocked_by`: source is blocked issue, target is blocker.
- `is_ready` is materialized and recomputed from scheduling-affecting blockers.
- Workflow transitions are explicit-only and validated against workflow versions.

## Consequences

- Requires robust cycle detection and recompute workers.
- Enables deterministic readiness and reliable agent work-picking.

## Alternatives Considered

- Flexible relation duplication and inferred transitions: easier authoring, ambiguous runtime behavior.
