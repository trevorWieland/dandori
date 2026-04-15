# ADR-0006: GitHub Bidirectional Sync and Conflict State Machine

- Status: Accepted
- Date: 2026-04-15

## Context

Dandori is authoritative but must synchronize with GitHub for issue collaboration.

## Decision

Implement bidirectional sync with deterministic conflict lifecycle:

`clean -> pending -> conflicted -> resolved`

- Inbound webhooks are verified and idempotent.
- Outbound sync is event-driven from outbox.
- Field-aware overlap determines true conflicts.

## Consequences

- Additional sync-state and conflict-resolution complexity.
- Predictable reconciliation behavior and reduced false conflicts.

## Alternatives Considered

- Last-write-wins only: simple implementation, high risk of silent data loss.
