//! Shared test harness for Dandori integration and contract tests.
//!
//! Centralises the Postgres testcontainer bootstrap (role creation, grants,
//! seed data), factory helpers for creating commands and events, and fake
//! publishers used across the worker/chaos tests. Every phase-1 test crate
//! is expected to depend on this crate rather than reinventing the setup.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use chrono::Utc;
use dandori_app_services::{OutboxPublisher, PublishError, PublishErrorKind};
use dandori_domain::{
    AuthContext, CommandId, CreateIssueCommandV1, IdempotencyKey, IssueCreatedEventV1, IssueId,
    IssuePriority,
};
use dandori_store::{OutboxMessage, PgStore, migrate_database};
use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

/// Canonical phase-1 test database. Two workspaces, one workflow + project
/// per workspace, plus an app-role-scoped [`PgStore`] ready to execute
/// repository code under RLS.
#[derive(Debug)]
pub struct TestDatabase {
    pub container: testcontainers::ContainerAsync<Postgres>,
    pub admin_pool: PgPool,
    pub app_store: PgStore,
    pub admin_database_url: String,
    pub app_database_url: String,
    pub workspace_a: Uuid,
    pub workspace_b: Uuid,
    pub workflow_a: Uuid,
    pub workflow_b: Uuid,
    pub project_a: Uuid,
    pub project_b: Uuid,
}

/// Boot a Postgres testcontainer, apply all Dandori migrations, create the
/// non-superuser `dandori_app` role with minimum grants, and seed two
/// workspaces with one project each. The returned [`TestDatabase`]'s
/// [`PgStore`] connects as `dandori_app` so RLS is always exercised.
pub async fn setup_database() -> TestDatabase {
    let container = Postgres::default().start().await.expect("start postgres");
    let host = container.get_host().await.expect("host");
    let port = container.get_host_port_ipv4(5432).await.expect("port");

    let admin_database_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    migrate_database(&admin_database_url)
        .await
        .expect("migrate");

    let admin_pool = PgPool::connect(&admin_database_url)
        .await
        .expect("connect admin");

    create_app_role(&admin_pool).await;

    let workspace_a = Uuid::now_v7();
    let workspace_b = Uuid::now_v7();
    let workflow_a = Uuid::now_v7();
    let workflow_b = Uuid::now_v7();
    let project_a = Uuid::now_v7();
    let project_b = Uuid::now_v7();

    sqlx::query("INSERT INTO workspace (id, name) VALUES ($1, 'ws-a'), ($2, 'ws-b')")
        .bind(workspace_a)
        .bind(workspace_b)
        .execute(&admin_pool)
        .await
        .expect("seed workspace");

    sqlx::query(
        "INSERT INTO workflow_version (id, workspace_id, name, version, checksum, states, transitions)
         VALUES
            ($1, $2, 'default', 1, 'sha256:a', '[]'::jsonb, '[]'::jsonb),
            ($3, $4, 'default', 1, 'sha256:b', '[]'::jsonb, '[]'::jsonb)",
    )
    .bind(workflow_a)
    .bind(workspace_a)
    .bind(workflow_b)
    .bind(workspace_b)
    .execute(&admin_pool)
    .await
    .expect("seed workflow");

    sqlx::query(
        "INSERT INTO project (id, workspace_id, name, workflow_version_id)
         VALUES
            ($1, $2, 'project-a', $3),
            ($4, $5, 'project-b', $6)",
    )
    .bind(project_a)
    .bind(workspace_a)
    .bind(workflow_a)
    .bind(project_b)
    .bind(workspace_b)
    .bind(workflow_b)
    .execute(&admin_pool)
    .await
    .expect("seed project");

    let app_database_url = format!("postgres://dandori_app:dandori_app@{host}:{port}/postgres");
    let app_store = PgStore::connect(&app_database_url)
        .await
        .expect("connect app");

    TestDatabase {
        container,
        admin_pool,
        app_store,
        admin_database_url,
        app_database_url,
        workspace_a,
        workspace_b,
        workflow_a,
        workflow_b,
        project_a,
        project_b,
    }
}

async fn create_app_role(admin_pool: &PgPool) {
    sqlx::query(
        "DO $$
            BEGIN
                IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'dandori_app') THEN
                    CREATE ROLE dandori_app
                        LOGIN
                        PASSWORD 'dandori_app'
                        NOSUPERUSER
                        NOCREATEDB
                        NOCREATEROLE
                        NOBYPASSRLS;
                END IF;
            END
        $$;",
    )
    .execute(admin_pool)
    .await
    .expect("create app role");

    sqlx::query("GRANT USAGE ON SCHEMA public TO dandori_app")
        .execute(admin_pool)
        .await
        .expect("grant schema usage");

    sqlx::query(
        "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO dandori_app",
    )
    .execute(admin_pool)
    .await
    .expect("grant table perms");

    sqlx::query(
        "GRANT EXECUTE ON FUNCTION list_workspace_ids_for_partition_lease() TO dandori_app",
    )
    .execute(admin_pool)
    .await
    .expect("grant partition lease function exec");
}

