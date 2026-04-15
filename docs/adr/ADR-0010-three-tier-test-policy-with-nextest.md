# ADR-0010: Three-Tier Test Policy with nextest

- Status: Accepted
- Date: 2026-04-15

## Context

Dandori requires fast feedback and deep correctness checks for domain invariants and integration behavior.

## Decision

Adopt three test tiers with `cargo nextest` as the execution default.

- Unit tests: colocated, no I/O/network.
- Integration tests: external boundary validation.
- Property/snapshot tests: invariant and contract stability checks.

No test skipping is allowed in committed code.

## Consequences

- Requires stronger test discipline and tooling setup.
- Better confidence under refactors and concurrency changes.

## Alternatives Considered

- Unstructured test strategy with `cargo test` only: simpler setup, weaker reliability and slower feedback.
