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

## OIDC/JWKS Configuration (Required)

API and MCP now run in strict fail-closed mode and require OIDC/JWKS configuration.

Required environment variables:

- `DANDORI_OIDC_ISSUER`
- `DANDORI_OIDC_AUDIENCE`
- exactly one of:
  - `DANDORI_OIDC_JWKS_PATH`
  - `DANDORI_OIDC_JWKS_URL`

No fallback dev secrets are enabled.

## MCP Runtime

`dandori-mcp` is a long-running stdio JSON-RPC server.

Example request flow:

1. Send `initialize`
2. Send `tools/list`
3. Send `tools/call` (`issue.create`, `issue.get`) with:
   - `token` (bearer JWT)
   - `arguments` (tool payload)

## Database Prerequisites

Phase 1 integration tests require Docker (for ephemeral PostgreSQL testcontainers).

## Workspace Layout

- `bin/` thin binaries (`dandori-api`, `dandori-mcp`, `dandori-worker`)
- `crates/` library crates for domain, contracts, policy, store, orchestration, and app services
- `docs/adr/` architecture decision records
- `frontend/` frontend scaffold placeholder
