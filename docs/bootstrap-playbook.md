# Dandori Public Bootstrap Playbook

## Repository Target

- Owner/repo: `trevorWieland/dandori`
- Visibility: Public
- License: Apache-2.0
- Default branch: `main`

## Baseline Governance

- Squash-only merges
- PR required on `main`
- Minimum one approval
- Dismiss stale approvals
- Require conversation resolution
- Require linear history
- Restrict force push and deletions
- Required status check: `quality-gate`

## CI Gate

- Single required gate job: `quality-gate`
- Workflow runs `just ci`
- Phase 1 deep checks run via `just phase1-gate` (included in `just ci`)

## Local Bootstrap

- Run `just bootstrap` on a clean clone
- Verify hooks with `lefthook run pre-commit`
- Validate full quality gate with `just ci`
- Configure strict auth env for API/MCP runtime:
  - `DANDORI_OIDC_ISSUER`
  - `DANDORI_OIDC_AUDIENCE`
  - one JWKS source (`DANDORI_OIDC_JWKS_PATH` or `DANDORI_OIDC_JWKS_URL`)

## Process Scaffolding

- ADR pack in `docs/adr/`
- PR and issue templates
- Label baseline from `.github/labels.json`
