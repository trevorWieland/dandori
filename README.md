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

The worker routes typed outbox events (`EventType::IssueCreatedV1`) through a
pluggable publisher. The default startup policy is **fail-closed**: the worker
refuses to run without an explicit publisher configuration.

- `DANDORI_OUTBOX_PUBLISH_URL` (required for production)
  - when set: worker publishes envelopes to this URL via the hardened HTTP
    publisher (explicit connect + request timeouts, bounded connection pool,
    and an in-process circuit breaker).
- `DANDORI_OUTBOX_ALLOW_NOOP_PUBLISHER` (dev escape hatch only)
  - set to `1` or `true` to explicitly opt in to the no-op publisher when
    `DANDORI_OUTBOX_PUBLISH_URL` is unset. The worker logs a warning on every
    start. Never set this in production.
- Transport tuning (optional):
  - `DANDORI_WORKER_HTTP_CONNECT_TIMEOUT_MS` (default `2000`)
  - `DANDORI_WORKER_HTTP_REQUEST_TIMEOUT_MS` (default `10000`)
  - `DANDORI_WORKER_PUBLISH_CONCURRENCY` (default `8`)
  - `DANDORI_WORKER_RETRY_JITTER_MS` (default `2000`)
  - `DANDORI_WORKER_CIRCUIT_FAILURE_THRESHOLD` (default `10`)
  - `DANDORI_WORKER_CIRCUIT_COOLDOWN_SECONDS` (default `30`)
- Dynamic partition leasing (replaces the former static workspace list):
  - `DANDORI_WORKER_INSTANCE_ID` (optional UUID; strongly recommended for
    stable lease ownership across restarts)
  - `DANDORI_WORKER_PARTITION_BATCH` (default `64`)
  - `DANDORI_WORKER_PARTITION_LEASE_SECONDS` (default `60`)

Workers discover workspaces directly from the database and acquire leases on
`worker_partition_lease` rows atomically (`INSERT … ON CONFLICT DO UPDATE
WHERE leased_until <= now`). Multiple workers can run in parallel without any
external coordinator and without a static sharding configuration.

## Failure Classification

Outbox publish failures are classified into transient vs terminal. Transient
errors (5xx, network, breaker open) honour the retry budget and backoff with
jitter. Terminal errors (4xx, unknown event type, serialization) dead-letter
immediately so the DLQ stays meaningful and the retry queue does not bloat.

## Workspace Layout

- `bin/` binaries (`dandori-api`, `dandori-mcp`, `dandori-worker`, `dandori-migrate`)
- `crates/` library crates for domain, contracts, policy, store, orchestration, and app services
- `docs/adr/` architecture decision records
- `frontend/` frontend scaffold placeholder
