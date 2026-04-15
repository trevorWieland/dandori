# ADR-0003: Event-First Write Path with Transactional Outbox

- Status: Accepted
- Date: 2026-04-15

## Context

Dandori needs durable auditability, replayability, and reliable asynchronous processing for projections, readiness, and sync.

## Decision

Every mutation transaction must atomically:

1. Update aggregate state.
2. Append an activity/domain event.
3. Enqueue an outbox record.

Outbox consumers are idempotent and retry-safe.

## Consequences

- Additional write-path complexity.
- Strong correctness under retries/failures and clear audit lineage.

## Alternatives Considered

- Direct side-effect dispatch without outbox: simpler code, high loss/duplication risk.
