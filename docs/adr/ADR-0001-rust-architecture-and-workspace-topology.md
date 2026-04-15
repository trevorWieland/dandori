# ADR-0001: Rust-First Architecture and Workspace Topology

- Status: Accepted
- Date: 2026-04-15

## Context

Dandori requires compile-time correctness, strong isolation boundaries, and high long-term maintainability across API, MCP, workers, and sync systems.

## Decision

Adopt Rust as the implementation language and enforce a Cargo workspace topology with thin binaries and bounded library crates.

- `bin/*` are composition roots only.
- Business logic resides in `crates/*`.
- Crate layering is CI-enforced.

## Consequences

- Higher upfront scaffolding effort than script-first approaches.
- Stronger compile-time safety and clearer modular ownership boundaries.
- Requires strict Rust toolchain and lint discipline.

## Alternatives Considered

- Python-first implementation: faster early iteration, weaker compile-time guarantees.
- Single-crate monolith: simpler start, poorer long-term modularity and boundary enforcement.
