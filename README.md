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

Run explicit database migrations:

```bash
just db-migrate
```

## OIDC/JWKS Configuration (Required)

API and MCP now run in strict fail-closed mode and require OIDC/JWKS configuration.

Required environment variables:

- `DANDORI_OIDC_ISSUER`
- `DANDORI_OIDC_AUDIENCE`
- exactly one of:
  - `DANDORI_OIDC_JWKS_PATH`
  - `DANDORI_OIDC_JWKS_URL`
- optional strict algorithm allowlist:
  - `DANDORI_OIDC_ALLOWED_ALGS` (comma-separated, e.g. `RS256,ES256,EdDSA`)
- optional JWKS rotation tuning:
  - `DANDORI_OIDC_JWKS_REFRESH_INTERVAL_MILLIS`
  - `DANDORI_OIDC_JWKS_REFRESH_TIMEOUT_MILLIS`
  - `DANDORI_OIDC_JWKS_REFRESH_MAX_BACKOFF_MILLIS`

No fallback dev secrets are enabled.

## Runtime Migration Posture

- API, MCP, and worker binaries default to `run_migrations = false`.
- To explicitly allow startup migrations for controlled local workflows, set:
  - `DANDORI_RUN_MIGRATIONS=true`
- API/MCP/worker/migrate all require `DANDORI_DATABASE_URL` explicitly.

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

## Worker Publisher Runtime

The worker supports a concrete HTTP publisher adapter.

- `DANDORI_OUTBOX_PUBLISH_URL` (optional)
  - when set: worker publishes `issue.created.v1` envelopes to this URL
  - when unset: worker uses a no-op publisher for local/dev flows
- Worker shard config:
  - `DANDORI_WORKER_WORKSPACE_IDS` (required, comma-separated UUIDs)
  - `DANDORI_WORKER_SHARD_INDEX` (optional, default `0`)
  - `DANDORI_WORKER_SHARD_TOTAL` (optional, default `1`)
  - `DANDORI_WORKER_INSTANCE_ID` (optional UUID, defaults to generated value)

## Workspace Layout

- `bin/` binaries (`dandori-api`, `dandori-mcp`, `dandori-worker`, `dandori-migrate`)
- `crates/` library crates for domain, contracts, policy, store, orchestration, and app services
- `docs/adr/` architecture decision records
- `frontend/` frontend scaffold placeholder
