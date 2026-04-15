# ADR-0002: Contract-First REST and MCP Parity

- Status: Accepted
- Date: 2026-04-15

## Context

Dandori is agent-first and must expose equivalent capabilities through REST and MCP without logic drift.

## Decision

Use a shared contract and app-service layer as the single business-logic source.

- REST and MCP adapters call the same command/query handlers.
- Interface parity is enforced by tests.
- No policy or domain logic is implemented in transport adapters.

## Consequences

- Slightly more initial architecture work.
- Prevents long-term interface drift and duplicate bug surfaces.

## Alternatives Considered

- Independent REST and MCP implementations: faster start, high drift risk.
