//! SQL strings for each Phase-1 migration step. Split from `migration.rs`
//! so the Rust wrapper types stay under the per-file line budget; the SQL
//! is a single logical block and is easiest to read in one place.

pub(super) const CREATE_ENUMS_AND_CORE_TABLES_SQL: &str = r#"
CREATE TYPE issue_state_category AS ENUM ('open', 'active', 'done', 'cancelled');
CREATE TYPE issue_priority AS ENUM ('low', 'medium', 'high', 'urgent');
CREATE TYPE outbox_status AS ENUM ('pending', 'leased', 'delivered', 'failed', 'dead_letter');

CREATE TABLE workspace (
    id uuid PRIMARY KEY,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 200),
    row_version bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE workflow_version (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 200),
    version integer NOT NULL,
    checksum text NOT NULL CHECK (char_length(checksum) BETWEEN 1 AND 128),
    states jsonb NOT NULL DEFAULT '[]'::jsonb,
    transitions jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, id),
    UNIQUE (workspace_id, name, version)
);

CREATE TABLE project (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 200),
    workflow_version_id uuid NOT NULL,
    row_version bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, id),
    FOREIGN KEY (workspace_id, workflow_version_id)
        REFERENCES workflow_version (workspace_id, id)
);

CREATE TABLE milestone (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    project_id uuid NOT NULL,
    title text NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200),
    due_at timestamptz,
    row_version bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, id),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES project (workspace_id, id)
);

CREATE TABLE issue (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    project_id uuid NOT NULL,
    milestone_id uuid,
    title text NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200),
    description text CHECK (description IS NULL OR char_length(description) <= 4000),
    state_category issue_state_category NOT NULL,
    priority issue_priority NOT NULL,
    archived_at timestamptz,
    row_version bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, id),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES project (workspace_id, id),
    FOREIGN KEY (workspace_id, milestone_id)
        REFERENCES milestone (workspace_id, id)
);
"#;

pub(super) const DROP_ENUMS_AND_CORE_TABLES_SQL: &str = r#"
DROP TABLE IF EXISTS issue;
DROP TABLE IF EXISTS milestone;
DROP TABLE IF EXISTS project;
DROP TABLE IF EXISTS workflow_version;
DROP TABLE IF EXISTS workspace;
DROP TYPE IF EXISTS outbox_status;
DROP TYPE IF EXISTS issue_priority;
DROP TYPE IF EXISTS issue_state_category;
"#;

pub(super) const CREATE_ACTIVITY_OUTBOX_AND_IDEMPOTENCY_SQL: &str = r#"
CREATE TABLE activity (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    project_id uuid NOT NULL,
    issue_id uuid,
    command_id uuid NOT NULL,
    actor_id uuid NOT NULL,
    event_type text NOT NULL CHECK (char_length(event_type) BETWEEN 1 AND 128),
    event_payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES project (workspace_id, id),
    FOREIGN KEY (workspace_id, issue_id)
        REFERENCES issue (workspace_id, id)
);

CREATE TABLE outbox (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    event_id uuid NOT NULL UNIQUE,
    event_type text NOT NULL CHECK (char_length(event_type) BETWEEN 1 AND 128),
    aggregate_type text NOT NULL CHECK (char_length(aggregate_type) BETWEEN 1 AND 64),
    aggregate_id uuid NOT NULL,
    occurred_at timestamptz NOT NULL,
    correlation_id uuid NOT NULL,
    payload jsonb NOT NULL,
    attempts integer NOT NULL DEFAULT 0,
    available_at timestamptz NOT NULL DEFAULT now(),
    status outbox_status NOT NULL DEFAULT 'pending',
    leased_at timestamptz,
    leased_until timestamptz,
    lease_token uuid,
    lease_owner uuid,
    published_at timestamptz,
    last_error text CHECK (last_error IS NULL OR char_length(last_error) <= 4000),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE idempotency_record (
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    command_name text NOT NULL CHECK (char_length(command_name) BETWEEN 1 AND 128),
    idempotency_key text NOT NULL CHECK (char_length(idempotency_key) BETWEEN 1 AND 128),
    request_fingerprint text NOT NULL CHECK (char_length(request_fingerprint) BETWEEN 1 AND 128),
    response_payload jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, command_name, idempotency_key)
);
"#;

pub(super) const DROP_ACTIVITY_OUTBOX_AND_IDEMPOTENCY_SQL: &str = r#"
DROP TABLE IF EXISTS idempotency_record;
DROP TABLE IF EXISTS outbox;
DROP TABLE IF EXISTS activity;
"#;

