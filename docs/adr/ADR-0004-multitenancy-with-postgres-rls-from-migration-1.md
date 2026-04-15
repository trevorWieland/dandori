# ADR-0004: Multi-Tenancy with PostgreSQL RLS from Migration 1

- Status: Accepted
- Date: 2026-04-15

## Context

Dandori must support multi-tenant isolation from day one with zero-trust defaults.

## Decision

Enable PostgreSQL Row Level Security from the first tenant-table migration.

- All tenant-owned rows include `workspace_id`.
- Tenant context is mandatory for API and worker DB sessions.
- No `BYPASSRLS` in service roles.

## Consequences

- More careful query/session design is required.
- Strong tenant isolation and compliance posture from inception.

## Alternatives Considered

- App-layer tenancy only (no RLS initially): faster initial dev, weaker isolation guarantees.
