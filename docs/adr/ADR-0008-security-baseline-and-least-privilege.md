# ADR-0008: Security Baseline and Least Privilege

- Status: Accepted
- Date: 2026-04-15

## Context

Agent-first automation increases blast radius if permissions and secret handling are weak.

## Decision

Enforce zero-trust security baseline:

- OIDC token validation (issuer/audience/signature/time checks).
- Least-privilege service accounts with explicit permission grants.
- `secrecy` wrappers for sensitive values.
- Immutable mutation/activity trails.
- No `BYPASSRLS`.

## Consequences

- Additional authn/authz plumbing effort.
- Stronger auditability and lower lateral movement risk.

## Alternatives Considered

- Coarse project-wide service permissions: simpler setup, unacceptable risk concentration.
