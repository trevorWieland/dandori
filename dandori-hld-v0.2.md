# Dandori (段取り) — Design Spec v0.2 (Execution Ready)

Status: Draft for implementation
Date: 2026-04-15

## 1) Executive Summary
This v0.2 keeps your core direction intact and removes ambiguity that would otherwise cause rework during implementation. It defines strict semantics for graph behavior, workflow evolution, event recording, and GitHub sync conflict handling.

Primary outcome: a spec that can be implemented without hidden policy decisions during coding.

## 2) V1 Scope and Non-Goals
### In Scope (V1)
- Multi-project planner with issue graph, workflows, RBAC, labels, milestones, custom fields
- Append-only activity log for all mutations
- Bidirectional GitHub issue sync with explicit conflict states
- MCP parity for all core REST mutations and reads

### Out of Scope (V1)
- True event-sourced rebuild-from-zero runtime model
- Cross-workspace federation
- Advanced scheduling (critical path with duration estimation)
- Pluggable queue framework migration (keep arq for V1)

## 3) Architecture Decisions (Locked)
### AD-001: Event model for V1
Decision: Use transactional state writes + append-only activity events in the same DB transaction.
Rationale: preserves auditability and replay for diagnostics while keeping implementation complexity bounded.
Consequence: "events as write path" becomes "events are mandatory mutation records," not pure event sourcing.

### AD-002: Canonical dependency direction
Decision: Store only canonical edges.
- `blocked_by`: `source` is blocked issue, `target` is blocker
- `parent_of`: `source` is parent, `target` is child
- Symmetric types store one canonical pair with deterministic ordering (`min(issue_id) -> max(issue_id)`) to prevent duplicates.
Rationale: removes readiness/query ambiguity and duplicate-edge bugs.

### AD-003: Readiness rule (corrected)
An issue is ready when:
1. Current workflow state category is `open`
2. For all outbound `blocked_by` edges (or any type with `affects_scheduling=true`), target issue category is `done`
3. Issue is not archived/deleted

### AD-004: Workflow versioning
Decision: workflows are immutable once assigned to a project with active issues.
- Create `workflow_version` and pin project to `workflow_version_id`
- Editing creates a new version; migration endpoint re-maps project issues with explicit rules
Rationale: avoids silent semantic drift and broken transitions.

### AD-005: RBAC resolution
Decision: deny-by-default with deterministic order.
1. Workspace admin allow-all
2. Project role template grants
3. Per-project overrides can only reduce permissions, except workspace admin acting explicitly
4. Service accounts require explicit allow list; empty permissions = no access

### AD-006: Sync conflict semantics
Decision: field-level 3-way merge metadata.
- Track per-issue `last_synced_snapshot` (hash map of synced fields)
- Conflict only when both local and remote changed same mapped field since snapshot
- Non-overlapping field edits auto-merge
Rationale: avoids false conflicts from naive version counters.

## 4) Data Model Deltas from v0.1
### Required additions
- `issue.archived_at timestamptz null` (soft-delete clarity)
- `issue.deleted_by_id uuid null`
- `activity.project_seq bigint` with unique `(project_id, project_seq)` for ordered replay
- `activity.idempotency_key text null` indexed for dedupe on retried writes
- `project.workflow_version_id uuid` replacing direct mutable `workflow_id`
- `workflow_version` table with frozen states/transitions payload + checksum
- `github_sync_state` table (preferred over JSON blob) for queryability and conflict workflow

### Required constraints
- `issue_relation` unique canonical key based on normalized direction
- DB check preventing relation insertion when `cross_project=false` and projects differ
- DB check for `source_id != target_id`

### Index upgrades
- Partial index for ready queue:
  - `(project_id, is_ready, workflow_state_id)` where `archived_at is null`
- `GIN` + expression indexes for high-use custom fields (project-specific hot paths)

## 5) API Contract Hardening
### Mutation standards
- Require `Idempotency-Key` header on all POST/PATCH/DELETE mutation endpoints
- Return `409` on state/version conflicts with machine-readable conflict payload
- Return `422` for illegal transitions including valid `transition_ids`

### Concurrency controls
- Include `row_version bigint` on mutable aggregates (`issue`, `project`, `milestone`)
- Require `If-Match` or body `row_version` for mutation to prevent lost updates

## 6) Graph & Workflow Execution Rules
### Relation insert algorithm
1. Validate relation type exists and actor has permission
2. Normalize edge direction to canonical form
3. If `affects_scheduling=true`, run cycle detection CTE
4. Insert edge + append `issue.relation_added` event in one transaction
5. Enqueue readiness recompute for impacted nodes

### Transition execution algorithm
1. Resolve project’s pinned workflow version
2. Verify transition exists for current state
3. Evaluate conditions (`is_ready`, assignee, required labels/fields)
4. Apply state change + side effects atomically
5. Append `issue.status_changed` event with old/new state metadata

## 7) GitHub Sync Policy (V1)
### Mapping
Keep current mapping with explicit exclusions (`custom_fields`, relation graph internal metadata).

### Inbound webhook handling
- Verify signature and delivery idempotency before enqueue
- Persist raw webhook envelope for audit/replay (7–30 day retention)
- Worker performs field-level diff against `last_synced_snapshot`

### Outbound handling
- Outbox pattern from activity log (`sync.outbound.pending` -> sent -> acked)
- Exponential backoff + dead-letter stream for repeated failures

### Conflict state machine
`clean -> pending -> conflicted -> resolved`
- `resolved` requires explicit resolution action and creates `sync.conflict_resolved` event

## 8) Observability, SLOs, and Guardrails
### SLOs
- P95 board query < 250ms at 500 issues/project
- P95 transition mutation < 120ms
- P95 readiness recompute completion < 2s after dependency/status mutation
- P95 webhook-to-local-apply < 10s

### Required telemetry
- Structured logs with `request_id`, `actor_id`, `project_id`, `issue_id`
- Metrics: queue depth, webhook lag, sync conflicts, transition failures, cycle-detect rejects
- Traces spanning API -> DB -> queue worker

### Runbooks required before production
- GitHub webhook outage
- Queue backlog drain
- Migration rollback for workflow versioning
- Sync conflict triage

## 9) Security & Compliance Baseline
- OIDC JWT validation with issuer/audience/exp/nbf checks and JWKS cache rotation
- Service-to-service tokens scoped per project and permission
- Audit log is immutable to non-admin users and never hard-deleted in V1
- Secrets loaded from env/file mounts only; no secrets in project YAML committed to repo

## 10) Delivery Gates
### Gate A: Core integrity
- Migration suite passes on clean DB and upgrade path
- Relation cycle detection validated with property tests
- Transition validation exhaustive tests on default workflow

### Gate B: Operational integrity
- Idempotent webhook replay proven in tests
- Queue worker retry/dead-letter behavior verified
- RBAC tests for every permission matrix row

### Gate C: Integration integrity
- tanren happy-path transition flow passes end-to-end
- nanoclaw ready-issue pickup flow passes end-to-end
- GitHub conflict UI/API flow validated with fixture scenarios

## 11) Open Decisions (Need explicit answers before implementation)
1. Should `developer` be allowed to edit any issue or only created/assigned issues?
2. Are workflow migrations allowed on projects with active issues without freeze windows?
3. Should sync support GitHub Projects fields in V1 or explicitly defer to V2?
4. What is retention policy for raw webhook envelopes and activity log partitions?

