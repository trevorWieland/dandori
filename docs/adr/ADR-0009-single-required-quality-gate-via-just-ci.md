# ADR-0009: Single Required Quality Gate via `just ci`

- Status: Accepted
- Date: 2026-04-15

## Context

Divergence between local verification and CI pipelines causes flaky merges and review churn.

## Decision

Define one canonical gate: `just ci`.
CI requires one job named `quality-gate` that executes `just ci`.

Gate includes formatting, linting, checks, tests, coverage, dependency audit, docs, and structural guards.

## Consequences

- Slightly longer gate runtime.
- Strong local/CI parity and predictable merge requirements.

## Alternatives Considered

- Many independent required jobs only: richer diagnostics, higher drift risk between local and CI behavior.
