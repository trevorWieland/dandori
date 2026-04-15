use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Database};

use crate::StoreError;

const CREATE_ENUMS_AND_CORE_TABLES_SQL: &str = r#"
CREATE TYPE issue_state_category AS ENUM ('open', 'active', 'done', 'cancelled');
CREATE TYPE issue_priority AS ENUM ('low', 'medium', 'high', 'urgent');
CREATE TYPE outbox_status AS ENUM ('pending', 'leased', 'delivered', 'failed', 'dead_letter');

CREATE TABLE workspace (
    id uuid PRIMARY KEY,
    name text NOT NULL,
    row_version bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE workflow_version (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    name text NOT NULL,
    version integer NOT NULL,
    checksum text NOT NULL,
    states jsonb NOT NULL DEFAULT '[]'::jsonb,
    transitions jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, id),
    UNIQUE (workspace_id, name, version)
);

CREATE TABLE project (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    name text NOT NULL,
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
    title text NOT NULL,
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
    title text NOT NULL,
    description text,
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

const DROP_ENUMS_AND_CORE_TABLES_SQL: &str = r#"
DROP TABLE IF EXISTS issue;
DROP TABLE IF EXISTS milestone;
DROP TABLE IF EXISTS project;
DROP TABLE IF EXISTS workflow_version;
DROP TABLE IF EXISTS workspace;
DROP TYPE IF EXISTS outbox_status;
DROP TYPE IF EXISTS issue_priority;
DROP TYPE IF EXISTS issue_state_category;
"#;

const CREATE_ACTIVITY_OUTBOX_AND_IDEMPOTENCY_SQL: &str = r#"
CREATE TABLE activity (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    project_id uuid NOT NULL,
    issue_id uuid,
    command_id uuid NOT NULL,
    actor_id uuid NOT NULL,
    event_type text NOT NULL,
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
    event_type text NOT NULL,
    aggregate_type text NOT NULL,
    aggregate_id uuid NOT NULL,
    occurred_at timestamptz NOT NULL,
    correlation_id uuid NOT NULL,
    payload jsonb NOT NULL,
    attempts integer NOT NULL DEFAULT 0,
    available_at timestamptz NOT NULL DEFAULT now(),
    status outbox_status NOT NULL DEFAULT 'pending',
    leased_at timestamptz,
    leased_until timestamptz,
    published_at timestamptz,
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE idempotency_record (
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    command_name text NOT NULL,
    idempotency_key text NOT NULL,
    request_fingerprint text NOT NULL,
    response_payload jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, command_name, idempotency_key)
);
"#;

const DROP_ACTIVITY_OUTBOX_AND_IDEMPOTENCY_SQL: &str = r#"
DROP TABLE IF EXISTS idempotency_record;
DROP TABLE IF EXISTS outbox;
DROP TABLE IF EXISTS activity;
"#;

const CREATE_INDEXES_SQL: &str = r#"
CREATE INDEX idx_project_workspace_id_id ON project(workspace_id, id);
CREATE INDEX idx_milestone_workspace_id_id ON milestone(workspace_id, id);
CREATE INDEX idx_issue_workspace_id_id ON issue(workspace_id, id);
CREATE INDEX idx_issue_workspace_project ON issue(workspace_id, project_id, archived_at);
CREATE INDEX idx_issue_workspace_project_state ON issue(workspace_id, project_id, state_category);
CREATE INDEX idx_activity_workspace_created ON activity(workspace_id, created_at DESC);
CREATE INDEX idx_outbox_poll_pending ON outbox(status, available_at, id)
    WHERE status IN ('pending', 'failed');
CREATE INDEX idx_outbox_lease_expiry ON outbox(status, leased_until)
    WHERE status = 'leased';
CREATE INDEX idx_outbox_retention ON outbox(status, published_at, updated_at);
CREATE INDEX idx_idempotency_expires_at ON idempotency_record(expires_at);
CREATE INDEX idx_idempotency_fingerprint ON idempotency_record(
    workspace_id,
    command_name,
    idempotency_key,
    request_fingerprint
);
"#;

const DROP_INDEXES_SQL: &str = r#"
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

const ENABLE_RLS_AND_POLICIES_SQL: &str = r#"
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

const DROP_RLS_AND_POLICIES_SQL: &str = r#"
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

pub async fn migrate_database(database_url: &str) -> Result<(), StoreError> {
    let db = Database::connect(database_url).await?;
    Migrator::up(&db, None).await?;
    Ok(())
}

pub(crate) struct Migrator;

#[sea_orm_migration::async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(CreateEnumsAndCoreTables),
            Box::new(CreateActivityOutboxAndIdempotency),
            Box::new(CreateIndexes),
            Box::new(EnableRlsAndPolicies),
        ]
    }
}

struct CreateEnumsAndCoreTables;

struct CreateActivityOutboxAndIdempotency;

struct CreateIndexes;

struct EnableRlsAndPolicies;

impl MigrationName for CreateEnumsAndCoreTables {
    fn name(&self) -> &str {
        "m20260415_000001_create_enums_and_core_tables"
    }
}

impl MigrationName for CreateActivityOutboxAndIdempotency {
    fn name(&self) -> &str {
        "m20260415_000002_create_activity_outbox_and_idempotency"
    }
}

impl MigrationName for CreateIndexes {
    fn name(&self) -> &str {
        "m20260415_000003_create_indexes"
    }
}

impl MigrationName for EnableRlsAndPolicies {
    fn name(&self) -> &str {
        "m20260415_000004_enable_rls_and_policies"
    }
}

#[sea_orm_migration::async_trait::async_trait]
impl MigrationTrait for CreateEnumsAndCoreTables {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(CREATE_ENUMS_AND_CORE_TABLES_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DROP_ENUMS_AND_CORE_TABLES_SQL)
            .await?;
        Ok(())
    }
}

#[sea_orm_migration::async_trait::async_trait]
impl MigrationTrait for CreateActivityOutboxAndIdempotency {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(CREATE_ACTIVITY_OUTBOX_AND_IDEMPOTENCY_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DROP_ACTIVITY_OUTBOX_AND_IDEMPOTENCY_SQL)
            .await?;
        Ok(())
    }
}

#[sea_orm_migration::async_trait::async_trait]
impl MigrationTrait for CreateIndexes {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(CREATE_INDEXES_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DROP_INDEXES_SQL)
            .await?;
        Ok(())
    }
}

#[sea_orm_migration::async_trait::async_trait]
impl MigrationTrait for EnableRlsAndPolicies {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(ENABLE_RLS_AND_POLICIES_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DROP_RLS_AND_POLICIES_SQL)
            .await?;
        Ok(())
    }
}
