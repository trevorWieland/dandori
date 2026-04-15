use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Database};

use crate::StoreError;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS workspace (
    id uuid PRIMARY KEY,
    name text NOT NULL,
    row_version bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS workflow_version (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    name text NOT NULL,
    version integer NOT NULL,
    checksum text NOT NULL,
    states jsonb NOT NULL DEFAULT '[]'::jsonb,
    transitions jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name, version)
);

CREATE TABLE IF NOT EXISTS project (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    name text NOT NULL,
    workflow_version_id uuid NOT NULL REFERENCES workflow_version(id),
    row_version bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS milestone (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    project_id uuid NOT NULL REFERENCES project(id),
    title text NOT NULL,
    due_at timestamptz,
    row_version bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS issue (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    project_id uuid NOT NULL REFERENCES project(id),
    milestone_id uuid REFERENCES milestone(id),
    title text NOT NULL,
    description text,
    state_category text NOT NULL CHECK (state_category IN ('open', 'active', 'done', 'cancelled')),
    priority text NOT NULL CHECK (priority IN ('low', 'medium', 'high', 'urgent')),
    archived_at timestamptz,
    row_version bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS activity (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    project_id uuid NOT NULL REFERENCES project(id),
    issue_id uuid REFERENCES issue(id),
    command_id uuid NOT NULL,
    actor_id uuid NOT NULL,
    event_type text NOT NULL,
    event_payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS outbox (
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
    status text NOT NULL DEFAULT 'pending',
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS idempotency_record (
    workspace_id uuid NOT NULL REFERENCES workspace(id),
    command_name text NOT NULL,
    idempotency_key text NOT NULL,
    command_id uuid NOT NULL,
    response_payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, command_name, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_project_workspace_id_id ON project(workspace_id, id);
CREATE INDEX IF NOT EXISTS idx_milestone_workspace_id_id ON milestone(workspace_id, id);
CREATE INDEX IF NOT EXISTS idx_issue_workspace_id_id ON issue(workspace_id, id);
CREATE INDEX IF NOT EXISTS idx_issue_workspace_project ON issue(workspace_id, project_id, archived_at);
CREATE INDEX IF NOT EXISTS idx_issue_workspace_project_state ON issue(workspace_id, project_id, state_category);
CREATE INDEX IF NOT EXISTS idx_outbox_poll_pending ON outbox(status, available_at, id) WHERE status = 'pending';
CREATE INDEX IF NOT EXISTS idx_activity_workspace_created ON activity(workspace_id, created_at DESC);

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

DROP POLICY IF EXISTS tenant_workspace_policy ON workspace;
CREATE POLICY tenant_workspace_policy ON workspace
    USING (id::text = current_setting('app.workspace_id', true))
    WITH CHECK (id::text = current_setting('app.workspace_id', true));

DROP POLICY IF EXISTS tenant_workflow_version_policy ON workflow_version;
CREATE POLICY tenant_workflow_version_policy ON workflow_version
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));

DROP POLICY IF EXISTS tenant_project_policy ON project;
CREATE POLICY tenant_project_policy ON project
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));

DROP POLICY IF EXISTS tenant_milestone_policy ON milestone;
CREATE POLICY tenant_milestone_policy ON milestone
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));

DROP POLICY IF EXISTS tenant_issue_policy ON issue;
CREATE POLICY tenant_issue_policy ON issue
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));

DROP POLICY IF EXISTS tenant_activity_policy ON activity;
CREATE POLICY tenant_activity_policy ON activity
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));

DROP POLICY IF EXISTS tenant_outbox_policy ON outbox;
CREATE POLICY tenant_outbox_policy ON outbox
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));

DROP POLICY IF EXISTS tenant_idempotency_policy ON idempotency_record;
CREATE POLICY tenant_idempotency_policy ON idempotency_record
    USING (workspace_id::text = current_setting('app.workspace_id', true))
    WITH CHECK (workspace_id::text = current_setting('app.workspace_id', true));
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS idempotency_record;
DROP TABLE IF EXISTS outbox;
DROP TABLE IF EXISTS activity;
DROP TABLE IF EXISTS issue;
DROP TABLE IF EXISTS milestone;
DROP TABLE IF EXISTS project;
DROP TABLE IF EXISTS workflow_version;
DROP TABLE IF EXISTS workspace;
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
        vec![Box::new(PhaseOneFoundationMigration)]
    }
}

#[derive(DeriveMigrationName)]
pub(crate) struct PhaseOneFoundationMigration;

#[sea_orm_migration::async_trait::async_trait]
impl MigrationTrait for PhaseOneFoundationMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN_SQL)
            .await?;
        Ok(())
    }
}