/// Factory for a [`CreateIssueCommandV1`] ready to feed into the store. Fills
/// in a stable fingerprint for retry-replay tests.
#[must_use]
pub fn make_create_issue_command(
    workspace_id: Uuid,
    project_id: Uuid,
    issue_id: Uuid,
    command_id: Uuid,
    idempotency_key: &str,
) -> CreateIssueCommandV1 {
    CreateIssueCommandV1 {
        command_id: CommandId(command_id),
        idempotency_key: IdempotencyKey(idempotency_key.to_owned()),
        request_fingerprint: format!("fingerprint:{workspace_id}:{project_id}:{idempotency_key}"),
        issue_id: IssueId(issue_id),
        workspace_id: workspace_id.into(),
        project_id: project_id.into(),
        milestone_id: None,
        title: "Ship phase 1".to_owned(),
        description: Some("integration-test".to_owned()),
        priority: IssuePriority::High,
        actor_id: Uuid::now_v7(),
    }
}

/// Build a matching [`IssueCreatedEventV1`] for a command produced by
/// [`make_create_issue_command`].
#[must_use]
pub fn make_issue_created_event(command: &CreateIssueCommandV1) -> IssueCreatedEventV1 {
    IssueCreatedEventV1 {
        event_id: Uuid::now_v7(),
        issue_id: command.issue_id,
        workspace_id: command.workspace_id,
        project_id: command.project_id,
        milestone_id: command.milestone_id,
        actor_id: command.actor_id,
        occurred_at: Utc::now(),
        title: command.title.clone(),
        description: command.description.clone(),
        priority: command.priority,
    }
}

#[must_use]
pub fn auth_context(workspace_id: Uuid, actor_id: Uuid) -> AuthContext {
    AuthContext {
        workspace_id: workspace_id.into(),
        actor_id,
    }
}

/// Publisher that always succeeds.
#[derive(Debug, Default)]
pub struct AlwaysOkPublisher;

#[async_trait]
impl OutboxPublisher for AlwaysOkPublisher {
    async fn publish_issue_created(
        &self,
        _message: &OutboxMessage,
        _event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError> {
        Ok(())
    }
}

/// Publisher that fails with a transient error every time. Useful for
/// exercising retry budget and breaker behaviour.
#[derive(Debug, Default)]
pub struct TransientFailurePublisher;

#[async_trait]
impl OutboxPublisher for TransientFailurePublisher {
    async fn publish_issue_created(
        &self,
        _message: &OutboxMessage,
        _event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError> {
        Err(PublishError {
            kind: PublishErrorKind::Transient,
            message: "temporary downstream outage".to_owned(),
        })
    }
}

/// Publisher that returns [`PublishErrorKind::Permanent`] every time.
#[derive(Debug, Default)]
pub struct PermanentFailurePublisher;

#[async_trait]
impl OutboxPublisher for PermanentFailurePublisher {
    async fn publish_issue_created(
        &self,
        _message: &OutboxMessage,
        _event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError> {
        Err(PublishError {
            kind: PublishErrorKind::Permanent,
            message: "permanent downstream rejection".to_owned(),
        })
    }
}

/// Publisher that fails transiently for the first `n` attempts, then
/// succeeds. Attempts are counted atomically so the publisher is safe to
/// share across concurrent worker runs.
#[derive(Debug)]
pub struct TransientThenOkPublisher {
    failures_remaining: AtomicU32,
    total_calls: AtomicU32,
}

impl TransientThenOkPublisher {
    #[must_use]
    pub fn new(n: u32) -> Arc<Self> {
        Arc::new(Self {
            failures_remaining: AtomicU32::new(n),
            total_calls: AtomicU32::new(0),
        })
    }

    #[must_use]
    pub fn total_calls(&self) -> u32 {
        self.total_calls.load(Ordering::Acquire)
    }
}

#[async_trait]
impl OutboxPublisher for TransientThenOkPublisher {
    async fn publish_issue_created(
        &self,
        _message: &OutboxMessage,
        _event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError> {
        self.total_calls.fetch_add(1, Ordering::AcqRel);
        if self.failures_remaining.load(Ordering::Acquire) == 0 {
            return Ok(());
        }
        self.failures_remaining.fetch_sub(1, Ordering::AcqRel);
        Err(PublishError {
            kind: PublishErrorKind::Transient,
            message: "simulated transient blip".to_owned(),
        })
    }
}
