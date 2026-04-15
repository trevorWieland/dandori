# Dandori

Dandori is a multi-tenant, agent-first project management control plane.

## Repository Status

This repository is the v0 scaffold for the Rust-first rewrite.

## Quality Gate

Run the canonical gate locally:

```bash
just ci
```

## Workspace Layout

- `bin/` thin binaries (`dandori-api`, `dandori-mcp`, `dandori-worker`)
- `crates/` library crates for domain, contracts, policy, store, orchestration, and app services
- `docs/adr/` architecture decision records
- `frontend/` frontend scaffold placeholder