pub(super) const CREATE_INDEXES_SQL: &str = r#"
CREATE INDEX idx_project_workspace_id_id ON project(workspace_id, id);
CREATE INDEX idx_milestone_workspace_id_id ON milestone(workspace_id, id);
CREATE INDEX idx_issue_workspace_id_id ON issue(workspace_id, id);
CREATE INDEX idx_issue_workspace_project ON issue(workspace_id, project_id, archived_at);
CREATE INDEX idx_issue_workspace_project_state ON issue(workspace_id, project_id, state_category);
CREATE INDEX idx_activity_workspace_created ON activity(workspace_id, created_at DESC);
CREATE INDEX idx_outbox_poll_pending ON outbox(workspace_id, available_at, id)
    WHERE status IN ('pending', 'failed');
CREATE INDEX idx_outbox_lease_expiry ON outbox(workspace_id, leased_until, id)
    WHERE status = 'leased';
CREATE INDEX idx_outbox_retention ON outbox(workspace_id, status, published_at, updated_at);
CREATE INDEX idx_idempotency_expires_at ON idempotency_record(expires_at);
CREATE INDEX idx_idempotency_fingerprint ON idempotency_record(
    workspace_id,
    command_name,
    idempotency_key,
    request_fingerprint
);
"#;

pub(super) const DROP_INDEXES_SQL: &str = r#"
DROP INDEX IF EXISTS idx_idempotency_expires_at;
DROP INDEX IF EXISTS idx_idempotency_fingerprint;
DROP INDEX IF EXISTS idx_outbox_retention;
DROP INDEX IF EXISTS idx_outbox_lease_expiry;
DROP INDEX IF EXISTS idx_outbox_poll_pending;
DROP INDEX IF EXISTS idx_activity_workspace_created;
DROP INDEX IF EXISTS idx_issue_workspace_project_state;
DROP INDEX IF EXISTS idx_issue_workspace_project;
DROP INDEX IF EXISTS idx_issue_workspace_id_id;
DROP INDEX IF EXISTS idx_milestone_workspace_id_id;
DROP INDEX IF EXISTS idx_project_workspace_id_id;
"#;

pub(super) const ENABLE_RLS_AND_POLICIES_SQL: &str = r#"
ALTER TABLE workspace ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_version ENABLE ROW LEVEL SECURITY;
ALTER TABLE project ENABLE ROW LEVEL SECURITY;
ALTER TABLE milestone ENABLE ROW LEVEL SECURITY;
ALTER TABLE issue ENABLE ROW LEVEL SECURITY;
ALTER TABLE activity ENABLE ROW LEVEL SECURITY;
ALTER TABLE outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE idempotency_record ENABLE ROW LEVEL SECURITY;

ALTER TABLE workspace FORCE ROW LEVEL SECURITY;
ALTER TABLE workflow_version FORCE ROW LEVEL SECURITY;
ALTER TABLE project FORCE ROW LEVEL SECURITY;
ALTER TABLE milestone FORCE ROW LEVEL SECURITY;
ALTER TABLE issue FORCE ROW LEVEL SECURITY;
ALTER TABLE activity FORCE ROW LEVEL SECURITY;
ALTER TABLE outbox FORCE ROW LEVEL SECURITY;
ALTER TABLE idempotency_record FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_workspace_policy ON workspace
    USING (id::text = current_setting('app.workspace_id', true))
    WITH CHECK (id::text = current_setting('app.workspace_id', true));

CREATE POLICY tenant_workflow_version_policy ON workflow_version
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));

CREATE POLICY tenant_project_policy ON project
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));

CREATE POLICY tenant_milestone_policy ON milestone
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));

CREATE POLICY tenant_issue_policy ON issue
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));

CREATE POLICY tenant_activity_policy ON activity
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));

CREATE POLICY tenant_outbox_policy ON outbox
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));

CREATE POLICY tenant_idempotency_policy ON idempotency_record
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));
"#;

pub(super) const DROP_RLS_AND_POLICIES_SQL: &str = r#"
DROP POLICY IF EXISTS tenant_idempotency_policy ON idempotency_record;
DROP POLICY IF EXISTS tenant_outbox_policy ON outbox;
DROP POLICY IF EXISTS tenant_activity_policy ON activity;
DROP POLICY IF EXISTS tenant_issue_policy ON issue;
DROP POLICY IF EXISTS tenant_milestone_policy ON milestone;
DROP POLICY IF EXISTS tenant_project_policy ON project;
DROP POLICY IF EXISTS tenant_workflow_version_policy ON workflow_version;
DROP POLICY IF EXISTS tenant_workspace_policy ON workspace;

