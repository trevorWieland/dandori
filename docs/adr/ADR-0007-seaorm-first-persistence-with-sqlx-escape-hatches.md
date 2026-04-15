# ADR-0007: SeaORM-First Persistence with sqlx Escape Hatches

- Status: Accepted
- Date: 2026-04-15

## Context

Dandori needs maintainable persistence defaults plus high-performance query paths for graph/CTE workloads.

## Decision

Use SeaORM as the primary persistence and migration framework.
Use targeted `sqlx` modules as explicit escape hatches for SQL paths SeaORM cannot express cleanly (for example: RLS session context, outbox lease/poll with `FOR UPDATE SKIP LOCKED`, and targeted fault-injection probes).
When using raw SQL escape hatches, enforce compile-time query checking with SQLx metadata in CI.

## Consequences

- Two persistence tools must be governed consistently.
- Better balance of maintainability and performance control.

## Alternatives Considered

- SeaORM-only: simpler stack, potential hot-path query constraints.
- sqlx-only: high control, more manual mapping/boilerplate.
