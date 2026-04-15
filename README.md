# Dandori

Dandori is a multi-tenant, agent-first project management control plane.

## Repository Status

This repository contains the Phase 1 domain/store/API+MCP vertical slice for the Rust-first rewrite.

## Bootstrap (Clean Clone)

```bash
just bootstrap
```

This installs core tooling and activates git hooks via `lefthook install` when available.

## Quality Gates

Run the canonical gate locally:

```bash
just ci
```

Run the phase-specific gate directly:

```bash
just phase1-gate
```

## Database Prerequisites

Phase 1 integration tests require Docker (for ephemeral PostgreSQL testcontainers).

## Workspace Layout

- `bin/` thin binaries (`dandori-api`, `dandori-mcp`, `dandori-worker`)
- `crates/` library crates for domain, contracts, policy, store, orchestration, and app services
- `docs/adr/` architecture decision records
- `frontend/` frontend scaffold placeholder