ALTER TABLE idempotency_record NO FORCE ROW LEVEL SECURITY;
ALTER TABLE outbox NO FORCE ROW LEVEL SECURITY;
ALTER TABLE activity NO FORCE ROW LEVEL SECURITY;
ALTER TABLE issue NO FORCE ROW LEVEL SECURITY;
ALTER TABLE milestone NO FORCE ROW LEVEL SECURITY;
ALTER TABLE project NO FORCE ROW LEVEL SECURITY;
ALTER TABLE workflow_version NO FORCE ROW LEVEL SECURITY;
ALTER TABLE workspace NO FORCE ROW LEVEL SECURITY;

ALTER TABLE idempotency_record DISABLE ROW LEVEL SECURITY;
ALTER TABLE outbox DISABLE ROW LEVEL SECURITY;
ALTER TABLE activity DISABLE ROW LEVEL SECURITY;
ALTER TABLE issue DISABLE ROW LEVEL SECURITY;
ALTER TABLE milestone DISABLE ROW LEVEL SECURITY;
ALTER TABLE project DISABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_version DISABLE ROW LEVEL SECURITY;
ALTER TABLE workspace DISABLE ROW LEVEL SECURITY;
"#;

pub(super) const CREATE_WORKER_PARTITION_LEASE_SQL: &str = r#"
ALTER TABLE workspace
    ADD COLUMN shard_bucket smallint;

UPDATE workspace
SET shard_bucket = (abs(hashtext(id::text)) % 1024)::smallint
WHERE shard_bucket IS NULL;

ALTER TABLE workspace
    ALTER COLUMN shard_bucket SET NOT NULL,
    ADD CONSTRAINT workspace_shard_bucket_range_check
        CHECK (shard_bucket BETWEEN 0 AND 1023);

CREATE INDEX idx_workspace_shard_bucket ON workspace(shard_bucket, id);

CREATE TABLE worker_partition_lease (
    workspace_id uuid PRIMARY KEY REFERENCES workspace(id) ON DELETE CASCADE,
    shard_bucket smallint NOT NULL,
    lease_owner uuid NOT NULL,
    leased_at timestamptz NOT NULL,
    leased_until timestamptz NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT worker_partition_lease_shard_bucket_range_check
        CHECK (shard_bucket BETWEEN 0 AND 1023)
);
CREATE INDEX idx_worker_partition_lease_owner_until
    ON worker_partition_lease (lease_owner, leased_until);
CREATE INDEX idx_worker_partition_lease_until
    ON worker_partition_lease (leased_until);
CREATE INDEX idx_worker_partition_lease_bucket_until
    ON worker_partition_lease (shard_bucket, leased_until);

-- Worker orchestration is cross-tenant by design. The partition-lease
-- acquisition path needs to see every workspace without bypassing tenant
-- RLS elsewhere, so we expose a minimal, read-only SECURITY DEFINER
-- function that returns the workspace id set for a bounded shard bucket
-- window. EXECUTE is explicitly REVOKEd from PUBLIC and GRANTed only to
-- the configured application role (if it exists at migration time);
-- additional roles may be granted out-of-band.
CREATE OR REPLACE FUNCTION list_workspace_ids_for_partition_lease(
    bucket_min integer,
    bucket_max integer,
    max_rows integer
)
RETURNS TABLE(id uuid, shard_bucket smallint)
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    SELECT id, shard_bucket
    FROM workspace
    WHERE shard_bucket BETWEEN bucket_min AND bucket_max
    ORDER BY shard_bucket, id
    LIMIT max_rows
$$;

REVOKE ALL ON FUNCTION list_workspace_ids_for_partition_lease(integer, integer, integer) FROM PUBLIC;

DO $$
DECLARE
    configured_role text := current_setting('dandori.app_role', true);
BEGIN
    IF configured_role IS NOT NULL
       AND configured_role <> ''
       AND EXISTS (SELECT 1 FROM pg_roles WHERE rolname = configured_role)
    THEN
        EXECUTE format(
            'GRANT EXECUTE ON FUNCTION list_workspace_ids_for_partition_lease(integer, integer, integer) TO %I',
            configured_role
        );
    END IF;
END
$$;
"#;

pub(super) const DROP_WORKER_PARTITION_LEASE_SQL: &str = r#"
DROP FUNCTION IF EXISTS list_workspace_ids_for_partition_lease(integer, integer, integer);
DROP INDEX IF EXISTS idx_worker_partition_lease_bucket_until;
DROP INDEX IF EXISTS idx_worker_partition_lease_until;
DROP INDEX IF EXISTS idx_worker_partition_lease_owner_until;
DROP TABLE IF EXISTS worker_partition_lease;
DROP INDEX IF EXISTS idx_workspace_shard_bucket;
ALTER TABLE workspace DROP CONSTRAINT IF EXISTS workspace_shard_bucket_range_check;
ALTER TABLE workspace DROP COLUMN IF EXISTS shard_bucket;
"#;
